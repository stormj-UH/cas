# CAS Architecture

## Workspace Layout

The root `Cargo.toml` defines a workspace. `cas-cli/` is the main binary crate; `crates/` contains library crates.

**Core data flow**: CLI commands and MCP tool calls both go through the store trait abstractions in `cas-cli/src/store/`, which wraps `cas-store` (SQLite) with notification and sync layers.

### cas-cli (main crate) — `cas-cli/src/`

| Module | Purpose |
|--------|---------|
| `main.rs` / `lib.rs` | Entry point, module declarations |
| `cli/` | Clap command definitions and handlers. `mod.rs` has the `Commands` enum — add new subcommands here. |
| `mcp/` | MCP server: `server/` (CasCore with cached OnceLock stores), `tools/` (55 tool handlers split into `core/` and `service/`), `daemon.rs` (embedded background maintenance), `socket.rs` (notification socket) |
| `store/` | Re-exports from `cas-store` + wrappers: `notifying_*.rs` (emit change notifications), `syncing_*.rs` (sync to `.claude/` filesystem), `layered.rs` (project + global store composition), `detect.rs` (find `.cas/` root) |
| `hooks/` | Claude Code hook event handlers (SessionStart, Stop, PostToolUse, etc.). `handlers/` has session, state, event, and middleware handlers. `scorer.rs` ranks context items for injection. |
| `migration/` | Forward-only schema migrations. `migrations/` has individual migration files (m001-m182+). `detector.rs` introspects existing schema. |
| `ui/` | Ratatui TUI components for factory view: `factory/`, `components/`, `widgets/`, `theme/`, `markdown/` |
| `config/` | Configuration loading from `.cas/config.yaml` |
| `orchestration/` | Agent name generation and orchestration logic |
| `worktree/` | Git worktree management for factory workers |
| `consolidation/` | Memory consolidation and decay |
| `extraction/` | AI-powered extraction of observations into structured memory |
| `bridge/` | Local helper server for external tool integration |
| `cloud/` | CAS Cloud sync (optional) |
| `sync/` | Filesystem sync to `.claude/rules/` and `.claude/skills/` |

### Workspace Crates — `crates/`

| Crate | Purpose |
|-------|---------|
| `cas-types` | Shared data types (Entry, Task, Rule, Skill, Agent, etc.) |
| `cas-store` | SQLite storage layer — trait definitions (`Store`, `TaskStore`, `RuleStore`, etc.) and `SqliteStore` implementation |
| `cas-search` | Full-text search via Tantivy (BM25 scoring) |
| `cas-core` | Core business logic, hooks framework, search index abstraction, skill/rule syncing |
| `cas-mcp` | MCP protocol types and request/response models |
| `cas-factory` | Factory session lifecycle: `FactoryCore`, config, director, recording, notifications |
| `cas-factory-protocol` | WebSocket message protocol between supervisor and worker agents |
| `cas-mux` | Terminal multiplexer layout and rendering (side-by-side/tabbed agent views) |
| `cas-pty` | PTY management for agent terminal sessions |
| `cas-recording` | Terminal session recording and playback |
| `cas-code` | Code analysis via tree-sitter |
| `cas-diffs` | Diff parsing, rendering, syntax highlighting |
| `cas-tui-test` | TUI testing framework |
| `ghostty_vt` / `ghostty_vt_sys` | Virtual terminal parser (based on Ghostty) |

### Key Patterns

**Store trait hierarchy**: `cas-store` defines traits (`Store`, `TaskStore`, `RuleStore`, `SkillStore`, `EntityStore`, `AgentStore`, `VerificationStore`, `WorktreeStore`). `SqliteStore` implements all of them. `cas-cli/src/store/` wraps these with notification and sync decorators.

**CasCore (MCP server)**: Lives in `cas-cli/src/mcp/server/mod.rs`. Caches all store instances in `OnceLock` fields — each store type opened exactly once per server lifetime. Has an embedded daemon for background maintenance (embedding generation every 2min, full maintenance every 30min).

**`cas serve` project-root resolution** (`cas-cli/src/mcp/server/runtime.rs::resolve_mcp_serve_root`): Priority order: (1) `CLAUDE_PROJECT_DIR` env var — Claude Code 2.1.139+ sets this when spawning a stdio MCP server, eliminating cwd-mismatch failures; (2) `CAS_ROOT` env var (explicit override); (3) git-worktree detection; (4) directory walk from cwd. Falls back silently to (2)–(4) when `CLAUDE_PROJECT_DIR` is unset or points at a non-existent path.

**CasContext**: In `cas-cli/src/store/mod.rs`. Resolves the `.cas/` directory once at CLI entry points and passes it through — enables deterministic test behavior.

**Hook scoring**: `cas-cli/src/hooks/scorer.rs` ranks context items (memories, tasks, rules, skills) by relevance for injection into SessionStart context, staying within a token budget.

**Team scope resolution chain** (`cas-cli/src/cloud/config.rs::active_team_id`, cas-ea2f5): When a write is dual-enqueued to the team push queue, the team UUID is resolved at `open_store` time via a four-step chain. (0) Kill-switch: if `team_auto_promote = Some(false)` in the project `.cas/cloud.json`, the result is always `None` — no team dual-enqueue regardless of other config. (1) Project-level explicit override: `team_id` in the project `.cas/cloud.json` wins unconditionally; set via `cas cloud team set <uuid>`. (2) User default: `default_team_id` in `~/.cas/cloud.json`, populated by `cas cloud team default <slug>` or automatically by `fetch_and_cache_teams` (`cloud/me.rs`) on `cas login`. (3) Implicit single-team auto-pick: if `teams[]` has exactly one entry and no `default_team_id` is set, that team is used automatically — no configuration needed. (4) `None` — ambiguous (0 or ≥2 teams without a nominated default) or not logged in. The testable inner `active_team_id_with_user_config(user_cfg: Option<&CloudConfig>)` accepts an injected user config for unit tests without disk I/O; the production `active_team_id()` reads from `user_level_cloud_json_path()` (honours the `CAS_USER_CLOUD_JSON` test-seam env var).
