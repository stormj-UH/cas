/// Integration tests for the cas-b51a supervisor-owned review pipeline.
///
/// These tests verify Stage 1 of the supervisor-owned review pipeline:
/// - AC1: CodeReviewConfig reads from cas config, defaults to "worker"
/// - AC2: owner=supervisor mode → PendingSupervisorReview transition
/// - AC3: owner=worker mode → existing behavior unchanged
/// - AC4: supervisor verify path works on PendingSupervisorReview tasks
/// - AC5: all 5 named test functions listed in spec
use crate::support::*;
use cas::config::{CodeReviewConfig, Config};
use cas::mcp::{CasCore, CasService};
use cas::store::{open_task_store, open_verification_store};
use cas::types::{TaskStatus, Verification, VerificationStatus};
use rmcp::handler::server::wrapper::Parameters;
use std::process::Command;

// ---------------------------------------------------------------------------
// Test helpers
// ---------------------------------------------------------------------------

/// RAII guard that installs factory-worker env vars for the duration of a
/// test and clears them on drop.
struct FactoryWorkerGuard;

impl FactoryWorkerGuard {
    fn enter() -> Self {
        unsafe {
            std::env::set_var("CAS_AGENT_ROLE", "worker");
            std::env::set_var("CAS_FACTORY_MODE", "1");
        }
        Self
    }
}

impl Drop for FactoryWorkerGuard {
    fn drop(&mut self) {
        unsafe {
            std::env::remove_var("CAS_AGENT_ROLE");
            std::env::remove_var("CAS_FACTORY_MODE");
        }
    }
}

/// Write a config.toml to the cas_dir that enables supervisor-owned review
/// and disables verification (so tests don't hit the verification jail).
fn write_supervisor_review_config(cas_dir: &std::path::Path) {
    let toml = r#"
[verification]
enabled = false

[code_review]
owner = "supervisor"
"#;
    std::fs::write(cas_dir.join("config.toml"), toml).expect("config.toml should be writable");
}

/// Write a config.toml with verification disabled and default (worker) code review.
fn write_worker_review_config(cas_dir: &std::path::Path) {
    let toml = r#"
[verification]
enabled = false
"#;
    std::fs::write(cas_dir.join("config.toml"), toml).expect("config.toml should be writable");
}

/// Init a minimal git repo at `project_root` with one staged change so that
/// `has_reviewable_changes()` returns true.
fn init_git_repo_with_staged_changes(project_root: &std::path::Path) {
    let git = |args: &[&str]| {
        Command::new("git")
            .args(args)
            .current_dir(project_root)
            .output()
            .expect("git command should run")
    };
    git(&["init", "-b", "main"]);
    git(&["config", "user.email", "test@example.com"]);
    git(&["config", "user.name", "Test"]);
    // Initial commit so HEAD exists
    std::fs::write(project_root.join("base.rs"), "fn main() {}\n")
        .expect("write should succeed");
    git(&["add", "base.rs"]);
    git(&["commit", "-m", "init"]);
    // Stage a reviewable Rust file so has_reviewable_changes() returns true
    std::fs::write(project_root.join("feature.rs"), "pub fn feature() -> u32 { 42 }\n")
        .expect("write should succeed");
    git(&["add", "feature.rs"]);
}

/// Init a git repo where the staged diff contains a `todo!()` violation so
/// `run_lightweight_structural_lint` returns `Fail`.
fn init_git_repo_with_lint_violation(project_root: &std::path::Path) {
    let git = |args: &[&str]| {
        Command::new("git")
            .args(args)
            .current_dir(project_root)
            .output()
            .expect("git command should run")
    };
    git(&["init", "-b", "main"]);
    git(&["config", "user.email", "test@example.com"]);
    git(&["config", "user.name", "Test"]);
    // Initial commit so HEAD exists.
    std::fs::write(project_root.join("base.rs"), "fn main() {}\n").expect("write should succeed");
    git(&["add", "base.rs"]);
    git(&["commit", "-m", "init"]);
    // Stage a file with a todo!() — this will appear as an added line in `git diff HEAD`.
    std::fs::write(
        project_root.join("wip.rs"),
        "pub fn incomplete() -> u32 { todo!(\"not implemented yet\") }\n",
    )
    .expect("write should succeed");
    git(&["add", "wip.rs"]);
}

// ---------------------------------------------------------------------------
// AC5 named tests
// ---------------------------------------------------------------------------

/// AC5 test 1: in supervisor-owned review mode, a factory worker close skips
/// the full cas-code-review skill dispatch and transitions the task to
/// `PendingSupervisorReview` instead of triggering `CODE_REVIEW_REQUIRED`.
#[tokio::test]
async fn test_worker_close_in_supervisor_mode_skips_cas_code_review() {
    let (temp, _core) = setup_cas();
    let _env_lock = env_test_lock();

    let cas_dir = temp.path().join(".cas");
    // Write config BEFORE first service creation so the OnceLock picks it up.
    write_supervisor_review_config(&cas_dir);

    // Init git repo at the project root so has_reviewable_changes() returns true.
    init_git_repo_with_staged_changes(temp.path());

    // Rebuild CasCore so it reads from our config.toml.
    let core = CasCore::with_daemon(cas_dir.clone(), None, None);
    let task_store = open_task_store(&cas_dir).unwrap();
    let service = CasService::new(core, None);

    let _worker_guard = FactoryWorkerGuard::enter();

    // Create and start a task.
    let create = task_req(serde_json::json!({
        "action": "create",
        "title": "Feature task for supervisor-mode test",
        "priority": 2,
        "task_type": "task",
    }));
    let created = service
        .task(Parameters(create))
        .await
        .expect("task.create should succeed");
    let id = extract_task_id(&extract_text(created))
        .expect("should have task ID")
        .to_string();

    service
        .task(Parameters(task_req(
            serde_json::json!({ "action": "start", "id": id }),
        )))
        .await
        .expect("task.start should succeed");

    // Close without a code_review_findings envelope — in supervisor mode this
    // should succeed and transition to pending_supervisor_review, NOT return
    // CODE_REVIEW_REQUIRED.
    let close_result = service
        .task(Parameters(task_req(serde_json::json!({
            "action": "close",
            "id": id,
            "reason": "All acceptance criteria met.",
        }))))
        .await
        .expect("task.close should return a result");

    let close_text = extract_text(close_result);

    // Must NOT see CODE_REVIEW_REQUIRED (the old worker-mode gate).
    assert!(
        !close_text.contains("CODE_REVIEW_REQUIRED"),
        "Supervisor-owned mode must skip CODE_REVIEW_REQUIRED gate; got: {close_text}"
    );

    // Must see the queued-for-supervisor-review confirmation.
    assert!(
        close_text.contains("supervisor review") || close_text.contains("pending_supervisor_review"),
        "Close response should confirm supervisor-review transition; got: {close_text}"
    );

    // Task status must be PendingSupervisorReview, NOT Closed.
    let task = task_store.get(&id).expect("task should exist");
    assert_eq!(
        task.status,
        TaskStatus::PendingSupervisorReview,
        "Task must be in PendingSupervisorReview state after supervisor-mode close"
    );
}

/// AC5 test 2: in worker-owned review mode (default), the existing behavior
/// is unchanged — a close without a review envelope returns CODE_REVIEW_REQUIRED.
#[tokio::test]
async fn test_worker_close_in_worker_mode_runs_cas_code_review_unchanged() {
    let (temp, _core) = setup_cas();
    let _env_lock = env_test_lock();

    let cas_dir = temp.path().join(".cas");
    // Use worker-mode config (verification disabled so we hit the code review gate).
    write_worker_review_config(&cas_dir);

    // Init git repo so has_reviewable_changes() = true.
    init_git_repo_with_staged_changes(temp.path());

    let core = CasCore::with_daemon(cas_dir.clone(), None, None);
    let service = CasService::new(core, None);

    let _worker_guard = FactoryWorkerGuard::enter();

    // Create and start task.
    let create = task_req(serde_json::json!({
        "action": "create",
        "title": "Worker-mode review test",
        "priority": 2,
        "task_type": "task",
    }));
    let created = service
        .task(Parameters(create))
        .await
        .expect("task.create should succeed");
    let id = extract_task_id(&extract_text(created))
        .expect("should have task ID")
        .to_string();

    service
        .task(Parameters(task_req(
            serde_json::json!({ "action": "start", "id": id }),
        )))
        .await
        .expect("task.start should succeed");

    // Close without a code_review_findings envelope — in worker mode this
    // should return CODE_REVIEW_REQUIRED (unchanged legacy behavior).
    let close_result = service
        .task(Parameters(task_req(serde_json::json!({
            "action": "close",
            "id": id,
            "reason": "All acceptance criteria met.",
        }))))
        .await
        .expect("task.close should return a result");

    let close_text = extract_text(close_result);

    assert!(
        close_text.contains("CODE_REVIEW_REQUIRED"),
        "Worker mode must still require CODE_REVIEW_REQUIRED gate; got: {close_text}"
    );

    // Task must still be InProgress (not closed, not pending review).
    let task_store = open_task_store(&cas_dir).unwrap();
    let task = task_store.get(&id).expect("task should exist");
    assert!(
        task.status != TaskStatus::Closed,
        "Task must remain open when CODE_REVIEW_REQUIRED fires"
    );
    assert_ne!(
        task.status,
        TaskStatus::PendingSupervisorReview,
        "Worker mode must NOT transition to PendingSupervisorReview"
    );
}

/// AC5 test 3: `PendingSupervisorReview` status persists through a store
/// restart (serialize → deserialize round-trip via SQLite).
#[tokio::test]
async fn test_pending_supervisor_review_status_persists_through_restart() {
    let (temp, _core) = setup_cas();
    let _env_lock = env_test_lock();

    let cas_dir = temp.path().join(".cas");
    let task_store = open_task_store(&cas_dir).unwrap();

    // Create a task and set it to PendingSupervisorReview directly.
    let mut task = cas::types::Task::new("cas-b51a-test-psr".to_string(), "PSR test".to_string());
    task.status = TaskStatus::PendingSupervisorReview;
    task_store.add(&task).expect("task.add should succeed");

    // Simulate a "restart" by opening a fresh store handle to the same DB.
    let task_store2 = open_task_store(&cas_dir).unwrap();
    let reloaded = task_store2.get("cas-b51a-test-psr").expect("task should exist after reload");

    assert_eq!(
        reloaded.status,
        TaskStatus::PendingSupervisorReview,
        "PendingSupervisorReview status must survive SQLite round-trip"
    );
    // Confirm is_open() / is_ready() semantics are preserved after reload.
    assert!(
        reloaded.is_open(),
        "PendingSupervisorReview task must be considered open"
    );
    assert!(
        !reloaded.is_ready(),
        "PendingSupervisorReview task must NOT be considered ready (for new worker pickup)"
    );
}

/// AC5 test 4: `mcp__cas__verification action=add` works on a task in
/// `PendingSupervisorReview` state — the supervisor can record a verdict
/// without any guard blocking them.
#[tokio::test]
async fn test_supervisor_verify_on_pending_review_task_works() {
    let (temp, _core) = setup_cas();
    let _env_lock = env_test_lock();

    let cas_dir = temp.path().join(".cas");
    let task_store = open_task_store(&cas_dir).unwrap();

    // Put a task directly into PendingSupervisorReview state.
    let mut task =
        cas::types::Task::new("cas-b51a-verify".to_string(), "Verify PSR task".to_string());
    task.status = TaskStatus::PendingSupervisorReview;
    task_store.add(&task).expect("task.add should succeed");

    // Supervisor records an approved verification row directly on the store
    // (mirrors what mcp__cas__verification action=add would do).
    let verification_store = open_verification_store(&cas_dir).unwrap();
    let ver_id = verification_store.generate_id().expect("should generate ID");
    let mut row = Verification::new(ver_id, "cas-b51a-verify".to_string());
    row.status = VerificationStatus::Approved;
    row.summary = "Code review complete — no P0 findings.".to_string();
    verification_store
        .add(&row)
        .expect("verification.add should succeed for PendingSupervisorReview task");

    // Verify the row is retrievable.
    let latest = verification_store
        .get_latest_for_task("cas-b51a-verify")
        .expect("store should not error")
        .expect("should find the verification row");
    assert_eq!(
        latest.status,
        VerificationStatus::Approved,
        "Supervisor verification row must be Approved"
    );
}

/// AC5 test 5: the `CodeReviewConfig` default owner is "worker" in factory
/// mode — Stage 1 backwards compat. Stage 2 (flip to "supervisor") is a
/// follow-on task and must not be activated by this code.
#[tokio::test]
async fn test_owner_config_default_is_worker_in_factory_mode() {
    // Test CodeReviewConfig direct default.
    let default_cfg = CodeReviewConfig::default();
    assert_eq!(
        default_cfg.owner, "worker",
        "CodeReviewConfig::default() owner must be 'worker' for Stage 1 backwards compat"
    );
    assert!(
        !default_cfg.supervisor_owned(),
        "supervisor_owned() must return false for default config"
    );

    // Test Config deserialization from empty TOML (no [code_review] section).
    let toml_no_section = "";
    let cfg: Config = toml::from_str(toml_no_section).expect("empty TOML should parse");
    assert!(
        cfg.code_review.is_none(),
        "Absent [code_review] section must deserialize to None"
    );
    // When code_review is None, supervisor_owned() defaults to false (worker mode).
    let supervisor_owned = cfg.code_review.as_ref().map(|c| c.supervisor_owned()).unwrap_or(false);
    assert!(
        !supervisor_owned,
        "Missing [code_review] section must default to worker mode (not supervisor)"
    );

    // Test explicit owner = "supervisor" round-trip.
    let toml_supervisor = "[code_review]\nowner = \"supervisor\"\n";
    let cfg2: Config = toml::from_str(toml_supervisor).expect("supervisor TOML should parse");
    let cr2 = cfg2.code_review.as_ref().expect("code_review section should be present");
    assert_eq!(cr2.owner, "supervisor", "TOML owner = 'supervisor' must round-trip");
    assert!(cr2.supervisor_owned(), "supervisor_owned() must be true for owner = 'supervisor'");

    // Test explicit owner = "worker" round-trip.
    let toml_worker = "[code_review]\nowner = \"worker\"\n";
    let cfg3: Config = toml::from_str(toml_worker).expect("worker TOML should parse");
    let cr3 = cfg3.code_review.as_ref().expect("code_review section should be present");
    assert!(!cr3.supervisor_owned(), "supervisor_owned() must be false for owner = 'worker'");
}

/// cas-b5ac: A close whose diff contains a `todo!()` call must be rejected by
/// the lightweight structural lint gate when owner=supervisor is configured.
/// The close must:
///   1. Return an MCP-level error (is_error=true), not Ok.
///   2. Name the offending lint rule in the error message.
///   3. Leave the task in InProgress — no transition to PendingSupervisorReview.
#[tokio::test]
async fn test_lint_fail_close_blocked_before_pending_supervisor_review() {
    let (temp, _core) = setup_cas();
    let _env_lock = env_test_lock();

    let cas_dir = temp.path().join(".cas");
    // Enable supervisor-owned review and disable verification.
    write_supervisor_review_config(&cas_dir);

    // Create a git repo whose staged diff includes a `todo!()` so the lint fires.
    init_git_repo_with_lint_violation(temp.path());

    let core = CasCore::with_daemon(cas_dir.clone(), None, None);
    let task_store = open_task_store(&cas_dir).unwrap();
    let service = CasService::new(core, None);

    let _worker_guard = FactoryWorkerGuard::enter();

    // Create and start a task.
    let create = task_req(serde_json::json!({
        "action": "create",
        "title": "WIP task with todo violation",
        "priority": 2,
        "task_type": "task",
    }));
    let created = service
        .task(Parameters(create))
        .await
        .expect("task.create should succeed");
    let id = extract_task_id(&extract_text(created))
        .expect("should have task ID")
        .to_string();

    service
        .task(Parameters(task_req(
            serde_json::json!({ "action": "start", "id": id }),
        )))
        .await
        .expect("task.start should succeed");

    // Attempt close — the staged diff contains `todo!()`, so lint must fail.
    let close_result = service
        .task(Parameters(task_req(serde_json::json!({
            "action": "close",
            "id": id,
            "reason": "Done.",
        }))))
        .await
        .expect("close returns Ok(CallToolResult) even when lint fails");

    // AC1: close must return is_error=true at the MCP level (not a silent success).
    assert_eq!(
        close_result.is_error,
        Some(true),
        "Lint-fail close must set is_error=true so the worker knows it was rejected"
    );

    let close_text = extract_text(close_result);

    // AC2: error message must name the offending lint rule.
    assert!(
        close_text.contains("todo!(") || close_text.contains("LIGHTWEIGHT LINT FAILED"),
        "Error message must identify the offending lint rule; got: {close_text}"
    );

    // AC3: task must remain InProgress — no PendingSupervisorReview transition on lint failure.
    let task = task_store.get(&id).expect("task should exist");
    assert_eq!(
        task.status,
        TaskStatus::InProgress,
        "Task must remain InProgress after lint failure; got: {:?}",
        task.status
    );
    assert_ne!(
        task.status,
        TaskStatus::PendingSupervisorReview,
        "Lint failure must NOT transition task to PendingSupervisorReview"
    );
}

// ---------------------------------------------------------------------------
// Helpers (local copies of patterns from verification_flow.rs)
// ---------------------------------------------------------------------------

fn task_req(value: serde_json::Value) -> cas_mcp::TaskRequest {
    serde_json::from_value(value).expect("TaskRequest should deserialize from test JSON")
}
