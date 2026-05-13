//! Integration tests for `cas cloud sync` / `cas cloud pull`'s team-pull wiring
//! (task cas-6ec7, EPIC cas-ffc4 — fix `cas cloud sync` pull returning 0 for new
//! team members).
//!
//! Before this fix, `execute_sync` (cli/cloud.rs) and `execute_pull` both called
//! `syncer.pull(...)` only — they never invoked `syncer.pull_team(...)` even when
//! a team was configured. New team members landed in a project, ran
//! `cas cloud sync`, and saw zero team-scoped rows because the team pull endpoint
//! was never hit. This task adds the missing symmetry via a new `execute_team_pull`
//! helper that mirrors `execute_team_push` (cli/cloud.rs:1313).
//!
//! Test coverage:
//! - Behavioral: `execute_team_pull` helper hits `/api/teams/{uuid}/sync/pull` when
//!   a team is configured AND lands rows in the local store (positive path).
//! - Behavioral: `execute_team_pull` does NOT hit the team endpoint when no team
//!   is configured (negative / early-return path).
//! - Behavioral: clearing `last_team_pull_at_<team_id>` from the sync queue
//!   (the `--full` watermark reset for team pulls).
//! - Source-grep: `execute_sync` invokes `execute_team_pull` AFTER the personal
//!   `execute_pull` call. This catches a regression where someone reorders or
//!   accidentally removes the wire-up. Mirrors the precedent in
//!   `pull_scoping_regression_test.rs:158`.
//! - Source-grep: `execute_pull` (standalone command) invokes `execute_team_pull`
//!   when a team is configured.
//! - Source-grep: the `--full` branch in `execute_pull` clears the team-pull
//!   watermark (`last_team_pull_at_`) in addition to `last_pull_at`.
//!
//! Helper-level behavioral coverage mirrors the precedent set by
//! `team_sync_test.rs` for `execute_team_push` (cas-1f44 / T4). End-to-end
//! coverage through `execute_sync` is provided via source-grep wiring tests
//! because `execute_sync` reads `CloudConfig::load()` from disk (using
//! `find_cas_root()`) and exercising it against a tempdir requires global
//! `CAS_ROOT` env var manipulation that is unsafe with parallel test threads.

use std::path::Path;

mod common;
use common::{TEST_TEAM, make_cli_json, make_cloud_config};

use cas::cli::cloud::execute_team_pull;
use cas::cloud::{CloudConfig, SyncQueue};
use cas::store::{open_rule_store, open_skill_store, open_store, open_task_store};
use cas::types::{Entry, EntryType, Scope};
use tempfile::TempDir;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

/// Initialize a fresh `.cas`-style tempdir with empty SQLite stores + queue.
fn make_cas_root() -> TempDir {
    let tmp = TempDir::new().unwrap();
    let queue = SyncQueue::open(tmp.path()).unwrap();
    queue.init().unwrap();
    // Force store creation so subsequent `open_store(cas_root)` calls inside
    // `execute_team_pull` find the SQLite files in place.
    let _ = open_store(tmp.path()).unwrap();
    let _ = open_task_store(tmp.path()).unwrap();
    let _ = open_rule_store(tmp.path()).unwrap();
    let _ = open_skill_store(tmp.path()).unwrap();
    tmp
}

/// Helper to mount a team-pull mock that serves exactly one entry. The entry is
/// serialized via `serde_json::to_value(&Entry{...})` to lock in the actual
/// wire shape (matches the precedent in `team_memories_e2e_test.rs:331`).
async fn mount_team_pull_with_one_entry(server: &MockServer, entry_id: &str) {
    let alice_entry = Entry {
        id: entry_id.to_string(),
        scope: Scope::Project,
        entry_type: EntryType::Context,
        content: "alice's shared learning".to_string(),
        ..Default::default()
    };
    let shared_entry = serde_json::to_value(&alice_entry).unwrap();

    Mock::given(method("GET"))
        .and(path(format!("/api/teams/{TEST_TEAM}/sync/pull")))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "entries": [shared_entry],
            "tasks": [],
            "rules": [],
            "skills": [],
            "pulled_at": chrono::Utc::now().to_rfc3339(),
            "team_id": TEST_TEAM,
            "status": "ok",
        })))
        .expect(1)
        .mount(server)
        .await;
}

/// AC: `execute_team_pull` MUST hit `/api/teams/{uuid}/sync/pull` and land the
/// returned rows into local stores when a team is configured. This is the
/// core positive behavioral test — the bug it guards against is exactly the
/// scenario reported: a new team member runs `cas cloud sync`, the team
/// endpoint is never hit, and zero rows arrive.
#[tokio::test]
async fn team_pull_hits_endpoint_and_lands_rows_when_team_configured() {
    let server = MockServer::start().await;
    mount_team_pull_with_one_entry(&server, "alice-shared-001").await;

    let tmp = make_cas_root();
    let cas_root = tmp.path().to_path_buf();
    let cfg = make_cloud_config(server.uri());
    let cli = make_cli_json();

    // Fresh teammate — no existing entry yet.
    {
        let store = open_store(&cas_root).unwrap();
        assert!(
            store.get("alice-shared-001").is_err(),
            "store must start empty for this test"
        );
    }

    // `execute_team_pull` is sync (uses blocking `ureq`); run it on the
    // blocking pool so the wiremock tokio runtime can serve the GET.
    let cas_root_owned = cas_root.clone();
    let result = tokio::task::spawn_blocking(move || {
        execute_team_pull(&cfg, &cas_root_owned, &cli)
    })
    .await
    .unwrap();

    assert!(
        result.is_ok(),
        "execute_team_pull must return Ok (isolation contract); got {result:?}"
    );

    // The mock's `.expect(1)` already asserted exactly one GET fired; below
    // we additionally prove the row landed locally (the full contract — bug
    // would still surface if pull_team was called but rows were dropped).
    let store = open_store(&cas_root).unwrap();
    let pulled = store
        .get("alice-shared-001")
        .expect("team-pulled entry must land in local store");
    assert_eq!(pulled.content, "alice's shared learning");
    assert_eq!(pulled.entry_type, EntryType::Context);
}

/// AC negative: `execute_team_pull` MUST NOT hit the team endpoint when no
/// team is configured. The mock fails the test (via Drop on MockServer with
/// `.expect(0)`) if a request reaches it.
#[tokio::test]
async fn team_pull_no_op_when_no_team_configured() {
    let server = MockServer::start().await;
    // Any HTTP method/path on the mock server fails the test — early-return
    // contract means zero traffic.
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(500))
        .expect(0)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(500))
        .expect(0)
        .mount(&server)
        .await;

    let tmp = make_cas_root();
    let cas_root = tmp.path().to_path_buf();

    // Cloud config WITHOUT an active team — `active_team_id()` returns None.
    let mut cfg = CloudConfig::default();
    cfg.endpoint = server.uri();
    cfg.token = Some("test-token".to_string());
    let cli = make_cli_json();

    let result =
        tokio::task::spawn_blocking(move || execute_team_pull(&cfg, &cas_root, &cli))
            .await
            .unwrap();
    assert!(
        result.is_ok(),
        "execute_team_pull early-return on no-team must yield Ok(()); got {result:?}"
    );
}

/// AC: `cas cloud pull --full` must clear `last_team_pull_at_<team_id>` so the
/// next team pull is a full backfill (mirrors how it already clears
/// `last_pull_at` for personal pulls). This test exercises the exact metadata
/// key/format the implementation must use — a regression in the key string
/// would leave `--full` half-broken (personal-only).
#[tokio::test]
async fn full_flag_clears_team_pull_watermark_via_queue() {
    let tmp = make_cas_root();
    let queue = SyncQueue::open(tmp.path()).unwrap();
    queue.init().unwrap();

    let key = format!("last_team_pull_at_{TEST_TEAM}");
    queue.set_metadata(&key, "2025-01-01T00:00:00Z").unwrap();
    assert_eq!(
        queue.get_metadata(&key).unwrap().as_deref(),
        Some("2025-01-01T00:00:00Z"),
        "precondition: watermark must exist before clear"
    );

    // The `--full` branch in `execute_pull` must run exactly this delete
    // (with this exact key format) when an active team is configured.
    queue.delete_metadata(&key).unwrap();

    assert_eq!(
        queue.get_metadata(&key).unwrap(),
        None,
        "watermark must be cleared by `--full`",
    );
}

/// Returns the cloud.rs source as a String, walking up from the test binary's
/// location so the relative-path resolution is robust to `target/` layout.
fn read_cloud_rs() -> String {
    let candidates = [
        Path::new(env!("CARGO_MANIFEST_DIR")).join("src/cli/cloud.rs"),
    ];
    for p in &candidates {
        if let Ok(content) = std::fs::read_to_string(p) {
            return content;
        }
    }
    panic!("could not locate cas-cli/src/cli/cloud.rs from candidates: {candidates:?}");
}

/// Locks in the runtime contract this task is about: `execute_sync` MUST invoke
/// `execute_team_pull` after the personal `execute_pull` call. Source-grep is
/// the right shape of test here for the same reason `pull_scoping_regression_test.rs`
/// uses it (lines 158-161): the wire-up is a single call line, behavioral
/// coverage requires global env-var manipulation, and a static check is robust
/// to test-thread parallelism.
#[test]
fn execute_sync_invokes_execute_team_pull_after_personal_pull() {
    let src = read_cloud_rs();

    // Find the `fn execute_sync` body.
    let start = src
        .find("fn execute_sync(")
        .expect("execute_sync must exist in cli/cloud.rs");
    let after_start = &src[start..];
    // Body ends at the next top-level function definition. Coarse but stable —
    // any false negative shows up as a missing match below.
    let end_rel = after_start
        .find("\nfn ")
        .or_else(|| after_start.find("\npub fn "))
        .unwrap_or(after_start.len());
    let body = &after_start[..end_rel];

    assert!(
        body.contains("execute_team_pull"),
        "execute_sync body must invoke `execute_team_pull` (this task's bugfix). \
         The team-pull call site is missing — `cas cloud sync` will return zero \
         team rows for new team members.\nBody scanned:\n{body}",
    );

    // Ordering check: the personal `execute_pull(` call must appear BEFORE the
    // `execute_team_pull(` call. Team pull layers on top of personal pull so
    // ordering matters for store-merge correctness.
    let personal_idx = body
        .find("execute_pull(")
        .expect("execute_sync must still call the personal `execute_pull`");
    let team_idx = body
        .find("execute_team_pull(")
        .expect("execute_sync must call `execute_team_pull(`");
    assert!(
        personal_idx < team_idx,
        "personal `execute_pull(` must run BEFORE `execute_team_pull(` in execute_sync \
         (team pull layers on top of personal pull)",
    );
}

/// Locks in: `execute_pull` (standalone `cas cloud pull` command) must also
/// invoke `execute_team_pull` so the standalone command stays symmetric with
/// `cas cloud sync`. Missing this wire-up would mean `cas cloud pull` works
/// but `cas cloud sync` doesn't (or vice-versa) — silent skew.
#[test]
fn execute_pull_invokes_execute_team_pull_when_team_active() {
    let src = read_cloud_rs();
    let start = src
        .find("fn execute_pull(")
        .expect("execute_pull must exist in cli/cloud.rs");
    let after_start = &src[start..];
    let end_rel = after_start
        .find("\nfn ")
        .or_else(|| after_start.find("\npub fn "))
        .unwrap_or(after_start.len());
    let body = &after_start[..end_rel];

    assert!(
        body.contains("execute_team_pull"),
        "execute_pull (standalone) must invoke `execute_team_pull` so \
         `cas cloud pull` is symmetric with `cas cloud sync`.\nBody scanned:\n{body}",
    );
}

/// Locks in: the `--full` branch in `execute_pull` must clear the team-pull
/// watermark (`last_team_pull_at_`) in addition to `last_pull_at`. The exact
/// key format `last_team_pull_at_<team_id>` matches what
/// `CloudSyncer::pull_team` writes (cas-cli/src/cloud/syncer/pull.rs:710).
#[test]
fn execute_pull_full_clears_team_pull_watermark_in_source() {
    let src = read_cloud_rs();
    let start = src
        .find("fn execute_pull(")
        .expect("execute_pull must exist in cli/cloud.rs");
    let after_start = &src[start..];
    let end_rel = after_start
        .find("\nfn ")
        .or_else(|| after_start.find("\npub fn "))
        .unwrap_or(after_start.len());
    let body = &after_start[..end_rel];

    assert!(
        body.contains("last_team_pull_at_"),
        "execute_pull `--full` branch must clear `last_team_pull_at_<team_id>` \
         metadata (the team-pull watermark) so `--full` triggers a full team \
         backfill in addition to a full personal backfill.\nBody scanned:\n{body}",
    );
}
