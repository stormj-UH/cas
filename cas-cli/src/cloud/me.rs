//! `/api/me` fetch + cache — T2 of EPIC cas-ab88.
//!
//! Calls `GET {endpoint}/api/me`, parses the `teams[]` and
//! `default_team_id` fields, and writes them into the user-level
//! `~/.cas/cloud.json` (or the path pointed to by `CAS_USER_CLOUD_JSON`
//! when set — the same test seam used by T3 and T4).
//!
//! # Failure model
//!
//! All callers treat the fetch as best-effort:
//! - Network errors → `FetchTeamsOutcome::NetworkError(msg)` — caller logs a warning.
//! - `401 Unauthorized` → `FetchTeamsOutcome::AuthFailed` — token is stale; caller warns.
//! - `200 OK` with `"teams": []` → `FetchTeamsOutcome::Empty` — valid, zero-membership user.
//! - `200 OK` with teams → `FetchTeamsOutcome::Updated { team_count }`.
//!
//! The function **never** propagates an error that would interrupt `cas login`
//! or `cas cloud sync`; all failure branches return a typed outcome instead.

use std::path::Path;

use chrono::Utc;
use tracing::warn;

use super::config::user_level_cloud_json_path;
use crate::cloud::{CloudConfig, TeamInfo};

/// Outcome of a `/api/me` fetch-and-cache attempt.
#[derive(Debug, PartialEq, Eq)]
pub enum FetchTeamsOutcome {
    /// Fetch succeeded and `team_count` team records were stored.
    Updated { team_count: usize },
    /// Fetch succeeded but the user belongs to zero teams.
    Empty,
    /// Server returned 401 — token is invalid or expired.
    AuthFailed,
    /// Network or parse failure; `msg` carries a human-readable reason.
    NetworkError(String),
}

/// Call `/api/me`, cache the resulting `teams[]` + `default_team_id` into the
/// user-level config, and return a typed outcome.
///
/// `endpoint` must not have a trailing slash. `token` is the Bearer token.
///
/// The user config path is resolved via `user_level_cloud_json_path()` (honours
/// the `CAS_USER_CLOUD_JSON` test-seam env var).  Returns
/// `FetchTeamsOutcome::NetworkError` when the path cannot be determined.
///
/// This is the public entry point for production callers (login paths, lazy
/// refresh in sync).  Tests that need path injection use
/// `fetch_and_cache_teams_inner` directly.
pub fn fetch_and_cache_teams(endpoint: &str, token: &str) -> FetchTeamsOutcome {
    match user_level_cloud_json_path() {
        Some(path) => {
            let cas_dir = match path.parent() {
                Some(d) => d.to_path_buf(),
                None => {
                    return FetchTeamsOutcome::NetworkError(
                        "cannot determine user .cas directory from cloud.json path".to_string(),
                    );
                }
            };
            fetch_and_cache_teams_inner(endpoint, token, &cas_dir)
        }
        None => FetchTeamsOutcome::NetworkError(
            "cannot determine home directory to locate user cloud config".to_string(),
        ),
    }
}

/// Testable inner implementation — accepts an injected `user_cas_dir` so
/// integration tests can point it at a tempdir instead of `~/.cas/`.
///
/// Reads the existing user config from `<user_cas_dir>/cloud.json` (if any),
/// merges in the fresh `teams[]` and `default_team_id` from the server, stamps
/// `teams_fetched_at`, and writes the result back.  All other config fields
/// (token, endpoint, team_id, …) are preserved.
pub fn fetch_and_cache_teams_inner(
    endpoint: &str,
    token: &str,
    user_cas_dir: &Path,
) -> FetchTeamsOutcome {
    let url = format!("{endpoint}/api/me");

    let response = ureq::get(&url)
        .set("Authorization", &format!("Bearer {token}"))
        .timeout(std::time::Duration::from_secs(10))
        .call();

    match response {
        Ok(resp) => {
            let body: serde_json::Value = match resp.into_json() {
                Ok(v) => v,
                Err(e) => {
                    return FetchTeamsOutcome::NetworkError(format!(
                        "/api/me response could not be parsed as JSON: {e}"
                    ));
                }
            };

            // Parse teams[].
            let teams: Vec<TeamInfo> = body
                .get("teams")
                .and_then(|v| serde_json::from_value(v.clone()).ok())
                .unwrap_or_default();

            // Parse optional default_team_id (may be null or absent).
            let default_team_id: Option<String> = body
                .get("default_team_id")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());

            let team_count = teams.len();

            // Read-modify-write user config.
            if let Err(e) = update_user_config(user_cas_dir, teams, default_team_id) {
                warn!(
                    error = %e,
                    user_cas_dir = %user_cas_dir.display(),
                    "failed to write teams to user cloud config after /api/me fetch"
                );
                return FetchTeamsOutcome::NetworkError(format!(
                    "could not persist teams to user config: {e}"
                ));
            }

            if team_count == 0 {
                FetchTeamsOutcome::Empty
            } else {
                FetchTeamsOutcome::Updated { team_count }
            }
        }

        Err(ureq::Error::Status(401, _)) => FetchTeamsOutcome::AuthFailed,

        Err(ureq::Error::Status(code, resp)) => {
            let body = resp.into_string().unwrap_or_default();
            FetchTeamsOutcome::NetworkError(format!(
                "/api/me returned HTTP {code}: {body}"
            ))
        }

        Err(ureq::Error::Transport(e)) => {
            FetchTeamsOutcome::NetworkError(format!("/api/me network error: {e}"))
        }
    }
}

/// Read `<user_cas_dir>/cloud.json` (falling back to default if absent),
/// replace `teams`, `default_team_id`, and stamp `teams_fetched_at`, then
/// write back.  All other fields are preserved.
fn update_user_config(
    user_cas_dir: &Path,
    teams: Vec<TeamInfo>,
    default_team_id: Option<String>,
) -> Result<(), crate::error::CasError> {
    let mut cfg = CloudConfig::load_from_cas_dir(user_cas_dir)?;
    cfg.teams = teams;
    // Only overwrite default_team_id when the server provides one; if the
    // server returns null and the user already has a local preference from
    // `cas cloud team default`, preserve the local preference.
    if default_team_id.is_some() {
        cfg.default_team_id = default_team_id;
    }
    cfg.teams_fetched_at = Some(Utc::now());

    // Ensure the directory exists before writing.
    if let Some(parent) = user_cas_dir.parent() {
        std::fs::create_dir_all(parent).ok();
    }
    std::fs::create_dir_all(user_cas_dir).ok();

    cfg.save_to_cas_dir(user_cas_dir)
}

/// Returns `true` when a fresh `/api/me` call is warranted:
///   - `teams` is empty (never fetched), OR
///   - `teams_fetched_at` is `None`, OR
///   - the last fetch was more than `max_age_secs` seconds ago.
///
/// Callers pass `86_400` (24 h) for the lazy-refresh path in `execute_sync`.
pub fn teams_cache_stale(cfg: &CloudConfig, max_age_secs: i64) -> bool {
    if cfg.teams.is_empty() {
        return true;
    }
    match cfg.teams_fetched_at {
        None => true,
        Some(fetched_at) => {
            let age = Utc::now().signed_duration_since(fetched_at);
            age.num_seconds() >= max_age_secs
        }
    }
}
