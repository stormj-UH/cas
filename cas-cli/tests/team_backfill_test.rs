//! Integration tests for T6 — first-run backfill UX after upgrade (cas-8f23).
//!
//! Exercises `cas::cloud::maybe_apply_team_backfill_inner` — the testable
//! inner seam that reads/writes user-level cloud.json in a tempdir.
//!
//! Also covers the `cas cloud team default --personal` → sets
//! `team_backfill_notified = true` behaviour that guards the backfill gate.
//!
//! Lives in `cas-cli/tests/` per the cas-1f44 pattern (integration behavior
//! for user-visible UX belongs in the integration tree, not colocated with
//! the module under test).

use std::sync::Mutex;

use cas::cloud::{BackfillOutcome, CloudConfig, TeamInfo, maybe_apply_team_backfill_inner};
use cas::cli::cloud::{CloudTeamDefaultArgs, execute_team_default_for_test};
use cas::cli::Cli;
use tempfile::TempDir;

/// Serialises `CAS_USER_CLOUD_JSON` mutations within this test binary.
static USER_CLOUD_LOCK: Mutex<()> = Mutex::new(());

// ─── helpers ────────────────────────────────────────────────────────────────

fn make_cli_json() -> Cli {
    Cli { json: true, full: false, verbose: false, command: None }
}

fn make_team(id: &str, slug: &str, name: &str) -> TeamInfo {
    TeamInfo {
        id: id.to_string(),
        slug: slug.to_string(),
        name: name.to_string(),
        role: "member".to_string(),
    }
}

fn write_user_config(cas_dir: &std::path::Path, cfg: &CloudConfig) {
    cfg.save_to_cas_dir(cas_dir).unwrap();
}

fn read_user_config(cas_dir: &std::path::Path) -> CloudConfig {
    CloudConfig::load_from_cas_dir(cas_dir).unwrap()
}

// ─── tests ──────────────────────────────────────────────────────────────────

/// Single-team user with no default set → auto-picks the sole team, marks notified.
#[test]
fn backfill_auto_sets_sole_team_and_marks_notified() {
    let temp = TempDir::new().unwrap();
    let mut cfg = CloudConfig::default();
    cfg.teams = vec![make_team("tid-solo", "solo-team", "Solo Team")];
    // default_team_id and team_backfill_notified both at default (None / false).
    write_user_config(temp.path(), &cfg);

    let outcome = maybe_apply_team_backfill_inner(temp.path());

    match outcome {
        BackfillOutcome::Applied { ref team_id, ref team_slug, .. } => {
            assert_eq!(team_id, "tid-solo");
            assert_eq!(team_slug, "solo-team");
        }
        other => panic!("expected Applied, got {other:?}"),
    }

    let saved = read_user_config(temp.path());
    assert_eq!(saved.default_team_id.as_deref(), Some("tid-solo"),
        "default_team_id must be persisted");
    assert!(saved.team_backfill_notified, "team_backfill_notified must be true after backfill");
}

/// Already-notified user → no-op regardless of team state.
#[test]
fn backfill_no_op_when_already_notified() {
    let temp = TempDir::new().unwrap();
    let mut cfg = CloudConfig::default();
    cfg.teams = vec![make_team("tid-1", "team-a", "Team A")];
    cfg.team_backfill_notified = true;
    write_user_config(temp.path(), &cfg);

    let outcome = maybe_apply_team_backfill_inner(temp.path());
    assert_eq!(outcome, BackfillOutcome::AlreadyNotified);

    // Config must be unchanged.
    let saved = read_user_config(temp.path());
    assert_eq!(saved.default_team_id, None);
}

/// User with zero team memberships → no notice.
#[test]
fn backfill_no_op_when_no_teams() {
    let temp = TempDir::new().unwrap();
    let cfg = CloudConfig::default(); // teams = []
    write_user_config(temp.path(), &cfg);

    let outcome = maybe_apply_team_backfill_inner(temp.path());
    assert_eq!(outcome, BackfillOutcome::NoMembership);

    let saved = read_user_config(temp.path());
    assert!(!saved.team_backfill_notified);
}

/// Multi-team user with no default → ambiguous, no auto-set.
#[test]
fn backfill_no_auto_set_on_multi_team_no_default() {
    let temp = TempDir::new().unwrap();
    let mut cfg = CloudConfig::default();
    cfg.teams = vec![
        make_team("tid-a", "team-a", "Team A"),
        make_team("tid-b", "team-b", "Team B"),
    ];
    write_user_config(temp.path(), &cfg);

    let outcome = maybe_apply_team_backfill_inner(temp.path());
    assert_eq!(outcome, BackfillOutcome::MultiTeamAmbiguous);

    let saved = read_user_config(temp.path());
    assert_eq!(saved.default_team_id, None, "must not auto-set when ambiguous");
    // We do NOT mark notified on ambiguous — a future `cas cloud team default`
    // + sync should still surface naturally without needing a forced re-run.
    assert!(!saved.team_backfill_notified);
}

/// Server already populated default_team_id (via T2 me.rs) — T6 shows the
/// notice and marks notified, without overwriting the already-correct value.
#[test]
fn backfill_notifies_when_default_already_set_by_server() {
    let temp = TempDir::new().unwrap();
    let mut cfg = CloudConfig::default();
    cfg.teams = vec![
        make_team("tid-a", "team-a", "Team A"),
        make_team("tid-b", "team-b", "Team B"),
    ];
    cfg.default_team_id = Some("tid-a".to_string()); // set by T2 server response
    write_user_config(temp.path(), &cfg);

    let outcome = maybe_apply_team_backfill_inner(temp.path());
    match outcome {
        BackfillOutcome::Applied { ref team_id, .. } => {
            assert_eq!(team_id, "tid-a");
        }
        other => panic!("expected Applied, got {other:?}"),
    }

    let saved = read_user_config(temp.path());
    assert_eq!(saved.default_team_id.as_deref(), Some("tid-a"),
        "existing default_team_id must be preserved");
    assert!(saved.team_backfill_notified);
}

/// After login, `fetch_and_cache_teams` writes `teams[]` into user config.
/// A subsequent `maybe_apply_team_backfill_inner` call (the same code path
/// that auth.rs calls via the production wrapper) must auto-promote the sole
/// team and mark the user as notified — matching what the wired login path does.
#[test]
fn backfill_fires_after_login_team_fetch() {
    let temp = TempDir::new().unwrap();

    // Simulate what fetch_and_cache_teams writes after a successful device-flow
    // login: teams[] populated, default_team_id still None (T2 fills teams but
    // leaves default_team_id for T6 to set).
    let mut cfg = CloudConfig::default();
    cfg.teams = vec![make_team("tid-login", "login-team", "Login Team")];
    // default_team_id and team_backfill_notified both at default (None / false).
    write_user_config(temp.path(), &cfg);

    // This is what the login path calls (via the production wrapper which
    // resolves user_cas_dir from CAS_USER_CLOUD_JSON; here we test the inner
    // seam directly to stay hermetic).
    let outcome = maybe_apply_team_backfill_inner(temp.path());

    match outcome {
        BackfillOutcome::Applied { ref team_id, ref team_slug, ref team_name } => {
            assert_eq!(team_id, "tid-login");
            assert_eq!(team_slug, "login-team");
            assert_eq!(team_name, "Login Team");
        }
        other => panic!("expected Applied after login-path backfill, got {other:?}"),
    }

    let saved = read_user_config(temp.path());
    assert_eq!(saved.default_team_id.as_deref(), Some("tid-login"),
        "login backfill must persist default_team_id");
    assert!(saved.team_backfill_notified,
        "login backfill must set team_backfill_notified=true");

    // Idempotency: a second call (e.g., re-login) must be a no-op.
    let outcome2 = maybe_apply_team_backfill_inner(temp.path());
    assert_eq!(outcome2, BackfillOutcome::AlreadyNotified,
        "second backfill call must be AlreadyNotified — must not re-fire on re-login");
}

/// `cas cloud team default --personal` must set `team_backfill_notified=true`
/// so the backfill never fires and overrides the explicit personal-scope choice.
#[test]
fn personal_flag_blocks_future_backfill() {
    let _guard = USER_CLOUD_LOCK.lock().unwrap_or_else(|p| p.into_inner());

    let temp = TempDir::new().unwrap();
    let mut cfg = CloudConfig::default();
    cfg.teams = vec![make_team("tid-solo", "solo-team", "Solo Team")];
    cfg.default_team_id = Some("tid-solo".to_string());
    write_user_config(temp.path(), &cfg);

    // User explicitly reverts to personal scope.
    execute_team_default_for_test(
        &CloudTeamDefaultArgs { slug_or_uuid: None, personal: true },
        &make_cli_json(),
        &temp.path().to_path_buf(),
    ).expect("--personal must succeed");

    let saved = read_user_config(temp.path());
    assert_eq!(saved.default_team_id, None, "--personal must clear default_team_id");
    assert!(saved.team_backfill_notified,
        "--personal must set team_backfill_notified=true to block future auto-backfill");

    // Running the backfill now must be a no-op.
    let outcome = maybe_apply_team_backfill_inner(temp.path());
    assert_eq!(outcome, BackfillOutcome::AlreadyNotified);
    // default_team_id must stay None.
    let saved2 = read_user_config(temp.path());
    assert_eq!(saved2.default_team_id, None);
}
