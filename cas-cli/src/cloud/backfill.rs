//! First-run backfill UX — T6 of EPIC cas-ab88.
//!
//! On the first `cas cloud sync` after upgrade, when the user has team
//! membership (populated by T2's `/api/me` fetch) but no explicit
//! `default_team_id` configured, this module auto-promotes to the sole team
//! (or uses the server-supplied default) and prints a one-time notice.
//!
//! # One-time gate
//!
//! `CloudConfig::team_backfill_notified` (user-level `~/.cas/cloud.json`)
//! is set to `true` after the notice fires, and also when the user runs
//! `cas cloud team default --personal`.  Once set, the backfill is
//! permanently a no-op for that user.

use std::path::Path;

use crate::cloud::config::user_level_cloud_json_path;
use crate::cloud::CloudConfig;

/// Outcome of [`maybe_apply_team_backfill_inner`].
#[derive(Debug, PartialEq, Eq)]
pub enum BackfillOutcome {
    /// Backfill auto-picked the default team (single-team user whose
    /// `default_team_id` was `None`).  The caller should print the
    /// first-run notice — this is the "we just changed something" path.
    AppliedSetDefault {
        /// The team UUID that was just set as the default.
        team_id: String,
        /// URL-safe slug (for display in the notice).
        team_slug: String,
        /// Display name (for display in the notice).
        team_name: String,
    },
    /// `default_team_id` was already set by the server (via T2 `/api/me`).
    /// We just marked `team_backfill_notified = true`; nothing changed for
    /// the user — no notice needed.
    AppliedAlreadyDefault,
    /// `team_backfill_notified` was already `true` — no-op.
    AlreadyNotified,
    /// `teams[]` is empty — user has no team membership; no notice.
    NoMembership,
    /// User belongs to multiple teams and the server supplied no
    /// `default_team_id` — cannot auto-pick; no notice, no auto-set.
    MultiTeamAmbiguous,
}

/// Testable inner implementation — accepts an injected `user_cas_dir` so
/// integration tests can point it at a tempdir instead of `~/.cas/`.
///
/// Reads user-level config, applies backfill logic, writes back if changed,
/// and returns a typed outcome.  The caller (production wrapper or
/// `execute_sync`) is responsible for printing any user-visible text.
pub fn maybe_apply_team_backfill_inner(user_cas_dir: &Path) -> BackfillOutcome {
    let mut cfg = match CloudConfig::load_from_cas_dir(user_cas_dir) {
        Ok(c) => c,
        Err(_) => CloudConfig::default(),
    };

    // One-time gate — also fires when user ran `--personal`.
    if cfg.team_backfill_notified {
        return BackfillOutcome::AlreadyNotified;
    }

    // No membership → nothing to promote.
    if cfg.teams.is_empty() {
        return BackfillOutcome::NoMembership;
    }

    // Determine the outcome variant and whether we need to mutate anything.
    if let Some(ref dtid) = cfg.default_team_id.clone() {
        // Server already set a default (via T2 me.rs) — just mark notified
        // and return the silent variant.  Nothing changed from the user's
        // perspective; no first-run notice needed.
        cfg.team_backfill_notified = true;
        if let Err(e) = std::fs::create_dir_all(user_cas_dir) {
            tracing::warn!(error = %e, "backfill: could not create user_cas_dir");
        }
        let _ = cfg.save_to_cas_dir(user_cas_dir);
        let _ = dtid; // explicit: we read it above only to detect presence
        BackfillOutcome::AppliedAlreadyDefault
    } else if cfg.teams.len() == 1 {
        // Implicit single-team auto-pick — we are setting the default for the
        // first time.  Print the first-run notice.
        let t = cfg.teams[0].clone();
        cfg.default_team_id = Some(t.id.clone());
        cfg.team_backfill_notified = true;
        if let Err(e) = std::fs::create_dir_all(user_cas_dir) {
            tracing::warn!(error = %e, "backfill: could not create user_cas_dir");
        }
        let _ = cfg.save_to_cas_dir(user_cas_dir);
        BackfillOutcome::AppliedSetDefault {
            team_id: t.id,
            team_slug: t.slug,
            team_name: t.name,
        }
    } else {
        // Multiple teams, no server default → ambiguous; no auto-set.
        BackfillOutcome::MultiTeamAmbiguous
    }
}

/// Production wrapper — resolves `~/.cas/` (or `CAS_USER_CLOUD_JSON` test
/// seam) and delegates to [`maybe_apply_team_backfill_inner`].
///
/// Returns `BackfillOutcome::NoMembership` when the home directory cannot
/// be determined (treated as "nothing to do").
pub fn maybe_apply_team_backfill() -> BackfillOutcome {
    match user_level_cloud_json_path() {
        Some(path) => match path.parent() {
            Some(dir) => maybe_apply_team_backfill_inner(dir),
            None => BackfillOutcome::NoMembership,
        },
        None => BackfillOutcome::NoMembership,
    }
}
