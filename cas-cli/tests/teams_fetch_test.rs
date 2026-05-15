//! Integration tests for T2 of EPIC cas-ab88:
//! `fetch_and_cache_teams_inner` + `/api/me` wiring.
//!
//! Tests use:
//! - `wiremock` to stand up a local HTTP server that mimics `/api/me`.
//! - An injected `TempDir` as the user cas dir (via
//!   `fetch_and_cache_teams_inner`) so `~/.cas/cloud.json` is never touched.
//!
//! Coverage:
//! 1. Happy path — `/api/me` returns `teams[]` and `default_team_id`;
//!    verify both are persisted to the user cas dir and
//!    `FetchTeamsOutcome::Updated { team_count }` is returned.
//! 2. Empty teams — `/api/me` returns `"teams": []` (zero memberships);
//!    verify `FetchTeamsOutcome::Empty` and `teams` is written as empty.
//! 3. 401 Unauthorized — verify `FetchTeamsOutcome::AuthFailed`; no panic,
//!    user config on disk is unchanged.
//! 4. `teams_cache_stale` helper — fresh timestamp is not stale, absent
//!    timestamp is stale, old timestamp is stale.

use cas::cloud::{CloudConfig, FetchTeamsOutcome, fetch_and_cache_teams_inner, teams_cache_stale};
use cas::cloud::TeamInfo;
use tempfile::TempDir;
use wiremock::matchers::{header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

// ── Happy path ───────────────────────────────────────────────────────────────

/// AC: when `/api/me` returns a team list, the teams are persisted to the
/// user-level cloud.json and `FetchTeamsOutcome::Updated { team_count }` is
/// returned.
#[tokio::test]
async fn fetch_teams_happy_path_persists_teams_to_user_config() {
    let server = MockServer::start().await;

    Mock::given(method("GET"))
        .and(path("/api/me"))
        .and(header("Authorization", "Bearer test-tok"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "user_id": "uid-001",
            "email": "alice@example.com",
            "teams": [
                {
                    "id":   "team-aaa",
                    "slug": "alpha-squad",
                    "name": "Alpha Squad",
                    "role": "owner"
                },
                {
                    "id":   "team-bbb",
                    "slug": "beta-squad",
                    "name": "Beta Squad",
                    "role": "member"
                }
            ],
            "default_team_id": "team-aaa"
        })))
        .expect(1)
        .mount(&server)
        .await;

    let tmp = TempDir::new().unwrap();
    let outcome = fetch_and_cache_teams_inner(&server.uri(), "test-tok", tmp.path());

    assert_eq!(
        outcome,
        FetchTeamsOutcome::Updated { team_count: 2 },
        "expected Updated with team_count 2"
    );

    // Verify teams were written to the user cas dir.
    let cfg = CloudConfig::load_from_cas_dir(tmp.path()).unwrap();
    assert_eq!(
        cfg.teams.len(),
        2,
        "two teams must be persisted"
    );
    assert_eq!(
        cfg.teams[0],
        TeamInfo {
            id:   "team-aaa".to_string(),
            slug: "alpha-squad".to_string(),
            name: "Alpha Squad".to_string(),
            role: "owner".to_string(),
        }
    );
    assert_eq!(
        cfg.default_team_id,
        Some("team-aaa".to_string()),
        "default_team_id from server must be persisted"
    );
    assert!(
        cfg.teams_fetched_at.is_some(),
        "teams_fetched_at must be set after a successful fetch"
    );

    server.verify().await;
}

// ── Empty teams ──────────────────────────────────────────────────────────────

/// AC: when `/api/me` returns `"teams": []`, `FetchTeamsOutcome::Empty` is
/// returned and an empty teams list is persisted (no panic).
#[tokio::test]
async fn fetch_teams_empty_membership_returns_empty_outcome() {
    let server = MockServer::start().await;

    Mock::given(method("GET"))
        .and(path("/api/me"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "user_id": "uid-002",
            "email":   "bob@example.com",
            "teams":   [],
            "default_team_id": null
        })))
        .expect(1)
        .mount(&server)
        .await;

    let tmp = TempDir::new().unwrap();
    let outcome = fetch_and_cache_teams_inner(&server.uri(), "tok-empty", tmp.path());

    assert_eq!(outcome, FetchTeamsOutcome::Empty);

    let cfg = CloudConfig::load_from_cas_dir(tmp.path()).unwrap();
    assert!(
        cfg.teams.is_empty(),
        "teams must be empty after zero-membership response"
    );
    assert!(
        cfg.teams_fetched_at.is_some(),
        "teams_fetched_at must be stamped even for empty response"
    );

    server.verify().await;
}

// ── 401 Unauthorized ─────────────────────────────────────────────────────────

/// AC: when `/api/me` returns 401, `FetchTeamsOutcome::AuthFailed` is returned
/// without panicking and the user config is unchanged (still default).
#[tokio::test]
async fn fetch_teams_401_returns_auth_failed() {
    let server = MockServer::start().await;

    Mock::given(method("GET"))
        .and(path("/api/me"))
        .respond_with(
            ResponseTemplate::new(401)
                .set_body_json(serde_json::json!({ "error": "invalid token" })),
        )
        .expect(1)
        .mount(&server)
        .await;

    let tmp = TempDir::new().unwrap();
    // Pre-seed with a pre-existing (non-empty) config to prove it's unchanged.
    let mut pre = CloudConfig::default();
    pre.teams = vec![TeamInfo {
        id:   "pre-existing-team".to_string(),
        slug: "pre".to_string(),
        name: "Pre".to_string(),
        role: "member".to_string(),
    }];
    pre.save_to_cas_dir(tmp.path()).unwrap();

    let outcome = fetch_and_cache_teams_inner(&server.uri(), "bad-token", tmp.path());

    assert_eq!(
        outcome,
        FetchTeamsOutcome::AuthFailed,
        "401 must produce AuthFailed"
    );

    // Confirm the pre-existing config was NOT touched.
    let cfg = CloudConfig::load_from_cas_dir(tmp.path()).unwrap();
    assert_eq!(
        cfg.teams.len(),
        1,
        "pre-existing teams must be unchanged after 401"
    );

    server.verify().await;
}

// ── teams_cache_stale unit tests ─────────────────────────────────────────────

/// A config with no teams and no `teams_fetched_at` is stale.
#[test]
fn teams_cache_stale_when_no_teams_and_no_timestamp() {
    let cfg = CloudConfig::default();
    assert!(
        teams_cache_stale(&cfg, 86_400),
        "empty teams with no timestamp must be stale"
    );
}

/// A config with teams but no `teams_fetched_at` is stale.
#[test]
fn teams_cache_stale_when_teams_but_no_timestamp() {
    let mut cfg = CloudConfig::default();
    cfg.teams = vec![TeamInfo {
        id:   "t1".to_string(),
        slug: "s1".to_string(),
        name: "S1".to_string(),
        role: "member".to_string(),
    }];
    // teams_fetched_at is None by default.
    assert!(
        teams_cache_stale(&cfg, 86_400),
        "teams present but no timestamp must be stale"
    );
}

/// A config with a recent `teams_fetched_at` is NOT stale.
#[test]
fn teams_cache_not_stale_when_freshly_fetched() {
    let mut cfg = CloudConfig::default();
    cfg.teams = vec![TeamInfo {
        id:   "t2".to_string(),
        slug: "s2".to_string(),
        name: "S2".to_string(),
        role: "owner".to_string(),
    }];
    cfg.teams_fetched_at = Some(chrono::Utc::now());
    assert!(
        !teams_cache_stale(&cfg, 86_400),
        "recently-fetched teams must NOT be stale"
    );
}

/// A config whose `teams_fetched_at` is older than the threshold is stale.
#[test]
fn teams_cache_stale_after_expiry() {
    let mut cfg = CloudConfig::default();
    cfg.teams = vec![TeamInfo {
        id:   "t3".to_string(),
        slug: "s3".to_string(),
        name: "S3".to_string(),
        role: "member".to_string(),
    }];
    // Stamp a time 25 h ago.
    cfg.teams_fetched_at = Some(chrono::Utc::now() - chrono::Duration::hours(25));
    assert!(
        teams_cache_stale(&cfg, 86_400),
        "teams fetched >24 h ago must be stale"
    );
}
