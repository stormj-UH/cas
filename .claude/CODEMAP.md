# cas — Codemap
> Auto-generated structural map. Regenerate with `/codemap` when the layout drifts (modules added, removed, or renamed).

## Top-level layout
- `cas-cli/` — Rust binary crate (`cas`); CLI commands, hooks, factory TUI, MCP server, bridge HTTP server, daemon
- `crates/` — 16 workspace member crates (see Workspace section)
- `docs/` — planning artifacts (brainstorms, ideation, requests/inbox, spikes, onboarding); requests/completed/ archives closed work
- `migration/` — one-shot migration scripts and phase logs (Phase 2/3/7/8 cloud move)
- `scripts/` — `worktree-boot.sh` + provisioning (`provision-hetzner.sh`); release/install scripts live in `~/.local/bin/`
- `homebrew/` — `cas.rb` Homebrew formula + update script
- `slack-bridge/` — separate TypeScript service for Slack integration
- `site/` — static landing page (`index.html`, PDF)
- `vendor/` — vendored upstream sources (`ghostty/`)
- `target/` — cargo build output (skip)
- `.claude/` — harness config (`settings.json`, `CODEMAP.md` — gitignored, regen per-developer); `.claude/agents/` + `.claude/skills/` are sync output of `cas integrate`
- `.codex/` — Codex CLI mirror: `agents/`, `skills/`, `config.toml`; auto-built from `.claude/` for `--supervisor-cli codex` / `--worker-cli codex`
- `.cas/` — agent state, factory config, codemap-pending tracker; `.cas/worktrees/` houses isolated factory worker checkouts
- `.context/` — gitignored vendored toolchains (currently `zig/`); export `ZIG=$PWD/.context/zig/zig` before any cargo build that pulls ghostty-vt
- `.fallow/` — fallow static-analysis cache (sub-second JS/TS audits invoked from the `fallow` skill)
- `.cargo/` — cargo config (release profile knobs, registry overrides)
- `.github/` — CI workflows
- Root files: `README.md`, `CHANGELOG.md`, `CONTRIBUTING.md`, `CLAUDE.md`, `CAS-DEEP-DIVE.md`, `LICENSE`, `Cargo.toml`/`Cargo.lock`, `.mcp.json`, `.env.worktree.template`, `casdemo.png`, `investigation-mcp-worktree.md`

## Workspace / packages
Top-level `Cargo.toml` defines a workspace (`resolver = 2`). The binary lives in `cas-cli`; everything else is a library crate consumed by it. Release profiles enforce `panic = "unwind"` (MCP catcher requirement).

- `cas-cli` — binary crate `cas`. Glue between CLI commands, hooks, TUI, MCP server, daemon, bridge
- `crates/cas-types` — shared types (Task, Agent, Memory, HookInput, etc.) used across all crates
- `crates/cas-store` — SQLite storage layer, schema, migrations; `TaskStore` trait + `remove_dependency_of_type` (cas-6009)
- `crates/cas-search` — hybrid search: BM25 + semantic vectors over memories/tasks/code
- `crates/cas-core` — business logic and hook context computation
- `crates/cas-code` — code indexing and symbol search
- `crates/cas-mcp` — MCP server protocol handlers
- `crates/cas-mcp-proxy` — MCP proxy engine
- `crates/cas-factory` — factory orchestration (worker spawn, lease, merge gates); per-worker spec cascade resolver; director event detector (idle-vs-pending guard, heartbeat-vs-activity guard)
- `crates/cas-factory-protocol` — wire types for factory client-server messaging
- `crates/cas-mux` — terminal multiplexer for factory TUI panes; per-worker `WorkerSpec` (cli/model/effort); alt-screen wheel-forwarding via `ScrollAction::AltScreen`
- `crates/cas-pty` — PTY management; `PtyConfig::claude` and `PtyConfig::codex` constructors
- `crates/cas-recording` — asciinema-style terminal recording
- `crates/cas-diffs` — diff parsing, rendering, syntax highlighting
- `crates/cas-tui-test` — PTY-based TUI test framework
- `crates/ghostty_vt` — safe Rust wrapper for libghostty-vt terminal emulation
- `crates/ghostty_vt_sys` — `-sys` crate with low-level bindings to libghostty-vt

## cas-cli (`cas-cli/src/`)

Binary entrypoint and the only crate users interact with directly. Contains every CLI subcommand, the hook dispatcher, the factory TUI, the MCP server bootstrap, and the bridge HTTP server.

- `main.rs`, `lib.rs` — entrypoint and library root
- `cli/` — every CLI subcommand:
  - `mod.rs` — top-level `clap` dispatch
  - `auth.rs`, `device.rs`, `cloud.rs` — cloud/auth flows
  - `codemap_cmd.rs` — `cas codemap status|pending|clear`; `status` now delegates to `check_codemap_freshness` (single source of truth, cas-2de1)
  - `project_overview_cmd.rs` — `cas project-overview clear`
  - `factory/` — factory subcommands; `factory/mod.rs` builds `FactoryConfig` and launches the daemon
  - `factory_tooling.rs` — `cas init` worktree helper templates
  - `hook.rs`, `hook/` — `cas hook` dispatcher (called from settings.json); exec-form `args: [...]` emitters in `config/config_gen.rs` (cas-9a60)
  - `hook_tests/` — golden-JSON hook tests
  - `init/`, `init.rs` — `cas init` (writes CLAUDE.md, .claude/, .cas/); `init/docs_and_skill.rs` houses `CAS_DIRECTIVE_CONTENT` template + ancestor-dedup walker (cas-253e)
  - `integrate/` — `cas integrate <platform> <action>` for Vercel/Neon/GitHub
  - `known_repos.rs`, `open.rs` — known-repos DB and project picker
  - `update/`, `update.rs`, `update_transaction.rs`, `update_tests/` — `cas update` rewrites managed_by:cas files atomically
  - `mcp_cmd.rs`, `memory.rs`, `queue.rs`, `worktree.rs`, `doctor.rs`, `status.rs`, `list.rs`, `sweep.rs`, `bridge.rs`, `changelog.rs`, `claude_md.rs`, `interactive.rs`
  - `config/`, `config_tui/`, `config_tui.rs` — config read/write + the config TUI
  - `statusline/`, `statusline.rs` — `cas statusline` for shell prompts
- `hooks/` — hook input handling
  - `mod.rs`, `handlers.rs`, `handlers/` — `SessionStart`, `PreToolUse`, `PostToolUse`, `Stop`, `Notification` handlers
  - `handlers/handlers_events/` — codemap freshness (git-only after cas-2de1), project-overview drift, notifications, pre-tool gates
  - `handlers/handlers_middle/` — post-tool, session-stop, session-hygiene; `post_tool.rs` carries `is_file_within_project` (cas-9aeb ripple-check scoping)
  - `handlers/handlers_tests/ripple_path_scope.rs` — project-boundary tests for ripple-check (cas-9aeb)
  - `handlers/handlers_session.rs`, `handlers_state.rs`, `session_hygiene.rs` — session lifecycle + WIP triage banner
  - `context.rs`, `scorer.rs`, `transcript.rs` — hook context assembly
- `mcp/` — MCP server
  - `daemon.rs`, `mod.rs`, `socket.rs` — server lifecycle, unix socket
  - `server/` — request routing (`mod.rs`, `prompts.rs`, `resources.rs`, `runtime.rs`)
  - `tools/core/` — every MCP tool, grouped: `agent_coordination/` (factory ops + `task_claiming` with supervisor force-transfer, cas-3ed5), `memory.rs` (auto-promote team_id from CloudConfig, cas-6d96), `rules.rs`, `search.rs`, `skills.rs`, `system.rs`, `task/` (close_ops with commit-claim gate + zero-commit routing — cas-490f, cas-ee2b), `workflow/`, `knowledge.rs`, `maintenance.rs`
  - `tools/service/`, `tools/types/` — tool plumbing; `RememberRequest.personal: Option<bool>` (cas-6d96)
  - `daemon_tests/`
- `store/` — storage adapter on top of cas-store
  - `mod.rs`, `layered.rs` — composed store (project + global)
  - `notifying_*.rs` (entry/rule/skill/task) — observer wrappers
  - `syncing.rs` + `syncing_*.rs` (entry/skill/task) — cloud-sync wrappers
  - `share_policy.rs` — sharing rules between project/team/global scopes
  - `markdown.rs` — markdown serialization for memories
  - `detect.rs`, `known_repos.rs` — repo/scope detection
  - `mock/` — in-memory test stores
- `daemon/` — background maintenance (decay, prune, checkpoint, queue, watcher, observation, indexing)
- `cloud/` — cloud sync (`coordinator.rs`, `syncer/` with strict `entity_matches_project` per cas-6479, `sync_queue/`, `config.rs`, `device.rs`)
- `sync/` — skill/agent sync from `builtins/` to `.claude/` (`mod.rs`, `skills.rs`, `skills_tests/`)
- `ui/` — TUI
  - `factory/` — multi-pane factory TUI (the `cas` binary launches into this)
  - `factory/boot.rs`, `factory/boot/` — startup sequencing
  - `factory/protocol.rs` — `ClientMessage::SpawnWorkers`, `MouseScrollUp/Down`, `Input` — daemon ↔ TUI/cloud client wire schema
  - `factory/daemon/` — daemon process lifecycle, cloud client, runtime (ws_client, gui_client, queue_and_events, teams, fork_first)
  - `factory/app/` — `FactoryApp` state, render/ops, panels_and_modes; `sidecar_and_selection.rs` houses `ScrollAction::AltScreen` + arrow-key forwarding for alt-screen TUIs (cas-3b18 / cas-d5fa lineage)
  - `factory/renderer/` — buffer composition for pane drawing
  - `factory/director/` — director pane prompts and rendering
  - `factory/buffer_backend.rs`, `factory/phoenix.rs`, `factory/notification.rs`, `factory/status_bar.rs`, `factory/input.rs`, `factory/layout.rs`, `factory/session.rs`, `factory/client.rs`, `factory/client_input.rs` (mouse-event → PTY plumbing)
  - `components/`, `widgets/`, `markdown/`, `theme/`
- `bridge/` — HTTP bridge server (web UI backend); `bridge/server/factory.rs` is the factory-start endpoint
- `builtins.rs` + `builtins/` — embedded skills, agents, content
  - `builtins/skills/` — Claude-variant SKILL.md files: cas-supervisor, cas-worker, cas-search, cas-brainstorm, cas-memory-management, cas-task-tracking, cas-code-review (5 always-on personas including `fallow.md`), cas-supervisor-checklist, cas-ideate, codemap, project-overview, fallow, session-learn, verify-before-claim, cas-nuxt-playwright (Nuxt 3/4 + Playwright E2E; replaces cas-playwright-debug)
  - `builtins/codex/skills/` — Codex-variant mirror (full parity)
  - `builtins/agents/` — task-verifier, learning-reviewer, rule-reviewer, duplicate-detector, factory-supervisor, etc.
  - `builtins/codex/agents/` — Codex-variant mirror of agents
  - `BUILTIN_SKILLS` / `CODEX_BUILTIN_SKILLS` arrays drive `cas sync`
  - `supervisor_guidance()` / `worker_guidance()` — SessionStart bundles
- `extraction/` — memory/learning extraction from transcripts; `extract_learnings_async/sync` are the existing path session-learn auto-trigger will parallel
- `consolidation/` — memory consolidation passes
- `hybrid_search/` — search frontend on top of cas-search
- `migration/` — schema migrations
- `notifications/` — notification dispatch
- `orchestration/` — worker name allocation
- `rules/` — rule loading and application
- `telemetry/`, `tracing/`, `otel.rs`, `sentry.rs`, `logging.rs` — observability
- `worktree/` — worktree creation, salvage, cleanup
- `harness_policy.rs` (single crate-level `env_test_lock` per cas-d25d), `agent_id.rs`, `duplicate_check.rs`, `error.rs`, `async_runtime.rs`

## crates/cas-factory (`crates/cas-factory/src/`)

Factory orchestration: spawn pipeline, lease management, merge gates, per-worker spec resolution, director event detection.

- `lib.rs` — public surface (`FactoryCore`, `FactoryConfig`, spec resolver re-exports)
- `core.rs` — `FactoryCore` (worker lifecycle, lease ownership, status reporting)
- `config.rs` — `FactoryConfig` (workers, names, supervisor/worker `cli`/`model`/`effort`, resolved specs)
- `spec_resolver.rs` — `resolve_specs(workers, sources)` 6-layer cascade (built-in → user `~/.cas/config.toml` → project `.cas/config.toml` → CLI flags → `--worker-spec` JSON)
- `tests/spec_resolver.rs` — 22 unit tests covering each cascade layer
- `director.rs` — `DirectorEventDetector.detect_changes` with idle-vs-pending guard (cas-afb7) + heartbeat-vs-activity guard (cas-1ec7); `AgentSummary.pending_messages` + `has_recent_worker_io_activity`
- `changes.rs`, `notify.rs`, `recording.rs` — supporting subsystems
- `session/` — session state and cleanup

## crates/cas-mux (`crates/cas-mux/src/`)

Terminal multiplexer that owns every PTY pane in the factory TUI.

- `lib.rs` — public surface (`Mux`, `MuxConfig`, `WorkerSpec`, `Effort`)
- `mux.rs` — `Mux` and `MuxConfig`; `factory()` constructor and `factory_pane_configs()` helper for tests
- `spec.rs` — `WorkerSpec { name, cli, model, effort }`, `Effort` enum with `as_claude_arg()` / `as_codex_config()`, `WorkerSpec::builtin_default()`
- `pane/` — `Pane` constructors; `in_alt_screen` tracking + `update_alt_screen()` scanner with carry-buffer for partial sequences; `build_worker_config` / `build_supervisor_config` branch on `cli` to `PtyConfig::claude` vs `PtyConfig::codex`
- `pane/tests.rs` — empty-pane scroll no-error contract (cas-3b18 characterization)
- `harness.rs`, `render.rs`, `error.rs`
- `mux_tests/` — `factory_pane_configs` tests verifying config → CLI argv chain

## docs/

Planning artifacts only — product/domain content lives in `docs/PRODUCT_OVERVIEW.md` (`project-overview` skill).

- `brainstorms/` — `YYYY-MM-DD-<topic>-requirements.md` from the `cas-brainstorm` skill
- `ideation/` — survivor lists from the `cas-ideate` skill
- `requests/` — cross-team BUG/FEATURE inboxes; active: `BUG-worker-pane-mouse-wheel-alt-screen.md`, `FEATURE-nuxt-playwright-skill.md`, `team-memories-filter-policy.md`, `RESOLVED-api-me-deploy-failed-type-check.md`, `SHIPPED-user-team-membership-endpoint.md`, `RESPONSE-user-team-membership-endpoint.md`
- `requests/completed/` — archived closed work (bulk move 2026-05-18): cloud-client-404, cloud-push-skipped, factory-session-observations, factory-write-permission-deadlock, remember-defaults-to-personal, EPIC-mcp-server-robustness, FEATURE-global-team-config, FEATURE-cloud-sync-pull-team-memories, FEATURE-cloud-sync-pull-return-specs, BUG-session-observations-2026-05-18
- `spikes/` — investigation outputs (e.g., `2026-05-01-factory-agent-teams-enrollment-spike.md`)
- `onboarding/` — onboarding notes (`macbook-from-zero.md`, etc.)
- Standalone planning docs at `docs/` root: `compound-engineering-roadmap.md`, `verifier-dispatch-trace.md`, `FEATURE-REQUEST-TEAM-PROJECT-MEMORIES.md`, `SCOPE-PROJECT-ID-REQUIRED.md`, `session-2026-05-15-orchestration-issues.md`

## Cross-cutting

- **Tests:** Rust convention — inline `#[cfg(test)] mod tests` per file, plus heavy `cas-cli/tests/` integration suite (40+ files): factory (`factory_server_test.rs`, `factory_latency_test.rs`, `factory_mcp_ops_test.rs`, `factory_codex_skill_guardrails.rs`, `distributed_factory_test.rs`, `multi_agent_test.rs`), team cloud sync (`team_pull_wiring_test.rs`, `team_pull_watermark_scope_test.rs`, `team_set_slug_resolution_test.rs`, `team_memories_e2e_test.rs`, `team_sync_test.rs`, `memory_share_test.rs`, `pull_scoping_regression_test.rs`, `push_skipped_test.rs`, `team_scope_e2e_test.rs`, `team_backfill_test.rs`, `teams_fetch_test.rs`), MCP (`mcp_protocol_test.rs`, `mcp_proxy_test.rs`, `mcp_tools_test*` — 160+ tests covering close-gate composition, dep_remove typing, transfer override, memory team-promote), bridge (`bridge_server_test.rs`, `bridge_server_sse_test.rs`), search (`search_scoring_test.rs`, `search_frontmatter_test.rs`, `search_utf8_regression_test.rs`), code review (`code_review_e2e_test.rs`, `code_review_parity_test.rs`), hooks (`hook_schema.rs`, `hooks_test/`), proptest fuzz (`proptest_test.rs`, `proptest/`), plus `auth_integration_test.rs`, `verify_before_claim_skill_test.rs`, `blame_attribution_test.rs`, `loop_test.rs`, `component_output_test.rs`, `openclaw_bridge_test.rs`, `cli_test.rs`. PTY-based TUI tests use `crates/cas-tui-test`. Parallel-safe envronment locking via crate-level `env_test_lock` in `harness_policy.rs` (cas-d25d).
- **Docs:** `README.md`, `CONTRIBUTING.md`, `CHANGELOG.md`, `CAS-DEEP-DIVE.md` at repo root; CLAUDE.md cascades from `~/CLAUDE.md` → `cas-src/CLAUDE.md` (per-project ancestor-dedup walker skips middle ancestors, cas-253e).
- **Tooling / scripts:** `scripts/worktree-boot.sh`, `scripts/provision-hetzner.sh`; release/install/bootstrap scripts live in `~/.local/bin/` (cas-update, cas-refresh, cas-login). `homebrew/cas.rb` is the formula.
- **Config:** `.claude/settings.json` (harness hooks + permissions; exec-form `args: [...]` emitters per cas-9a60, requires CC ≥ 2.1.142), `.codex/config.toml` (Codex CLI registers cas MCP server), `.mcp.json` (MCP servers — playwright uses `${HOME}`, neon uses hosted HTTP MCP at `mcp.neon.tech/mcp`), `.cas/config.toml` (factory knobs + `[code_review] owner = "supervisor"` default), `Cargo.toml` (workspace + profiles, `panic = "unwind"` enforced).
- **Migration:** one-shot scripts in `migration/` (Phase 2/3/7/8 logs from the cloud move). Not active build infra.

## Entrypoints

- CLI: `cas-cli/src/main.rs` → binary `cas` (users run `cas`)
- TUI: `cas-cli/src/ui/factory/app/mod.rs` (the `cas` binary defaults to launching the factory TUI)
- MCP server: `cas-cli/src/mcp/daemon.rs` (started via `cas serve`, managed as a long-running daemon)
- HTTP bridge: `cas-cli/src/bridge/server/` (web UI backend, `cas bridge serve`)
- Hook dispatch: `cas-cli/src/cli/hook.rs` (`cas hook <event>` invoked from `.claude/settings.json`)
- Tests: `cargo test -p cas` for cas-cli; `cargo test --workspace` for everything
- Build: `cargo build --release` then restart any running `cas serve` (factory work depends on the daemon matching HEAD)
