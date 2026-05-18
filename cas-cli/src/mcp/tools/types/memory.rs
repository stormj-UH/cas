use rmcp::schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::mcp::tools::types::defaults::{
    default_entry_type, default_importance, default_recent, default_scope_project,
};

#[derive(Debug, Deserialize, JsonSchema)]
pub struct RememberRequest {
    /// The content to remember
    #[schemars(
        description = "The content to remember. Can be a fact, preference, context, or observation."
    )]
    pub content: String,

    /// Entry type
    #[schemars(
        description = "Type of memory: 'learning' (default), 'preference', 'context', or 'observation'"
    )]
    #[serde(default = "default_entry_type")]
    pub entry_type: String,

    /// Optional tags for categorization
    #[schemars(
        description = "Comma-separated tags for categorization (e.g., 'rust,cli,important')"
    )]
    #[serde(default)]
    pub tags: Option<String>,

    /// Optional title
    #[schemars(description = "Optional short title for the entry")]
    #[serde(default)]
    pub title: Option<String>,

    /// Importance score
    #[schemars(description = "Importance score from 0.0 to 1.0 (default: 0.5)")]
    #[serde(default = "default_importance")]
    pub importance: f32,

    /// Storage scope
    #[schemars(
        description = "Scope: 'global' (user prefs, general learnings) or 'project' (default, project-specific context)"
    )]
    #[serde(default = "default_scope_project")]
    pub scope: String,

    /// Valid from timestamp (RFC3339)
    #[schemars(description = "When this fact becomes valid (RFC3339 format)")]
    #[serde(default)]
    pub valid_from: Option<String>,

    /// Valid until timestamp (RFC3339)
    #[schemars(description = "When this fact expires (RFC3339 format)")]
    #[serde(default)]
    pub valid_until: Option<String>,

    /// Team ID for team-scoped entries
    #[schemars(description = "Team ID to share this entry with a team")]
    #[serde(default)]
    pub team_id: Option<String>,

    /// Skip pre-insert overlap detection. Reserved for bulk imports and
    /// tests that intentionally create overlapping memories. Normal callers
    /// should leave this unset so duplicates are caught at creation time.
    #[schemars(
        description = "Skip overlap detection (bulk imports / tests only — defaults to false)"
    )]
    #[serde(default)]
    pub bypass_overlap: Option<bool>,

    /// Overlap-handling mode. Phase 1 supports `"interactive"` (default) —
    /// on high-overlap the call returns a structured `Blocked` response and
    /// the caller decides what to do. `"autofix"` is reserved for Phase 2
    /// (auto-update of the existing memory) and currently returns an
    /// explicit "not supported" error.
    #[schemars(description = "Overlap handling mode: 'interactive' (default) | 'autofix' (reserved, Phase 2)")]
    #[serde(default)]
    pub mode: Option<String>,

    /// Force a personal (non-team) note even in a team-linked project.
    ///
    /// By default, `cas remember` in a project that has an active team will
    /// automatically scope the entry to that team so other members receive it
    /// on their next pull (`team_auto_promote`). Set `personal=true` to opt
    /// out for a one-off private note that stays in your personal sync queue.
    ///
    /// Ignored when `team_id` is set explicitly — an explicit `team_id` always
    /// wins regardless of this flag.
    #[schemars(
        description = "Set true to keep the note personal (skip team auto-promote) even in a team-linked project"
    )]
    #[serde(default)]
    pub personal: Option<bool>,
}

// ============================================================================
// MemoryRememberResponse (cas-e382)
//
// Structured response returned by `mcp__cas__memory action=remember`. The
// JSON payload is carried on `CallToolResult::structured_content` so that
// agents can pattern-match on the tagged `status` field without parsing the
// free-text message. A human-readable text block is also included for
// legacy text-only clients.
// ============================================================================

/// Per-dimension overlap score breakdown returned inside `Blocked`.
/// Mirrors [`cas_core::memory::DimensionScores`] but lives in the MCP
/// response layer so the public wire format is decoupled from the internal
/// scoring type.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DimensionBreakdown {
    pub problem_statement: u8,
    pub root_cause: u8,
    pub solution_approach: u8,
    pub referenced_files: u8,
    pub tags: u8,
    /// Combined module + track mismatch penalty. Always ≤ 0.
    pub penalty: i8,
    /// Net score after penalty, floored at 0. Ranges 0..=5.
    pub net: u8,
}

/// Tagged-union response shape for `action=remember`.
///
/// Phase 1 emits two variants: `Created` (covers low + moderate overlap,
/// with `refresh_recommended` as a flag for the cross-ref-cap case) and
/// `Blocked` (high overlap, score 4–5). Phase 2 may add new variants for
/// `mode=autofix` outcomes — the serde tagged enum shape makes that
/// backwards-compatible as long as new variants use new `status` tag
/// names, so we do not reserve placeholder variants here.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum MemoryRememberResponse {
    /// The memory was successfully inserted. `related_memories` is empty
    /// for a low-overlap insert, or populated with the slugs of each
    /// cross-referenced match on a moderate-overlap insert. When at least
    /// one of those matches has already hit the 3-link cap, the
    /// `refresh_recommended` flag is set so the caller knows to surface a
    /// refresh prompt.
    Created {
        slug: String,
        related_memories: Vec<String>,
        refresh_recommended: bool,
    },

    /// The memory was blocked because a high-overlap match (score 4–5)
    /// already exists. The caller should follow `recommendation` (typically
    /// update the existing entry in place). `other_high_scoring` carries
    /// any additional slugs that also scored 4+ (empty in the common case).
    Blocked {
        reason: BlockReason,
        existing_slug: String,
        dimension_scores: DimensionBreakdown,
        recommended_action: RecommendedAction,
        other_high_scoring: Vec<String>,
    },
}

/// Reason a memory insert was blocked. Currently only `HighOverlap` is
/// emitted; the enum exists so future block reasons (e.g. quota, validation)
/// fit the same wire shape.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum BlockReason {
    HighOverlap,
}

/// Recommended follow-up action when a block is returned. Mirrors
/// [`cas_core::memory::OverlapRecommendation`] with a stable wire name.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RecommendedAction {
    UpdateExisting,
    SurfaceForUserDecision,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct RecentRequest {
    /// Number of entries
    #[schemars(description = "Number of recent entries to return (default: 10)")]
    #[serde(default = "default_recent")]
    pub n: usize,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct MemoryTierRequest {
    /// Entry ID
    #[schemars(description = "ID of the entry")]
    pub id: String,

    /// Memory tier
    #[schemars(description = "Memory tier: 'working', 'cold', or 'archive'")]
    pub tier: String,
}
