//! Cloud sync commands for CAS
//!
//! Enables syncing CAS data with CAS Cloud service.

use clap::{Parser, Subcommand};
use std::io;
use std::path::Path;
use std::time::Duration;

use crate::cli::Cli;
use crate::cloud::{CloudConfig, get_project_canonical_id};
use crate::ui::components::Formatter;
use crate::ui::theme::ActiveTheme;

use crate::store::{
    SqliteStore, open_commit_link_store, open_event_store, open_file_change_store,
    open_prompt_store, open_rule_store, open_skill_store, open_spec_store, open_store,
    open_task_store, open_worktree_store,
};

#[derive(Subcommand)]
pub enum CloudCommands {
    /// Show cloud sync status
    Status,
    /// Show sync queue (pending changes)
    Queue(CloudQueueArgs),
    /// Push local data to cloud
    Push(CloudPushArgs),
    /// Pull data from cloud
    Pull(CloudPullArgs),
    /// Full sync (push then pull)
    Sync(CloudSyncArgs),
    /// Configure the active team for team-scoped sync operations
    #[command(subcommand)]
    Team(CloudTeamCommands),
    /// List team projects in cloud
    Projects(CloudProjectsArgs),
    /// Pull team memories for the current project
    TeamMemories(CloudTeamMemoriesArgs),
    /// Remove foreign-project entities from local DB and re-pull
    PurgeForeign(CloudPurgeForeignArgs),
}

/// Subcommands for `cas cloud team`
#[derive(Subcommand)]
pub enum CloudTeamCommands {
    /// Set the active team by UUID
    ///
    /// The team is persisted in `~/.cas/cloud.json` and used by team-scoped
    /// sync operations (push to `/api/teams/{uuid}/sync/push`, pull via
    /// `cas cloud team-memories`).
    ///
    /// Only UUID input is supported today — slug resolution requires a
    /// cloud-side endpoint that is not yet available. Find your team UUID
    /// in the CAS Cloud dashboard under team settings.
    Set(CloudTeamSetArgs),
    /// Show the currently configured team
    Show,
    /// Clear the configured team (no more team-scoped sync)
    Clear,
}

#[derive(Parser)]
pub struct CloudTeamSetArgs {
    /// Team UUID (e.g., 550e8400-e29b-41d4-a716-446655440000)
    pub id: String,
}

#[derive(Parser)]
pub struct CloudPushArgs {
    /// Push only entries
    #[arg(long)]
    pub entries_only: bool,

    /// Push only tasks
    #[arg(long)]
    pub tasks_only: bool,

    /// Dry run (don't actually push)
    #[arg(long)]
    pub dry_run: bool,
}

#[derive(Parser)]
pub struct CloudPullArgs {
    /// Pull only entries
    #[arg(long)]
    pub entries_only: bool,

    /// Pull only tasks
    #[arg(long)]
    pub tasks_only: bool,

    /// Pull all data (ignore last sync time)
    #[arg(long)]
    pub full: bool,
}

#[derive(Parser)]
pub struct CloudSyncArgs {
    /// Dry run (don't actually sync)
    #[arg(long)]
    pub dry_run: bool,
}

#[derive(Parser)]
pub struct CloudProjectsArgs {
    /// Team UUID override (defaults to the team configured via `cas cloud team set`)
    #[arg(long)]
    pub team: Option<String>,
}

#[derive(Parser)]
pub struct CloudTeamMemoriesArgs {
    /// Show what would be pulled without merging
    #[arg(long)]
    pub dry_run: bool,

    /// Ignore last sync timestamp, pull everything
    #[arg(long)]
    pub full: bool,
}

#[derive(Parser)]
pub struct CloudPurgeForeignArgs {
    /// Preview what would be purged without deleting
    #[arg(long)]
    pub dry_run: bool,
}

#[derive(Parser)]
pub struct CloudQueueArgs {
    /// Show detailed list of queued items
    #[arg(long, short)]
    pub verbose: bool,

    /// Maximum items to show
    #[arg(long, default_value = "20")]
    pub limit: usize,

    /// Clear failed items older than N days
    #[arg(long)]
    pub prune: Option<i64>,

    /// Clear all items from the queue
    #[arg(long)]
    pub clear: bool,
}

pub fn execute(cmd: &CloudCommands, cli: &Cli, cas_root: &Path) -> anyhow::Result<()> {
    match cmd {
        CloudCommands::Status => execute_status(cli, cas_root),
        CloudCommands::Queue(args) => execute_queue(args, cli, cas_root),
        CloudCommands::Push(args) => execute_push(args, cli, cas_root),
        CloudCommands::Pull(args) => execute_pull(args, cli, cas_root),
        CloudCommands::Sync(args) => execute_sync(args, cli, cas_root),
        CloudCommands::Team(cmd) => execute_team(cmd, cli),
        CloudCommands::Projects(args) => execute_projects(args, cli),
        CloudCommands::TeamMemories(args) => execute_team_memories(args, cli, cas_root),
        CloudCommands::PurgeForeign(args) => execute_purge_foreign(args, cli, cas_root),
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
// TEAM — set / show / clear the active team
// ═══════════════════════════════════════════════════════════════════════════════

/// HTTP timeout for the pre-flight team-membership probe.
///
/// Same magnitude as the coordinator's default — long enough to absorb a cold
/// Neon/Vercel cache, short enough that a misconfigured endpoint fails
/// visibly instead of hanging the shell.
const TEAM_PROBE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(15);

/// Validate a string is a canonical UUID (36 chars, 8-4-4-4-12 hex).
///
/// Both upper- and lower-case hex are accepted; the value is normalised to
/// lowercase on return. Non-canonical UUID forms (braces `{...}`,
/// no-hyphen 32-char, URN `urn:uuid:...`) that the `uuid` crate would
/// otherwise accept are explicitly rejected — the server stores team UUIDs
/// in canonical form, and silently accepting variants would let two strings
/// represent the same team locally while compare-unequal elsewhere.
///
/// Returns a short error (no "find your UUID here" guidance); the CLI
/// layer wraps that with endpoint-specific help via `format_uuid_error`.
fn parse_team_uuid(input: &str) -> Result<String, String> {
    // Canonical hyphenated form is exactly 36 chars; reject braces,
    // URN-prefixed, and no-hyphen 32-char variants before uuid::try_parse
    // normalises them silently.
    if input.len() != 36 {
        return Err(format!("expected a team UUID, got `{input}`"));
    }
    match uuid::Uuid::try_parse(input) {
        Ok(u) => Ok(u.as_hyphenated().to_string()),
        Err(_) => Err(format!("expected a team UUID, got `{input}`")),
    }
}

/// Wrap a `parse_team_uuid` error with endpoint-aware guidance on where to
/// find the team UUID. Kept separate so the parser stays pure.
fn format_uuid_error(short: &str, endpoint: &str) -> String {
    // Strip any trailing slash so the interpolated URL is clean.
    let base = endpoint.trim_end_matches('/');
    format!(
        "{short}.\n\n  Slug resolution is not yet supported. Find your team UUID at:\n    {base}/dashboard/teams\n\n  If a teammate has already set up cloud sync, check their\n  `~/.cas/cloud.json` for the `team_id` field and pass that UUID to\n  `cas cloud team set <uuid>`."
    )
}

/// Result of probing team membership via `GET /api/teams/{uuid}/projects`.
#[derive(Debug, PartialEq, Eq)]
enum TeamProbeOutcome {
    /// Server returned 2xx — user is a member of the team.
    Member,
    /// Server returned 401 — the token is invalid or expired.
    Unauthorized,
    /// Server returned 403 — valid token, but user is not a team member.
    NotAMember,
    /// Server returned 404 — team UUID does not exist.
    NotFound,
    /// Network error or unexpected status code.
    Error(String),
}

/// Probe team membership by hitting `GET /api/teams/{uuid}/projects`.
///
/// This endpoint already enforces `validateTeamMembership` server-side and is
/// cheap (no body), so it is the natural pre-flight check before persisting
/// `team_id` to cloud.json. Factored out for testability with wiremock.
fn probe_team_membership(endpoint: &str, token: &str, team_uuid: &str) -> TeamProbeOutcome {
    let url = format!("{endpoint}/api/teams/{team_uuid}/projects");
    match ureq::get(&url)
        .timeout(TEAM_PROBE_TIMEOUT)
        .set("Authorization", &format!("Bearer {token}"))
        .call()
    {
        Ok(_) => TeamProbeOutcome::Member,
        Err(ureq::Error::Status(401, _)) => TeamProbeOutcome::Unauthorized,
        Err(ureq::Error::Status(403, _)) => TeamProbeOutcome::NotAMember,
        Err(ureq::Error::Status(404, _)) => TeamProbeOutcome::NotFound,
        Err(ureq::Error::Status(code, _)) => {
            TeamProbeOutcome::Error(format!("unexpected HTTP {code}"))
        }
        Err(e) => TeamProbeOutcome::Error(format!("network error: {e}")),
    }
}

fn execute_team(cmd: &CloudTeamCommands, cli: &Cli) -> anyhow::Result<()> {
    match cmd {
        CloudTeamCommands::Set(args) => execute_team_set(args, cli),
        CloudTeamCommands::Show => execute_team_show(cli),
        CloudTeamCommands::Clear => execute_team_clear(cli),
    }
}

fn execute_team_set(args: &CloudTeamSetArgs, cli: &Cli) -> anyhow::Result<()> {
    // Load config before parsing so the error path can build an
    // endpoint-aware dashboard URL ("find your team UUID at …").
    let mut config = CloudConfig::load()?;

    let uuid = parse_team_uuid(&args.id).map_err(|short| {
        anyhow::anyhow!("{}", format_uuid_error(&short, &config.endpoint))
    })?;

    let token = config
        .token
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("Not logged in. Run 'cas login' first."))?
        .clone();

    match probe_team_membership(&config.endpoint, &token, &uuid) {
        TeamProbeOutcome::Member => {
            // The membership probe endpoint doesn't return the slug. Set
            // team_id directly and leave team_slug None so downstream
            // callers (execute_projects, hooks/context.rs) fall through to
            // their "show UUID" / "your team" default instead of rendering
            // a sentinel string. A future cloud-side slug resolver (tracked
            // in T7 docs) can populate team_slug when it lands.
            config.team_id = Some(uuid.clone());
            config.team_slug = None;
            config.save()?;

            if cli.json {
                let out = serde_json::json!({
                    "status": "ok",
                    "team_id": uuid,
                    "team_slug": serde_json::Value::Null,
                });
                println!("{}", out);
            } else {
                let theme = ActiveTheme::default();
                let mut out = io::stdout();
                let mut fmt = Formatter::stdout(&mut out, theme);
                let success_color = fmt.theme().palette.status_success;
                fmt.newline()?;
                fmt.write_colored("  \u{2713} ", success_color)?;
                fmt.write_raw("Active team set")?;
                fmt.newline()?;
                fmt.write_muted("  UUID: ")?;
                fmt.write_raw(&uuid)?;
                fmt.newline()?;
                fmt.write_muted("  Slug resolution deferred — see `cas cloud team show`")?;
                fmt.newline()?;
            }
            Ok(())
        }
        TeamProbeOutcome::Unauthorized => {
            anyhow::bail!("Token invalid or expired. Run 'cas login' to re-authenticate.")
        }
        TeamProbeOutcome::NotAMember => {
            anyhow::bail!("You are not a member of team {uuid}.")
        }
        TeamProbeOutcome::NotFound => {
            anyhow::bail!("Team {uuid} not found on {}.", config.endpoint)
        }
        TeamProbeOutcome::Error(msg) => {
            anyhow::bail!("Failed to verify team membership: {msg}")
        }
    }
}

fn execute_team_show(cli: &Cli) -> anyhow::Result<()> {
    let config = CloudConfig::load()?;

    match (&config.team_id, &config.team_slug) {
        (Some(id), slug) => {
            if cli.json {
                let out = serde_json::json!({
                    "team_id": id,
                    "team_slug": slug,
                });
                println!("{}", out);
            } else {
                let theme = ActiveTheme::default();
                let mut out = io::stdout();
                let mut fmt = Formatter::stdout(&mut out, theme);
                fmt.newline()?;
                fmt.write_muted("  Team ID:   ")?;
                fmt.write_raw(id)?;
                fmt.newline()?;
                fmt.write_muted("  Team slug: ")?;
                fmt.write_raw(slug.as_deref().unwrap_or("<not resolved>"))?;
                fmt.newline()?;
            }
        }
        (None, _) => {
            if cli.json {
                let out = serde_json::json!({
                    "team_id": serde_json::Value::Null,
                });
                println!("{}", out);
            } else {
                let theme = ActiveTheme::default();
                let mut out = io::stdout();
                let mut fmt = Formatter::stdout(&mut out, theme);
                let warning_color = fmt.theme().palette.status_warning;
                fmt.newline()?;
                fmt.write_colored("  \u{25CF} ", warning_color)?;
                fmt.write_raw("No team configured")?;
                fmt.newline()?;
                fmt.write_raw("  Run ")?;
                fmt.write_accent("cas cloud team set <uuid>")?;
                fmt.write_raw(" to set the active team.")?;
                fmt.newline()?;
            }
        }
    }
    Ok(())
}

fn execute_team_clear(cli: &Cli) -> anyhow::Result<()> {
    let mut config = CloudConfig::load()?;
    let was_set = config.team_id.is_some();
    config.clear_team();
    config.save()?;

    if cli.json {
        let out = serde_json::json!({ "status": "ok", "was_set": was_set });
        println!("{}", out);
    } else {
        let theme = ActiveTheme::default();
        let mut out = io::stdout();
        let mut fmt = Formatter::stdout(&mut out, theme);
        let success_color = fmt.theme().palette.status_success;
        fmt.newline()?;
        fmt.write_colored("  \u{2713} ", success_color)?;
        fmt.write_raw(if was_set {
            "Active team cleared"
        } else {
            "No team was configured"
        })?;
        fmt.newline()?;
    }
    Ok(())
}

// ═══════════════════════════════════════════════════════════════════════════════
// LOGIN - Polished TUI with Device Flow
// ═══════════════════════════════════════════════════════════════════════════════

fn execute_status(cli: &Cli, cas_root: &Path) -> anyhow::Result<()> {
    let config = CloudConfig::load()?;

    if config.token.is_none() {
        if cli.json {
            println!(r#"{{"status":"not_logged_in"}}"#);
        } else {
            let theme = ActiveTheme::default();
            let mut out = io::stdout();
            let mut fmt = Formatter::stdout(&mut out, theme);
            let warning_color = fmt.theme().palette.status_warning;
            fmt.write_colored("  \u{25CF} ", warning_color)?;
            fmt.write_raw("Not logged in to CAS Cloud")?;
            fmt.newline()?;
            fmt.write_raw("  Run ")?;
            fmt.write_accent("cas login")?;
            fmt.write_raw(" to authenticate")?;
            fmt.newline()?;
        }
        return Ok(());
    }

    {
        let status_url = format!("{}/api/sync/status", config.endpoint);
        let token = config.token.as_ref().unwrap();

        match ureq::get(&status_url)
            .set("Authorization", &format!("Bearer {token}"))
            .call()
        {
            Ok(resp) => {
                let body: serde_json::Value = resp.into_json()?;

                if cli.json {
                    println!("{}", serde_json::to_string(&body)?);
                } else {
                    let theme = ActiveTheme::default();
                    let mut out = io::stdout();
                    let mut fmt = Formatter::stdout(&mut out, theme);
                    let success_color = fmt.theme().palette.status_success;
                    let warning_color = fmt.theme().palette.status_warning;

                    fmt.newline()?;
                    fmt.write_colored("  \u{25CF} ", success_color)?;
                    fmt.write_raw("CAS Cloud")?;
                    fmt.newline()?;
                    fmt.newline()?;

                    if let Some(email) = &config.email {
                        fmt.write_muted("  Email:  ")?;
                        fmt.write_raw(email)?;
                        fmt.newline()?;
                    }
                    fmt.write_muted("  Server: ")?;
                    fmt.write_raw(&config.endpoint)?;
                    fmt.newline()?;

                    if let Some(state) = body.get("sync_state") {
                        fmt.newline()?;
                        fmt.write_muted("  Entries: ")?;
                        fmt.write_raw(
                            &state
                                .get("entry_count")
                                .unwrap_or(&serde_json::json!(0))
                                .to_string(),
                        )?;
                        fmt.newline()?;
                        fmt.write_muted("  Tasks:  ")?;
                        fmt.write_raw(
                            &state
                                .get("task_count")
                                .unwrap_or(&serde_json::json!(0))
                                .to_string(),
                        )?;
                        fmt.newline()?;
                    }

                    // Show local queue stats
                    if let Ok(queue) = crate::cloud::SyncQueue::open(cas_root) {
                        if queue.init().is_ok() {
                            if let Ok(stats) = queue.stats(5) {
                                if stats.total > 0 {
                                    fmt.newline()?;
                                    fmt.write_colored("  \u{25CF} ", warning_color)?;
                                    fmt.write_raw("Sync Queue")?;
                                    fmt.newline()?;
                                    fmt.write_raw(&format!(
                                        "    {} pending, {} failed",
                                        stats.pending, stats.failed
                                    ))?;
                                    fmt.newline()?;
                                    fmt.write_raw("    Run ")?;
                                    fmt.write_accent("cas cloud queue")?;
                                    fmt.write_raw(" for details")?;
                                    fmt.newline()?;
                                }
                            }
                        }
                    }
                    fmt.newline()?;
                }
            }
            Err(ureq::Error::Status(401, _)) => {
                if cli.json {
                    println!(r#"{{"status":"error","message":"Invalid token"}}"#);
                } else {
                    let theme = ActiveTheme::default();
                    let mut err = io::stderr();
                    let mut fmt = Formatter::stdout(&mut err, theme);
                    let error_color = fmt.theme().palette.status_error;
                    fmt.write_colored("  \u{2717} ", error_color)?;
                    fmt.write_raw("Session expired")?;
                    fmt.newline()?;
                    fmt.write_raw("  Run ")?;
                    fmt.write_accent("cas login")?;
                    fmt.write_raw(" to re-authenticate")?;
                    fmt.newline()?;
                }
            }
            Err(e) => {
                anyhow::bail!("Failed to connect: {e}");
            }
        }
    }

    Ok(())
}

// ═══════════════════════════════════════════════════════════════════════════════
// QUEUE - View and manage sync queue
// ═══════════════════════════════════════════════════════════════════════════════

fn execute_queue(args: &CloudQueueArgs, cli: &Cli, cas_root: &Path) -> anyhow::Result<()> {
    use crate::cloud::SyncQueue;

    let queue = SyncQueue::open(cas_root)?;
    queue.init()?;

    // Handle clear operation
    if args.clear {
        queue.clear()?;
        if cli.json {
            println!(r#"{{"status":"ok","message":"Queue cleared"}}"#);
        } else {
            let theme = ActiveTheme::default();
            let mut out = io::stdout();
            let mut fmt = Formatter::stdout(&mut out, theme);
            fmt.success("Queue cleared")?;
        }
        return Ok(());
    }

    // Handle prune operation
    if let Some(days) = args.prune {
        let max_retries = 5; // Default max retries
        let pruned = queue.prune_failed(days, max_retries)?;
        if cli.json {
            println!(r#"{{"status":"ok","pruned":{pruned}}}"#);
        } else {
            let theme = ActiveTheme::default();
            let mut out = io::stdout();
            let mut fmt = Formatter::stdout(&mut out, theme);
            fmt.success(&format!(
                "Pruned {} failed items older than {} days",
                pruned, days
            ))?;
        }
        return Ok(());
    }

    // Show queue stats
    let max_retries = 5;
    let stats = queue.stats(max_retries)?;

    if cli.json {
        if args.verbose {
            let items = queue.list_all(args.limit)?;
            println!(
                "{}",
                serde_json::json!({
                    "stats": stats,
                    "items": items
                })
            );
        } else {
            println!("{}", serde_json::to_string(&stats)?);
        }
    } else {
        let theme = ActiveTheme::default();
        let mut out = io::stdout();
        let mut fmt = Formatter::stdout(&mut out, theme);

        if stats.total == 0 {
            let success_color = fmt.theme().palette.status_success;
            fmt.write_colored("  \u{25CF} ", success_color)?;
            fmt.write_raw("Sync queue is empty")?;
            fmt.newline()?;
            return Ok(());
        }

        let accent_color = fmt.theme().palette.accent;
        let error_color = fmt.theme().palette.status_error;
        let warning_color = fmt.theme().palette.status_warning;

        fmt.newline()?;
        fmt.write_colored("  \u{25CF} ", accent_color)?;
        fmt.write_raw("Sync Queue")?;
        fmt.newline()?;
        fmt.newline()?;
        fmt.write_muted("  Total:   ")?;
        fmt.write_raw(&stats.total.to_string())?;
        fmt.newline()?;
        fmt.write_muted("  Pending: ")?;
        fmt.write_raw(&stats.pending.to_string())?;
        fmt.newline()?;
        fmt.write_muted("  Failed:  ")?;
        fmt.write_raw(&stats.failed.to_string())?;
        fmt.newline()?;

        if !stats.by_type.is_empty() {
            fmt.newline()?;
            fmt.write_muted("  By type:")?;
            fmt.newline()?;
            for (entity_type, count) in &stats.by_type {
                fmt.write_raw(&format!("    {entity_type}: {count}"))?;
                fmt.newline()?;
            }
        }

        if let Some(oldest) = &stats.oldest_item {
            fmt.newline()?;
            fmt.write_muted("  Oldest: ")?;
            fmt.write_raw(oldest)?;
            fmt.newline()?;
        }

        // Show detailed list if verbose
        if args.verbose {
            let items = queue.list_all(args.limit)?;
            if !items.is_empty() {
                fmt.newline()?;
                fmt.write_muted("  Queued items:")?;
                fmt.newline()?;
                for item in items {
                    fmt.write_raw("    ")?;
                    if item.retry_count >= max_retries {
                        fmt.write_colored("\u{2717}", error_color)?;
                    } else if item.retry_count > 0 {
                        fmt.write_colored("\u{21BB}", warning_color)?;
                    } else {
                        fmt.write_muted("\u{25CB}")?;
                    }
                    fmt.write_raw(&format!(
                        " {} {} ({})",
                        item.operation.as_str(),
                        item.entity_id,
                        item.entity_type.as_str()
                    ))?;
                    fmt.newline()?;

                    if item.retry_count > 0 {
                        fmt.write_muted("      ")?;
                        fmt.write_raw(&format!(" retries: {}", item.retry_count))?;
                        fmt.newline()?;
                    }
                    if let Some(err) = &item.last_error {
                        fmt.write_muted("      ")?;
                        fmt.write_raw(&format!(" error: {}", err))?;
                        fmt.newline()?;
                    }
                }
            }
        }
        fmt.newline()?;
    }

    Ok(())
}

// ═══════════════════════════════════════════════════════════════════════════════
// PUSH
// ═══════════════════════════════════════════════════════════════════════════════

fn execute_push(args: &CloudPushArgs, cli: &Cli, cas_root: &Path) -> anyhow::Result<()> {
    let config = CloudConfig::load()?;
    let token = config
        .token
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("Not logged in. Run 'cas login' first"))?;

    let store = open_store(cas_root)?;
    let task_store = open_task_store(cas_root)?;
    let rule_store = open_rule_store(cas_root)?;
    let skill_store = open_skill_store(cas_root)?;
    let sqlite_store = SqliteStore::open(cas_root)?;
    let spec_store = open_spec_store(cas_root)?;
    let event_store = open_event_store(cas_root)?;
    let prompt_store = open_prompt_store(cas_root)?;
    let file_change_store = open_file_change_store(cas_root)?;
    let commit_link_store = open_commit_link_store(cas_root)?;

    // Collect data to push
    let mut entries_json = Vec::new();
    let mut tasks_json = Vec::new();
    let mut rules_json = Vec::new();
    let mut skills_json = Vec::new();
    let mut sessions_json = Vec::new();
    let mut specs_json = Vec::new();
    let mut events_json = Vec::new();
    let mut prompts_json = Vec::new();
    let mut file_changes_json = Vec::new();
    let mut commit_links_json = Vec::new();

    if !args.tasks_only {
        let entries = store.list()?;
        for entry in entries {
            entries_json.push(serde_json::to_value(&entry)?);
        }
    }

    if !args.entries_only {
        let tasks = task_store.list(None)?;
        for task in tasks {
            tasks_json.push(serde_json::to_value(&task)?);
        }
    }

    // Always push rules and skills
    let rules = rule_store.list()?;
    for rule in rules {
        rules_json.push(serde_json::to_value(&rule)?);
    }

    let skills = skill_store.list(None)?;
    for skill in skills {
        skills_json.push(serde_json::to_value(&skill)?);
    }

    // Always push sessions (they're lightweight)
    let sessions = sqlite_store
        .list_sessions_since(chrono::Utc::now() - chrono::Duration::days(90))
        .unwrap_or_default();
    for session in sessions {
        sessions_json.push(serde_json::to_value(&session)?);
    }

    // Always push specs
    let specs = spec_store.list(None)?;
    for spec in specs {
        specs_json.push(serde_json::to_value(&spec)?);
    }

    // Push events (last 90 days)
    let events = event_store.list_recent(10000).unwrap_or_default();
    for event in events {
        events_json.push(serde_json::to_value(&event)?);
    }

    // Push prompts (last 90 days)
    let prompts = prompt_store.list_recent(10000).unwrap_or_default();
    for prompt in prompts {
        prompts_json.push(serde_json::to_value(&prompt)?);
    }

    // Push file changes (last 90 days)
    let file_changes = file_change_store.list_recent(10000).unwrap_or_default();
    for fc in file_changes {
        file_changes_json.push(serde_json::to_value(&fc)?);
    }

    // Push commit links (last 90 days)
    let commit_links = commit_link_store.list_recent(10000).unwrap_or_default();
    for cl in commit_links {
        commit_links_json.push(serde_json::to_value(&cl)?);
    }

    // Push worktrees
    let mut worktrees_json = Vec::new();
    if let Ok(worktree_store) = open_worktree_store(cas_root) {
        let worktrees = worktree_store.list().unwrap_or_default();
        for wt in worktrees {
            worktrees_json.push(serde_json::to_value(&wt)?);
        }
    }

    // Push task dependencies
    let mut task_deps_json = Vec::new();
    if !args.entries_only {
        let deps = task_store.list_dependencies(None).unwrap_or_default();
        for dep in deps {
            task_deps_json.push(serde_json::to_value(&dep)?);
        }
    }

    if args.dry_run {
        if cli.json {
            println!(
                "{}",
                serde_json::json!({
                    "dry_run": true,
                    "entries": entries_json.len(),
                    "tasks": tasks_json.len(),
                    "rules": rules_json.len(),
                    "skills": skills_json.len(),
                    "sessions": sessions_json.len(),
                    "specs": specs_json.len(),
                    "events": events_json.len(),
                    "prompts": prompts_json.len(),
                    "file_changes": file_changes_json.len(),
                    "commit_links": commit_links_json.len(),
                    "task_dependencies": task_deps_json.len(),
                    "worktrees": worktrees_json.len(),
                })
            );
        } else {
            let theme = ActiveTheme::default();
            let mut out = io::stdout();
            let mut fmt = Formatter::stdout(&mut out, theme);
            fmt.write_accent("  \u{2192} ")?;
            fmt.write_raw("Dry run - would push:")?;
            fmt.newline()?;
            fmt.write_raw(&format!("    {} entries", entries_json.len()))?;
            fmt.newline()?;
            fmt.write_raw(&format!("    {} tasks", tasks_json.len()))?;
            fmt.newline()?;
            fmt.write_raw(&format!("    {} rules", rules_json.len()))?;
            fmt.newline()?;
            fmt.write_raw(&format!("    {} skills", skills_json.len()))?;
            fmt.newline()?;
            fmt.write_raw(&format!("    {} sessions", sessions_json.len()))?;
            fmt.newline()?;
            fmt.write_raw(&format!("    {} specs", specs_json.len()))?;
            fmt.newline()?;
            fmt.write_raw(&format!("    {} events", events_json.len()))?;
            fmt.newline()?;
            fmt.write_raw(&format!("    {} prompts", prompts_json.len()))?;
            fmt.newline()?;
            fmt.write_raw(&format!("    {} file changes", file_changes_json.len()))?;
            fmt.newline()?;
            fmt.write_raw(&format!("    {} commit links", commit_links_json.len()))?;
            fmt.newline()?;
            fmt.write_raw(&format!("    {} task dependencies", task_deps_json.len()))?;
            fmt.newline()?;
            fmt.write_raw(&format!("    {} worktrees", worktrees_json.len()))?;
            fmt.newline()?;
        }
        return Ok(());
    }

    {
        use crate::ui::components::{
            Component, ProgressBar, ProgressBarMsg, clear_inline, render_inline_view,
            rerender_inline,
        };

        let push_url = format!("{}/api/sync/push", config.endpoint);

        // Build batches: split large collections into chunks to avoid 413 errors
        const BATCH_SIZE: usize = 50;

        let resource_types: Vec<(&str, &[serde_json::Value])> = vec![
            ("entries", &entries_json),
            ("tasks", &tasks_json),
            ("rules", &rules_json),
            ("skills", &skills_json),
            ("sessions", &sessions_json),
            ("specs", &specs_json),
            ("events", &events_json),
            ("prompts", &prompts_json),
            ("file_changes", &file_changes_json),
            ("commit_links", &commit_links_json),
            ("task_dependencies", &task_deps_json),
            ("worktrees", &worktrees_json),
        ];

        // Build list of batches: each batch is a JSON payload with chunked data
        let mut batches: Vec<serde_json::Value> = Vec::new();

        // Find the max number of chunks needed across all resource types
        let max_chunks = resource_types
            .iter()
            .map(|(_, items)| (items.len() + BATCH_SIZE - 1) / BATCH_SIZE.max(1))
            .max()
            .unwrap_or(1)
            .max(1);

        let project_id = get_project_canonical_id()
            .ok_or_else(|| anyhow::anyhow!("Cannot sync: not inside a CAS project directory"))?;

        for chunk_idx in 0..max_chunks {
            let start = chunk_idx * BATCH_SIZE;
            let mut payload = serde_json::Map::new();

            for (name, items) in &resource_types {
                let end = (start + BATCH_SIZE).min(items.len());
                let chunk = if start < items.len() {
                    &items[start..end]
                } else {
                    &[]
                };
                payload.insert(name.to_string(), serde_json::json!(chunk));
            }

            // Required by server for project scoping
            payload.insert(
                "project_canonical_id".to_string(),
                serde_json::json!(project_id),
            );
            // Client version for server-side compatibility checks
            payload.insert(
                "client_version".to_string(),
                serde_json::json!(env!("CARGO_PKG_VERSION")),
            );
            payload.insert(
                "client_build".to_string(),
                serde_json::json!(option_env!("CAS_GIT_HASH").unwrap_or("unknown")),
            );
            // Include team_id if configured
            if let Some(team_id) = &config.team_id {
                payload.insert("team_id".to_string(), serde_json::json!(team_id));
            }

            batches.push(serde_json::Value::Object(payload));
        }

        let total_items: usize = resource_types.iter().map(|(_, items)| items.len()).sum();
        let num_batches = batches.len();

        let theme = ActiveTheme::default();
        let (mut progress_bar, mut prev_lines) = if !cli.json {
            let bar = ProgressBar::new(total_items as u64).with_message("Pushing");
            let lines = render_inline_view(&bar, &theme)?;
            (Some(bar), lines)
        } else {
            (None, 0u16)
        };

        // Aggregate totals across batches
        let resource_names = [
            "entries",
            "tasks",
            "rules",
            "skills",
            "sessions",
            "specs",
            "events",
            "prompts",
            "file_changes",
            "commit_links",
            "task_dependencies",
            "worktrees",
        ];
        let mut totals: std::collections::HashMap<String, (u64, u64)> = resource_names
            .iter()
            .map(|n| (n.to_string(), (0u64, 0u64)))
            .collect();
        let mut items_pushed = 0u64;

        for (batch_idx, payload) in batches.iter().enumerate() {
            // Count items in this batch
            let batch_items: usize = resource_names
                .iter()
                .map(|name| {
                    payload
                        .get(name)
                        .and_then(|v| v.as_array())
                        .map_or(0, |a| a.len())
                })
                .sum();

            if let Some(ref mut bar) = progress_bar {
                if num_batches > 1 {
                    bar.update(ProgressBarMsg::SetMessage(format!(
                        "Pushing (batch {}/{})",
                        batch_idx + 1,
                        num_batches
                    )));
                }
                bar.update(ProgressBarMsg::Tick);
                prev_lines = rerender_inline(bar, prev_lines, &theme)?;
            }

            let response = ureq::AgentBuilder::new()
                .timeout(Duration::from_secs(120))
                .build()
                .post(&push_url)
                .set("Authorization", &format!("Bearer {token}"))
                .set("Content-Type", "application/json")
                .send_json(payload);

            match response {
                Ok(resp) => {
                    let body: serde_json::Value = resp.into_json()?;

                    // Accumulate per-resource totals
                    for name in &resource_names {
                        if let Some(res) = body.get(name) {
                            let ins = res.get("inserted").and_then(|v| v.as_u64()).unwrap_or(0);
                            let upd = res.get("updated").and_then(|v| v.as_u64()).unwrap_or(0);
                            let entry = totals.entry(name.to_string()).or_insert((0, 0));
                            entry.0 += ins;
                            entry.1 += upd;
                        }
                    }

                    items_pushed += batch_items as u64;
                    if let Some(ref mut bar) = progress_bar {
                        bar.update(ProgressBarMsg::Set(items_pushed));
                        bar.update(ProgressBarMsg::Tick);
                        prev_lines = rerender_inline(bar, prev_lines, &theme)?;
                    }
                }
                Err(ureq::Error::Status(402, resp)) => {
                    if progress_bar.is_some() {
                        clear_inline(prev_lines)?;
                    }

                    let body: serde_json::Value = resp.into_json().unwrap_or_default();

                    if cli.json {
                        println!("{}", serde_json::to_string(&body)?);
                    } else {
                        let mut out = io::stdout();
                        let mut fmt = Formatter::stdout(&mut out, ActiveTheme::default());
                        let error_color = fmt.theme().palette.status_error;

                        let message = body
                            .get("message")
                            .and_then(|m| m.as_str())
                            .unwrap_or("Sync limit exceeded");
                        fmt.newline()?;
                        fmt.write_colored(&format!("  \u{2717} {message}"), error_color)?;
                        fmt.newline()?;
                        fmt.newline()?;
                    }
                    return Ok(());
                }
                Err(ureq::Error::Status(401, _)) => {
                    if progress_bar.is_some() {
                        clear_inline(prev_lines)?;
                    }
                    if cli.json {
                        println!(r#"{{"status":"error","message":"Invalid or expired token"}}"#);
                    } else {
                        let mut err = io::stderr();
                        let mut fmt = Formatter::stdout(&mut err, ActiveTheme::default());
                        let error_color = fmt.theme().palette.status_error;
                        fmt.write_colored("  \u{2717} ", error_color)?;
                        fmt.write_raw("Session expired")?;
                        fmt.newline()?;
                        fmt.write_raw("  Run ")?;
                        fmt.write_accent("cas login")?;
                        fmt.write_raw(" to re-authenticate")?;
                        fmt.newline()?;
                    }
                    return Ok(());
                }
                Err(e) => {
                    if progress_bar.is_some() {
                        clear_inline(prev_lines)?;
                    }
                    return Err(e.into());
                }
            }
        }

        if progress_bar.is_some() {
            clear_inline(prev_lines)?;
        }

        if cli.json {
            let json_totals: serde_json::Map<String, serde_json::Value> = totals
                .iter()
                .map(|(k, (ins, upd))| {
                    (
                        k.clone(),
                        serde_json::json!({"inserted": ins, "updated": upd}),
                    )
                })
                .collect();
            println!(
                "{}",
                serde_json::to_string(&serde_json::Value::Object(json_totals))?
            );
        } else {
            let mut out = io::stdout();
            let mut fmt = Formatter::stdout(&mut out, ActiveTheme::default());
            fmt.success("Push complete")?;
            let display_order = [
                ("entries", "Entries"),
                ("tasks", "Tasks"),
                ("rules", "Rules"),
                ("skills", "Skills"),
                ("sessions", "Sessions"),
                ("specs", "Specs"),
                ("events", "Events"),
                ("prompts", "Prompts"),
                ("file_changes", "File changes"),
                ("commit_links", "Commit links"),
                ("worktrees", "Worktrees"),
            ];
            for (key, label) in &display_order {
                if let Some(&(ins, upd)) = totals.get(*key) {
                    if ins > 0 || upd > 0 {
                        fmt.write_raw(&format!("    {label}: {ins} inserted, {upd} updated"))?;
                        fmt.newline()?;
                    }
                }
            }
        }
    }

    Ok(())
}

// ═══════════════════════════════════════════════════════════════════════════════
// PULL
// ═══════════════════════════════════════════════════════════════════════════════

fn execute_pull(args: &CloudPullArgs, cli: &Cli, cas_root: &Path) -> anyhow::Result<()> {
    use std::sync::Arc;

    use crate::cloud::{CloudSyncer, CloudSyncerConfig, SyncQueue};

    let config = CloudConfig::load()?;
    if config.token.is_none() {
        anyhow::bail!("Not logged in. Run 'cas login' first");
    }

    // Stores synced by CloudSyncer::pull. cas-ed15 collapsed the unscoped
    // inline path through the scoped syncer for entries/tasks/rules/skills;
    // cas-bba4 re-adds the remaining 5 entity kinds (specs/events/prompts/
    // file_changes/commit_links) — also scoped — so `cas cloud pull` once
    // again imports the full set without re-introducing the leak.
    let store = open_store(cas_root)?;
    let task_store = open_task_store(cas_root)?;
    let rule_store = open_rule_store(cas_root)?;
    let skill_store = open_skill_store(cas_root)?;
    let spec_store = open_spec_store(cas_root)?;
    let event_store = open_event_store(cas_root)?;
    let prompt_store = open_prompt_store(cas_root)?;
    let file_change_store = open_file_change_store(cas_root)?;
    let commit_link_store = open_commit_link_store(cas_root)?;

    {
        use crate::ui::components::{Spinner, clear_inline, render_inline_view};

        let theme = ActiveTheme::default();
        let prev_lines = if !cli.json {
            let spinner = Spinner::new("Pulling from cloud...");
            render_inline_view(&spinner, &theme)?
        } else {
            0u16
        };

        // Construct the scoped syncer. Same pattern as `execute_sync` /
        // `execute_purge_foreign` (cli/cloud.rs:2106). `CloudSyncer::pull`
        // hard-fails when `get_project_canonical_id()` is `None` and always
        // appends `?project_id=<urlencoded>` to `/api/sync/pull`.
        let queue = SyncQueue::open(cas_root)?;
        queue.init()?;

        // --full: clear the watermark so the syncer issues a full (no `since=`)
        // pull. This preserves the prior `--full` semantics under the new path.
        //
        // When a team is also configured, clear the team-pull watermark too
        // (`last_team_pull_at_<team_id>`, written by `CloudSyncer::pull_team`
        // — see cas-cli/src/cloud/syncer/pull.rs:710) so `--full` triggers a
        // full team backfill in addition to a full personal backfill. Task
        // cas-6ec7 added this — without it, `--full` was half-broken
        // (personal cleared, team kept its old watermark and only fetched
        // deltas).
        if args.full {
            queue.delete_metadata("last_pull_at")?;
            if let Some(team_id) = config.active_team_id() {
                queue.delete_metadata(&format!("last_team_pull_at_{team_id}"))?;
            }
        }

        let syncer = CloudSyncer::new(
            Arc::new(queue),
            // Clone: the outer `config` is reused after this scope to call
            // `execute_team_pull` (cas-6ec7 wire-up).
            config.clone(),
            CloudSyncerConfig::default(),
        );

        let pull_result = syncer.pull(
            store.as_ref(),
            task_store.as_ref(),
            rule_store.as_ref(),
            skill_store.as_ref(),
            spec_store.as_ref(),
            event_store.as_ref(),
            prompt_store.as_ref(),
            file_change_store.as_ref(),
            commit_link_store.as_ref(),
        )?;

        // The `--entries-only` / `--tasks-only` flags previously gated the
        // client-side imports of those two kinds. CloudSyncer::pull does not
        // take filter arguments; preserving these as no-ops keeps the CLI
        // contract stable for callers that pass them. The flags will become
        // semantically meaningful again if syncer-level filtering is added.
        let _ = (args.entries_only, args.tasks_only);

        let entries_count = pull_result.pulled_entries;
        let tasks_count = pull_result.pulled_tasks;
        let rules_count = pull_result.pulled_rules;
        let skills_count = pull_result.pulled_skills;
        let specs_count = pull_result.pulled_specs;
        let events_count = pull_result.pulled_events;
        let prompts_count = pull_result.pulled_prompts;
        let file_changes_count = pull_result.pulled_file_changes;
        let commit_links_count = pull_result.pulled_commit_links;

        if prev_lines > 0 {
            clear_inline(prev_lines)?;
        }

        if cli.json {
            println!(
                "{}",
                serde_json::json!({
                    "status": "ok",
                    "entries": entries_count,
                    "tasks": tasks_count,
                    "rules": rules_count,
                    "skills": skills_count,
                    "specs": specs_count,
                    "events": events_count,
                    "prompts": prompts_count,
                    "file_changes": file_changes_count,
                    "commit_links": commit_links_count,
                    "errors": pull_result.errors,
                })
            );
        } else {
            let mut out = io::stdout();
            let mut fmt = Formatter::stdout(&mut out, ActiveTheme::default());
            fmt.success("Pull complete")?;
            fmt.write_raw(&format!("    {entries_count} entries synced"))?;
            fmt.newline()?;
            fmt.write_raw(&format!("    {tasks_count} tasks synced"))?;
            fmt.newline()?;
            fmt.write_raw(&format!("    {rules_count} rules synced"))?;
            fmt.newline()?;
            fmt.write_raw(&format!("    {skills_count} skills synced"))?;
            fmt.newline()?;
            fmt.write_raw(&format!("    {specs_count} specs synced"))?;
            fmt.newline()?;
            fmt.write_raw(&format!("    {events_count} events synced"))?;
            fmt.newline()?;
            fmt.write_raw(&format!("    {prompts_count} prompts synced"))?;
            fmt.newline()?;
            fmt.write_raw(&format!("    {file_changes_count} file changes synced"))?;
            fmt.newline()?;
            fmt.write_raw(&format!("    {commit_links_count} commit links synced"))?;
            fmt.newline()?;
            if !pull_result.errors.is_empty() {
                let warning_color = fmt.theme().palette.status_warning;
                fmt.write_colored("  \u{26A0} ", warning_color)?;
                fmt.write_raw(&format!("{} pull errors:", pull_result.errors.len()))?;
                fmt.newline()?;
                for err in &pull_result.errors {
                    fmt.write_muted("    - ")?;
                    fmt.write_raw(err)?;
                    fmt.newline()?;
                }
            }
        }
    }

    // Team pull layers on top of personal pull when a team is configured.
    // cas-6ec7: `cas cloud pull` was missing this call, leaving new team
    // members with zero team-scoped rows on a fresh `cas cloud pull`. The
    // helper is a no-op when `active_team_id()` is None and isolates its
    // own errors so it cannot regress personal-pull results.
    execute_team_pull(&config, cas_root, cli)?;

    Ok(())
}

// ═══════════════════════════════════════════════════════════════════════════════
// SYNC
// ═══════════════════════════════════════════════════════════════════════════════

/// Orchestrates `cas cloud sync` — personal push, team push, then personal pull
/// (which transitively does team pull when a team is configured).
///
/// `pub` so `cas-cli/tests/team_pull_wiring_test.rs` can exercise the
/// end-to-end wire-up against a wiremock server. Production callers go
/// through the CLI dispatcher; this is not intended for external public-API
/// use. Mirrors the same `pub` + `#[doc(hidden)]` pattern as
/// `execute_team_push` / `execute_team_pull`.
#[doc(hidden)]
pub fn execute_sync(args: &CloudSyncArgs, cli: &Cli, cas_root: &Path) -> anyhow::Result<()> {
    execute_push(
        &CloudPushArgs {
            entries_only: false,
            tasks_only: false,
            dry_run: args.dry_run,
        },
        cli,
        cas_root,
    )?;

    if !args.dry_run {
        // Drain the team queue before pulling — when a team is configured,
        // writes since the last sync were dual-enqueued by T3's syncing
        // wrappers; this is where the team rows reach the server. Team
        // push failure is isolated from the personal drain above (which
        // already succeeded by now) and from the pull below (best-effort).
        let cloud_config = CloudConfig::load()?;
        execute_team_push(&cloud_config, cas_root, cli)?;

        // Personal pull AND team pull happen transitively here:
        // `execute_pull` invokes `execute_team_pull` at its tail when an
        // active team is configured (cas-6ec7). `execute_sync` does NOT
        // call `execute_team_pull` itself — duplicating the call would
        // fire the team-pull HTTP request twice per sync (the second
        // call returns 0 rows because the first advanced the `since=`
        // watermark, but the wasted round-trip is still observable). The
        // behavioral wiremock test in `team_pull_wiring_test.rs`
        // (`execute_sync_hits_each_pull_endpoint_exactly_once_when_team_configured`)
        // locks this invariant in with `.expect(1)` on both endpoints.
        execute_pull(
            &CloudPullArgs {
                entries_only: false,
                tasks_only: false,
                full: false,
            },
            cli,
            cas_root,
        )?;
    }

    Ok(())
}

/// Drain the team queue into `POST /api/teams/{uuid}/sync/push` when a
/// team is configured. No-op when no active team.
///
/// Contract: always returns `Ok(())` — team-push failures are reported
/// via `report_team_push_*` and isolated from the surrounding sync so
/// the personal drain already done stays, and the pull that follows
/// still runs. Items that fail to push remain in the team queue
/// (re-enqueued by `push_team` itself) for the next sync cycle.
///
/// `pub` so `cas-cli/tests/team_sync_test.rs` can exercise the helper
/// directly with a wiremock server. Not intended for external use.
#[doc(hidden)]
pub fn execute_team_push(
    cloud_config: &CloudConfig,
    cas_root: &Path,
    cli: &Cli,
) -> anyhow::Result<()> {
    let Some(team_id) = cloud_config.active_team_id() else {
        return Ok(());
    };
    let team_id = team_id.to_string();

    let queue = match crate::cloud::SyncQueue::open(cas_root) {
        Ok(q) => {
            if let Err(e) = q.init() {
                tracing::warn!(
                    target: "cas::sync",
                    error = %e,
                    "team sync queue init failed; draining aborted",
                );
                // Isolation contract: reporter errors must not escape.
                let _ = report_team_push_error(cli, &format!("Team sync queue init failed: {e}"));
                return Ok(());
            }
            q
        }
        Err(e) => {
            let _ = report_team_push_error(cli, &format!("Could not open sync queue: {e}"));
            return Ok(());
        }
    };

    let syncer = crate::cloud::CloudSyncer::new(
        std::sync::Arc::new(queue),
        cloud_config.clone(),
        crate::cloud::CloudSyncerConfig::default(),
    );

    // `let _ =` on reporter calls: a formatter/IO error from the display
    // path must not propagate out and block the caller's pull step.
    match syncer.push_team(&team_id) {
        Ok(result) => {
            if result.errors.is_empty() {
                let _ = report_team_push_result(cli, &team_id, &result);
            } else {
                let _ = report_team_push_partial(cli, &team_id, &result);
            }
        }
        Err(e) => {
            let _ = report_team_push_error(cli, &format!("Team push failed: {e}"));
        }
    }
    Ok(())
}

fn report_team_push_result(
    cli: &Cli,
    team_id: &str,
    result: &crate::cloud::SyncResult,
) -> anyhow::Result<()> {
    if cli.json {
        println!("{}", team_push_json(team_id, result, &[]));
    } else {
        // `total_pushed()` sums all 12 entity types — not just
        // entries/tasks/rules/skills — so a sync that only pushes
        // sessions/events/prompts still surfaces in the human output.
        if result.total_pushed() > 0 {
            let theme = ActiveTheme::default();
            let mut out = io::stdout();
            let mut fmt = Formatter::stdout(&mut out, theme);
            let success_color = fmt.theme().palette.status_success;
            fmt.write_colored("  \u{2713} ", success_color)?;
            fmt.write_raw(&format!(
                "Team push: {} entries, {} tasks, {} rules, {} skills ({} total)",
                result.pushed_entries,
                result.pushed_tasks,
                result.pushed_rules,
                result.pushed_skills,
                result.total_pushed(),
            ))?;
            fmt.newline()?;
        }
    }
    Ok(())
}

/// Shared JSON shape for `report_team_push_{result,partial}` — consumers
/// see a consistent `{team_push: {...}}` object whether the push fully
/// succeeded or partially failed. `errors` is always present (empty for
/// full success), and every `pushed_*` count is always present.
fn team_push_json(
    team_id: &str,
    result: &crate::cloud::SyncResult,
    extra_errors: &[String],
) -> serde_json::Value {
    let mut errors = result.errors.clone();
    errors.extend(extra_errors.iter().cloned());
    serde_json::json!({
        "team_push": {
            "team_id": team_id,
            "pushed_entries": result.pushed_entries,
            "pushed_tasks": result.pushed_tasks,
            "pushed_rules": result.pushed_rules,
            "pushed_skills": result.pushed_skills,
            "pushed_sessions": result.pushed_sessions,
            "pushed_verifications": result.pushed_verifications,
            "pushed_events": result.pushed_events,
            "pushed_prompts": result.pushed_prompts,
            "pushed_file_changes": result.pushed_file_changes,
            "pushed_commit_links": result.pushed_commit_links,
            "pushed_agents": result.pushed_agents,
            "pushed_worktrees": result.pushed_worktrees,
            "total_pushed": result.total_pushed(),
            "duration_ms": result.duration_ms,
            "errors": errors,
        }
    })
}

fn report_team_push_partial(
    cli: &Cli,
    team_id: &str,
    result: &crate::cloud::SyncResult,
) -> anyhow::Result<()> {
    if cli.json {
        // Same shape as the full-success path so JSON consumers can
        // always read pushed counts regardless of outcome.
        println!("{}", team_push_json(team_id, result, &[]));
    } else {
        let theme = ActiveTheme::default();
        let mut out = io::stdout();
        let mut fmt = Formatter::stdout(&mut out, theme);
        let warning_color = fmt.theme().palette.status_warning;
        fmt.write_colored("  \u{26A0} ", warning_color)?;
        fmt.write_raw(&format!(
            "Team push encountered {} error(s); items re-queued for next sync",
            result.errors.len()
        ))?;
        fmt.newline()?;
        for err in &result.errors {
            fmt.write_muted("    - ")?;
            fmt.write_raw(err)?;
            fmt.newline()?;
        }
    }
    Ok(())
}

fn report_team_push_error(cli: &Cli, msg: &str) -> anyhow::Result<()> {
    if cli.json {
        // Empty SyncResult + the single fatal error as a string — keeps
        // shape consistent with success/partial paths.
        let empty = crate::cloud::SyncResult::default();
        println!(
            "{}",
            team_push_json("", &empty, std::slice::from_ref(&msg.to_string()))
        );
    } else {
        let theme = ActiveTheme::default();
        let mut out = io::stdout();
        let mut fmt = Formatter::stdout(&mut out, theme);
        let warning_color = fmt.theme().palette.status_warning;
        fmt.write_colored("  \u{26A0} ", warning_color)?;
        fmt.write_raw(msg)?;
        fmt.newline()?;
    }
    Ok(())
}

/// Pull team data into the local stores from `GET /api/teams/{uuid}/sync/pull`
/// when a team is configured. No-op when no active team.
///
/// Contract: always returns `Ok(())` — team-pull failures are reported via
/// `report_team_pull_*` and isolated from the surrounding sync so the
/// personal pull that ran just before stays, and any caller chained after
/// (e.g. `execute_sync` exit) still completes cleanly. Mirrors the isolation
/// contract of `execute_team_push` (cli/cloud.rs:1313).
///
/// Signature note: `pull_team` currently takes 4 stores (entries / tasks /
/// rules / skills) — NOT the full 9-store set that personal `pull` takes.
/// Per task cas-6ec7 spec, this helper preserves that parity. Extending
/// `pull_team` to specs / events / prompts / file_changes / commit_links is
/// a separate scope expansion.
///
/// `pub` so `cas-cli/tests/team_pull_wiring_test.rs` can exercise the helper
/// directly with a wiremock server, matching the precedent set by
/// `execute_team_push` for `team_sync_test.rs`. Not intended for external
/// (public-API) use.
#[doc(hidden)]
pub fn execute_team_pull(
    cloud_config: &CloudConfig,
    cas_root: &Path,
    cli: &Cli,
) -> anyhow::Result<()> {
    let Some(team_id) = cloud_config.active_team_id() else {
        return Ok(());
    };
    let team_id = team_id.to_string();

    let queue = match crate::cloud::SyncQueue::open(cas_root) {
        Ok(q) => {
            if let Err(e) = q.init() {
                tracing::warn!(
                    target: "cas::sync",
                    error = %e,
                    "team sync queue init failed; team pull aborted",
                );
                // Isolation contract: reporter errors must not escape.
                let _ = report_team_pull_error(cli, &format!("Team sync queue init failed: {e}"));
                return Ok(());
            }
            q
        }
        Err(e) => {
            let _ = report_team_pull_error(cli, &format!("Could not open sync queue: {e}"));
            return Ok(());
        }
    };

    // Stores synced by `pull_team`: entries / tasks / rules / skills (only).
    // Per cas-6ec7 spec, this is intentional parity with the current
    // `pull_team` signature — adding the remaining 5 entity kinds is a
    // separate scope expansion.
    let store = match open_store(cas_root) {
        Ok(s) => s,
        Err(e) => {
            let _ = report_team_pull_error(cli, &format!("Could not open entry store: {e}"));
            return Ok(());
        }
    };
    let task_store = match open_task_store(cas_root) {
        Ok(s) => s,
        Err(e) => {
            let _ = report_team_pull_error(cli, &format!("Could not open task store: {e}"));
            return Ok(());
        }
    };
    let rule_store = match open_rule_store(cas_root) {
        Ok(s) => s,
        Err(e) => {
            let _ = report_team_pull_error(cli, &format!("Could not open rule store: {e}"));
            return Ok(());
        }
    };
    let skill_store = match open_skill_store(cas_root) {
        Ok(s) => s,
        Err(e) => {
            let _ = report_team_pull_error(cli, &format!("Could not open skill store: {e}"));
            return Ok(());
        }
    };

    let syncer = crate::cloud::CloudSyncer::new(
        std::sync::Arc::new(queue),
        cloud_config.clone(),
        crate::cloud::CloudSyncerConfig::default(),
    );

    // `let _ =` on reporter calls: a formatter/IO error from the display
    // path must not propagate out and block subsequent caller steps.
    match syncer.pull_team(
        &team_id,
        store.as_ref(),
        task_store.as_ref(),
        rule_store.as_ref(),
        skill_store.as_ref(),
    ) {
        Ok(result) => {
            if result.errors.is_empty() {
                let _ = report_team_pull_result(cli, &team_id, &result);
            } else {
                let _ = report_team_pull_partial(cli, &team_id, &result);
            }
        }
        Err(e) => {
            let _ = report_team_pull_error(cli, &format!("Team pull failed: {e}"));
        }
    }
    Ok(())
}

/// Shared JSON shape for `report_team_pull_{result,partial,error}` —
/// consumers see a consistent `{team_pull: {...}}` object regardless of
/// outcome. Mirrors `team_push_json`'s shape so JSON consumers can branch
/// on the wrapper key.
fn team_pull_json(
    team_id: &str,
    result: &crate::cloud::SyncResult,
    extra_errors: &[String],
) -> serde_json::Value {
    let mut errors = result.errors.clone();
    errors.extend(extra_errors.iter().cloned());
    serde_json::json!({
        "team_pull": {
            "team_id": team_id,
            "pulled_entries": result.pulled_entries,
            "pulled_tasks": result.pulled_tasks,
            "pulled_rules": result.pulled_rules,
            "pulled_skills": result.pulled_skills,
            "conflicts_resolved": result.conflicts_resolved,
            "duration_ms": result.duration_ms,
            "errors": errors,
        }
    })
}

fn report_team_pull_result(
    cli: &Cli,
    team_id: &str,
    result: &crate::cloud::SyncResult,
) -> anyhow::Result<()> {
    if cli.json {
        println!("{}", team_pull_json(team_id, result, &[]));
    } else {
        // Suppress no-op output when nothing was pulled — keeps the human
        // sync log uncluttered for the steady-state case (matches the
        // `total_pushed() > 0` guard in `report_team_push_result`).
        let total = result.pulled_entries
            + result.pulled_tasks
            + result.pulled_rules
            + result.pulled_skills;
        if total > 0 {
            let theme = ActiveTheme::default();
            let mut out = io::stdout();
            let mut fmt = Formatter::stdout(&mut out, theme);
            let success_color = fmt.theme().palette.status_success;
            fmt.write_colored("  \u{2713} ", success_color)?;
            fmt.write_raw(&format!(
                "Team pull: {} entries, {} tasks, {} rules, {} skills ({} total)",
                result.pulled_entries,
                result.pulled_tasks,
                result.pulled_rules,
                result.pulled_skills,
                total,
            ))?;
            fmt.newline()?;
        }
    }
    Ok(())
}

fn report_team_pull_partial(
    cli: &Cli,
    team_id: &str,
    result: &crate::cloud::SyncResult,
) -> anyhow::Result<()> {
    if cli.json {
        // Same shape as the full-success path so JSON consumers can always
        // read pulled counts regardless of outcome.
        println!("{}", team_pull_json(team_id, result, &[]));
    } else {
        let theme = ActiveTheme::default();
        let mut out = io::stdout();
        let mut fmt = Formatter::stdout(&mut out, theme);
        let warning_color = fmt.theme().palette.status_warning;
        fmt.write_colored("  \u{26A0} ", warning_color)?;
        fmt.write_raw(&format!(
            "Team pull encountered {} error(s); partial results applied",
            result.errors.len()
        ))?;
        fmt.newline()?;
        for err in &result.errors {
            fmt.write_muted("    - ")?;
            fmt.write_raw(err)?;
            fmt.newline()?;
        }
    }
    Ok(())
}

fn report_team_pull_error(cli: &Cli, msg: &str) -> anyhow::Result<()> {
    if cli.json {
        // Empty SyncResult + the single fatal error as a string — keeps
        // shape consistent with success/partial paths.
        let empty = crate::cloud::SyncResult::default();
        println!(
            "{}",
            team_pull_json("", &empty, std::slice::from_ref(&msg.to_string()))
        );
    } else {
        let theme = ActiveTheme::default();
        let mut out = io::stdout();
        let mut fmt = Formatter::stdout(&mut out, theme);
        let warning_color = fmt.theme().palette.status_warning;
        fmt.write_colored("  \u{26A0} ", warning_color)?;
        fmt.write_raw(msg)?;
        fmt.newline()?;
    }
    Ok(())
}

// ═══════════════════════════════════════════════════════════════════════════════
// PROJECTS - List team projects
// ═══════════════════════════════════════════════════════════════════════════════

fn execute_projects(args: &CloudProjectsArgs, cli: &Cli) -> anyhow::Result<()> {
    let config = CloudConfig::load()?;
    let token = config
        .token
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("Not logged in. Run 'cas login' first"))?;

    // Resolve team_id: --team flag overrides config
    let team_id = args
        .team
        .as_deref()
        .or(config.team_id.as_deref())
        .or(config.team_slug.as_deref());

    let team_id = match team_id {
        Some(id) => id,
        None => {
            if cli.json {
                println!(r#"{{"status":"error","message":"No team configured"}}"#);
            } else {
                let theme = ActiveTheme::default();
                let mut out = io::stdout();
                let mut fmt = Formatter::stdout(&mut out, theme);
                let warning_color = fmt.theme().palette.status_warning;
                fmt.write_colored("  \u{25CF} ", warning_color)?;
                fmt.write_raw("No team configured. Run ")?;
                fmt.write_accent("cas cloud team set <uuid>")?;
                fmt.write_raw(" first.")?;
                fmt.newline()?;
            }
            return Ok(());
        }
    };

    let url = format!("{}/api/teams/{}/projects", config.endpoint, team_id);

    match ureq::get(&url)
        .set("Authorization", &format!("Bearer {token}"))
        .call()
    {
        Ok(resp) => {
            let body: crate::cloud::TeamProjectsResponse = resp.into_json()?;

            if cli.json {
                println!("{}", serde_json::to_string(&body.projects)?);
            } else {
                let theme = ActiveTheme::default();
                let mut out = io::stdout();
                let mut fmt = Formatter::stdout(&mut out, theme);

                fmt.newline()?;
                let team_display = args
                    .team
                    .as_deref()
                    .or(config.team_slug.as_deref())
                    .unwrap_or(team_id);
                fmt.write_muted("  Team: ")?;
                fmt.write_accent(team_display)?;
                fmt.newline()?;
                fmt.newline()?;

                if body.projects.is_empty() {
                    fmt.write_muted("  No projects found.")?;
                    fmt.newline()?;
                } else {
                    // Calculate column widths for aligned output
                    let max_name = body
                        .projects
                        .iter()
                        .map(|p| p.name.len())
                        .max()
                        .unwrap_or(0)
                        .max(4);
                    let max_canonical = body
                        .projects
                        .iter()
                        .map(|p| p.canonical_id.len())
                        .max()
                        .unwrap_or(0)
                        .max(4);

                    for project in &body.projects {
                        let contrib_label = if project.contributor_count == 1 {
                            "contributor"
                        } else {
                            "contributors"
                        };
                        let mem_label = if project.memory_count == 1 {
                            "memory"
                        } else {
                            "memories"
                        };
                        fmt.write_raw(&format!(
                            "    {:<name_w$}   {:<canonical_w$}   {} {:<14}  {} {}",
                            project.name,
                            project.canonical_id,
                            project.contributor_count,
                            contrib_label,
                            project.memory_count,
                            mem_label,
                            name_w = max_name,
                            canonical_w = max_canonical,
                        ))?;
                        fmt.newline()?;
                    }
                }
                fmt.newline()?;
            }
        }
        Err(ureq::Error::Status(401, _)) => {
            if cli.json {
                println!(r#"{{"status":"error","message":"Invalid or expired token"}}"#);
            } else {
                let theme = ActiveTheme::default();
                let mut err = io::stderr();
                let mut fmt = Formatter::stdout(&mut err, theme);
                let error_color = fmt.theme().palette.status_error;
                fmt.write_colored("  \u{2717} ", error_color)?;
                fmt.write_raw("Session expired")?;
                fmt.newline()?;
                fmt.write_raw("  Run ")?;
                fmt.write_accent("cas login")?;
                fmt.write_raw(" to re-authenticate")?;
                fmt.newline()?;
            }
        }
        Err(ureq::Error::Status(403, _)) => {
            anyhow::bail!("You're not a member of this team.");
        }
        Err(e) => {
            anyhow::bail!("Failed to fetch projects: {e}");
        }
    }

    Ok(())
}

// ═══════════════════════════════════════════════════════════════════════════════
// TEAM MEMORIES
// ═══════════════════════════════════════════════════════════════════════════════

fn execute_team_memories(
    args: &CloudTeamMemoriesArgs,
    cli: &Cli,
    cas_root: &Path,
) -> anyhow::Result<()> {
    use crate::cloud::{TeamMemoriesResponse, TeamProjectsResponse};
    use crate::ui::components::{Spinner, clear_inline, render_inline_view};

    let mut config = CloudConfig::load()?;

    let team_id = config
        .team_id
        .as_ref()
        .ok_or_else(|| {
            anyhow::anyhow!("No team configured. Run `cas cloud team set <uuid>` first.")
        })?
        .clone();

    let canonical_id = crate::cloud::get_project_canonical_id().ok_or_else(|| {
        anyhow::anyhow!("Not inside a CAS project directory.")
    })?;

    let token = config
        .token
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("Not logged in. Run 'cas login' first."))?
        .clone();

    let theme = ActiveTheme::default();
    let prev_lines = if !cli.json {
        let spinner = Spinner::new("Pulling team memories...");
        render_inline_view(&spinner, &theme)?
    } else {
        0u16
    };

    // Step 1: Find the project UUID by listing team projects
    let projects_url = format!("{}/api/teams/{}/projects", config.endpoint, team_id);
    let projects_resp = ureq::get(&projects_url)
        .set("Authorization", &format!("Bearer {token}"))
        .timeout(Duration::from_secs(30))
        .call();

    let projects_body: TeamProjectsResponse = match projects_resp {
        Ok(resp) => resp.into_json()?,
        Err(ureq::Error::Status(401, _)) => {
            if prev_lines > 0 {
                clear_inline(prev_lines)?;
            }
            anyhow::bail!("Session expired. Run `cas login` to re-authenticate.");
        }
        Err(ureq::Error::Status(403, _)) => {
            if prev_lines > 0 {
                clear_inline(prev_lines)?;
            }
            anyhow::bail!("You're not a member of this team.");
        }
        Err(e) => {
            if prev_lines > 0 {
                clear_inline(prev_lines)?;
            }
            anyhow::bail!("Failed to list team projects: {e}");
        }
    };

    let project = projects_body
        .projects
        .iter()
        .find(|p| p.canonical_id == canonical_id);

    let project_uuid = match project {
        Some(p) => p.id.clone(),
        None => {
            if prev_lines > 0 {
                clear_inline(prev_lines)?;
            }
            anyhow::bail!(
                "This project hasn't been synced to the team yet. Run `cas cloud sync` while a team is configured (see `cas cloud team set <uuid>`) to register it."
            );
        }
    };

    // Step 2: Fetch team memories for this project
    let mut memories_url = format!(
        "{}/api/teams/{}/projects/{}/memories",
        config.endpoint, team_id, project_uuid
    );

    if !args.full {
        if let Some(since) = config.get_team_memory_sync(&canonical_id) {
            memories_url = format!("{memories_url}?since={since}");
        }
    }

    let memories_resp = ureq::get(&memories_url)
        .set("Authorization", &format!("Bearer {token}"))
        .timeout(Duration::from_secs(60))
        .call();

    let body: TeamMemoriesResponse = match memories_resp {
        Ok(resp) => resp.into_json()?,
        Err(ureq::Error::Status(401, _)) => {
            if prev_lines > 0 {
                clear_inline(prev_lines)?;
            }
            anyhow::bail!("Session expired. Run `cas login` to re-authenticate.");
        }
        Err(ureq::Error::Status(403, _)) => {
            if prev_lines > 0 {
                clear_inline(prev_lines)?;
            }
            anyhow::bail!("You're not a member of this team.");
        }
        Err(ureq::Error::Status(404, _)) => {
            if prev_lines > 0 {
                clear_inline(prev_lines)?;
            }
            anyhow::bail!("Project not found in this team.");
        }
        Err(e) => {
            if prev_lines > 0 {
                clear_inline(prev_lines)?;
            }
            anyhow::bail!("Failed to fetch team memories: {e}");
        }
    };

    let entry_count = body.memories.entries.len();
    let rule_count = body.memories.rules.len();
    let skill_count = body.memories.skills.len();
    let contributor_count = body.contributors.len();

    // Dry run: just show counts
    if args.dry_run {
        if prev_lines > 0 {
            clear_inline(prev_lines)?;
        }

        if cli.json {
            println!(
                "{}",
                serde_json::json!({
                    "dry_run": true,
                    "entries": entry_count,
                    "rules": rule_count,
                    "skills": skill_count,
                    "contributors": contributor_count,
                })
            );
        } else {
            let mut out = io::stdout();
            let mut fmt = Formatter::stdout(&mut out, theme);
            fmt.write_accent("  \u{2192} ")?;
            fmt.write_raw(&format!(
                "Would pull: {} entries, {} rules, {} skills from {} contributors",
                entry_count, rule_count, skill_count, contributor_count
            ))?;
            fmt.newline()?;
        }
        return Ok(());
    }

    // Check if there's anything to merge
    if entry_count == 0 && rule_count == 0 && skill_count == 0 {
        if prev_lines > 0 {
            clear_inline(prev_lines)?;
        }
        if cli.json {
            println!(r#"{{"status":"ok","message":"up_to_date"}}"#);
        } else {
            let mut out = io::stdout();
            let mut fmt = Formatter::stdout(&mut out, theme);
            let success_color = fmt.theme().palette.status_success;
            fmt.write_colored("  \u{2713} ", success_color)?;
            fmt.write_raw("Team memories are up to date.")?;
            fmt.newline()?;
        }
        return Ok(());
    }

    // Merge into local stores using LWW
    let store = open_store(cas_root)?;
    let rule_store = open_rule_store(cas_root)?;
    let skill_store = open_skill_store(cas_root)?;

    let mut entries_merged = 0usize;
    let mut entries_skipped = 0usize;
    let mut rules_merged = 0usize;
    let mut rules_skipped = 0usize;
    let mut skills_merged = 0usize;
    let mut skills_skipped = 0usize;

    // Merge entries (LWW by last_accessed or created)
    for entry in body.memories.entries {
        match store.get(&entry.id) {
            Ok(local) => {
                let local_time = local.last_accessed.unwrap_or(local.created);
                let remote_time = entry.last_accessed.unwrap_or(entry.created);
                if remote_time > local_time {
                    store.update(&entry)?;
                    entries_merged += 1;
                } else {
                    entries_skipped += 1;
                }
            }
            Err(_) => {
                store.add(&entry)?;
                entries_merged += 1;
            }
        }
    }

    // Merge rules (LWW by last_accessed or created)
    for rule in body.memories.rules {
        match rule_store.get(&rule.id) {
            Ok(local) => {
                let local_time = local.last_accessed.unwrap_or(local.created);
                let remote_time = rule.last_accessed.unwrap_or(rule.created);
                if remote_time > local_time {
                    rule_store.update(&rule)?;
                    rules_merged += 1;
                } else {
                    rules_skipped += 1;
                }
            }
            Err(_) => {
                rule_store.add(&rule)?;
                rules_merged += 1;
            }
        }
    }

    // Merge skills (LWW by updated_at)
    for skill in body.memories.skills {
        match skill_store.get(&skill.id) {
            Ok(local) => {
                if skill.updated_at > local.updated_at {
                    skill_store.update(&skill)?;
                    skills_merged += 1;
                } else {
                    skills_skipped += 1;
                }
            }
            Err(_) => {
                skill_store.add(&skill)?;
                skills_merged += 1;
            }
        }
    }

    // Save sync timestamp
    if let Some(pulled_at) = &body.pulled_at {
        config.set_team_memory_sync(&canonical_id, pulled_at);
        config.save()?;
    }

    if prev_lines > 0 {
        clear_inline(prev_lines)?;
    }

    if cli.json {
        println!(
            "{}",
            serde_json::json!({
                "status": "ok",
                "entries": { "merged": entries_merged, "skipped": entries_skipped },
                "rules": { "merged": rules_merged, "skipped": rules_skipped },
                "skills": { "merged": skills_merged, "skipped": skills_skipped },
                "contributors": contributor_count,
            })
        );
    } else {
        let mut out = io::stdout();
        let mut fmt = Formatter::stdout(&mut out, theme);
        fmt.success("Team memories synced")?;
        if entries_merged > 0 {
            fmt.write_raw(&format!("    {} entries merged", entries_merged))?;
            fmt.newline()?;
        }
        if rules_merged > 0 {
            fmt.write_raw(&format!("    {} rules merged", rules_merged))?;
            fmt.newline()?;
        }
        if skills_merged > 0 {
            fmt.write_raw(&format!("    {} skills merged", skills_merged))?;
            fmt.newline()?;
        }
        if entries_skipped + rules_skipped + skills_skipped > 0 {
            fmt.write_muted(&format!(
                "    {} skipped (local newer)",
                entries_skipped + rules_skipped + skills_skipped
            ))?;
            fmt.newline()?;
        }
    }

    Ok(())
}

// ═══════════════════════════════════════════════════════════════════════════════
// PURGE-FOREIGN - Remove foreign-project entities and re-pull
// ═══════════════════════════════════════════════════════════════════════════════

fn execute_purge_foreign(
    args: &CloudPurgeForeignArgs,
    cli: &Cli,
    cas_root: &Path,
) -> anyhow::Result<()> {
    use std::sync::Arc;

    use crate::cloud::{CloudSyncer, CloudSyncerConfig, SyncQueue, get_project_canonical_id};

    let config = CloudConfig::load()?;
    if config.token.is_none() {
        anyhow::bail!("Not logged in. Run 'cas login' first");
    }

    let project_id = get_project_canonical_id()
        .ok_or_else(|| anyhow::anyhow!("Cannot determine project ID. Not inside a CAS project?"))?;

    let store = open_store(cas_root)?;
    let task_store = open_task_store(cas_root)?;
    let rule_store = open_rule_store(cas_root)?;
    let skill_store = open_skill_store(cas_root)?;
    // cas-bba4: extra stores required by the extended `CloudSyncer::pull`
    // signature. purge-foreign only deletes content entities (entries/tasks/
    // rules/skills), so these are passed through purely to satisfy the
    // scoped pull contract — the 5 new entity kinds are repopulated from
    // cloud after the local content wipe.
    let spec_store = open_spec_store(cas_root)?;
    let event_store = open_event_store(cas_root)?;
    let prompt_store = open_prompt_store(cas_root)?;
    let file_change_store = open_file_change_store(cas_root)?;
    let commit_link_store = open_commit_link_store(cas_root)?;

    // Count entities before purge
    let entries_before = store.list().map(|v| v.len()).unwrap_or(0);
    let tasks_before = task_store.list(None).map(|v| v.len()).unwrap_or(0);
    let rules_before = rule_store.list().map(|v| v.len()).unwrap_or(0);
    let skills_before = skill_store.list(None).map(|v| v.len()).unwrap_or(0);
    let total_before = entries_before + tasks_before + rules_before + skills_before;

    if cli.json {
        if args.dry_run {
            println!(
                r#"{{"dry_run":true,"project_id":"{}","entities_before":{{"entries":{},"tasks":{},"rules":{},"skills":{},"total":{}}}}}"#,
                project_id, entries_before, tasks_before, rules_before, skills_before, total_before,
            );
            return Ok(());
        }
    } else {
        let theme = ActiveTheme::default();
        let mut out = io::stdout();
        let mut fmt = Formatter::stdout(&mut out, theme);
        fmt.newline()?;
        fmt.write_accent("  Purge Foreign Entities")?;
        fmt.newline()?;
        fmt.newline()?;
        fmt.write_muted("  Project: ")?;
        fmt.write_raw(&project_id)?;
        fmt.newline()?;
        fmt.write_muted("  Before:  ")?;
        fmt.write_raw(&format!(
            "{} entries, {} tasks, {} rules, {} skills ({} total)",
            entries_before, tasks_before, rules_before, skills_before, total_before,
        ))?;
        fmt.newline()?;

        if args.dry_run {
            fmt.newline()?;
            fmt.write_muted("  (dry run — no changes made)")?;
            fmt.newline()?;
            fmt.write_raw("  Run without --dry-run to purge and re-pull.")?;
            fmt.newline()?;
            return Ok(());
        }
    }

    // Step 1: Back up the database
    let db_path = cas_root.join("cas.db");
    let timestamp = chrono::Utc::now().format("%Y%m%d_%H%M%S");
    let backup_path = cas_root.join(format!("cas.db.pre-purge-{timestamp}"));
    if db_path.exists() {
        std::fs::copy(&db_path, &backup_path)?;
    }

    // Step 2: Delete all content entities via direct SQL
    // (Preserves: sync_queue, sync_metadata, agents, sessions, verifications,
    //  events, prompts, file_changes, commit_links, worktrees, dependencies, task_leases)
    {
        let conn = rusqlite::Connection::open(&db_path)?;
        conn.execute_batch(
            "DELETE FROM entries;
             DELETE FROM tasks;
             DELETE FROM dependencies;
             DELETE FROM rules;
             DELETE FROM skills;",
        )?;
        // Reset last_pull_at so re-pull fetches everything
        conn.execute(
            "DELETE FROM sync_metadata WHERE key = 'last_pull_at'",
            [],
        )?;
    }

    // Step 3: Re-pull from cloud with project-scoped filtering
    let queue = SyncQueue::open(cas_root)?;
    queue.init()?;
    let syncer = CloudSyncer::new(
        Arc::new(queue),
        config,
        CloudSyncerConfig::default(),
    );

    let pull_result = syncer.pull(
        store.as_ref(),
        task_store.as_ref(),
        rule_store.as_ref(),
        skill_store.as_ref(),
        spec_store.as_ref(),
        event_store.as_ref(),
        prompt_store.as_ref(),
        file_change_store.as_ref(),
        commit_link_store.as_ref(),
    )?;

    // Count entities after re-pull
    let entries_after = store.list().map(|v| v.len()).unwrap_or(0);
    let tasks_after = task_store.list(None).map(|v| v.len()).unwrap_or(0);
    let rules_after = rule_store.list().map(|v| v.len()).unwrap_or(0);
    let skills_after = skill_store.list(None).map(|v| v.len()).unwrap_or(0);
    let total_after = entries_after + tasks_after + rules_after + skills_after;

    let purged = total_before.saturating_sub(total_after);

    if cli.json {
        println!(
            r#"{{"project_id":"{}","backup":"{}","entities_before":{{"entries":{},"tasks":{},"rules":{},"skills":{},"total":{}}},"entities_after":{{"entries":{},"tasks":{},"rules":{},"skills":{},"total":{}}},"purged":{},"pull_errors":{}}}"#,
            project_id,
            backup_path.display(),
            entries_before, tasks_before, rules_before, skills_before, total_before,
            entries_after, tasks_after, rules_after, skills_after, total_after,
            purged,
            serde_json::to_string(&pull_result.errors).unwrap_or_default(),
        );
    } else {
        let theme = ActiveTheme::default();
        let mut out = io::stdout();
        let mut fmt = Formatter::stdout(&mut out, theme);
        fmt.write_muted("  After:   ")?;
        fmt.write_raw(&format!(
            "{} entries, {} tasks, {} rules, {} skills ({} total)",
            entries_after, tasks_after, rules_after, skills_after, total_after,
        ))?;
        fmt.newline()?;
        fmt.write_muted("  Purged:  ")?;
        fmt.write_raw(&format!("{} foreign entities removed", purged))?;
        fmt.newline()?;
        fmt.write_muted("  Backup:  ")?;
        fmt.write_raw(&backup_path.to_string_lossy())?;
        fmt.newline()?;

        if !pull_result.errors.is_empty() {
            fmt.newline()?;
            let warning_color = fmt.theme().palette.status_warning;
            fmt.write_colored("  \u{26A0} ", warning_color)?;
            fmt.write_raw(&format!("{} pull errors:", pull_result.errors.len()))?;
            fmt.newline()?;
            for err in &pull_result.errors {
                fmt.write_muted("    - ")?;
                fmt.write_raw(err)?;
                fmt.newline()?;
            }
        }

        fmt.newline()?;
        let success_color = fmt.theme().palette.status_success;
        fmt.write_colored("  \u{2713} ", success_color)?;
        fmt.write_raw("Purge complete. Pending local changes in sync queue are preserved.")?;
        fmt.newline()?;
    }

    Ok(())
}

#[cfg(test)]
mod team_cmd_tests {
    use super::*;
    use tempfile::TempDir;
    use wiremock::matchers::{header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    #[test]
    fn parse_team_uuid_accepts_canonical() {
        let uuid = parse_team_uuid("550e8400-e29b-41d4-a716-446655440000").unwrap();
        assert_eq!(uuid, "550e8400-e29b-41d4-a716-446655440000");
    }

    #[test]
    fn parse_team_uuid_normalises_uppercase() {
        let uuid = parse_team_uuid("550E8400-E29B-41D4-A716-446655440000").unwrap();
        assert_eq!(uuid, "550e8400-e29b-41d4-a716-446655440000");
    }

    #[test]
    fn parse_team_uuid_rejects_slug() {
        let err = parse_team_uuid("petra-stella").unwrap_err();
        assert!(err.contains("expected a team UUID"));
        assert!(err.contains("petra-stella"));
    }

    #[test]
    fn parse_team_uuid_rejects_empty() {
        assert!(parse_team_uuid("").is_err());
    }

    #[test]
    fn parse_team_uuid_rejects_too_short() {
        assert!(parse_team_uuid("abc-123").is_err());
    }

    #[test]
    fn parse_team_uuid_rejects_no_hyphen_form() {
        // uuid crate would parse this as a simple form; our length gate
        // rejects it so the stored value never drifts from canonical.
        let err = parse_team_uuid("550e8400e29b41d4a716446655440000").unwrap_err();
        assert!(err.contains("expected a team UUID"));
    }

    #[test]
    fn parse_team_uuid_rejects_braced_form() {
        let err = parse_team_uuid("{550e8400-e29b-41d4-a716-446655440000}").unwrap_err();
        assert!(err.contains("expected a team UUID"));
    }

    #[test]
    fn parse_team_uuid_rejects_urn_form() {
        let err = parse_team_uuid("urn:uuid:550e8400-e29b-41d4-a716-446655440000").unwrap_err();
        assert!(err.contains("expected a team UUID"));
    }

    #[test]
    fn format_uuid_error_uses_endpoint_dashboard_url() {
        let msg = format_uuid_error(
            "expected a team UUID, got `petra-stella`",
            "https://cas.dev",
        );
        assert!(msg.contains("got `petra-stella`"));
        assert!(msg.contains("https://cas.dev/dashboard/teams"));
        assert!(msg.contains("Slug resolution is not yet supported"));
        assert!(msg.contains("cloud.json"));
    }

    #[test]
    fn format_uuid_error_strips_trailing_slash_on_endpoint() {
        let msg = format_uuid_error("expected a team UUID, got `x`", "https://custom.host/");
        // Should not produce a double-slash in the URL.
        assert!(msg.contains("https://custom.host/dashboard/teams"));
        assert!(!msg.contains("//dashboard"));
    }

    #[test]
    fn config_set_team_roundtrip() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("cloud.json");

        let mut config = CloudConfig::default();
        config.set_team("550e8400-e29b-41d4-a716-446655440000", "<unknown>");
        config.save_to(&path).unwrap();

        let loaded = CloudConfig::load_from(&path).unwrap();
        assert_eq!(
            loaded.team_id.as_deref(),
            Some("550e8400-e29b-41d4-a716-446655440000")
        );
        assert_eq!(loaded.team_slug.as_deref(), Some("<unknown>"));

        let mut loaded = loaded;
        loaded.clear_team();
        loaded.save_to(&path).unwrap();

        let reloaded = CloudConfig::load_from(&path).unwrap();
        assert!(reloaded.team_id.is_none());
        assert!(reloaded.team_slug.is_none());
    }

    // These probe_membership tests use `tokio::task::spawn_blocking` to call
    // the synchronous `ureq`-based `probe_team_membership` from inside
    // `#[tokio::test]` (which runs on a current-thread runtime). `wiremock`
    // binds the MockServer on the test's tokio runtime; the blocking call
    // executes on tokio's separate blocking pool. `await`-ing the join
    // handle drives the runtime so the mock can serve the request — if you
    // ever replace this pattern, be sure the HTTP call still has a live
    // runtime to answer it on the other side.
    #[tokio::test]
    async fn probe_membership_returns_member_on_200() {
        let server = MockServer::start().await;
        let uuid = "550e8400-e29b-41d4-a716-446655440000";
        Mock::given(method("GET"))
            .and(path(format!("/api/teams/{uuid}/projects")))
            .and(header("Authorization", "Bearer test-token"))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(serde_json::json!({ "projects": [] })),
            )
            .mount(&server)
            .await;
        let endpoint = server.uri();

        let outcome = tokio::task::spawn_blocking(move || {
            probe_team_membership(&endpoint, "test-token", uuid)
        })
        .await
        .unwrap();

        assert_eq!(outcome, TeamProbeOutcome::Member);
    }

    #[tokio::test]
    async fn probe_membership_returns_unauthorized_on_401() {
        let server = MockServer::start().await;
        let uuid = "550e8400-e29b-41d4-a716-446655440000";
        Mock::given(method("GET"))
            .and(path(format!("/api/teams/{uuid}/projects")))
            .respond_with(ResponseTemplate::new(401))
            .mount(&server)
            .await;
        let endpoint = server.uri();

        let outcome = tokio::task::spawn_blocking(move || {
            probe_team_membership(&endpoint, "bad-token", uuid)
        })
        .await
        .unwrap();

        assert_eq!(outcome, TeamProbeOutcome::Unauthorized);
    }

    #[tokio::test]
    async fn probe_membership_returns_not_a_member_on_403() {
        let server = MockServer::start().await;
        let uuid = "550e8400-e29b-41d4-a716-446655440000";
        Mock::given(method("GET"))
            .and(path(format!("/api/teams/{uuid}/projects")))
            .respond_with(ResponseTemplate::new(403))
            .mount(&server)
            .await;
        let endpoint = server.uri();

        let outcome = tokio::task::spawn_blocking(move || {
            probe_team_membership(&endpoint, "test-token", uuid)
        })
        .await
        .unwrap();

        assert_eq!(outcome, TeamProbeOutcome::NotAMember);
    }

    #[tokio::test]
    async fn probe_membership_returns_not_found_on_404() {
        let server = MockServer::start().await;
        let uuid = "00000000-0000-0000-0000-000000000000";
        Mock::given(method("GET"))
            .and(path(format!("/api/teams/{uuid}/projects")))
            .respond_with(ResponseTemplate::new(404))
            .mount(&server)
            .await;
        let endpoint = server.uri();

        let outcome = tokio::task::spawn_blocking(move || {
            probe_team_membership(&endpoint, "test-token", uuid)
        })
        .await
        .unwrap();

        assert_eq!(outcome, TeamProbeOutcome::NotFound);
    }

    #[tokio::test]
    async fn probe_membership_returns_error_on_500() {
        let server = MockServer::start().await;
        let uuid = "550e8400-e29b-41d4-a716-446655440000";
        Mock::given(method("GET"))
            .and(path(format!("/api/teams/{uuid}/projects")))
            .respond_with(ResponseTemplate::new(500))
            .mount(&server)
            .await;
        let endpoint = server.uri();

        let outcome = tokio::task::spawn_blocking(move || {
            probe_team_membership(&endpoint, "test-token", uuid)
        })
        .await
        .unwrap();

        match outcome {
            TeamProbeOutcome::Error(msg) => assert_eq!(msg, "unexpected HTTP 500"),
            other => panic!("expected Error, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn probe_membership_returns_error_on_network_failure() {
        // Port 1 is never open on a normal machine; ureq will fail with a
        // transport error. We don't pin the exact wording because ureq's
        // Display differs across platforms, but the prefix is ours.
        let endpoint = "http://127.0.0.1:1".to_string();
        let uuid = "550e8400-e29b-41d4-a716-446655440000";

        let outcome = tokio::task::spawn_blocking(move || {
            probe_team_membership(&endpoint, "test-token", uuid)
        })
        .await
        .unwrap();

        match outcome {
            TeamProbeOutcome::Error(msg) => {
                assert!(
                    msg.starts_with("network error:"),
                    "expected `network error: ...`, got: {msg}"
                );
            }
            other => panic!("expected Error, got {other:?}"),
        }
    }
}

// T4 team-sync tests now live in `cas-cli/tests/team_sync_test.rs` — an
// integration-test binary that exercises `execute_team_push` end-to-end
// with wiremock. Extracted per the task-verifier feedback on cas-1f44:
// tests are easier to find in the integration tree than buried in this
// 2.4k-line CLI file.
