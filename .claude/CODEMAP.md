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
- `.claude/` — harness config (`settings.json`), `.claude/agents/` project-local subagents (e.g., `macos-onboarding-reviewer`), `.claude/skills/` project-local skill surface (sync output of `cas integrate`)
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
- `crates/cas-factory` — factory orchestration (worker spawn, lease, merge pipeline)
- `crates/cas-factory-protocol` — wire types for factory client-server messaging
- `crates/cas-mux` — terminal multiplexer for factory TUI panes
- `crates/cas-pty` — PTY management
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
  - `factory/` — factory subcommands (`is-wedged`, `kill`, `debug`)
  - `factory_tooling.rs` — `cas init` worktree helper templates (`.env.worktree.template`, `worktree-boot.sh`, gitignore entries)
  - `hook.rs`, `hook/` — `cas hook` dispatcher (called from settings.json)
  - `hook_tests/` — golden-JSON hook tests
  - `init/`, `init.rs` — `cas init` (writes CLAUDE.md, .claude/, .cas/)
  - `integrate/` — `cas integrate <platform> <action>` for Vercel/Neon/GitHub auto-integration; `vercel.rs`, `neon.rs`, `github.rs`, `proxy.rs`, `integrations.rs`, `keep_block.rs`, `templates/`, `fixtures/`
  - `known_repos.rs` — `cas known-repos list|seed` over `~/.cas/cas.db::known_repos`
  - `open.rs` — `cas open` interactive TUI project picker (scans `~/projects/`)
  - `update/`, `update.rs`, `update_transaction.rs`, `update_tests/` — `cas update` rewrites managed_by:cas files atomically with rollback
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
- `cloud/` — cloud sync
  - `coordinator.rs`, `syncer/`, `sync_queue/` — push/pull
  - `config.rs`, `device.rs`
- `sync/` — skill/agent sync from `builtins/` to `.claude/`
  - `mod.rs`, `skills.rs`, `skills_tests/`
- `ui/` — TUI
  - `factory/` — multi-pane factory TUI (the `cas` binary launches into this)
  - `components/`, `widgets/`, `markdown/`, `theme/`
- `builtins.rs` + `builtins/` — embedded skills, agents, and content
  - `builtins/skills/` — claude-variant SKILL.md files (cas-* skills, codemap, project-overview, fallow); each cas-code-review/references/personas/ now includes fallow.md (5th always-on persona)
  - `builtins/codex/skills/` — codex-variant mirror
  - `cas-cli/.cursor/skills/` — Cursor IDE skill mirror (subset of builtins for editor sessions)
  - `builtins/agents/` — task-verifier, learning-reviewer, rule-reviewer, duplicate-detector, etc.
  - `BUILTIN_SKILLS` / `CODEX_BUILTIN_SKILLS` arrays drive `cas sync`
  - `supervisor_guidance()` / `worker_guidance()` — SessionStart bundles
- `bridge/` — codex/cli bridges
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

## docs/

Planning artifacts only — product/domain content goes in `docs/PRODUCT_OVERVIEW.md` (see `project-overview` skill).

- `brainstorms/` — `YYYY-MM-DD-<topic>-requirements.md` from the `cas-brainstorm` skill
- `ideation/` — survivor lists from the `cas-ideate` skill
- `requests/` — cross-team BUG/FEATURE inboxes; `requests/completed/` is closed work
- `spikes/` — investigation outputs
- `onboarding/` — onboarding notes (`macbook-from-zero.md`, etc.)
- `compound-engineering-roadmap.md`, `verifier-dispatch-trace.md`, `FEATURE-REQUEST-*`, `SCOPE-*` — standalone planning docs

## Cross-cutting

- **Tests:** Rust convention — inline `#[cfg(test)] mod tests` in each file, plus `cas-cli/tests/` integration tests (e.g., `integrate_lifecycle_test.rs`, `mcp_proxy_test.rs`, `code_review_e2e_test.rs`). PTY-based TUI tests use `crates/cas-tui-test`.
- **Docs:** `README.md`, `CONTRIBUTING.md`, `CHANGELOG.md`, `CAS-DEEP-DIVE.md` at repo root; CLAUDE.md cascades from `~/CLAUDE.md` → `Petrastella/CLAUDE.md` → `cas-src/CLAUDE.md`.
- **Tooling / scripts:** `scripts/worktree-boot.sh`; release/install/bootstrap scripts live in `~/.local/bin/`. `homebrew/cas.rb` is the formula.
- **Config:** `.claude/settings.json` (harness hooks + permissions), `.mcp.json` (MCP servers), `.cas/config.toml` (factory knobs), `Cargo.toml` (workspace + profiles).
- **Migration:** one-shot scripts in `migration/` (Phase 2/3/7/8 logs from the cloud move). Not active build infra.

## Entrypoints

- CLI: `cas-cli/src/main.rs` → binary `cas` (also aliased; users run `cas`)
- TUI: `cas-cli/src/ui/factory/app/mod.rs` (the `cas` binary defaults to launching the factory TUI)
- MCP server: `cas-cli/src/mcp/daemon.rs` (started via `cas serve` and managed as a long-running daemon)
- Hook dispatch: `cas-cli/src/cli/hook.rs` (`cas hook <event>` invoked from `.claude/settings.json`)
- Tests: `cargo test -p cas` for cas-cli; `cargo test --workspace` for everything
- Build: `cargo build --release` then restart any running `cas serve` (factory work depends on the daemon matching HEAD)
