use crate::support::*;
use cas::mcp::tools::*;
use cas::store::{
    init_cas_dir, open_agent_store, open_task_store, open_verification_store, open_worktree_store,
};
use cas::types::{Verification, VerificationType, Worktree};
use rmcp::handler::server::wrapper::Parameters;
use tempfile::TempDir;

// cas-3bd4: env_test_lock() now lives in `support.rs` so `setup_cas()`
// can hold it while clearing factory env vars. Tests that need to set
// `CAS_AGENT_ROLE=supervisor` via `ScopedSupervisorEnv` MUST call
// `setup_cas()` FIRST and then acquire `env_test_lock()` — see the
// support.rs docs. Acquiring before calling `setup_cas` would deadlock
// because std `Mutex` is not re-entrant.

#[tokio::test]
async fn test_task_close_blocked_without_verification() {
    let (temp, service) = setup_cas();
    let _env_lock = env_test_lock();
    let cas_dir = temp.path().join(".cas");

    // Initialize verification store
    let verification_store = open_verification_store(&cas_dir).unwrap();

    // Create task
    let req = TaskCreateRequest {
        title: "Task requiring verification".to_string(),
        description: None,
        priority: 2,
        task_type: "task".to_string(),
        labels: None,
        notes: None,
        blocked_by: None,
        design: None,
        acceptance_criteria: None,
        external_ref: None,
        assignee: None,
        demo_statement: None,
        execution_note: None,
        epic: None,
    };

    let result = service
        .cas_task_create(Parameters(req))
        .await
        .expect("task_create should succeed");

    let text = extract_text(result);
    let id = extract_task_id(&text).expect("should have task ID");

    // Start task
    let start_req = IdRequest { id: id.to_string() };
    let _ = service
        .cas_task_start(Parameters(start_req))
        .await
        .expect("task_start should succeed");

    // Try to close task without verification - should be blocked
    let close_req = TaskCloseRequest {
        id: id.to_string(),
        reason: Some("Completed".to_string()),
        bypass_code_review: None,
code_review_findings: None,
    };
    let result = service
        .cas_task_close(Parameters(close_req))
        .await
        .expect("task_close should return a result");

    let text = extract_text(result);
    assert!(
        text.contains("VERIFICATION REQUIRED"),
        "Close should be blocked without verification: {text}"
    );
    assert!(
        text.contains("Task(subagent_type=\"task-verifier\""),
        "Close warning must include explicit Task() spawn syntax: {text}"
    );

    // A durable dispatch-request verification row must be persisted so the
    // close attempt is observable (no more fire-and-forget). The verdict
    // row will be written later by the task-verifier subagent.
    let latest = verification_store
        .get_latest_for_task(id)
        .unwrap()
        .expect("dispatch-request verification row should exist after close");
    assert_eq!(
        latest.status,
        cas::types::VerificationStatus::Error,
        "Dispatch-request row should have Error status until the subagent writes a verdict"
    );
    assert!(
        latest.summary.contains("Dispatch requested"),
        "Dispatch-request row summary should identify itself: {}",
        latest.summary
    );
}

#[tokio::test]
async fn test_task_close_sets_assignee_for_worktree_merge_jail() {
    let (temp, service) = setup_cas();
    let _env_lock = env_test_lock();
    let cas_dir = temp.path().join(".cas");

    std::fs::write(
        cas_dir.join("config.toml"),
        r#"[verification]
enabled = false

[worktrees]
enabled = true
require_merge_on_epic_close = true
"#,
    )
    .expect("should write config");

    let req = TaskCreateRequest {
        title: "Task with worktree".to_string(),
        description: None,
        priority: 2,
        task_type: "task".to_string(),
        labels: None,
        notes: None,
        blocked_by: None,
        design: None,
        acceptance_criteria: None,
        external_ref: None,
        assignee: None,
        demo_statement: None,
        execution_note: None,
        epic: None,
    };

    let result = service
        .cas_task_create(Parameters(req))
        .await
        .expect("task_create should succeed");

    let text = extract_text(result);
    let id = extract_task_id(&text).expect("should have task ID");

    let worktree_store = open_worktree_store(&cas_dir).expect("open worktree store");
    worktree_store.init().expect("init worktree store");
    let worktree_id = Worktree::generate_id();
    let worktree = Worktree::new(
        worktree_id.clone(),
        "cas/test-worktree".to_string(),
        "main".to_string(),
        temp.path().join("worktree"),
    );
    worktree_store.add(&worktree).expect("should add worktree");

    let task_store = open_task_store(&cas_dir).expect("open task store");
    let mut task = task_store.get(id).expect("task should exist");
    task.worktree_id = Some(worktree_id);
    task_store.update(&task).expect("should update task");

    let close_req = TaskCloseRequest {
        id: task.id.clone(),
        reason: Some("Done".to_string()),
        bypass_code_review: None,
code_review_findings: None,
    };
    let result = service
        .cas_task_close(Parameters(close_req))
        .await
        .expect("task_close should return result");

    let text = extract_text(result);
    assert!(
        text.contains("WORKTREE MERGE REQUIRED"),
        "Close should be blocked for merge: {text}"
    );

    let task = task_store.get(&task.id).expect("task should exist");
    assert!(
        task.pending_worktree_merge,
        "pending_worktree_merge should be set"
    );

    let agent_store = open_agent_store(&cas_dir).expect("open agent store");
    let agent_id = agent_store
        .list(None)
        .expect("list agents")
        .first()
        .map(|a| a.id.clone())
        .expect("agent should exist");
    assert_eq!(
        task.assignee.as_deref(),
        Some(agent_id.as_str()),
        "assignee should be set to current agent"
    );
}

/// cas-895d: a worker completes their work, writes tests, runs build, and
/// calls `task.close` — all while leaving the actual edits uncommitted in
/// their worktree. The pre-fix close path accepted this because
/// verification and the additive-only gate never looked at working-tree
/// state; the work got GC'd with the worktree.
///
/// Post-fix, the close path runs `git status --porcelain` against the
/// worker's worktree and rejects closes with any tracked modifications,
/// staged-but-uncommitted additions, deletes, or renames. Only committed
/// work — or genuinely scratch untracked files — may pass.
///
/// This test wires up a real git repo as the "worker worktree", attaches
/// it to a task via `task.worktree_id`, and exercises the close path
/// directly. verification_enabled=false so the test isolates the new
/// gate from the task-verifier flow.
#[tokio::test]
async fn test_task_close_blocks_on_uncommitted_worker_worktree() {
    use std::process::Command;

    let (temp, service) = setup_cas();
    let _env_lock = env_test_lock();
    let cas_dir = temp.path().join(".cas");

    // Disable verification so we isolate the cas-895d uncommitted-work
    // gate from the task-verifier jail.
    std::fs::write(
        cas_dir.join("config.toml"),
        r#"[verification]
enabled = false
"#,
    )
    .expect("write config");

    // Create a real git repo in a tempdir to play the role of a worker
    // worktree. One committed file, so HEAD exists and `git status`
    // behaves normally.
    let worktree_path = temp.path().join("worker-worktree");
    std::fs::create_dir_all(&worktree_path).expect("mkdir worktree");
    let git = |args: &[&str]| {
        let ok = Command::new("git")
            .args(args)
            .current_dir(&worktree_path)
            .env("GIT_AUTHOR_NAME", "t")
            .env("GIT_AUTHOR_EMAIL", "t@t")
            .env("GIT_COMMITTER_NAME", "t")
            .env("GIT_COMMITTER_EMAIL", "t@t")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .status()
            .expect("git")
            .success();
        assert!(ok, "git {args:?} failed");
    };
    git(&["init", "-q", "-b", "main"]);
    std::fs::write(worktree_path.join("seed.txt"), "seed\n").unwrap();
    git(&["add", "seed.txt"]);
    git(&["commit", "-q", "-m", "seed"]);

    // Register the worktree in cas and attach it to a task.
    let worktree_store = open_worktree_store(&cas_dir).expect("open worktree store");
    worktree_store.init().expect("init worktree store");
    let worktree_id = Worktree::generate_id();
    let worktree = Worktree::new(
        worktree_id.clone(),
        "cas/895d-worker".to_string(),
        "main".to_string(),
        worktree_path.clone(),
    );
    worktree_store.add(&worktree).expect("add worktree");

    let create_req = TaskCreateRequest {
        title: "cas-895d regression: committed-state close gate".to_string(),
        description: None,
        priority: 2,
        task_type: "task".to_string(),
        labels: None,
        notes: None,
        blocked_by: None,
        design: None,
        acceptance_criteria: None,
        external_ref: None,
        assignee: None,
        demo_statement: None,
        execution_note: None,
        epic: None,
    };
    let id = extract_task_id(&extract_text(
        service
            .cas_task_create(Parameters(create_req))
            .await
            .expect("task_create"),
    ))
    .expect("task id")
    .to_string();

    let task_store = open_task_store(&cas_dir).expect("open task store");
    let mut task = task_store.get(&id).expect("task exists");
    task.status = cas::types::TaskStatus::InProgress;
    task.worktree_id = Some(worktree_id.clone());
    task_store.update(&task).expect("update task");

    // Scenario A: worker modified an existing tracked file but never
    // committed. Closing must fail with UNCOMMITTED WORK.
    std::fs::write(worktree_path.join("seed.txt"), "worker edit\n").unwrap();
    let close_req = TaskCloseRequest {
        id: id.clone(),
        reason: Some("claims to be done".to_string()),
        bypass_code_review: None,
        code_review_findings: None,
    };
    let resp = extract_text(
        service
            .cas_task_close(Parameters(close_req))
            .await
            .expect("close returns result"),
    );
    assert!(
        resp.contains("UNCOMMITTED WORK"),
        "uncommitted tracked edit must reject close: {resp}"
    );
    assert!(
        resp.contains("seed.txt"),
        "error must name the dirty file: {resp}"
    );
    assert_ne!(
        task_store.get(&id).expect("task exists").status,
        cas::types::TaskStatus::Closed,
        "rejected close must not transition task to Closed"
    );

    // Scenario B: worker staged a new file but never committed. Same
    // lost-work scenario — must still block (status `A `).
    std::fs::write(worktree_path.join("seed.txt"), "seed\n").unwrap(); // revert
    std::fs::write(worktree_path.join("new.rs"), "fn main() {}\n").unwrap();
    git(&["add", "new.rs"]);
    let close_req = TaskCloseRequest {
        id: id.clone(),
        reason: Some("claims to be done".to_string()),
        bypass_code_review: None,
        code_review_findings: None,
    };
    let resp = extract_text(
        service
            .cas_task_close(Parameters(close_req))
            .await
            .expect("close returns result"),
    );
    assert!(
        resp.contains("UNCOMMITTED WORK"),
        "staged-but-uncommitted must reject close: {resp}"
    );
    assert!(
        resp.contains("new.rs"),
        "error must name the new file: {resp}"
    );

    // Scenario C: worker actually commits their work. Close must now
    // succeed (verification is disabled in this test's config).
    git(&["commit", "-q", "-m", "feat: add new.rs"]);
    let close_req = TaskCloseRequest {
        id: id.clone(),
        reason: Some("Committed and ready".to_string()),
        bypass_code_review: None,
        code_review_findings: None,
    };
    let resp = extract_text(
        service
            .cas_task_close(Parameters(close_req))
            .await
            .expect("close returns result"),
    );
    assert!(
        resp.contains("Closed task:"),
        "committed work must pass the gate: {resp}"
    );
    assert_eq!(
        task_store.get(&id).expect("task exists").status,
        cas::types::TaskStatus::Closed,
        "committed close must transition task to Closed"
    );
}

/// cas-bc1b regression: `execution_note=additive-only` close must inspect
/// the **worker branch's committed history**, not the main worktree's
/// unstaged state. Before the fix the additive-only check ran
/// `git diff --name-status HEAD` in `cas_root.parent()` (the main
/// worktree), so a pristine worker branch with a purely-additive commit
/// would be rejected because of an unrelated dirty file in main.
///
/// This test wires up:
///   * A real git repo with `main` committed and a `factory/worker`
///     branch forked off — standing in for the worker worktree.
///   * A cas worktree row pointing at that path with parent_branch="main".
///   * A task with execution_note=additive-only and that worktree_id.
///
/// The worker commits one purely-additive file on their branch, then
/// dirties an **unrelated** tracked file and leaves it uncommitted
/// (simulating the cas-4333 Cargo.lock drift). Close must succeed: the
/// branch diff is additive, and the uncommitted drift is ignored
/// because the check inspects committed history, not unstaged state.
#[tokio::test]
async fn test_additive_only_uses_worker_branch_not_main_worktree() {
    use std::process::Command;

    let (temp, service) = setup_cas();
    let _env_lock = env_test_lock();
    let cas_dir = temp.path().join(".cas");

    // Disable verification — we're testing the additive-only gate.
    // Also implicitly disables cas-895d uncommitted-work gate from
    // firing on the drift file (we want *this* test to prove the
    // additive-only fix works independently of the cas-895d gate).
    //
    // Actually: cas-895d's gate fires BEFORE additive-only and rejects
    // any dirty worker worktree. Since the simulated drift is in the
    // same worktree, cas-895d would catch it first. To isolate the
    // cas-bc1b fix, we intentionally leave the worker worktree clean
    // and rely on the fact that pre-fix code would have looked at the
    // MAIN worktree (cas_root.parent()) where unrelated drift lives.
    // Since cas_root.parent() here is a tempdir (not a git repo),
    // we can't put a stray file there and prove anything — instead,
    // prove the fix by committing a modification on the branch and
    // asserting the gate now catches it (which it wouldn't have
    // under the legacy `git diff HEAD` in main path — that one is
    // empty in tempdir because tempdir isn't a git repo).
    //
    // The "post-fix catches branch modifications" angle is the
    // cleaner assertion: pre-fix, the check ran in a non-git tempdir
    // and returned empty for every scenario; post-fix, it runs in
    // the worker branch and sees the real commits.
    std::fs::write(
        cas_dir.join("config.toml"),
        r#"[verification]
enabled = false
"#,
    )
    .expect("write config");

    // Real git repo playing the role of a worker worktree.
    let worktree_path = temp.path().join("worker-worktree");
    std::fs::create_dir_all(&worktree_path).expect("mkdir worktree");
    let git = |args: &[&str]| {
        let ok = Command::new("git")
            .args(args)
            .current_dir(&worktree_path)
            .env("GIT_AUTHOR_NAME", "t")
            .env("GIT_AUTHOR_EMAIL", "t@t")
            .env("GIT_COMMITTER_NAME", "t")
            .env("GIT_COMMITTER_EMAIL", "t@t")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .status()
            .expect("git")
            .success();
        assert!(ok, "git {args:?} failed");
    };
    git(&["init", "-q", "-b", "main"]);
    std::fs::write(worktree_path.join("existing.txt"), "original\n").unwrap();
    git(&["add", "existing.txt"]);
    git(&["commit", "-q", "-m", "main: initial"]);
    git(&["checkout", "-q", "-b", "factory/worker"]);

    // Register the worktree with parent_branch="main".
    let worktree_store = open_worktree_store(&cas_dir).expect("open worktree store");
    worktree_store.init().expect("init worktree store");
    let worktree_id = Worktree::generate_id();
    let worktree = Worktree::new(
        worktree_id.clone(),
        "factory/worker".to_string(),
        "main".to_string(),
        worktree_path.clone(),
    );
    worktree_store.add(&worktree).expect("add worktree");

    let task_store = open_task_store(&cas_dir).expect("open task store");

    let additive_req = |title: &str| TaskCreateRequest {
        title: title.to_string(),
        description: None,
        priority: 2,
        task_type: "task".to_string(),
        labels: None,
        notes: None,
        blocked_by: None,
        design: None,
        acceptance_criteria: None,
        external_ref: None,
        assignee: None,
        demo_statement: None,
        execution_note: Some("additive-only".to_string()),
        epic: None,
    };

    // --- Scenario A: worker branch has a purely-additive commit.
    //     Close must succeed.
    std::fs::write(worktree_path.join("new.rs"), "fn main() {}\n").unwrap();
    git(&["add", "new.rs"]);
    git(&["commit", "-q", "-m", "feat: add new.rs"]);
    let id_a = extract_task_id(&extract_text(
        service
            .cas_task_create(Parameters(additive_req("cas-bc1b: additive branch commit")))
            .await
            .expect("task_create"),
    ))
    .expect("task id")
    .to_string();
    {
        let mut t = task_store.get(&id_a).expect("task");
        t.status = cas::types::TaskStatus::InProgress;
        t.worktree_id = Some(worktree_id.clone());
        task_store.update(&t).expect("update task");
    }
    let resp_a = extract_text(
        service
            .cas_task_close(Parameters(TaskCloseRequest {
                id: id_a.clone(),
                reason: Some("committed and additive".to_string()),
                bypass_code_review: None,
                code_review_findings: None,
            }))
            .await
            .expect("close returns"),
    );
    assert!(
        resp_a.contains("Closed task:"),
        "purely-additive branch commit must pass: {resp_a}"
    );
    assert_eq!(
        task_store.get(&id_a).expect("task").status,
        cas::types::TaskStatus::Closed
    );

    // --- Scenario B: worker branch also has a commit modifying an
    //     existing tracked file. Additive-only must now reject. Pre-fix
    //     this would have been missed entirely — the check ran in the
    //     main worktree (not a git repo in the test) and silently no-
    //     oped.
    std::fs::write(worktree_path.join("existing.txt"), "worker edit\n").unwrap();
    git(&["add", "existing.txt"]);
    git(&["commit", "-q", "-m", "fix: edit existing.txt"]);
    let id_b = extract_task_id(&extract_text(
        service
            .cas_task_create(Parameters(additive_req("cas-bc1b: modifying branch commit")))
            .await
            .expect("task_create"),
    ))
    .expect("task id")
    .to_string();
    {
        let mut t = task_store.get(&id_b).expect("task");
        t.status = cas::types::TaskStatus::InProgress;
        t.worktree_id = Some(worktree_id.clone());
        task_store.update(&t).expect("update task");
    }
    let resp_b = extract_text(
        service
            .cas_task_close(Parameters(TaskCloseRequest {
                id: id_b.clone(),
                reason: Some("claims to be additive".to_string()),
                bypass_code_review: None,
                code_review_findings: None,
            }))
            .await
            .expect("close returns"),
    );
    assert!(
        resp_b.contains("ADDITIVE-ONLY VIOLATION"),
        "committed modification on worker branch must trigger additive-only gate: {resp_b}"
    );
    assert!(
        resp_b.contains("existing.txt"),
        "error must name the modified file: {resp_b}"
    );
    assert_ne!(
        task_store.get(&id_b).expect("task").status,
        cas::types::TaskStatus::Closed,
        "violation must not transition task to Closed"
    );
}

/// cas-895d + cas-bc1b follow-up regression: a task with `worktree_id = None`
/// (non-isolated worker, or direct CLI flow) must skip the close gates
/// entirely, even when the main repo is a live git repo with dirty state.
///
/// This plugs the test-harness hole the earlier cas-895d and cas-bc1b
/// tests created: they both used non-git tempdirs as `cas_root.parent()`,
/// so the gates silently no-oped regardless of whether they had the
/// worktree-scoping logic right. Production use has a real git repo
/// with active drift, and running either gate there would reject every
/// close of a non-isolated task.
///
/// Scenarios:
///   * Uncommitted-work gate (cas-895d) — must not fire.
///   * Additive-only gate (cas-bc1b) — must not fire even with
///     `execution_note=additive-only` and committed modifications on
///     the main branch.
#[tokio::test]
async fn test_close_gates_skipped_for_non_isolated_task_with_dirty_main() {
    use std::process::Command;

    let (temp, service) = setup_cas();
    let _env_lock = env_test_lock();
    let cas_dir = temp.path().join(".cas");

    // Disable verification so we isolate the close gates.
    std::fs::write(
        cas_dir.join("config.toml"),
        r#"[verification]
enabled = false
"#,
    )
    .expect("write config");

    // Turn the directory containing `.cas/` into a real git repo with
    // an active session's worth of dirty state:
    //   * one committed file on main
    //   * one modified tracked file (simulates supervisor mid-edit)
    //   * one staged new file (simulates another non-isolated worker)
    //   * one modification to an existing file committed on main but
    //     not on this task's branch (simulates cas-bc1b scenario on
    //     a non-isolated worker — there IS no branch, so the check
    //     must not fire)
    //
    // Pre-refinement cas-895d+cas-bc1b, both gates would run against
    // this tree and reject the close because of the dirty/staged
    // state that has nothing to do with the task. Post-refinement,
    // both gates skip entirely because `task.worktree_id == None`.
    let project_root = temp.path();
    let git = |args: &[&str]| {
        let ok = Command::new("git")
            .args(args)
            .current_dir(project_root)
            .env("GIT_AUTHOR_NAME", "t")
            .env("GIT_AUTHOR_EMAIL", "t@t")
            .env("GIT_COMMITTER_NAME", "t")
            .env("GIT_COMMITTER_EMAIL", "t@t")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .status()
            .expect("git")
            .success();
        assert!(ok, "git {args:?} failed");
    };
    // Initialize with .cas ignored so the cas metadata doesn't show up
    // as dirt (it isn't what we're testing here).
    //
    // The drift files are deliberately docs-only (`.md`) so they don't
    // also trip the cas-b39f code-review gate — that gate correctly
    // scans the main tree for reviewable changes and would require a
    // findings envelope. It's an independent concern from the
    // cas-895d/cas-bc1b fix this test is validating, so we pick
    // non-reviewable content for the drift. The cas-895d gate itself
    // checks every non-`??` status line regardless of file type, so
    // `.md` dirt exercises it just as well as `.rs`.
    git(&["init", "-q", "-b", "main"]);
    std::fs::write(project_root.join(".gitignore"), ".cas/\n").unwrap();
    std::fs::write(project_root.join("shared.md"), "# shared\n\n- one\n").unwrap();
    git(&["add", ".gitignore", "shared.md"]);
    git(&["commit", "-q", "-m", "main: initial"]);

    // Now dirty the main tree the way a live session would:
    //   - modify shared.md (unstaged)
    //   - stage a brand-new file
    std::fs::write(
        project_root.join("shared.md"),
        "# shared\n\n- one\n- two\n",
    )
    .unwrap();
    std::fs::write(
        project_root.join("supervisor_wip.md"),
        "# in flight\n",
    )
    .unwrap();
    git(&["add", "supervisor_wip.md"]);

    // --- Scenario A: uncommitted-work gate (cas-895d) MUST NOT fire
    //     for a task with no worktree_id, even with the above drift.
    let task_store = open_task_store(&cas_dir).expect("open task store");

    let create_req = TaskCreateRequest {
        title: "Non-isolated task over dirty main (cas-895d skip)".to_string(),
        description: None,
        priority: 2,
        task_type: "task".to_string(),
        labels: None,
        notes: None,
        blocked_by: None,
        design: None,
        acceptance_criteria: None,
        external_ref: None,
        assignee: None,
        demo_statement: None,
        execution_note: None,
        epic: None,
    };
    let id = extract_task_id(&extract_text(
        service
            .cas_task_create(Parameters(create_req))
            .await
            .expect("task_create"),
    ))
    .expect("task id")
    .to_string();
    let _ = service
        .cas_task_start(Parameters(IdRequest { id: id.clone() }))
        .await
        .expect("start");

    let close_req = TaskCloseRequest {
        id: id.clone(),
        reason: Some("non-isolated direct CLI flow".to_string()),
        bypass_code_review: None,
        code_review_findings: None,
    };
    let resp = extract_text(
        service
            .cas_task_close(Parameters(close_req))
            .await
            .expect("close returns"),
    );
    assert!(
        resp.contains("Closed task:"),
        "non-isolated task must not be rejected by cas-895d gate on \
         dirty main worktree: {resp}"
    );
    assert!(
        !resp.contains("UNCOMMITTED WORK"),
        "cas-895d gate must not fire for non-isolated tasks: {resp}"
    );
    assert_eq!(
        task_store.get(&id).expect("task").status,
        cas::types::TaskStatus::Closed
    );

    // --- Scenario B: additive-only gate (cas-bc1b) MUST NOT fire for a
    //     non-isolated task, even with execution_note=additive-only.
    //     For this we also commit a *modification* on main to prove
    //     the gate isn't running a branch-diff against the working
    //     tree's history either — the task has no branch of its own.
    git(&["add", "shared.md"]);
    git(&["commit", "-q", "-m", "main: extend shared.md"]);

    let create_additive_req = TaskCreateRequest {
        title: "Non-isolated additive-only task (cas-bc1b skip)".to_string(),
        description: None,
        priority: 2,
        task_type: "task".to_string(),
        labels: None,
        notes: None,
        blocked_by: None,
        design: None,
        acceptance_criteria: None,
        external_ref: None,
        assignee: None,
        demo_statement: None,
        execution_note: Some("additive-only".to_string()),
        epic: None,
    };
    let additive_id = extract_task_id(&extract_text(
        service
            .cas_task_create(Parameters(create_additive_req))
            .await
            .expect("task_create"),
    ))
    .expect("task id")
    .to_string();
    let _ = service
        .cas_task_start(Parameters(IdRequest {
            id: additive_id.clone(),
        }))
        .await
        .expect("start");

    let close_req = TaskCloseRequest {
        id: additive_id.clone(),
        reason: Some("additive-only non-isolated".to_string()),
        bypass_code_review: None,
        code_review_findings: None,
    };
    let resp = extract_text(
        service
            .cas_task_close(Parameters(close_req))
            .await
            .expect("close returns"),
    );
    assert!(
        resp.contains("Closed task:"),
        "non-isolated additive-only task must not be rejected by \
         cas-bc1b gate on dirty main worktree: {resp}"
    );
    assert!(
        !resp.contains("ADDITIVE-ONLY VIOLATION"),
        "cas-bc1b gate must not fire for non-isolated tasks: {resp}"
    );
    assert_eq!(
        task_store.get(&additive_id).expect("task").status,
        cas::types::TaskStatus::Closed
    );
}

/// cas-895d complement: a task with no attached worktree and a clean
/// project root still passes the gate. Ensures the gate doesn't break
/// non-factory (direct CLI) flows where there's no worktree to inspect.
#[tokio::test]
async fn test_task_close_passes_without_worktree_and_clean_cwd() {
    let (temp, service) = setup_cas();
    let _env_lock = env_test_lock();
    let cas_dir = temp.path().join(".cas");

    std::fs::write(
        cas_dir.join("config.toml"),
        r#"[verification]
enabled = false
"#,
    )
    .expect("write config");

    let create_req = TaskCreateRequest {
        title: "Notes-only task".to_string(),
        description: None,
        priority: 2,
        task_type: "task".to_string(),
        labels: None,
        notes: None,
        blocked_by: None,
        design: None,
        acceptance_criteria: None,
        external_ref: None,
        assignee: None,
        demo_statement: None,
        execution_note: None,
        epic: None,
    };
    let id = extract_task_id(&extract_text(
        service
            .cas_task_create(Parameters(create_req))
            .await
            .expect("task_create"),
    ))
    .expect("task id")
    .to_string();

    let _ = service
        .cas_task_start(Parameters(IdRequest { id: id.clone() }))
        .await
        .expect("start");

    // cas_root.parent() for the test is the temp dir which is not a
    // git repo → check_uncommitted_work returns empty → close passes.
    let close_req = TaskCloseRequest {
        id: id.clone(),
        reason: Some("done, no files touched".to_string()),
        bypass_code_review: None,
        code_review_findings: None,
    };
    let resp = extract_text(
        service
            .cas_task_close(Parameters(close_req))
            .await
            .expect("close returns result"),
    );
    assert!(
        resp.contains("Closed task:"),
        "non-git project root must not block close: {resp}"
    );
}

#[tokio::test]
async fn test_epic_close_requires_epic_verification_type() {
    let (temp, service) = setup_cas();
    let _env_lock = env_test_lock();
    let cas_dir = temp.path().join(".cas");

    let verification_store = open_verification_store(&cas_dir).unwrap();

    // Create epic
    let req = TaskCreateRequest {
        title: "Epic requiring epic verification".to_string(),
        description: None,
        priority: 2,
        task_type: "epic".to_string(),
        labels: None,
        notes: None,
        blocked_by: None,
        design: None,
        acceptance_criteria: None,
        external_ref: None,
        assignee: None,
        demo_statement: None,
        execution_note: None,
        epic: None,
    };

    let result = service
        .cas_task_create(Parameters(req))
        .await
        .expect("task_create should succeed");

    let text = extract_text(result);
    let id = extract_task_id(&text).expect("should have task ID");

    // Start epic
    let start_req = IdRequest { id: id.to_string() };
    let _ = service
        .cas_task_start(Parameters(start_req))
        .await
        .expect("task_start should succeed");

    // Close without verification should be blocked
    let close_req = TaskCloseRequest {
        id: id.to_string(),
        reason: Some("Completed".to_string()),
        bypass_code_review: None,
code_review_findings: None,
    };
    let result = service
        .cas_task_close(Parameters(close_req))
        .await
        .expect("task_close should return a result");
    let text = extract_text(result);
    assert!(
        text.contains("VERIFICATION REQUIRED"),
        "Epic close should be blocked without verification: {text}"
    );

    // Add a task-level verification - should NOT unblock epic close
    let task_ver = Verification::approved(
        "ver-epic-task".to_string(),
        id.to_string(),
        "Task-level verification".to_string(),
    );
    verification_store.add(&task_ver).unwrap();

    let close_req = TaskCloseRequest {
        id: id.to_string(),
        reason: Some("Completed".to_string()),
        bypass_code_review: None,
code_review_findings: None,
    };
    let result = service
        .cas_task_close(Parameters(close_req))
        .await
        .expect("task_close should return a result");
    let text = extract_text(result);
    assert!(
        text.contains("VERIFICATION REQUIRED"),
        "Epic close should still be blocked with task-level verification: {text}"
    );

    // Add epic-level verification - should unblock
    let mut epic_ver = Verification::approved(
        "ver-epic-ok".to_string(),
        id.to_string(),
        "Epic verification passed".to_string(),
    );
    epic_ver.verification_type = VerificationType::Epic;
    verification_store.add(&epic_ver).unwrap();

    let close_req = TaskCloseRequest {
        id: id.to_string(),
        reason: Some("Completed".to_string()),
        bypass_code_review: None,
code_review_findings: None,
    };
    let result = service
        .cas_task_close(Parameters(close_req))
        .await
        .expect("task_close should succeed");
    let text = extract_text(result);
    assert!(
        text.contains("Closed") || text.contains("closed"),
        "Epic should close with epic verification: {text}"
    );
}

#[tokio::test]
async fn test_task_lifecycle_with_verification() {
    let (temp, service) = setup_cas();
    let _env_lock = env_test_lock();
    let cas_dir = temp.path().join(".cas");

    // Initialize verification store
    let verification_store = open_verification_store(&cas_dir).unwrap();

    // Create task
    let req = TaskCreateRequest {
        title: "Lifecycle task".to_string(),
        description: None,
        priority: 2,
        task_type: "task".to_string(),
        labels: None,
        notes: None,
        blocked_by: None,
        design: None,
        acceptance_criteria: None,
        external_ref: None,
        assignee: None,
        demo_statement: None,
        execution_note: None,
        epic: None,
    };

    let result = service
        .cas_task_create(Parameters(req))
        .await
        .expect("task_create should succeed");

    let text = extract_text(result);
    let id = extract_task_id(&text).expect("should have task ID");

    // Start task
    let start_req = IdRequest { id: id.to_string() };
    let result = service
        .cas_task_start(Parameters(start_req))
        .await
        .expect("task_start should succeed");

    let text = extract_text(result);
    assert!(text.contains("Started") || text.contains("in_progress"));

    // Create an approved verification record
    let verification = Verification::approved(
        "ver-test".to_string(),
        id.to_string(),
        "All checks passed".to_string(),
    );
    verification_store.add(&verification).unwrap();

    // Close task - should succeed now with verification
    let close_req = TaskCloseRequest {
        id: id.to_string(),
        reason: Some("Completed successfully".to_string()),
        bypass_code_review: None,
code_review_findings: None,
    };
    let result = service
        .cas_task_close(Parameters(close_req))
        .await
        .expect("task_close should succeed");

    let text = extract_text(result);
    assert!(
        text.contains("Closed") || text.contains("closed"),
        "Task should close with verification: {text}"
    );
    assert!(
        text.contains("verified"),
        "Should indicate verification: {text}"
    );
}

#[tokio::test]
async fn test_task_close_blocked_with_rejected_verification() {
    use cas::types::VerificationIssue;

    let (temp, service) = setup_cas();
    let _env_lock = env_test_lock();
    let cas_dir = temp.path().join(".cas");

    // Initialize verification store
    let verification_store = open_verification_store(&cas_dir).unwrap();

    // Create task
    let req = TaskCreateRequest {
        title: "Task with rejected verification".to_string(),
        description: None,
        priority: 2,
        task_type: "task".to_string(),
        labels: None,
        notes: None,
        blocked_by: None,
        design: None,
        acceptance_criteria: None,
        external_ref: None,
        assignee: None,
        demo_statement: None,
        execution_note: None,
        epic: None,
    };

    let result = service
        .cas_task_create(Parameters(req))
        .await
        .expect("task_create should succeed");

    let text = extract_text(result);
    let id = extract_task_id(&text).expect("should have task ID");

    // Start task
    let start_req = IdRequest { id: id.to_string() };
    let _ = service
        .cas_task_start(Parameters(start_req))
        .await
        .expect("task_start should succeed");

    // Create a rejected verification record with issues
    let issues = vec![VerificationIssue::new(
        "src/main.rs".to_string(),
        "todo_comment".to_string(),
        "TODO comment found".to_string(),
    )];
    let verification = Verification::rejected(
        "ver-reject".to_string(),
        id.to_string(),
        "Found incomplete work".to_string(),
        issues,
    );
    verification_store.add(&verification).unwrap();

    // Try to close task - should be blocked due to rejected verification
    let close_req = TaskCloseRequest {
        id: id.to_string(),
        reason: Some("Completed".to_string()),
        bypass_code_review: None,
code_review_findings: None,
    };
    let result = service
        .cas_task_close(Parameters(close_req))
        .await
        .expect("task_close should return a result");

    let text = extract_text(result);
    assert!(
        text.contains("VERIFICATION FAILED"),
        "Close should be blocked with rejected verification: {text}"
    );
    assert!(text.contains("1 issue"), "Should show issue count: {text}");
}

/// Regression test for cas-7de3: `task.close` must either dispatch a verifier
/// (creating a verification row) or close the task with an explicit skip
/// reason recorded in notes/metadata. The pre-fix behavior returned a
/// `⚠️ VERIFICATION REQUIRED` warning string while leaving the task in
/// `InProgress` with no verification row — a fire-and-forget that silently
/// drops the close attempt. This test fails on main and passes once the
/// dispatch/skip path is wired up.
#[tokio::test]
async fn test_task_close_runs_verifier_or_skips_cleanly() {
    let (temp, service) = setup_cas();
    let _env_lock = env_test_lock();
    let cas_dir = temp.path().join(".cas");
    let task_store = open_task_store(&cas_dir).unwrap();
    let verification_store = open_verification_store(&cas_dir).unwrap();

    // Create + start a task.
    let req = TaskCreateRequest {
        title: "Dispatch-on-close regression task".to_string(),
        description: None,
        priority: 2,
        task_type: "task".to_string(),
        labels: None,
        notes: None,
        blocked_by: None,
        design: None,
        acceptance_criteria: None,
        external_ref: None,
        assignee: None,
        demo_statement: None,
        execution_note: None,
        epic: None,
    };
    let result = service
        .cas_task_create(Parameters(req))
        .await
        .expect("task_create should succeed");
    let id = extract_task_id(&extract_text(result))
        .expect("should have task ID")
        .to_string();

    let _ = service
        .cas_task_start(Parameters(IdRequest { id: id.clone() }))
        .await
        .expect("task_start should succeed");

    // Close with a clean, acceptance-criteria-satisfying reason. This is the
    // exact shape of close call that triggered the cas-7de3 regression: the
    // handler is supposed to dispatch a verifier (or record a skip), not just
    // print a warning and leave the task open.
    let close_req = TaskCloseRequest {
        id: id.clone(),
        reason: Some("Completed all acceptance criteria. Deployed to prod.".to_string()),
        bypass_code_review: None,
code_review_findings: None,
    };
    let result = service
        .cas_task_close(Parameters(close_req))
        .await
        .expect("task_close should return a result");
    let response_text = extract_text(result);

    // Re-read DB state after the call.
    let task_after = task_store.get(&id).expect("task should still exist");
    let verification_row = verification_store
        .get_latest_for_task(&id)
        .expect("verification lookup should not error");

    let dispatched_verifier = verification_row.is_some();
    let closed_with_skip_reason = task_after.status == cas::types::TaskStatus::Closed
        && (task_after.notes.to_lowercase().contains("verification skipped")
            || task_after
                .close_reason
                .as_deref()
                .map(|r| r.to_lowercase().contains("verification skipped"))
                .unwrap_or(false));

    assert!(
        dispatched_verifier || closed_with_skip_reason,
        "task.close must either dispatch a verifier (create a verification row) \
         or close the task with an explicit skip reason. Got:\n\
         \x20 - response text: {response_text}\n\
         \x20 - task status after close: {:?}\n\
         \x20 - verification row present: {dispatched_verifier}\n\
         \x20 - task notes: {:?}\n\
         \x20 - task close_reason: {:?}\n\
         This is the cas-7de3 regression: the handler returned a fire-and-forget \
         warning without actually running verification or recording a skip.",
        task_after.status,
        task_after.notes,
        task_after.close_reason,
    );
}

// === cas-26e1: supervisor escape hatch ===
//
// These tests lock down the supervisor-close bypass that shipped in
// close_ops.rs lines 64-82 (`assignee_inactive` path). Precedent: gabber-studio
// April 2-3 session `f21e74e7-3c57-4cf6-a295-ca6b8e113e79` closed ~12 worker
// tasks via this hatch after workers wedged (cas-bd17, cas-d6b0, cas-ce02,
// cas-79e9, cas-74b7, cas-6f19, cas-901d, cas-e3a3, cas-80de, cas-c5be,
// cas-ff22, cas-2bf7).
//
// The hatch is STRUCTURAL, not a reason-string match: it fires when BOTH
// `is_supervisor_from_env()` is true AND the task's assignee is missing /
// not-found / heartbeat-expired. The "verification skipped — assignee inactive"
// string is only a display note the handler appends to the success message
// (close_ops.rs:487); the supervisor's close_reason does not gate the hatch.
//
// These tests MUST still pass after cas-4acd narrowed the per-tool
// verification jail at server/mod.rs:646-663 to stop exempting `task.close`
// for factory workers. That narrowing affects the pre-handler jail; the bypass
// itself lives inside close_ops.rs and is unaffected — these tests verify
// that directly.

/// Small RAII guard so CAS_AGENT_ROLE is always cleared on drop, even on
/// panic, to avoid leaking the var into sibling tests that don't set it.
struct ScopedSupervisorEnv;

impl ScopedSupervisorEnv {
    fn new() -> Self {
        // SAFETY: setup_cas documents the same --test-threads=1-or-accept-race
        // contract. We set during the test body only and unconditionally
        // remove on drop.
        unsafe {
            std::env::set_var("CAS_AGENT_ROLE", "supervisor");
        }
        Self
    }
}

impl Drop for ScopedSupervisorEnv {
    fn drop(&mut self) {
        unsafe {
            std::env::remove_var("CAS_AGENT_ROLE");
        }
    }
}

/// Positive: supervisor closes an orphaned task (no assignee) → bypass fires.
/// Task goes to Closed without running the verifier and without writing a
/// verification row. The close_reason passed by the supervisor is preserved
/// on the task and the response carries the
/// "(verification skipped — assignee inactive)" marker.
#[tokio::test]
async fn test_close_supervisor_bypass_orphaned_task() {
    let (temp, service) = setup_cas();
    let _env_lock = env_test_lock();
    let cas_dir = temp.path().join(".cas");
    let task_store = open_task_store(&cas_dir).unwrap();
    let verification_store = open_verification_store(&cas_dir).unwrap();

    // Create + start a task, then strip its assignee to simulate the
    // orphaned-worker state the hatch is designed to recover from.
    let req = TaskCreateRequest {
        title: "Orphaned worker task for escape-hatch test".to_string(),
        description: None,
        priority: 2,
        task_type: "task".to_string(),
        labels: None,
        notes: None,
        blocked_by: None,
        design: None,
        acceptance_criteria: None,
        external_ref: None,
        assignee: None,
        demo_statement: None,
        execution_note: None,
        epic: None,
    };
    let create_text = extract_text(
        service
            .cas_task_create(Parameters(req))
            .await
            .expect("task_create should succeed"),
    );
    let id = extract_task_id(&create_text)
        .expect("should have task ID")
        .to_string();

    // Note: cas_task_start would set the assignee to the current test agent,
    // which would then be "alive" and short-circuit the inactive path. We want
    // the orphaned branch (`No assignee at all → orphaned`), so we set status
    // directly and leave assignee = None.
    let mut task = task_store.get(&id).expect("task should exist");
    task.status = cas::types::TaskStatus::InProgress;
    task.assignee = None;
    task_store.update(&task).expect("should update task");

    // Now flip the process into supervisor mode for the close call only.
    let _guard = ScopedSupervisorEnv::new();

    let close_req = TaskCloseRequest {
        id: id.clone(),
        reason: Some("verification skipped — assignee inactive".to_string()),
        bypass_code_review: None,
code_review_findings: None,
    };
    let result = service
        .cas_task_close(Parameters(close_req))
        .await
        .expect("task_close should succeed via supervisor bypass");
    let response_text = extract_text(result);

    assert!(
        response_text.contains("Closed"),
        "bypass close should report success: {response_text}"
    );
    // cas-3bd4: orphaned (no-assignee) closes now cite the accurate
    // reason — "orphaned task, no assignee" — instead of the catch-all
    // "assignee inactive" phrase that was always emitted regardless of
    // actual assignee state.
    assert!(
        response_text.contains("verification skipped — orphaned task, no assignee"),
        "response must carry the orphaned-task bypass marker: {response_text}"
    );
    assert!(
        !response_text.contains("VERIFICATION REQUIRED"),
        "bypass must not drop into the jail path: {response_text}"
    );

    let task_after = task_store.get(&id).expect("task should exist");
    assert_eq!(
        task_after.status,
        cas::types::TaskStatus::Closed,
        "supervisor bypass must transition task to Closed"
    );
    assert_eq!(
        task_after.close_reason.as_deref(),
        Some("verification skipped — assignee inactive"),
        "supervisor close_reason must be preserved verbatim"
    );
    assert!(
        task_after.notes.to_lowercase().contains("verification skipped"),
        "close_reason must also appear in the task notes timeline: {}",
        task_after.notes
    );

    // Per cas-82d6: the bypass path MUST write a durable `Skipped`
    // verification row so downstream workers that inherit a BlockedBy on
    // this task are not jailed by `check_pending_verification` (which used
    // to only accept `Approved`). The row is the audit trail for "closed
    // without running the verifier".
    let verification_row = verification_store
        .get_latest_for_task(&id)
        .expect("verification lookup should not error")
        .expect("supervisor bypass must write a Skipped verification row");
    assert_eq!(
        verification_row.status,
        cas::types::VerificationStatus::Skipped,
        "bypass row must be Skipped, got {:?}",
        verification_row.status
    );
}

/// Positive: supervisor closes a task whose assignee points at an agent that
/// does not exist in the agent store. This exercises the "assignee not found →
/// treat as inactive" branch distinct from the None-assignee branch above.
#[tokio::test]
async fn test_close_supervisor_bypass_ghost_assignee() {
    let (temp, service) = setup_cas();
    let _env_lock = env_test_lock();
    let cas_dir = temp.path().join(".cas");
    let task_store = open_task_store(&cas_dir).unwrap();
    let verification_store = open_verification_store(&cas_dir).unwrap();

    let req = TaskCreateRequest {
        title: "Task assigned to a ghost agent".to_string(),
        description: None,
        priority: 2,
        task_type: "task".to_string(),
        labels: None,
        notes: None,
        blocked_by: None,
        design: None,
        acceptance_criteria: None,
        external_ref: None,
        assignee: None,
        demo_statement: None,
        execution_note: None,
        epic: None,
    };
    let id = extract_task_id(&extract_text(
        service
            .cas_task_create(Parameters(req))
            .await
            .expect("task_create should succeed"),
    ))
    .expect("should have task ID")
    .to_string();

    let mut task = task_store.get(&id).expect("task should exist");
    task.status = cas::types::TaskStatus::InProgress;
    task.assignee = Some("ghost-agent-does-not-exist".to_string());
    task_store.update(&task).expect("should update task");

    let _guard = ScopedSupervisorEnv::new();

    let close_req = TaskCloseRequest {
        id: id.clone(),
        reason: Some("verification skipped — assignee inactive (ghost agent)".to_string()),
        bypass_code_review: None,
code_review_findings: None,
    };
    let response_text = extract_text(
        service
            .cas_task_close(Parameters(close_req))
            .await
            .expect("task_close should succeed via supervisor bypass"),
    );

    // cas-3bd4: a ghost assignee (agent row missing from the store) is
    // now reported as "assignee unknown" — the pre-cas-3bd4 path
    // always said "assignee inactive" regardless of the true state,
    // because `agent_store.get(name)` unwrap_or(true) collapsed every
    // lookup failure into the same bucket. The new path keeps the
    // supervisor bypass behavior but cites the real reason.
    assert!(
        response_text.contains("Closed")
            && response_text.contains("verification skipped — assignee unknown"),
        "ghost-assignee bypass should close and mark skipped: {response_text}"
    );

    let task_after = task_store.get(&id).expect("task should exist");
    assert_eq!(task_after.status, cas::types::TaskStatus::Closed);
    // Per cas-82d6: bypass now writes a Skipped row so downstream
    // BlockedBy consumers don't hit the MCP jail.
    let row = verification_store
        .get_latest_for_task(&id)
        .expect("verification lookup should not error")
        .expect("ghost-assignee bypass must write a Skipped verification row");
    assert_eq!(row.status, cas::types::VerificationStatus::Skipped);
}

/// cas-3bd4 regression: a factory worker's `task.assignee` stores the agent's
/// display *name* (e.g. `"mighty-viper-52"`), not its session id. The pre-fix
/// `agent_store.get(task.assignee)` therefore always failed, `unwrap_or(true)`
/// treated the assignee as inactive, and supervisor closes silently succeeded
/// with the misleading message `"verification skipped — assignee inactive"`
/// even when the worker was demonstrably alive and holding a fresh lease.
///
/// Post-fix, the close path resolves liveness from the task's active lease
/// (`TaskLease.agent_id` is the real session id), which survives the name/id
/// mismatch. A supervisor closing such a task without `bypass_code_review=true`
/// must now drop into the normal verification path; with the flag set, the
/// close proceeds but the audit message cites "supervisor bypass", never
/// "assignee inactive".
#[tokio::test]
async fn test_close_supervisor_active_worker_assignee_by_name() {
    let (temp, service) = setup_cas();
    let _env_lock = env_test_lock();
    let cas_dir = temp.path().join(".cas");
    let task_store = open_task_store(&cas_dir).unwrap();
    let verification_store = open_verification_store(&cas_dir).unwrap();
    let agent_store = open_agent_store(&cas_dir).expect("open agent store");

    // Register a fresh, alive agent with a distinct display name so the
    // id-vs-name mismatch is unambiguous.
    let mut worker = cas::types::Agent::new(
        "test-worker-by-name".to_string(),
        "mighty-viper-99".to_string(),
    );
    worker.agent_type = cas::types::AgentType::Worker;
    worker.role = cas::types::AgentRole::Worker;
    worker.heartbeat(); // ensure fresh last_heartbeat + Active status
    agent_store.register(&worker).expect("register worker");

    let create_req = TaskCreateRequest {
        title: "Task held by a by-name assignee".to_string(),
        description: None,
        priority: 2,
        task_type: "task".to_string(),
        labels: None,
        notes: None,
        blocked_by: None,
        design: None,
        acceptance_criteria: None,
        external_ref: None,
        assignee: None,
        demo_statement: None,
        execution_note: None,
        epic: None,
    };
    let id = extract_task_id(&extract_text(
        service
            .cas_task_create(Parameters(create_req))
            .await
            .expect("task_create should succeed"),
    ))
    .expect("task id")
    .to_string();

    // Store the assignee as the NAME (production bug shape) and put the
    // task in-progress, then claim it on behalf of the worker so the lease
    // carries the real session id.
    let mut task = task_store.get(&id).expect("task exists");
    task.status = cas::types::TaskStatus::InProgress;
    task.assignee = Some("mighty-viper-99".to_string());
    task_store.update(&task).expect("update task");
    agent_store
        .try_claim(&id, &worker.id, 600, Some("worker lease for cas-3bd4 repro"))
        .expect("worker claim should succeed");

    // Flip the caller to supervisor for the close attempt.
    let _guard = ScopedSupervisorEnv::new();

    // --- Attempt 1: no bypass flag. The close MUST drop into the normal
    //     verification path (worker is alive + holding a lease), not the
    //     bypass branch. Pre-fix this path falsely reported the worker as
    //     inactive and closed the task.
    let close_req = TaskCloseRequest {
        id: id.clone(),
        reason: Some("worker finished, asking supervisor to close".to_string()),
        bypass_code_review: None,
        code_review_findings: None,
    };
    let response_text = extract_text(
        service
            .cas_task_close(Parameters(close_req))
            .await
            .expect("task_close returns a result"),
    );
    assert!(
        response_text.contains("VERIFICATION REQUIRED"),
        "active-by-name assignee must NOT trigger inactive bypass — got: {response_text}"
    );
    assert!(
        !response_text.contains("Closed task:"),
        "no bypass flag + active assignee must not transition to Closed: {response_text}"
    );
    assert!(
        !response_text.contains("assignee inactive"),
        "active assignee must never be reported as inactive: {response_text}"
    );
    assert_ne!(
        task_store.get(&id).expect("task exists").status,
        cas::types::TaskStatus::Closed,
        "active assignee + no bypass must leave the task open"
    );

    // --- Attempt 2: with bypass_code_review=true. The close proceeds but
    //     the audit message must cite "supervisor bypass", not "assignee
    //     inactive".
    let close_req = TaskCloseRequest {
        id: id.clone(),
        reason: Some("supervisor forced close after alignment".to_string()),
        bypass_code_review: Some(true),
        code_review_findings: None,
    };
    let response_text = extract_text(
        service
            .cas_task_close(Parameters(close_req))
            .await
            .expect("task_close returns a result"),
    );
    assert!(
        response_text.contains("Closed task:"),
        "supervisor + bypass_code_review must close the task: {response_text}"
    );
    assert!(
        response_text.contains("verification skipped — supervisor bypass"),
        "audit suffix must cite supervisor bypass, not assignee inactive: {response_text}"
    );
    assert!(
        !response_text.contains("assignee inactive"),
        "active assignee must never be reported as inactive even with bypass: {response_text}"
    );
    assert_eq!(
        task_store.get(&id).expect("task exists").status,
        cas::types::TaskStatus::Closed,
        "supervisor bypass must transition task to Closed"
    );

    // Audit trail: the Skipped verification row must record the real
    // reason, not the legacy "assignee inactive or orphaned task" string.
    let row = verification_store
        .get_latest_for_task(&id)
        .expect("verification lookup")
        .expect("supervisor bypass must write a Skipped row");
    assert_eq!(row.status, cas::types::VerificationStatus::Skipped);
    let summary_lc = row.summary.to_lowercase();
    assert!(
        summary_lc.contains("supervisor bypass") && summary_lc.contains("bypass_code_review"),
        "Skipped row summary must name the real reason: {}",
        row.summary
    );
    assert!(
        !summary_lc.contains("inactive") && !summary_lc.contains("orphaned"),
        "Skipped row summary must not inherit the legacy inactive/orphaned wording: {}",
        row.summary
    );
}

/// Negative: supervisor closes a task whose assignee is the currently-alive
/// test agent. `is_heartbeat_expired(300)` is false for a freshly registered
/// agent, so the bypass does NOT fire and close drops into the normal
/// verification path. This pins the bypass to the specific inactive-assignee
/// precondition and proves the hatch isn't a catch-all "supervisor closes
/// anything" escape.
///
/// After cas-4acd narrowed the per-tool jail at server/mod.rs:646-663 to stop
/// exempting `task.close` for factory workers, the jail text returned here
/// comes from `close_ops.rs` (VERIFICATION REQUIRED) — exactly what we assert.
#[tokio::test]
async fn test_close_supervisor_no_bypass_when_assignee_alive() {
    let (temp, service) = setup_cas();
    let _env_lock = env_test_lock();
    let cas_dir = temp.path().join(".cas");
    let task_store = open_task_store(&cas_dir).unwrap();
    let verification_store = open_verification_store(&cas_dir).unwrap();

    // Grab the alive test agent registered by setup_cas.
    let agent_store = open_agent_store(&cas_dir).expect("open agent store");
    let alive_agent_id = agent_store
        .list(None)
        .expect("list agents")
        .first()
        .map(|a| a.id.clone())
        .expect("setup_cas should register a test agent");

    let req = TaskCreateRequest {
        title: "Task with an alive assignee".to_string(),
        description: None,
        priority: 2,
        task_type: "task".to_string(),
        labels: None,
        notes: None,
        blocked_by: None,
        design: None,
        acceptance_criteria: None,
        external_ref: None,
        assignee: None,
        demo_statement: None,
        execution_note: None,
        epic: None,
    };
    let id = extract_task_id(&extract_text(
        service
            .cas_task_create(Parameters(req))
            .await
            .expect("task_create should succeed"),
    ))
    .expect("should have task ID")
    .to_string();

    let mut task = task_store.get(&id).expect("task should exist");
    task.status = cas::types::TaskStatus::InProgress;
    task.assignee = Some(alive_agent_id);
    task_store.update(&task).expect("should update task");

    let _guard = ScopedSupervisorEnv::new();

    let close_req = TaskCloseRequest {
        id: id.clone(),
        // Intentionally still use the "verification skipped" phrase to prove
        // the bypass is structural (assignee state), not reason-driven. Even
        // with this phrase, an alive assignee must keep the jail engaged.
        reason: Some("verification skipped — assignee inactive".to_string()),
        bypass_code_review: None,
code_review_findings: None,
    };
    let response_text = extract_text(
        service
            .cas_task_close(Parameters(close_req))
            .await
            .expect("task_close should return a result"),
    );

    assert!(
        response_text.contains("VERIFICATION REQUIRED"),
        "alive assignee must NOT trigger the bypass — expected VERIFICATION REQUIRED: {response_text}"
    );
    assert!(
        !response_text.contains("Closed task:"),
        "alive assignee path must not report a closed task: {response_text}"
    );

    let task_after = task_store.get(&id).expect("task should exist");
    assert_ne!(
        task_after.status,
        cas::types::TaskStatus::Closed,
        "alive assignee + supervisor must not transition task to Closed"
    );

    // A dispatch-request verification row should have been persisted for the
    // normal path (cas-7de3 regression coverage). This also confirms the
    // close attempt exercised the dispatch branch, not the bypass branch.
    let verification_row = verification_store
        .get_latest_for_task(&id)
        .expect("verification lookup should not error")
        .expect("alive-assignee close should persist a dispatch-request row");
    assert_eq!(
        verification_row.status,
        cas::types::VerificationStatus::Error,
        "dispatch-request row should have Error status until a verdict lands"
    );
}
// =============================================================================
// cas-9a3a: task-verifier spawn regression
//
// These tests lock in the post-cas-4acd contract between the three layers
// involved in verifier dispatch:
//
//   1. `authorize_agent_action` (cas-cli/src/mcp/server/mod.rs) — the narrowed
//      factory-worker exemption. All mutations EXCEPT `task.close` remain
//      exempt for workers; `task.close` falls through to
//      `check_pending_verification`. This preserves the bba6fbf fix for the
//      mutation-cascade problem while restoring the jail lever on the one
//      action that actually triggers verifier dispatch.
//   2. `cas_task_close` (close_ops.rs) — writes a durable dispatch-request
//      Verification row and returns a warning with explicit
//      `Task(subagent_type="task-verifier", prompt="Verify task <id>")` syntax.
//   3. The pre_tool hook (pre_tool.rs:164-242) — on a Task/Agent spawn with
//      subagent_type="task-verifier", clears `pending_verification` for the
//      current agent's jailed tasks. The hook path is exercised end-to-end by
//      `cas-cli/tests/e2e/hook_e2e/jail_core.rs::test_agent_tool_spawns_task_verifier_and_unjails`
//      (feature-gated behind `claude_rs_e2e`; see docs/verifier-dispatch-trace.md).
//      The tests below simulate the post-hook state by clearing
//      `pending_verification` directly and writing an approved Verification
//      row, which is what the hook + task-verifier subagent would have done.
// =============================================================================

/// Guard that installs factory-worker env vars for the duration of a test
/// and clears them on drop. Matches the pattern in `setup_cas()` —
/// cargo test is single-threaded or accepts the race on env vars.
struct FactoryWorkerEnv;

impl FactoryWorkerEnv {
    fn enter() -> Self {
        // SAFETY: see setup_cas() comment — tests accept the race on env vars.
        unsafe {
            std::env::set_var("CAS_AGENT_ROLE", "worker");
            std::env::set_var("CAS_FACTORY_MODE", "1");
        }
        Self
    }
}

impl Drop for FactoryWorkerEnv {
    fn drop(&mut self) {
        unsafe {
            std::env::remove_var("CAS_AGENT_ROLE");
            std::env::remove_var("CAS_FACTORY_MODE");
        }
    }
}

/// Build a TaskRequest with only the fields a test needs, via JSON so we
/// don't have to list every Optional field on the struct.
fn task_req(value: serde_json::Value) -> cas_mcp::TaskRequest {
    serde_json::from_value(value).expect("TaskRequest should deserialize from test JSON")
}

/// Narrowed jail — positive case.
///
/// A factory worker who holds an in-progress task with no approved
/// verification must be blocked by `authorize_agent_action` when they
/// attempt `task.close`. Before cas-4acd this path was exempt and the
/// worker saw a passive warning from close_ops instead; after the fix the
/// MCP layer itself rejects the call with `VERIFICATION_JAIL_BLOCKED` and
/// explicit Task() spawn instructions.
#[tokio::test]
async fn test_factory_worker_close_hits_narrowed_jail() {
    let (temp, core) = setup_cas();
    let _env_lock = env_test_lock();
    let _cas_dir = temp.path().join(".cas");
    let service = CasService::new(core, None);
    let _env = FactoryWorkerEnv::enter();

    // Create and start a task so it's leased + InProgress with no verification.
    let create = task_req(serde_json::json!({
        "action": "create",
        "title": "Factory worker close-path jail regression",
        "priority": 2,
        "task_type": "task",
    }));
    let created = service
        .task(Parameters(create))
        .await
        .expect("task.create should succeed for factory worker");
    let id = extract_task_id(&extract_text(created))
        .expect("should have task ID")
        .to_string();

    let start = task_req(serde_json::json!({ "action": "start", "id": id }));
    service
        .task(Parameters(start))
        .await
        .expect("task.start should succeed — not jailed yet");

    // Attempt to close. Must hit the narrowed jail in authorize_agent_action
    // with an explicit McpError — NOT a soft warning from close_ops.
    let close = task_req(serde_json::json!({
        "action": "close",
        "id": id,
        "reason": "Completed all acceptance criteria. Deployed to prod.",
    }));
    let err = service
        .task(Parameters(close))
        .await
        .expect_err("close must be blocked by the narrowed MCP jail for factory workers");
    let msg = err.message.to_string();
    assert!(
        msg.contains("VERIFICATION_JAIL_BLOCKED"),
        "narrowed jail must return VERIFICATION_JAIL_BLOCKED, got: {msg}"
    );
    // cas-778a: factory workers cannot spawn task-verifier themselves.
    // The jail error for factory workers must recommend forwarding to supervisor
    // via mcp__cas__coordination, NOT the Task() spawn syntax.
    assert!(
        msg.contains("mcp__cas__coordination"),
        "factory worker jail error must recommend mcp__cas__coordination, got: {msg}"
    );
    assert!(
        !msg.contains("Task(subagent_type=\"task-verifier\""),
        "factory worker jail error must NOT instruct spawning task-verifier (workers can't), got: {msg}"
    );
}

/// cas-82d6: a `Skipped` verification row (supervisor bypass audit trail)
/// must satisfy both the MCP jail (`check_pending_verification`) and the
/// close_ops verification gate. Without this, downstream workers that pick
/// up the same task via resumption — or anyone re-closing a task already
/// bypassed — would be trapped by `VERIFICATION_JAIL_BLOCKED`.
#[tokio::test]
async fn test_skipped_verification_row_satisfies_jail_and_close() {
    let (temp, core) = setup_cas();
    let _env_lock = env_test_lock();
    let cas_dir = temp.path().join(".cas");
    let verification_store = open_verification_store(&cas_dir).unwrap();
    let service = CasService::new(core, None);
    let _env = FactoryWorkerEnv::enter();

    // Create + start a task so it's leased + InProgress.
    let created = service
        .task(Parameters(task_req(serde_json::json!({
            "action": "create",
            "title": "Task with a pre-existing Skipped verification row",
            "priority": 2,
            "task_type": "task",
        }))))
        .await
        .expect("create");
    let id = extract_task_id(&extract_text(created))
        .expect("id")
        .to_string();
    service
        .task(Parameters(task_req(serde_json::json!({
            "action": "start",
            "id": id.clone(),
        }))))
        .await
        .expect("start");

    // Insert a Skipped verification row as if a supervisor had previously
    // closed this task via the orphaned-assignee bypass and then it got
    // resumed/reopened.
    let ver_id = verification_store.generate_id().expect("gen ver id");
    let mut row = cas::types::Verification::skipped(
        ver_id,
        id.clone(),
        "cas-82d6 test fixture — supervisor bypass audit row".to_string(),
    );
    row.verification_type = VerificationType::Task;
    verification_store.add(&row).expect("add skipped row");

    // Close as factory worker. Without the cas-82d6 fix this would hit the
    // narrowed MCP jail (check_pending_verification only accepted Approved)
    // OR the close_ops gate (only accepted Approved). With the fix, Skipped
    // is treated as "has verification record → proceed".
    let result = service
        .task(Parameters(task_req(serde_json::json!({
            "action": "close",
            "id": id.clone(),
            "reason": "Completed all acceptance criteria.",
        }))))
        .await
        .expect("close must succeed when a Skipped row exists");
    let text = extract_text(result);
    assert!(
        text.contains("Closed"),
        "close should succeed with Skipped row present, got: {text}"
    );
    assert!(
        !text.contains("VERIFICATION REQUIRED"),
        "Skipped row must satisfy close_ops gate, got: {text}"
    );
    assert!(
        !text.contains("VERIFICATION_JAIL_BLOCKED"),
        "Skipped row must satisfy MCP jail, got: {text}"
    );
}

/// Narrowed jail — negative case (bba6fbf cascade fix preserved).
///
/// The same factory worker holding a jailed task must still be able to
/// perform OTHER mutations (here, `task.update` on an unrelated task).
/// Only `task.close` triggers the jail now.
#[tokio::test]
async fn test_factory_worker_non_close_mutation_still_exempt() {
    let (_temp, core) = setup_cas();
    let _env_lock = env_test_lock();
    let service = CasService::new(core, None);
    let _env = FactoryWorkerEnv::enter();

    // Task A: will be leased + jailed (no verification record).
    let jailed = service
        .task(Parameters(task_req(serde_json::json!({
            "action": "create",
            "title": "Jailed task A",
            "priority": 2,
            "task_type": "task",
        }))))
        .await
        .expect("create A");
    let jailed_id = extract_task_id(&extract_text(jailed))
        .expect("A id")
        .to_string();
    service
        .task(Parameters(task_req(serde_json::json!({
            "action": "start",
            "id": jailed_id.clone(),
        }))))
        .await
        .expect("start A");

    // Task B: unrelated, should still be mutable.
    let other = service
        .task(Parameters(task_req(serde_json::json!({
            "action": "create",
            "title": "Unrelated task B",
            "priority": 2,
            "task_type": "task",
        }))))
        .await
        .expect("create B");
    let other_id = extract_task_id(&extract_text(other))
        .expect("B id")
        .to_string();

    // An update on task B is a mutating action. With the narrowed jail it
    // must still be allowed for a factory worker even though task A is
    // blocking a hypothetical close.
    let update = service
        .task(Parameters(task_req(serde_json::json!({
            "action": "update",
            "id": other_id,
            "priority": 1,
        }))))
        .await
        .expect("non-close mutation must remain exempt from the narrowed jail");
    let update_text = extract_text(update);
    assert!(
        !update_text.contains("VERIFICATION_JAIL_BLOCKED"),
        "update on unrelated task must not be blocked: {update_text}"
    );
}

/// Full happy path: hook clears jail, verifier writes approved row, close
/// succeeds.
///
/// This simulates the post-pre_tool-hook state. The hook path itself is
/// covered by the e2e test noted in the section header; here we lock in
/// that close_ops.rs correctly observes hook-clearance + approved row and
/// completes the close cleanly.
#[tokio::test]
async fn test_task_close_succeeds_after_verifier_clearance() {
    let (temp, core) = setup_cas();
    let _env_lock = env_test_lock();
    let cas_dir = temp.path().join(".cas");
    let task_store = open_task_store(&cas_dir).unwrap();
    let verification_store = open_verification_store(&cas_dir).unwrap();
    let service = CasService::new(core, None);
    let _env = FactoryWorkerEnv::enter();

    let created = service
        .task(Parameters(task_req(serde_json::json!({
            "action": "create",
            "title": "Post-hook clearance happy path",
            "priority": 2,
            "task_type": "task",
        }))))
        .await
        .expect("create");
    let id = extract_task_id(&extract_text(created))
        .expect("id")
        .to_string();
    service
        .task(Parameters(task_req(serde_json::json!({
            "action": "start",
            "id": id.clone(),
        }))))
        .await
        .expect("start");

    // Simulate the pre_tool hook: clear pending_verification on the agent's
    // jailed task. (The real hook sets this flag first when close is
    // attempted; here we bypass that attempt since it's covered by
    // test_factory_worker_close_hits_narrowed_jail above.)
    let mut task = task_store.get(&id).expect("task fetch");
    task.pending_verification = false;
    task.updated_at = chrono::Utc::now();
    task_store.update(&task).expect("clear pending_verification");

    // Simulate the task-verifier subagent writing an approved verification
    // row via mcp__cas__verification add. This is what the hook+subagent
    // sequence produces on a successful verification run.
    let ver = Verification::approved(
        "ver-9a3a-cleared".to_string(),
        id.clone(),
        "Simulated: hook cleared jail, subagent approved work".to_string(),
    );
    verification_store.add(&ver).expect("record approval");

    // Close must now succeed cleanly — the narrowed jail sees an approved
    // verification and lets it through, close_ops records the closure.
    let closed = service
        .task(Parameters(task_req(serde_json::json!({
            "action": "close",
            "id": id.clone(),
            "reason": "Completed after verifier clearance.",
        }))))
        .await
        .expect("close should succeed after hook cleared jail + approved row");
    let close_text = extract_text(closed);
    assert!(
        close_text.to_lowercase().contains("closed"),
        "successful close response must mention closure: {close_text}"
    );

    let final_task = task_store.get(&id).expect("task after close");
    assert_eq!(
        final_task.status,
        cas::types::TaskStatus::Closed,
        "task must be persisted as Closed after the successful close"
    );
}

/// cas-c29a: verification jail within-task deadlock.
///
/// A task enters `pending_verification` on the first close attempt and the
/// dispatch-request row is persisted in `Error` status. If the task-verifier
/// subagent crashes or is never spawned, that row stays stale forever and
/// every close retry returns `VERIFICATION REQUIRED` in a loop.
///
/// This test fabricates a dispatch-request row with a `created_at` older than
/// the 10-minute jail timeout, then calls close again. Expected: close
/// auto-escalates — returns `VERIFICATION TIMED OUT`, clears
/// `pending_verification`, and replaces the stale row with a timeout diagnostic.
#[tokio::test]
async fn test_close_auto_escalates_stale_verification_dispatch() {
    let (temp, service) = setup_cas();
    let _env_lock = env_test_lock();
    let cas_dir = temp.path().join(".cas");

    let verification_store = open_verification_store(&cas_dir).unwrap();
    let task_store = open_task_store(&cas_dir).unwrap();

    // Create + start task.
    let req = TaskCreateRequest {
        title: "Stuck in verification jail".to_string(),
        description: None,
        priority: 2,
        task_type: "task".to_string(),
        labels: None,
        notes: None,
        blocked_by: None,
        design: None,
        acceptance_criteria: None,
        external_ref: None,
        assignee: None,
        demo_statement: None,
        execution_note: None,
        epic: None,
    };
    let result = service
        .cas_task_create(Parameters(req))
        .await
        .expect("task_create");
    let id = extract_task_id(&extract_text(result))
        .expect("task id")
        .to_string();
    let _ = service
        .cas_task_start(Parameters(IdRequest { id: id.clone() }))
        .await
        .expect("task_start");

    // First close — sets pending_verification and writes dispatch-request row.
    let _ = service
        .cas_task_close(Parameters(TaskCloseRequest {
            id: id.clone(),
            reason: Some("Completed".to_string()),
            bypass_code_review: None,
code_review_findings: None,
        }))
        .await
        .expect("first close returns a result");

    let task_after_first = task_store.get(&id).expect("task exists");
    assert!(
        task_after_first.pending_verification,
        "first close must set pending_verification"
    );

    // Age the dispatch row beyond the 10-minute jail timeout.
    let mut dispatch = verification_store
        .get_latest_for_task(&id)
        .expect("get dispatch row")
        .expect("dispatch row exists");
    assert_eq!(dispatch.status, cas::types::VerificationStatus::Error);
    assert!(dispatch.summary.starts_with("Dispatch requested"));
    dispatch.created_at = chrono::Utc::now() - chrono::Duration::seconds(700);
    verification_store
        .update(&dispatch)
        .expect("age dispatch row");

    // Second close — should auto-escalate instead of looping.
    let result = service
        .cas_task_close(Parameters(TaskCloseRequest {
            id: id.clone(),
            reason: Some("Completed".to_string()),
            bypass_code_review: None,
code_review_findings: None,
        }))
        .await
        .expect("second close returns a result");
    let text = extract_text(result);
    assert!(
        text.contains("VERIFICATION TIMED OUT"),
        "retry after timeout must report escalation, got: {text}"
    );
    assert!(
        !text.contains("VERIFICATION REQUIRED"),
        "escalation must not fall back to the standard jail message"
    );

    // pending_verification must be cleared so the task is no longer jailed.
    let task_after_escalation = task_store.get(&id).expect("task exists");
    assert!(
        !task_after_escalation.pending_verification,
        "auto-escalation must clear pending_verification"
    );

    // The dispatch row should have been updated with a timeout diagnostic.
    let timed_out = verification_store
        .get_latest_for_task(&id)
        .expect("get row")
        .expect("row exists");
    assert_eq!(timed_out.status, cas::types::VerificationStatus::Error);
    assert!(
        timed_out.summary.contains("timed out"),
        "stale dispatch row must be rewritten with timeout diagnostic: {}",
        timed_out.summary
    );
}

/// cas-3086: end-to-end. A worker runs cas-code-review, passes the clean
/// ReviewOutcome envelope into `task.close`, and the close is rejected on
/// the verification jail. The envelope must be persisted on the task's
/// deliverables. A follow-up supervisor close — verification already
/// approved, **no** `bypass_code_review=true`, **no** `code_review_findings`
/// replayed — must succeed because the persisted receipt is forwarded
/// into the gate.
///
/// This is the expensive-bypass cycle Report §7 is killing: before this
/// fix, supervisor-close had to either set `bypass_code_review=true`
/// (wrong-shape audit) or re-invoke the multi-persona reviewer ($0.30–0.50
/// per retry) even though the worker had already run the review.
#[tokio::test]
async fn test_close_forwards_persisted_review_envelope_after_jail() {
    use std::process::Command;

    let (temp, service) = setup_cas();
    let _env_lock = env_test_lock();
    let cas_dir = temp.path().join(".cas");
    let task_store = open_task_store(&cas_dir).unwrap();
    let verification_store = open_verification_store(&cas_dir).unwrap();

    // Make the project root (cas_root.parent()) a real git repo with
    // staged code changes so the cas-code-review gate actually fires —
    // otherwise `has_reviewable_changes` returns false and the gate
    // silently skips, which would mask the forwarded-envelope logic.
    let project_root = temp.path();
    let git = |args: &[&str]| {
        let ok = Command::new("git")
            .args(args)
            .current_dir(project_root)
            .env("GIT_AUTHOR_NAME", "t")
            .env("GIT_AUTHOR_EMAIL", "t@t")
            .env("GIT_COMMITTER_NAME", "t")
            .env("GIT_COMMITTER_EMAIL", "t@t")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .status()
            .expect("git")
            .success();
        assert!(ok, "git {args:?} failed");
    };
    git(&["init", "-q", "-b", "main"]);
    std::fs::write(project_root.join("seed.txt"), "seed\n").unwrap();
    git(&["add", "seed.txt"]);
    git(&["commit", "-q", "-m", "seed"]);
    // Stage a real code change so is_reviewable_path returns true.
    std::fs::create_dir_all(project_root.join("src")).unwrap();
    std::fs::write(project_root.join("src/lib.rs"), "fn f() {}\n").unwrap();
    git(&["add", "src/lib.rs"]);

    let req = TaskCreateRequest {
        title: "cas-3086: persisted-envelope forwarding".to_string(),
        description: None,
        priority: 2,
        task_type: "task".to_string(),
        labels: None,
        notes: None,
        blocked_by: None,
        design: None,
        acceptance_criteria: None,
        external_ref: None,
        assignee: None,
        demo_statement: None,
        execution_note: None,
        epic: None,
    };
    let id = extract_task_id(&extract_text(
        service
            .cas_task_create(Parameters(req))
            .await
            .expect("task_create"),
    ))
    .expect("task id")
    .to_string();

    {
        let mut t = task_store.get(&id).expect("task exists");
        t.status = cas::types::TaskStatus::InProgress;
        task_store.update(&t).expect("update");
    }

    // Worker builds a clean envelope (zero residual findings) and hands
    // it in on the first close attempt.
    let clean_envelope = serde_json::json!({
        "residual": [],
        "pre_existing": [],
        "mode": "autofix",
    })
    .to_string();

    let first_close_text = extract_text(
        service
            .cas_task_close(Parameters(TaskCloseRequest {
                id: id.clone(),
                reason: Some("worker ran review, retrying close".to_string()),
                bypass_code_review: None,
                code_review_findings: Some(clean_envelope.clone()),
            }))
            .await
            .expect("close returns"),
    );
    assert!(
        first_close_text.contains("VERIFICATION REQUIRED"),
        "first close must hit verification jail: {first_close_text}"
    );

    // The envelope must have been persisted on the task BEFORE the jail
    // rejection ran. This is the cas-3086 invariant: worker's review
    // receipt survives unrelated close-gate rejections.
    let after_jail = task_store.get(&id).expect("task exists");
    assert_eq!(
        after_jail.deliverables.review_envelope.as_deref(),
        Some(clean_envelope.as_str()),
        "envelope must be persisted even when close is rejected on the verification jail"
    );

    // Simulate the task-verifier subagent writing an approved verdict.
    let ver = Verification::approved(
        "ver-cas-3086".to_string(),
        id.clone(),
        "verified".to_string(),
    );
    verification_store.add(&ver).expect("add verification");

    // Supervisor closes — no bypass_code_review, no code_review_findings.
    // Pre-fix: gate would return CODE_REVIEW_REQUIRED because the
    // request has no envelope and nothing was persisted. Post-fix: the
    // persisted envelope is forwarded, the gate proceeds.
    let _guard = ScopedSupervisorEnv::new();
    let supervisor_close_text = extract_text(
        service
            .cas_task_close(Parameters(TaskCloseRequest {
                id: id.clone(),
                reason: Some("closing on worker's behalf; review already passed".to_string()),
                bypass_code_review: None,
                code_review_findings: None,
            }))
            .await
            .expect("supervisor close returns"),
    );

    assert!(
        supervisor_close_text.contains("Closed"),
        "supervisor close must succeed via forwarded envelope: {supervisor_close_text}"
    );
    assert!(
        !supervisor_close_text.contains("CODE_REVIEW_REQUIRED"),
        "supervisor close must NOT demand a fresh envelope: {supervisor_close_text}"
    );
    assert!(
        !supervisor_close_text.contains("bypass_code_review"),
        "supervisor close should not have needed the bypass path: {supervisor_close_text}"
    );

    let closed = task_store.get(&id).expect("task exists");
    assert_eq!(closed.status, cas::types::TaskStatus::Closed);
}

// =============================================================================
// cas-a90f3: verification.add supervisor authz error message clarity
//
// The original rejection — "Supervisors can only verify epics, not individual
// tasks" — was misleading. Field-confirmed in gabber-studio logs: the rule
// actually depends on whether the task has a *currently live* assignee at
// call time. Several supervisor calls on individual tasks succeed (orphaned,
// dead/expired assignee, supervisor-is-assignee, task-verifier subagent
// context); the rejection only fires for the active-assignee case.
//
// This test pins the new error wording: it must name the rule (active
// assignee), include the offending assignee id, list the three supervisor
// exemptions, and give a concrete remediation path.
// =============================================================================

/// Minimal CasCore rooted in `temp` with a *Supervisor-role* agent
/// pre-set as the current session. `support::setup_cas` always registers a
/// Standard-role agent and pins it via OnceLock, so we can't reuse it for
/// this test — we need the verification-tools authz path to see
/// `agent.role == AgentRole::Supervisor`.
///
/// Mirrors `support::setup_cas`'s factory-env-clearing block (it briefly
/// holds `env_test_lock()` for the mutation, matching the support.rs
/// ordering contract). Callers should `let _env_lock = env_test_lock();`
/// **after** this returns to hold the lock for the test body — std `Mutex`
/// is not re-entrant, so taking it before would deadlock.
///
/// Returns the temp dir guard, the core (used by tests as `service` —
/// MCP tool methods are defined directly on `CasCore`), and the supervisor
/// session id.
fn setup_cas_with_supervisor_session() -> (TempDir, cas::mcp::CasCore, String) {
    // Clear factory env vars under the shared env lock so a parallel
    // sibling test cannot observe a torn read. Match the four vars
    // `support::setup_cas` clears so the two helpers do not drift.
    {
        let _env_guard = env_test_lock();
        // SAFETY: we hold the process-wide env lock for the duration of
        // this block; no other test thread can observe a torn env read.
        unsafe {
            std::env::remove_var("CAS_AGENT_ROLE");
            std::env::remove_var("CAS_FACTORY_MODE");
            std::env::remove_var("CAS_FACTORY_SUPERVISOR_CLI");
            std::env::remove_var("CAS_FACTORY_WORKER_CLI");
        }
    }

    let temp = TempDir::new().expect("temp dir");
    let cas_root = init_cas_dir(temp.path()).expect("init_cas_dir");

    let agent_store = open_agent_store(&cas_root).expect("open agent store");
    let supervisor_id = format!("supervisor-test-cas-a90f3-{}", std::process::id());
    let mut supervisor =
        cas::types::Agent::new(supervisor_id.clone(), "alpha-supervisor".to_string());
    supervisor.role = cas::types::AgentRole::Supervisor;
    supervisor.heartbeat();
    agent_store
        .register(&supervisor)
        .expect("register supervisor");

    let core = cas::mcp::CasCore::with_daemon(cas_root, None, None);
    core.set_agent_id_for_testing(supervisor_id.clone());

    (temp, core, supervisor_id)
}

/// Supervisor calls `verification.add` on a task held by a live worker. The
/// rejection error must:
///   1. Not echo the old "Supervisors can only verify epics" wording
///   2. Name the actual rule (active-assignee precondition)
///   3. Embed the offending assignee id and the task id
///   4. List the three supervisor exemptions (orphaned / inactive / self)
///   5. Provide a concrete remediation (release lease) and clarify that
///      epics may always be verified by supervisors
#[tokio::test]
async fn test_verification_add_supervisor_active_assignee_error_message() {
    // Per support.rs ordering contract: setup helper FIRST (it briefly
    // grabs the lock to clear factory env vars), then acquire the lock
    // for the test body. std `Mutex` is not re-entrant — reversing the
    // order would deadlock. Clearing the factory env vars ensures
    // `worker_harness_from_env()` falls back to Claude (subagents=true)
    // and the supervisor authz branch actually runs.
    let (temp, service, _supervisor_id) = setup_cas_with_supervisor_session();
    let _env_lock = env_test_lock();
    let cas_dir = temp.path().join(".cas");
    let agent_store = open_agent_store(&cas_dir).expect("open agent store");
    let task_store = open_task_store(&cas_dir).expect("open task store");

    // Register a fresh, alive worker — distinct from the supervisor session
    // and freshly heartbeated so `is_alive() && !is_heartbeat_expired(300)`.
    let worker_id = format!("fresh-worker-cas-a90f3-{}", std::process::id());
    let mut worker = cas::types::Agent::new(worker_id.clone(), "wild-cheetah-29".to_string());
    worker.agent_type = cas::types::AgentType::Worker;
    worker.role = cas::types::AgentRole::Worker;
    worker.heartbeat();
    agent_store.register(&worker).expect("register worker");

    // Create a regular (non-Epic) task and assign the live worker to it.
    let create_req = TaskCreateRequest {
        title: "Live worker task — supervisor must not verify behind their back".to_string(),
        description: None,
        priority: 2,
        task_type: "task".to_string(),
        labels: None,
        notes: None,
        blocked_by: None,
        design: None,
        acceptance_criteria: None,
        external_ref: None,
        assignee: None,
        demo_statement: None,
        execution_note: None,
        epic: None,
    };
    let id = extract_task_id(&extract_text(
        service
            .cas_task_create(Parameters(create_req))
            .await
            .expect("task_create should succeed"),
    ))
    .expect("task id")
    .to_string();

    let mut task = task_store.get(&id).expect("task exists");
    task.status = cas::types::TaskStatus::InProgress;
    task.assignee = Some(worker_id.clone());
    task_store.update(&task).expect("update task");

    // Supervisor attempts to add a verification for the worker's task.
    let err = service
        .cas_verification_add(Parameters(VerificationAddRequest {
            task_id: id.clone(),
            status: "approved".to_string(),
            summary: "supervisor trying to verify behind worker's back".to_string(),
            confidence: None,
            issues: None,
            files_reviewed: None,
            duration_ms: None,
            verification_type: None,
        }))
        .await
        .expect_err("verification.add must reject supervisor while worker is alive");

    // (0) The error must remain a client-side INVALID_PARAMS, not an
    //     INTERNAL_ERROR — the latter changes MCP client retry semantics
    //     and operator-facing surfacing.
    assert_eq!(
        err.code,
        rmcp::model::ErrorCode::INVALID_PARAMS,
        "rejection must remain a client error, not server error"
    );

    let msg = err.message.to_string();

    // (1) Old misleading wording must be gone.
    assert!(
        !msg.contains("Supervisors can only verify epics, not individual tasks"),
        "rejection must not use the old misleading wording: {msg}"
    );
    // (2) New wording must name the actual rule.
    assert!(
        msg.contains("active assignee"),
        "rejection must describe the active-assignee rule: {msg}"
    );
    // (3) Embed task + assignee identifiers so the operator knows *which* task
    //     and *who* is blocking.
    assert!(
        msg.contains(&worker_id),
        "rejection must include the offending assignee id ({worker_id}): {msg}"
    );
    assert!(
        msg.contains(&id),
        "rejection must include the task id ({id}): {msg}"
    );
    // (4) List the three exemptions.
    assert!(
        msg.contains("orphaned"),
        "rejection must mention the orphaned-task exemption: {msg}"
    );
    assert!(
        msg.contains("inactive"),
        "rejection must mention the inactive-assignee exemption: {msg}"
    );
    assert!(
        msg.contains("self-implemented") || msg.contains("supervisor IS the assignee"),
        "rejection must mention the supervisor-is-assignee exemption: {msg}"
    );
    // (5) Concrete remediation + epic clarification.
    assert!(
        msg.contains("release") || msg.contains("Release"),
        "rejection must mention the release-lease remediation: {msg}"
    );
    assert!(
        msg.contains("Epics may always be verified"),
        "rejection must clarify that epics are always verifiable by supervisors: {msg}"
    );

    // The check is the only thing we touched; the underlying authz behavior
    // — rejecting the call — must still hold. No verification row should
    // have been written.
    let verification_store = open_verification_store(&cas_dir).expect("verification store");
    let row = verification_store
        .get_latest_for_task(&id)
        .expect("verification lookup");
    assert!(
        row.is_none(),
        "rejected verification.add must NOT persist a verification row: {row:?}"
    );
}

// =============================================================================
// cas-778a: Worker-owned verification via clean ReviewOutcome envelope
//
// Factory workers call cas-code-review (mode=autofix) before closing. The
// resulting ReviewOutcome envelope is the worker's verification step. When the
// envelope is structurally valid and has no P0 in residual or pre_existing,
// close_ops should short-circuit the verification gate and write a Skipped row
// instead of arming the jail (pending_verification=true). Tests go through
// service.cas_task_close directly (bypassing the MCP jail) to isolate close_ops
// behavior.
// =============================================================================

/// A valid ReviewOutcome JSON with empty residual — what a clean cas-code-review
/// run returns after the autofix loop resolves every finding.
const CLEAN_ENVELOPE: &str = r#"{"residual":[],"pre_existing":[],"mode":"autofix"}"#;

/// A ReviewOutcome JSON with a P0 finding in residual — the autofix loop could
/// not resolve a blocker. The verification gate must still arm the jail.
const P0_RESIDUAL_ENVELOPE: &str = r#"{
    "residual": [{
        "title": "Critical security vulnerability",
        "severity": "P0",
        "file": "src/foo.rs",
        "line": 1,
        "why_it_matters": "Allows authentication bypass on the close path",
        "autofix_class": "manual",
        "owner": "human",
        "confidence": 0.95,
        "evidence": ["unsafe { std::mem::transmute(user_id) }"],
        "pre_existing": false
    }],
    "pre_existing": [],
    "mode": "autofix"
}"#;

/// A ReviewOutcome JSON with a P0 finding in residual but with the per-finding
/// `pre_existing: true` flag set — the forgery vector fixed by cas-778a.
///
/// `evaluate_gate()` skips findings with `pre_existing: true`, so without the
/// additional explicit P0 check added to `worker_review_envelope_is_clean`,
/// this envelope would pass both the gate call AND the `pre_existing`-array
/// check and bypass the verification jail. After the fix, it must block.
const P0_RESIDUAL_PRE_EXISTING_TRUE_ENVELOPE: &str = r#"{
    "residual": [{
        "title": "Auth bypass via privilege escalation",
        "severity": "P0",
        "file": "src/auth.rs",
        "line": 42,
        "why_it_matters": "Allows unauthenticated access to admin endpoints",
        "autofix_class": "manual",
        "owner": "human",
        "confidence": 0.95,
        "evidence": ["src/auth.rs:42 — missing role check"],
        "pre_existing": true
    }],
    "pre_existing": [],
    "mode": "autofix"
}"#;

/// cas-778a AC1: factory worker calling cas_task_close with a structurally
/// valid, empty-residual envelope closes successfully without the verification
/// jail being armed.
///
/// Specifically:
/// - The close returns "Closed task:" (not "VERIFICATION REQUIRED")
/// - task.pending_verification is false (jail NOT armed)
/// - A Verification row with status=Skipped and the expected summary is written
/// - The task status is Closed
#[tokio::test]
async fn test_worker_close_with_clean_review_envelope_proceeds() {
    let (temp, service) = setup_cas();
    let _env_lock = env_test_lock();
    let cas_dir = temp.path().join(".cas");
    let task_store = open_task_store(&cas_dir).unwrap();
    let verification_store = open_verification_store(&cas_dir).unwrap();
    let _env = FactoryWorkerEnv::enter();

    // Create a task.
    let req = TaskCreateRequest {
        title: "cas-778a: worker-owned verification happy path".to_string(),
        description: None,
        priority: 2,
        task_type: "task".to_string(),
        labels: None,
        notes: None,
        blocked_by: None,
        design: None,
        acceptance_criteria: None,
        external_ref: None,
        assignee: None,
        demo_statement: None,
        execution_note: None,
        epic: None,
    };
    let create_text = extract_text(
        service
            .cas_task_create(Parameters(req))
            .await
            .expect("task_create should succeed"),
    );
    let id = extract_task_id(&create_text)
        .expect("should have task ID")
        .to_string();

    // Start the task (sets InProgress + active lease).
    service
        .cas_task_start(Parameters(IdRequest { id: id.clone() }))
        .await
        .expect("task_start should succeed");

    // Close with a clean ReviewOutcome envelope. The verification gate
    // should short-circuit, write a Skipped row, and proceed with close.
    let close_req = TaskCloseRequest {
        id: id.clone(),
        reason: Some("All acceptance criteria met. cas-code-review autofix returned clean envelope.".to_string()),
        bypass_code_review: None,
        code_review_findings: Some(CLEAN_ENVELOPE.to_string()),
    };
    let result = service
        .cas_task_close(Parameters(close_req))
        .await
        .expect("task_close should return a result");
    let text = extract_text(result);

    assert!(
        text.contains("Closed task:"),
        "close with clean envelope must succeed: {text}"
    );
    assert!(
        !text.contains("VERIFICATION REQUIRED"),
        "clean envelope must not trigger VERIFICATION REQUIRED: {text}"
    );

    // Jail must NOT have been armed.
    let task = task_store.get(&id).expect("task should exist");
    assert!(
        !task.pending_verification,
        "pending_verification must be false — jail must not be armed for clean envelope"
    );
    assert_eq!(
        task.status,
        cas::types::TaskStatus::Closed,
        "task must be Closed after worker-owned verification close"
    );

    // A Skipped verification row must have been written for the audit trail.
    let ver = verification_store
        .get_latest_for_task(&id)
        .expect("verification store lookup should succeed")
        .expect("a Skipped verification row must exist after worker-owned verification close");
    assert_eq!(
        ver.status,
        cas::types::VerificationStatus::Skipped,
        "verification row status must be Skipped, got: {:?}",
        ver.status
    );
    assert!(
        ver.summary.contains("Worker-owned verification"),
        "Skipped row summary must mention 'Worker-owned verification': {}",
        ver.summary
    );

    // The envelope must be persisted to task deliverables for the downstream
    // code_review_gate's second-pass re-validation.
    let refreshed_task = task_store.get(&id).expect("task should exist after close");
    assert_eq!(
        refreshed_task.deliverables.review_envelope.as_deref(),
        Some(CLEAN_ENVELOPE),
        "review_envelope must be persisted to task deliverables for downstream gate re-validation"
    );
}

/// cas-778a P0 forgery-fix: factory worker close with a P0 in residual[] that
/// carries `pre_existing: true` on the *per-finding* field must NOT short-
/// circuit. Before the fix, `evaluate_gate()` would skip such a finding
/// (treating it as baseline noise), making the residual appear clean.
/// After the fix, `worker_review_envelope_is_clean` explicitly rejects any P0
/// in residual regardless of the per-finding flag.
#[tokio::test]
async fn test_worker_close_with_p0_residual_pre_existing_true_still_blocked() {
    let (temp, service) = setup_cas();
    let _env_lock = env_test_lock();
    let cas_dir = temp.path().join(".cas");
    let task_store = open_task_store(&cas_dir).unwrap();
    let _env = FactoryWorkerEnv::enter();

    let req = TaskCreateRequest {
        title: "cas-778a: P0-in-residual with pre_existing=true must block".to_string(),
        description: None,
        priority: 2,
        task_type: "task".to_string(),
        labels: None,
        notes: None,
        blocked_by: None,
        design: None,
        acceptance_criteria: None,
        external_ref: None,
        assignee: None,
        demo_statement: None,
        execution_note: None,
        epic: None,
    };
    let create_text = extract_text(
        service
            .cas_task_create(Parameters(req))
            .await
            .expect("task_create should succeed"),
    );
    let id = extract_task_id(&create_text)
        .expect("should have task ID")
        .to_string();

    service
        .cas_task_start(Parameters(IdRequest { id: id.clone() }))
        .await
        .expect("task_start should succeed");

    // Close with the forgery envelope: P0 in residual[] with pre_existing=true.
    // The bypass must be blocked — a P0 is a P0 regardless of per-finding flag.
    let close_req = TaskCloseRequest {
        id: id.clone(),
        reason: Some("Done (hiding P0 via pre_existing=true forgery)".to_string()),
        bypass_code_review: None,
        code_review_findings: Some(P0_RESIDUAL_PRE_EXISTING_TRUE_ENVELOPE.to_string()),
    };
    let result = service
        .cas_task_close(Parameters(close_req))
        .await
        .expect("task_close should return a result");
    let text = extract_text(result);

    assert!(
        text.contains("VERIFICATION REQUIRED"),
        "P0-in-residual with pre_existing=true must still require verification: {text}"
    );
    assert!(
        !text.contains("Closed task:"),
        "forgery envelope must NOT allow close to succeed: {text}"
    );

    // Jail must be armed.
    let task = task_store.get(&id).expect("task should exist");
    assert!(
        task.pending_verification,
        "pending_verification must be true — jail must be armed for forgery envelope"
    );
    assert_ne!(
        task.status,
        cas::types::TaskStatus::Closed,
        "task must NOT be Closed when the forgery envelope is rejected"
    );
}

/// cas-778a AC2: factory worker close with a P0 in residual is NOT short-
/// circuited — the verification gate must still arm the jail and return
/// VERIFICATION REQUIRED, just as before the fix.
#[tokio::test]
async fn test_worker_close_with_p0_residual_still_blocked() {
    let (temp, service) = setup_cas();
    let _env_lock = env_test_lock();
    let cas_dir = temp.path().join(".cas");
    let task_store = open_task_store(&cas_dir).unwrap();
    let _env = FactoryWorkerEnv::enter();

    let req = TaskCreateRequest {
        title: "cas-778a: worker P0 residual must still block".to_string(),
        description: None,
        priority: 2,
        task_type: "task".to_string(),
        labels: None,
        notes: None,
        blocked_by: None,
        design: None,
        acceptance_criteria: None,
        external_ref: None,
        assignee: None,
        demo_statement: None,
        execution_note: None,
        epic: None,
    };
    let create_text = extract_text(
        service
            .cas_task_create(Parameters(req))
            .await
            .expect("task_create should succeed"),
    );
    let id = extract_task_id(&create_text)
        .expect("should have task ID")
        .to_string();

    service
        .cas_task_start(Parameters(IdRequest { id: id.clone() }))
        .await
        .expect("task_start should succeed");

    // Close with an envelope that has a P0 in residual. The gate must
    // NOT short-circuit — verification is required.
    let close_req = TaskCloseRequest {
        id: id.clone(),
        reason: Some("Done (but has P0 issue)".to_string()),
        bypass_code_review: None,
        code_review_findings: Some(P0_RESIDUAL_ENVELOPE.to_string()),
    };
    let result = service
        .cas_task_close(Parameters(close_req))
        .await
        .expect("task_close should return a result");
    let text = extract_text(result);

    assert!(
        text.contains("VERIFICATION REQUIRED"),
        "P0-in-residual envelope must still require verification: {text}"
    );
    assert!(
        !text.contains("Closed task:"),
        "close with P0 envelope must NOT succeed: {text}"
    );

    // Jail must be armed (pending_verification=true).
    let task = task_store.get(&id).expect("task should exist");
    assert!(
        task.pending_verification,
        "pending_verification must be true — jail must be armed for P0 envelope"
    );
    assert_ne!(
        task.status,
        cas::types::TaskStatus::Closed,
        "task must NOT be Closed when verification is required"
    );
}

/// cas-778a AC3: factory worker close with a malformed (non-JSON) envelope is
/// NOT short-circuited — the verification gate must still arm the jail.
#[tokio::test]
async fn test_worker_close_with_malformed_envelope_still_blocked() {
    let (temp, service) = setup_cas();
    let _env_lock = env_test_lock();
    let cas_dir = temp.path().join(".cas");
    let task_store = open_task_store(&cas_dir).unwrap();
    let _env = FactoryWorkerEnv::enter();

    let req = TaskCreateRequest {
        title: "cas-778a: worker malformed envelope must still block".to_string(),
        description: None,
        priority: 2,
        task_type: "task".to_string(),
        labels: None,
        notes: None,
        blocked_by: None,
        design: None,
        acceptance_criteria: None,
        external_ref: None,
        assignee: None,
        demo_statement: None,
        execution_note: None,
        epic: None,
    };
    let create_text = extract_text(
        service
            .cas_task_create(Parameters(req))
            .await
            .expect("task_create should succeed"),
    );
    let id = extract_task_id(&create_text)
        .expect("should have task ID")
        .to_string();

    service
        .cas_task_start(Parameters(IdRequest { id: id.clone() }))
        .await
        .expect("task_start should succeed");

    // Close with malformed JSON. The gate must NOT short-circuit.
    let close_req = TaskCloseRequest {
        id: id.clone(),
        reason: Some("Done (but envelope is garbage)".to_string()),
        bypass_code_review: None,
        code_review_findings: Some("{not valid json at all".to_string()),
    };
    let result = service
        .cas_task_close(Parameters(close_req))
        .await
        .expect("task_close should return a result");
    let text = extract_text(result);

    assert!(
        text.contains("VERIFICATION REQUIRED"),
        "malformed envelope must still require verification: {text}"
    );
    assert!(
        !text.contains("Closed task:"),
        "close with malformed envelope must NOT succeed: {text}"
    );

    // Jail must be armed.
    let task = task_store.get(&id).expect("task should exist");
    assert!(
        task.pending_verification,
        "pending_verification must be true — jail must be armed for malformed envelope"
    );
    assert_ne!(
        task.status,
        cas::types::TaskStatus::Closed,
        "task must NOT be Closed when verification is required"
    );
}
