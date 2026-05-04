# cas — Codemap
> Auto-generated structural map. Regenerate with `/codemap` when the layout drifts (modules added, removed, or renamed).

## Top-level layout
- `cas-cli/` — Rust binary crate (`cas`); CLI commands, hooks, TUI, MCP server entrypoint
- `crates/` — workspace member crates (16 crates; see below)
- `docs/` — planning docs (brainstorms, ideation, requests, spikes, onboarding)
- `migration/` — one-shot migration scripts and phase logs (cloud move)
- `scripts/` — `worktree-boot.sh` only (the rest live in `~/.local/bin/`)
- `homebrew/` — `cas.rb` formula + update script
- `slack-bridge/` — separate TypeScript service for Slack integration
- `site/` — static landing page (`index.html`, PDF)
- `vendor/` — vendored upstream sources (`ghostty/`)
- `target/` — cargo build output (skip)
- `.claude/` — harness config (`settings.json`), `.claude/agents/` project-local subagents, `.claude/skills/` project-local skill surface (sync output of `cas integrate`)
- `.codex/` — Codex CLI mirror: `agents/`, `skills/`, `config.toml`. Auto-built from `.claude/` by the codex-mirror flow (see commit 83165a3); used when running with `--supervisor-cli codex` or `--worker-cli codex`
- `.cas/` — agent state, factory config, codemap-pending tracker

## Workspace / packages
Top-level `Cargo.toml` defines a workspace. The binary lives in `cas-cli`; everything else is a library crate consumed by it.

- `cas-cli` — binary crate `cas`. Glue between CLI commands, hooks, TUI, MCP server, and the daemon.
- `crates/cas-types` — shared types (Task, Agent, Memory, HookInput, etc.) used across all crates
- `crates/cas-store` — SQLite storage layer, schema, migrations
- `crates/cas-search` — hybrid search: BM25 + semantic vectors over memories/tasks/code
- `crates/cas-core` — business logic and hook context computation
- `crates/cas-code` — code indexing and symbol search
- `crates/cas-mcp` — MCP server protocol handlers
- `crates/cas-mcp-proxy` — MCP proxy engine
- `crates/cas-factory` — factory orchestration (worker spawn, lease, merge pipeline); per-worker spec cascade resolver
- `crates/cas-factory-protocol` — wire types for factory client-server messaging
- `crates/cas-mux` — terminal multiplexer for factory TUI panes; per-worker `WorkerSpec` (cli/model/effort)
- `crates/cas-pty` — PTY management; `PtyConfig::claude` and `PtyConfig::codex` constructors
- `crates/cas-recording` — asciinema-style terminal recording
- `crates/cas-diffs` — diff parsing, rendering, syntax highlighting
- `crates/cas-tui-test` — PTY-based TUI test framework
- `crates/ghostty_vt` — safe Rust wrapper for libghostty-vt terminal emulation
- `crates/ghostty_vt_sys` — `-sys` crate with low-level bindings to libghostty-vt

## cas-cli (`cas-cli/src/`)

Binary entrypoint and the only crate users interact with directly. Contains every CLI subcommand, the hook dispatcher, the factory TUI, and the MCP server bootstrap.

- `main.rs`, `lib.rs` — entrypoint and library root
- `cli/` — every CLI subcommand (one file per command):
  - `mod.rs` — top-level `clap` dispatch
  - `auth.rs`, `device.rs`, `cloud.rs` — cloud/auth flows
  - `codemap_cmd.rs` — `cas codemap status|pending|clear`
  - `project_overview_cmd.rs` — `cas project-overview clear`
  - `factory/` — factory subcommands (`is-wedged`, `kill`, `debug`, `daemon`, etc.); `factory/mod.rs` builds `FactoryConfig` and launches the daemon
  - `factory_tooling.rs` — `cas init` worktree helper templates (`.env.worktree.template`, `worktree-boot.sh`, gitignore entries)
  - `hook.rs`, `hook/` — `cas hook` dispatcher (called from settings.json)
  - `hook_tests/` — golden-JSON hook tests
  - `init/`, `init.rs` — `cas init` (writes CLAUDE.md, .claude/, .cas/)
  - `integrate/` — `cas integrate <platform> <action>` for Vercel/Neon/GitHub auto-integration
  - `known_repos.rs`, `open.rs` — known-repos DB and project picker
  - `update/`, `update.rs`, `update_transaction.rs`, `update_tests/` — `cas update` rewrites managed_by:cas files atomically
  - `mcp_cmd.rs`, `memory.rs`, `queue.rs`, `worktree.rs`, `doctor.rs`, `status.rs`, `list.rs`, `sweep.rs`, `bridge.rs`, `changelog.rs`, `claude_md.rs`, `interactive.rs`
  - `config/`, `config_tui/`, `config_tui.rs` — config read/write + the config TUI
  - `statusline/`, `statusline.rs` — `cas statusline` for shell prompts
- `hooks/` — hook input handling
  - `mod.rs`, `handlers.rs`, `handlers/` — `SessionStart`, `PreToolUse`, `PostToolUse`, `Stop`, `Notification` handlers
  - `handlers/handlers_events/` — codemap freshness, project-overview drift, notifications, pre-tool gates
  - `handlers/handlers_middle/` — post-tool, session-stop, session-hygiene
  - `handlers/session_hygiene.rs` — SessionStart WIP triage banner
  - `context.rs`, `scorer.rs`, `transcript.rs` — hook context assembly
  - `types.rs` — hook input/output schema
- `mcp/` — MCP server
  - `daemon.rs`, `mod.rs`, `socket.rs` — server lifecycle, unix socket
  - `server/` — request routing
  - `tools/` — every MCP tool (`task`, `memory`, `coordination`, `search`, `pattern`, `rule`, `skill`, `spec`, `system`, `team`, `verification`)
  - `daemon_tests/`
- `store/` — storage adapter on top of cas-store
  - `mod.rs`, `layered.rs` — composed store (project + global)
  - `notifying_*.rs`, `syncing_*.rs` — observer + cloud-sync wrappers per entity
  - `markdown.rs` — markdown serialization for memories
  - `detect.rs` — repo/scope detection
- `daemon/` — background maintenance
  - `mod.rs`, `maintenance.rs` — periodic cycle (decay, prune, checkpoint)
  - `decay.rs`, `indexing.rs`, `observation.rs`, `queue.rs`, `watcher.rs`
- `cloud/` — cloud sync (`coordinator.rs`, `syncer/`, `sync_queue/`, `config.rs`, `device.rs`)
- `sync/` — skill/agent sync from `builtins/` to `.claude/` (`mod.rs`, `skills.rs`, `skills_tests/`)
- `ui/` — TUI
  - `factory/` — multi-pane factory TUI (the `cas` binary launches into this)
  - `factory/protocol.rs` — `ClientMessage::SpawnWorkers` and the daemon ↔ TUI/cloud client wire schema
  - `factory/daemon/` — daemon process lifecycle, cloud client, runtime (ws_client, gui_client, queue_and_events, teams)
  - `factory/app/` — `FactoryApp` state, render/ops, init, epic_workers
  - `factory/director/` — director pane prompts and rendering
  - `components/`, `widgets/`, `markdown/`, `theme/`
- `bridge/` — HTTP bridge server for the local web UI; `bridge/server/factory.rs` is the factory-start endpoint
- `builtins.rs` + `builtins/` — embedded skills, agents, and content
  - `builtins/skills/` — Claude-variant SKILL.md files (cas-* skills, codemap, project-overview, fallow); cas-code-review now has 5 always-on personas including `fallow.md`
  - `builtins/codex/skills/` — Codex-variant mirror (full parity with `builtins/skills/`)
  - `builtins/agents/` — task-verifier, learning-reviewer, rule-reviewer, duplicate-detector, factory-supervisor, etc.
  - `builtins/codex/agents/` — Codex-variant mirror of agents
  - `BUILTIN_SKILLS` / `CODEX_BUILTIN_SKILLS` arrays drive `cas sync`
  - `supervisor_guidance()` / `worker_guidance()` — SessionStart bundles
- `extraction/` — memory/learning extraction from transcripts
- `consolidation/` — memory consolidation passes
- `hybrid_search/` — search frontend on top of cas-search
- `migration/` — schema migrations
- `notifications/` — notification dispatch
- `orchestration/` — worker name allocation
- `rules/` — rule loading and application
- `telemetry/`, `tracing/`, `otel.rs`, `sentry.rs`, `logging.rs` — observability
- `worktree/` — worktree creation, salvage, cleanup
- `harness_policy.rs`, `agent_id.rs`, `duplicate_check.rs`, `error.rs`, `async_runtime.rs`

## crates/cas-factory (`crates/cas-factory/src/`)

Factory orchestration: spawn pipeline, lease management, merge gates, per-worker spec resolution.

- `lib.rs` — public surface (`FactoryCore`, `FactoryConfig`, spec resolver re-exports)
- `core.rs` — `FactoryCore` (worker lifecycle, lease ownership, status reporting)
- `config.rs` — `FactoryConfig` struct (workers, names, supervisor/worker `cli`/`model`/`effort`, `resolved_worker_specs`, `resolved_supervisor_spec`)
- `spec_resolver.rs` — `resolve_specs(workers, sources)` 6-layer cascade (built-in → user `~/.cas/config.toml` → project `.cas/config.toml` → CLI flags → `--worker-spec` JSON)
- `tests/spec_resolver.rs` — 22 unit tests covering each cascade layer
- `changes.rs`, `notify.rs`, `recording.rs`, `director.rs` — supporting subsystems
- `session/` — session state and cleanup

## crates/cas-mux (`crates/cas-mux/src/`)

Terminal multiplexer that owns every PTY pane in the factory TUI.

- `lib.rs` — public surface (`Mux`, `MuxConfig`, `WorkerSpec`, `Effort`)
- `mux.rs` — `Mux` and `MuxConfig`; `factory()` constructor and `factory_pane_configs()` helper for tests
- `spec.rs` — `WorkerSpec { name, cli, model, effort }`, `Effort` enum with `as_claude_arg()` / `as_codex_config()`, `WorkerSpec::builtin_default()`
- `pane/` — `Pane` constructors; `build_worker_config` / `build_supervisor_config` branch on `cli` to `PtyConfig::claude` vs `PtyConfig::codex`
- `harness.rs`, `render.rs`, `error.rs`
- `mux_tests/` — `factory_pane_configs` tests verifying config → CLI argv chain

## docs/

Planning artifacts only — product/domain content goes in `docs/PRODUCT_OVERVIEW.md` (see `project-overview` skill).

- `brainstorms/` — `YYYY-MM-DD-<topic>-requirements.md` from the `cas-brainstorm` skill
- `ideation/` — survivor lists from the `cas-ideate` skill
- `requests/` — cross-team BUG/FEATURE inboxes; `requests/completed/` is closed work
- `spikes/` — investigation outputs (e.g., `2026-05-01-factory-agent-teams-enrollment-spike.md`)
- `onboarding/` — onboarding notes (`macbook-from-zero.md`, etc.)
- `compound-engineering-roadmap.md`, `verifier-dispatch-trace.md`, `FEATURE-REQUEST-*`, `SCOPE-*` — standalone planning docs

## Cross-cutting

- **Tests:** Rust convention — inline `#[cfg(test)] mod tests` per file, plus `cas-cli/tests/` integration tests (`integrate_lifecycle_test.rs`, `mcp_proxy_test.rs`, `code_review_e2e_test.rs`). PTY-based TUI tests use `crates/cas-tui-test`.
- **Docs:** `README.md`, `CONTRIBUTING.md`, `CHANGELOG.md`, `CAS-DEEP-DIVE.md` at repo root; CLAUDE.md cascades from `~/CLAUDE.md` → `Petrastella/CLAUDE.md` → `cas-src/CLAUDE.md`.
- **Tooling / scripts:** `scripts/worktree-boot.sh`; release/install/bootstrap scripts live in `~/.local/bin/`. `homebrew/cas.rb` is the formula.
- **Config:** `.claude/settings.json` (harness hooks + permissions), `.codex/config.toml` (Codex CLI registers cas MCP server), `.mcp.json` (MCP servers), `.cas/config.toml` (factory knobs), `Cargo.toml` (workspace + profiles).
- **Migration:** one-shot scripts in `migration/` (Phase 2/3/7/8 logs from the cloud move). Not active build infra.

## Entrypoints

- CLI: `cas-cli/src/main.rs` → binary `cas` (users run `cas`)
- TUI: `cas-cli/src/ui/factory/app/mod.rs` (the `cas` binary defaults to launching the factory TUI)
- MCP server: `cas-cli/src/mcp/daemon.rs` (started via `cas serve`, managed as a long-running daemon)
- Hook dispatch: `cas-cli/src/cli/hook.rs` (`cas hook <event>` invoked from `.claude/settings.json`)
- Tests: `cargo test -p cas` for cas-cli; `cargo test --workspace` for everything
- Build: `cargo build --release` then restart any running `cas serve` (factory work depends on the daemon matching HEAD)
