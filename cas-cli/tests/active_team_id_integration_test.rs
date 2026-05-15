//! Integration tests for the active_team_id() user-level resolution chain
//! (cas-ea2f5, T3 of EPIC cas-ab88).
//!
//! Verifies that when a project-level cloud.json has no `team_id` but a
//! user-level cloud.json (simulated via the `CAS_USER_CLOUD_JSON` env-var
//! override) has `default_team_id` set, `open_store` dual-enqueues writes to
//! the team queue — i.e. `active_team_id()` surfaces the user default and the
//! syncing store picks it up at open time.

use std::sync::Mutex;

use cas::cloud::{CloudConfig, SyncQueue};
use cas::store::open_store;
use cas::types::{Entry, EntryType, Scope};
use tempfile::TempDir;

/// Serialises all mutations of `CAS_USER_CLOUD_JSON` across threads within
/// this test binary (each integration test file compiles to its own binary,
/// so this mutex is sufficient for intra-file serialisation).
static USER_CLOUD_LOCK: Mutex<()> = Mutex::new(());

/// Team UUID written to user-level cloud.json as `default_team_id`.
const USER_DEFAULT_TEAM: &str = "user-default-0000-0000-000000000001";

/// Build a project-level CloudConfig with a token (so `is_logged_in()` is
/// true) but without `team_id` — simulates a fresh checkout where the user
/// hasn't run `cas cloud team set`.
fn make_project_cloud_config_no_team() -> CloudConfig {
    let mut cfg = CloudConfig::default();
    cfg.endpoint = "http://localhost:0".to_string();
    cfg.token = Some("test-token".to_string());
    // team_id intentionally absent
    cfg
}

/// Build a user-level CloudConfig with `default_team_id` set.
fn make_user_cloud_config_with_default(default_team_id: &str) -> CloudConfig {
    let mut cfg = CloudConfig::default();
    cfg.default_team_id = Some(default_team_id.to_string());
    cfg
}

fn queue_len_for_team(cas_dir: &std::path::Path, team_id: &str) -> usize {
    let queue = SyncQueue::open(cas_dir).unwrap();
    queue.init().unwrap();
    queue
        .pending_for_team(team_id, 1000, 10)
        .map(|rows| rows.len())
        .unwrap_or(0)
}

/// When the project-level cloud.json has no team_id but the user-level
/// cloud.json sets default_team_id, `store.add()` (the `cas remember` path)
/// dual-enqueues to the team queue.
#[test]
fn remember_dual_enqueues_via_user_level_default_team_id() {
    let _guard = USER_CLOUD_LOCK.lock().unwrap_or_else(|p| p.into_inner());

    // Create a temp dir for the user-level ~/.cas/cloud.json substitute.
    let user_home_temp = TempDir::new().unwrap();
    let user_cloud_json = user_home_temp.path().join("cloud.json");
    make_user_cloud_config_with_default(USER_DEFAULT_TEAM)
        .save_to(&user_cloud_json)
        .unwrap();

    // Point active_team_id() at our controlled user-level cloud.json.
    // SAFETY: serialised by USER_CLOUD_LOCK; no concurrent env mutation.
    unsafe { std::env::set_var("CAS_USER_CLOUD_JSON", &user_cloud_json); }

    // Create a project cas_dir with a token but no team_id.
    let project_temp = TempDir::new().unwrap();
    make_project_cloud_config_no_team()
        .save_to_cas_dir(project_temp.path())
        .unwrap();

    // open_store calls active_team_id() at construction time.
    // Because the project config has no team_id, the resolution chain
    // falls through to the user-level default_team_id set above.
    let store = open_store(project_temp.path()).expect("open_store must succeed");

    // Add a project-scoped entry — SyncingEntryStore should dual-enqueue it
    // to the team queue because active_team_id() returned USER_DEFAULT_TEAM.
    let entry = Entry {
        id: "2026-05-15-t3-integration".to_string(),
        scope: Scope::Project,
        entry_type: EntryType::Learning,
        content: "T3 integration: user-level default_team_id".to_string(),
        ..Default::default()
    };
    store.add(&entry).expect("store.add must succeed");

    // Verify the entry landed in the team queue for USER_DEFAULT_TEAM.
    let queue_rows = queue_len_for_team(project_temp.path(), USER_DEFAULT_TEAM);
    assert!(
        queue_rows > 0,
        "expected ≥1 row in team queue for {USER_DEFAULT_TEAM}, got {queue_rows}"
    );

    unsafe { std::env::remove_var("CAS_USER_CLOUD_JSON"); }
}

/// Project-level team_id wins over user-level default_team_id — the store
/// should enqueue to the project team, not the user default.
#[test]
fn project_team_id_beats_user_default_in_open_store() {
    let _guard = USER_CLOUD_LOCK.lock().unwrap_or_else(|p| p.into_inner());

    const PROJECT_TEAM: &str = "project-team-0000-0000-000000000002";

    let user_home_temp = TempDir::new().unwrap();
    let user_cloud_json = user_home_temp.path().join("cloud.json");
    make_user_cloud_config_with_default(USER_DEFAULT_TEAM)
        .save_to(&user_cloud_json)
        .unwrap();

    unsafe { std::env::set_var("CAS_USER_CLOUD_JSON", &user_cloud_json); }

    let project_temp = TempDir::new().unwrap();
    // Build a project config WITH an explicit team_id.
    let mut project_cfg = make_project_cloud_config_no_team();
    project_cfg.set_team(PROJECT_TEAM, "project-team");
    project_cfg.save_to_cas_dir(project_temp.path()).unwrap();

    let store = open_store(project_temp.path()).expect("open_store must succeed");

    let entry = Entry {
        id: "2026-05-15-t3-project-override".to_string(),
        scope: Scope::Project,
        entry_type: EntryType::Learning,
        content: "T3 integration: project override".to_string(),
        ..Default::default()
    };
    store.add(&entry).expect("store.add must succeed");

    // Entry lands in the PROJECT_TEAM queue, not the user default.
    let project_rows = queue_len_for_team(project_temp.path(), PROJECT_TEAM);
    let user_rows = queue_len_for_team(project_temp.path(), USER_DEFAULT_TEAM);
    assert!(
        project_rows > 0,
        "expected ≥1 row in project team queue {PROJECT_TEAM}, got {project_rows}"
    );
    assert_eq!(
        user_rows, 0,
        "user-default team queue must stay empty when project team_id is set"
    );

    unsafe { std::env::remove_var("CAS_USER_CLOUD_JSON"); }
}
