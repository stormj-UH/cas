# Changelog

All notable changes to CAS are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project follows [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [2.13.0] - 2026-05-05

### Changed

#### Default code-review ownership flipped from `worker` to `supervisor` (EPIC cas-cac3 / cas-b51a Stage 2+3)

**The default `[code_review] owner` is now `"supervisor"`.** Projects with no `[code_review]` block in `.cas/config.toml` now use supervisor-owned review by default — no opt-in required.

- **Workers run only the lightweight structural lint at close (<1s).** The multi-persona review pipeline is no longer invoked inline at `task.close` by default. Tasks transition to `pending_supervisor_review` after a clean lint pass; workers are immediately free to pick up the next task.
- **Supervisor runs `/cas-code-review mode=interactive` at cherry-pick time (per-task) and at EPIC→base merge (integration sweep).** See `cas-supervisor/references/workflow.md` Phase 3 step 5 and Phase 4 step 3 for the exact invocation sequence.
- **Pin to legacy behavior** with `[code_review] owner = "worker"` in `.cas/config.toml`. This restores the original inline dispatch (~14 min per close) for teams that want it.
- **`close_ops.rs` absent-section fix (cas-865b):** `.unwrap_or(false)` at the runtime close gate replaced with `.unwrap_or_else(|| CodeReviewConfig::default().supervisor_owned())` so projects with no `[code_review]` block track the config-layer default instead of being hardcoded to worker mode.
- **Skill prose updated:** `cas-worker` workflow (steps re-numbered), `cas-supervisor` workflow (cherry-pick and integration review steps added), `cas-code-review` SKILL.md (ownership table, mode reference, purpose section all reflect new default).

## [2.12.0] - 2026-05-04

### Added

#### Per-worker CLI/model/effort overrides — heterogeneous factory teams (EPIC cas-b3db)

Supervisors can now spawn workers on different AI harnesses within a single factory session. A Claude supervisor can coordinate a Codex worker (or vice versa) without restarting the daemon.

- **`mcp__cas__coordination action=spawn_workers cli=codex`** — new `cli`, `model`, and `effort` fields on the `spawn_workers` coordination action route per-spawn harness overrides through the full stack: MCP → spawn-queue (m201 migration adds `worker_spec` column) → cloud handler → daemon protocol → `finish_worker_spawn`.
- **`cas factory --worker-spec '{"cli":"codex","name":"alice"}'`** — new `--worker-spec` CLI flag resolves and persists per-worker specs at daemon boot; `WorkerSpec::codex_default(name)` constructor added.
- **`MuxConfig.resolved_worker_specs`** — `Mux` struct replaces the three scalar `worker_cli/model/effort` fields with `default_worker_spec: WorkerSpec` + `worker_specs: HashMap<String, WorkerSpec>`. `factory_pane_configs` and `add_worker` use per-worker spec lookup with fallback chain (explicit > map > default).
- **Live re-resolution at spawn time** — `sync_worker_config_from_live_settings()` called at `finish_worker_spawn` and `respawn_worker` re-reads the live `LlmConfig` from disk so `cas config set llm.worker.harness codex` takes effect without daemon restart.
- **Codex effort arg wired** — `PtyConfig::codex` now emits `-c model_reasoning_effort=<level>` when effort is `Some`; previously silently dropped.
- **Heterogeneous spawn smoke test** (`cas-5570`) — `heterogeneous_spawn` integration test in `crates/cas-mux/tests/` confirms Claude-supervisor-spawns-Codex-worker roundtrip. Supervisor skill docs updated with `cli`/`model`/`effort` parameter table and heterogeneous-team example in both `.claude` and `.codex` mirrors.

#### Supervisor-owned code-review pipeline (cas-b51a)

Moves the expensive multi-persona `cas-code-review` skill dispatch from the worker's close path to the supervisor, cutting the per-close latency cost.

- **`[code_review] owner = "worker" | "supervisor"` config knob** — new `CodeReviewConfig` section in `config.toml`. Default is `"worker"` (Stage 1 backwards-compat; Stage 2 flip is a follow-on).
- **`PendingSupervisorReview` task status** — new status value between `InProgress` and `Closed`. When `owner = "supervisor"`, a worker close that passes the lightweight lint gate transitions the task to `PendingSupervisorReview` instead of triggering `CODE_REVIEW_REQUIRED`. Worker is immediately free to pick up the next task.
- **Lightweight structural lint gate** — fast (<1s) pre-close check run by the worker on the raw diff before handing off to the supervisor. Catches `unimplemented!()`, `todo!()`, `dbg!()`, and >5-consecutive-line commented-out blocks. Lint failure returns a structured error naming the violation; the task stays `InProgress`.
- **5 integration tests** in `supervisor_review_flow.rs` covering: supervisor-mode skips `CODE_REVIEW_REQUIRED`, worker-mode unchanged, `PendingSupervisorReview` SQLite round-trip, supervisor verification on pending task, config default is `"worker"`.
- **Supervisor skill docs** (`cas-supervisor.md`, `code-review-queue.md`) updated with queue-management workflow and lint-fail response guidance.

#### Verification jail self-cert (cas-778a / cas-4c64 / cas-164c)

Clean `ReviewOutcome` envelopes now self-certify the worker close path. Workers no longer need to forward to the supervisor when `VERIFICATION_JAIL_BLOCKED` fires on a clean close — the system detects a valid envelope and clears the gate automatically. The old forwarding dance only applies on pre-2.12.0 binaries.

### Changed

#### dbg!() lint tightened + lint-fail integration test (cas-adf0 + cas-b5ac)

- **`contains("dbg!(")` replaces three-part OR** — the lightweight lint's `dbg!` check previously missed `=dbg!(...)` and `let x=dbg!(...)` (no space before `dbg`). Replaced with a single `contains("dbg!(")` that catches all forms regardless of preceding whitespace.
- **4 new unit tests** covering bare, with-space, no-space-after-equals, and embedded forms.
- **Integration test for lint-fail close path** (`test_lint_fail_close_blocked_before_pending_supervisor_review`) — asserts `is_error=true`, error names the offending lint rule, and task remains `InProgress` (no `PendingSupervisorReview` transition on lint failure).

## [2.11.0] - 2026-05-01

### Added

#### Factory close-merge enforcement (EPIC cas-754b)

Closes the silent data-loss vector where `task action=close bypass_code_review=true` could mark tasks Closed without verifying the worker's `factory/<assignee>` branch was merged into the parent epic. Field evidence from gabber-studio cas-6e07 (2026-05-01): 7 stranded tasks, ~21 commits, ~3000 LOC nearly disappeared. Second occurrence in 48h.

- **Per-task close-merge gate (cas-95ce):** `mcp__cas__task action=close` on a non-epic task now rejects when `factory/<assignee>` has commits not on the parent epic. Bypass-immune at the type level (the helper signature does not consume a bypass flag) and at the physical level (gate runs structurally upstream of `bypass_code_review` evaluation). Error names the stranded commit count, factory branch, parent epic branch, and remediation.
- **Epic-close gate (cas-8f8f):** `mcp__cas__task action=close` on an Epic-type task walks every child's factory branch and rejects when any child is stranded. Same bypass-immunity. Caught a P1 critical in autofix: the original `unwrap_or_default()` on a SQLite-backed lookup would have failed open and defeated the entire enforcement. Now propagates as `INTERNAL_ERROR`.
- **`mcp__cas__coordination action=epic_status id=<epic-id>` diagnostic (cas-8f8f):** new callable surface returning a markdown table per child task (assignee | factory branch | unmerged count | last commit | task ID + status). Useful for in-flight audits before attempting epic close.
- **`cas-supervisor-checklist` skill update (cas-8f8f):** "Before Closing an EPIC" section now references `epic_status` as the canonical check and notes that the gate is automatic (defense-in-depth, no longer manual-only).

### Changed

- **`mcp__cas__verification action=add` authz error (cas-a90f3):** the misleading "Supervisors can only verify epics, not individual tasks" rejection has been replaced with a message that names the actual rule (active-assignee-based) and lists the three exemptions (orphaned / inactive assignee / supervisor IS the assignee). Error embeds the offending assignee ID, gives concrete remediation (`mcp__cas__task action=release`), and clarifies that epics remain always supervisor-verifiable. Predicate renamed `assignee_inactive` → `assignee_inactive_or_absent` to make `unwrap_or(true)` semantics self-documenting (logic unchanged).

### Operator guidance

After upgrading, the new gates fire on `task.close` calls. If a worker hits the gate during close, the supervisor must merge `factory/<assignee>` into the parent epic before the close will succeed (this is the desired ordering and matches how the other workflow guidance now reads). For pre-existing stranded factory branches (e.g. gabber-studio cas-6e07), salvage with: `git checkout <epic-branch> && git merge --no-ff factory/<worker>`.

## [2.10.1] - 2026-04-29

### Changed

- **Shared proxy transport (cas-36fd0):** new `cli/integrate/proxy.rs`
  module exposes `ProxyClient` with the proxy lifecycle (`proxy_config_path`,
  `call`, `block_on`, `unwrap_envelope`). Both `ProxyVercelClient` and
  `LiveNeonClient` are now thin wrappers — ~165 LOC of duplicated boilerplate
  retired. Future `Live<X>Client` implementations inherit the wiring.
- Speculative neon parser tolerance shapes (`orgs/data` alias, flat
  `describe_project`) removed until proven against real envelopes; bail
  messages cite cas-36fd0 and request bug filing on real upstream drift.
- `default_database` "neondb" silent fallback → explicit bail with
  provisioning recovery hint.

## [2.10.0] - 2026-04-29

### Added

#### Vercel/Neon/GitHub Auto-Integration (EPIC cas-b65f)
- `cas integrate <vercel|neon|github> [init|refresh|verify]` standalone subcommands.
  - **Vercel**: detects `vercel.json` / `@vercel/*` deps, fuzzy-matches via
    `mcp__vercel__list_projects`, captures team + project + env→branch mapping.
  - **Neon**: detects Prisma + `@neondatabase/*` / `@prisma/adapter-neon`, prompts
    for org when multiple exist, captures `org_id` + `projectId` + `databaseName` +
    branches via `mcp__neon__{list_organizations,list_projects,describe_project,describe_branch}`.
  - **GitHub**: parses `git remote -v` (https + ssh forms), records `owner/repo`.
- `cas init` runs platform detection and prompts Y/N per detected platform,
  delegating to the corresponding `cas integrate <platform> init` in-process.
  Idempotent on re-run: existing populated SKILL.md flips the prompt to
  "Refresh? [y/N]" with default N.
- `--no-integrations`, `--vercel <id>`, `--neon <id>`, `--github <repo>` flags
  for non-interactive `cas init` use.
- Generated SKILL files land in **both** `.claude/skills/<name>/` and
  `.cursor/skills/<name>/` so both harnesses pick them up.
- `<!-- keep <name> -->` … `<!-- /keep <name> -->` named keep blocks preserve
  user-owned IDs across `refresh` regenerations. `--update-ids` opts into
  re-fetching IDs from the platform MCP.
- `<!-- cas:full_name=... -->` identity tag convention for canonical project
  identity inside keep blocks; sanitized to neutralize markdown injection.
- `cas doctor` audits integration freshness via per-platform `verify_report`
  and surfaces stale IDs as warnings (not errors); MCP-down reports as
  `skipped — MCP not configured` rather than failing the doctor run.
- Optional opt-in `[integrations] session_start_warn = true` in
  `.cas/config.toml` emits a low-severity SessionStart banner when integrations
  go stale. Default off — preserves the codemap banner's signal.

#### Codemap Skill (cas-4d84)
- `/codemap` skill ships in `.claude/skills/codemap/`, builtins, and codex
  variant. Generates `.claude/CODEMAP.md` and resets the freshness counter
  via `cas codemap clear` after writing. Closes the long-standing gap where
  hooks referenced a `/codemap` slash command that did not exist.

### Changed

#### Factory Skill Bundles (cas-61af)
- `cas-supervisor.md` split from 44 KB into a 6.8 KB SKILL.md + six
  references (`preflight`, `intake`, `planning`, `workflow`,
  `worker-recovery`, `reference`).
- `cas-worker.md` split from 22 KB into a 5.7 KB SKILL.md + three
  references (`close-gate`, `recovery`, `details`).
- `supervisor_guidance()` and `worker_guidance()` no longer bundle
  `cas-task-tracking`, `cas-memory-management`, or `cas-search` — those are
  autonomous skills the agent invokes via the Skill tool. Bundled payload
  dropped from ~61 KB / ~35 KB to ~10 KB / ~5.5 KB respectively.
- Test ceiling at 12 KB enforces the bundle stays small enough that the
  Claude Code harness does not truncate the SessionStart additionalContext
  to a 2 KB preview.

#### Cross-cutting Hardening (cas-fc38)
- New `cli/integrate/fs.rs` shared module: `atomic_write`,
  `atomic_write_create_dirs`, `read_capped` (4 MiB cap with symlink
  rejection), `is_regular_file`, `locate_repo_root[_from]` (with `git -C`
  discipline that resolves the inner repo on submodule / nested-worktree
  invocations).
- New `cli/integrate/md.rs` shared module: `escape_md_cell`,
  `escape_md_cell_code`, `emit_cas_full_name_tag`, `parse_cas_full_name_tag`.
- `IntegrationStatus` split: `TransportError` distinct from `Stale` so a
  failed MCP call is no longer misreported as a stale ID.
- All three platform handlers consume the shared helpers — atomic-write
  semantics, symlink defense, file-size cap, markdown escaping, and
  identity tag behave uniformly.

#### Team Memories
- `cas cloud team set|show|clear` subcommands to configure the active team
  (UUID input; slug resolution deferred pending cloud-side endpoint).
- `cas memory share <id>|--since <duration>|--all [--dry-run]` for retroactive
  backfill of pre-existing personal memories to the team push queue.
- `cas memory unshare <id>` to mark a memory `share=Private` (blocks future
  team dual-enqueue; does not retract cloud-side copies).
- `share: Option<ShareScope>` (`Private`/`Team`) persisted on Entry, Rule,
  Skill, and Task via SQLite migrations `m037`/`m060`/`m082`/`m121`.
- Automatic dual-enqueue: when a team is configured via
  `cas cloud team set`, `cas memory remember` in any Project-scoped
  non-Preference context queues the entry to both personal and team
  push queues. `cas cloud sync` drains both.
- Coarse kill-switch: `cloud.json.team_auto_promote: false` disables the
  automatic promotion without requiring the team to be cleared.
- Integration test suite: `team_sync_test.rs`, `memory_share_test.rs`,
  `team_memories_e2e_test.rs` cover the full push → pull pipeline.

### Changed

- `mcp-proxy` is now a default Cargo feature so `cas integrate vercel|neon`
  ships out of the box — the wired `ProxyVercelClient` / `LiveNeonClient`
  require it.
- `cas cloud team-memories`'s "no team configured" error now correctly
  directs users to `cas cloud team set <uuid>` (previously referenced a
  non-existent subcommand with `<slug>` argument).
- `cas cloud team set|show|clear` subcommands to configure the active team
  (UUID input; slug resolution deferred pending cloud-side endpoint).
- `cas memory share <id>|--since <duration>|--all [--dry-run]` for retroactive
  backfill of pre-existing personal memories to the team push queue.
- `cas memory unshare <id>` to mark a memory `share=Private` (blocks future
  team dual-enqueue; does not retract cloud-side copies).
- `share: Option<ShareScope>` (`Private`/`Team`) persisted on Entry, Rule,
  Skill, and Task via SQLite migrations `m037`/`m060`/`m082`/`m121`.
- Automatic dual-enqueue: when a team is configured via
  `cas cloud team set`, `cas memory remember` in any Project-scoped
  non-Preference context queues the entry to both personal and team
  push queues. `cas cloud sync` drains both.
- Coarse kill-switch: `cloud.json.team_auto_promote: false` disables the
  automatic promotion without requiring the team to be cleared.
- Integration test suite: `team_sync_test.rs`, `memory_share_test.rs`,
  `team_memories_e2e_test.rs` cover the full push → pull pipeline.

### Changed

- `cas cloud team-memories`'s "no team configured" error now correctly
  directs users to `cas cloud team set <uuid>` (previously referenced a
  non-existent subcommand with `<slug>` argument).

## [2.0.0] - 2026-04-12

### Added

#### Factory System
- Multi-agent factory with supervisor/worker architecture and isolated git worktrees.
- Director event system for task dispatch, worker lifecycle, and epic completion notifications.
- Worker startup confirmation flag to detect crash-on-startup failures.
- Orphaned task reclamation — supervisor can claim tasks from dead workers.
- Coordinator messaging system with priority levels, delivery confirmation, and outbox replay.
- Verification jail exemption for factory workers to prevent universal tool blocking.
- Worker idle/stale notification dedup and suppression.
- Minions theme with ASCII art and themed boot screen for factory workers.

#### Cloud Sync
- Bidirectional cloud sync with Petra Stella Cloud — push/pull tasks, memories, rules.
- Cloud sync queue with shutdown drain, startup push, 10s idle gate, 60s interval.
- Circuit breaker for TLS retry spam with capped event buffer.
- `cas cloud projects` and `cas cloud team-memories` commands.
- `cas cloud purge-foreign` for orphaned dependency cleanup.
- Project-scoped pull requests to prevent cross-project data leaks.

#### MCP Proxy
- `cas-mcp-proxy` crate — proxies upstream MCP servers (Playwright, Neon, GitHub, Vercel, Context7) through CAS. Workers get 2 tools instead of 50+.
- Config-aware hot-reload for proxy server connections.
- Search with keyword matching and server filtering.
- Integration tests, catalog caching, and README.

#### TUI
- Tokyo Night theme variant.
- OSC 52 clipboard copy and auto-inject on image paste.
- `cas open` interactive TUI project picker.
- Tab forwarding to PTY for autocomplete (Ctrl+P for sidecar).
- Clipboard fallback via client-side write with visual feedback.
- Mouse click to focus panes, Ctrl+Arrow pane cycling, Shift+drag text selection.
- Native terminal selection (replaces custom selection implementation).

#### Compound Engineering
- `cas-code-review` skill — multi-persona code review with 7 reviewer personas (correctness, testing, maintainability, project-standards + conditional security, performance, adversarial). Includes bounded autofix loop, confidence gates, fingerprint dedup, and review-to-task routing.
- `cas-brainstorm` and `cas-ideate` skills for structured ideation.
- `git-history-analyzer` and `issue-intelligence-analyst` agent types.
- Multi-persona review merge pipeline with cross-reviewer agreement boost.
- Pre-insert memory overlap detection with configurable threshold actions.
- Implementation Unit Template for EPIC subtask specifications.
- `execution_note` field on tasks: `test-first`, `characterization-first`, `additive-only` postures with enforcement at close.

#### Skills & Agents
- Comprehensive `cas-worker` skill with build failure triage, MCP connectivity guidance, tool selection guide, context exhaustion detection, task reassignment protocol, and section reorder for critical-path-first flow.
- Adversarial supervisor posture with intake gate, scope lock, and rejection authority.
- Partnership posture for supervisor — counter-propose, trajectory gate, situational awareness.
- `cas-supervisor` skill with EPIC sizing heuristics, worker failure recovery, and merge conflict guidance.
- `cas-memory-management` skill with multi-file schema and overlap workflow.
- `cas-search` skill with filter grammar, code symbol search, and module-scoped candidate API.
- CODEMAP system — auto-maintained breadcrumb navigation map with structural change detection hooks.

#### Infrastructure
- Hetzner CCX23 provisioning script for remote CAS server (Ashburn VA).
- Slack bridge: Bolt app scaffolding with per-user daemon architecture, SSE adapter, message formatter, file upload passthrough with security sanitization.
- `cas-install.sh` — portable curl one-liner installer.
- WebSocket endpoint for factory daemon.
- SSE plain-text pane output and tail endpoint.
- Auto-attach prompt with `--attach`/`--new` flags for existing sessions.
- `cas serve` HTTP bridge for Slack integration.

#### Store & Performance
- Sequence table for ID generation (replaces per-insert MAX+LIKE scan).
- SQLite `prepare_cached()` for all statement caching.
- Jitter on SQLite write-retry backoff to break convoy pattern.
- Recursive CTE for dependency cycle-check (replaces iterative BFS).
- Tantivy IndexWriter caching (saves 50MB per write allocation).
- BM25 search index caching and QueryParser reuse.
- Batch code symbol DB inserts in indexing daemon.
- `ImmediateTx` wrapper for atomic store operations.

### Changed

- Bumped version to 2.0.0 with simplified release workflow targeting `pippenz/cas`.
- Config format migrated from YAML to TOML (automatic merge of stale settings).
- `project_canonical_id` derived from folder name instead of git remote URL (required on all cloud pushes).
- Default cloud sync interval reduced from 300s to 60s.
- MCP tool prefix standardized to `mcp__cas__`.
- Worker skill reordered for critical-path-first flow: Task Types and Execution Posture before close procedures.
- Code review section compressed from 65 to 30 lines — pipeline internals moved to `cas-code-review` skill.
- Rules section merged into Rules of Engagement; Valid Actions merged into Schema Cheat Sheet.
- Legacy `code-reviewer` agent deprecated in favor of `cas-code-review` skill.

### Fixed

- **TUI**: Off-by-one in Ghostty VT style run column indices clipping left edge of pane content. Tab click detection using variable-width positions instead of equal-width assumption. Scroll viewport double-compensation when Ghostty preserves viewport position. Task panel flashing empty due to read race between task list and dependency queries. Dark theme contrast — `border_default`, `border_muted`, `hint_description` bumped for readability. Epic state updated before filter in `refresh_data()`.
- **Factory**: Verification jail cascade where one task's pending verification blocked all tools. `CAS_FACTORY_MODE` phantom env var — `pre_tool.rs` required it alongside `CAS_AGENT_ROLE` but no code ever set it. Director dispatching blocked/closed tasks (terminal-status guard added). Supervisor self-verification deadlock. Worktree workers missing MCP access due to gitignored `.mcp.json`/`.claude/` (fixed with symlinks). Duplicate hooks causing PreToolUse errors (`cas hook cleanup` added).
- **Cloud**: WebSocket TLS for `tokio-tungstenite`. HTTP TLS for `ureq` client. Fallback `project_id` for filesystem-root CAS projects. 403/404 error handling with pluralized labels.
- **Store**: N+1 queries in `task_store.rs`. Unbounded `IN` clauses and `LIKE` scans. 8 excessive indexes dropped to reduce write amplification. Lease races and cleanup/prune methods with transaction safety.
- **Close**: Additive-only gate now diffs worker branch commits (not main). Skip close-gate checks for non-isolated tasks. Reject close when worker tree has uncommitted work. Status-update race condition where `status=blocked` overwrites concurrent supervisor close.
- **Other**: `rustls` CryptoProvider installed at startup to prevent daemon crash. Secrets moved from provision script to `~/.config/cas/env` (push protection). GitHub auth token used in self-update to avoid API rate limits.

## [1.0.0] - 2026-03-12

### Added
- Initial open-source release of CAS.
- Factory TUI screenshot in README.
- `.env.worktree.template` for worker environment setup.

### Changed
- Release workflow updated for GitHub Actions with Homebrew auto-update.
- MCP config sync added to `cas update` flow.

### Fixed
- Migration v165 crash when `verifications` table doesn't exist.
- Release workflow secret check moved from job-level to step script.

## [0.6.2] - 2026-02-25

### Added
- Interactive terminal dialog (Ctrl+T) in factory TUI with show/hide/kill.
- MCP proxy catalog caching for SessionStart context injection.
- Billing interval switching buttons (monthly/yearly) with savings display.
- Resume subscription button on cancellation notice.
- `cas changelog` command to show release notes from GitHub releases.

### Changed
- Cloud sync on MCP startup runs in background with 5s timeout (non-blocking).
- Heartbeat uses shorter 5s timeout and spawn_blocking to avoid stalling async loop.
- Refactored cloud routes: org_billing_settings → billing_settings, org_members → members.
- Release bump workflow now requires a matching CHANGELOG.md section.

### Fixed
- Debounced Ctrl+C interrupt to prevent accidental double-sends.
- Update version check now compares versions properly.
- Stripe portal return URL redirects back to billing page instead of settings.
- Removed duplicate type export in types/index.ts.

## [0.5.7] - 2026-02-15

### Fixed
- Avoided macOS factory startup crash by using subprocess daemon mode with attach/socket retries.
- Hardened UTF-8-safe truncation behavior in touched UI/tooling paths to prevent char-boundary panics.

### Changed
- Standardized release-train crate versions to `0.5.7`.

## [0.5.6] - 2026-02-15

### Fixed
- Cleared clippy warnings under `-D warnings` across touched workspace crates.

### Changed
- Standardized release-train crate versions to `0.5.6`.
- Updated local git hook rustfmt invocation to use Rust 2024 edition.

## [0.5.5] - 2026-02-15

### Changed
- Published `0.5.5` release and synchronized release-train crate versions.

## [0.5.4] - 2026-02-15

### Changed
- Improved Supabase auth login UX and callback branding.

## [0.5.3] - 2026-02-15

### Changed
- Initial release carrying Supabase auth login UX and callback branding improvements.

## [0.5.2] - 2026-02-13

### Changed
- Bumped release-train versions to `0.5.2`.

## [0.5.1] - 2026-02-11

### Fixed
- Fixed Sentry transport panic triggered during `cas login`.

## [0.5.0] - 2026-02-11

### Fixed
- Added missing Sentry transport feature to prevent login-time crash.

## [0.4.0] - 2026-01-10

### Added
- Consolidated MCP tool format with unified naming.
- Sort and task type filtering for MCP and CLI.
- ID-based search and CLI/MCP feature parity.
- Git worktree support for task isolation.
- Schema migration system for database upgrades.
- Verification system with task-based exit blocking.
- Statusbar anchoring support.

### Changed
- Extracted `cas-core` and `cas-mcp` crates for better modularity.
- Removed `#[tool_router]` macro from CasCore for compile-time improvement.
- MCP enabled by default in `cas init --yes`.
- Removed legacy MCP mode and added `list_changed` notifications.

### Fixed
- Removed duplicate store implementations from `cas-cli`.
- Fixed scope persistence in crate extraction.
- Task verifier now uses CLI and checks project rules.

## [0.3.0]

### Added
- Initial stable release with core functionality.

[Unreleased]: https://github.com/pippenz/cas/compare/v2.12.0...HEAD
[2.13.0]: https://github.com/pippenz/cas/compare/v2.12.0...v2.13.0
[2.12.0]: https://github.com/pippenz/cas/compare/v2.11.0...v2.12.0
[2.11.0]: https://github.com/pippenz/cas/compare/v2.10.1...v2.11.0
[2.10.1]: https://github.com/pippenz/cas/compare/v2.10.0...v2.10.1
[2.10.0]: https://github.com/pippenz/cas/compare/v2.0.0...v2.10.0
[2.0.0]: https://github.com/pippenz/cas/compare/v1.0...v2.0.0
[1.0.0]: https://github.com/pippenz/cas/compare/v0.6.2...v1.0
[0.6.2]: https://github.com/pippenz/cas/compare/v0.5.7...v0.6.2
[0.5.7]: https://github.com/pippenz/cas/compare/v0.5.6...v0.5.7
[0.5.6]: https://github.com/pippenz/cas/compare/v0.5.5...v0.5.6
[0.5.5]: https://github.com/pippenz/cas/compare/v0.5.4...v0.5.5
[0.5.4]: https://github.com/pippenz/cas/compare/v0.5.3...v0.5.4
[0.5.3]: https://github.com/pippenz/cas/compare/v0.5.2...v0.5.3
[0.5.2]: https://github.com/pippenz/cas/compare/v0.5.1...v0.5.2
[0.5.1]: https://github.com/pippenz/cas/compare/v0.5.0...v0.5.1
[0.5.0]: https://github.com/pippenz/cas/compare/v0.4.0...v0.5.0
[0.4.0]: https://github.com/pippenz/cas/compare/v0.3.0...v0.4.0
