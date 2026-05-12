---
from: solid-cobra-88 (worker) via supervisor true-gazelle-21
date: 2026-05-12
priority: P0
task: cas-8edb
related: cas-778a, cas-4c64, cas-164c (v2.12.0 fix), cas-b51a, cas-865b, cas-cac3 (v2.13.0 supervisor-owned review)
---

# BUG: VERIFICATION_JAIL_BLOCKED firing on clean worker closes under owner=supervisor

## Summary

After v2.13.0 (cas-865b, May 4) flipped the default `[code_review] owner` from
`"worker"` to `"supervisor"`, every clean factory-worker `task.close` on a
post-v2.13.0 binary returned:

```
VERIFICATION_JAIL_BLOCKED: Mutating operation task.close blocked.
Task <id> (...) requires verification before any mutations are allowed.
Forward to supervisor via: mcp__cas__coordination action=message target=supervisor ...
```

This is the same surface as the v2.12.0 bug reported in
`BUG-verification-jail-blocks-clean-worker-closes.md`, but the root cause is
distinct.

## Why the v2.12.0 fix did not cover this

The v2.12.0 fix (cas-778a / cas-4c64 / cas-164c) introduced a **worker-owned
self-cert short-circuit** inside `cas_task_close`:

> If a factory worker submits a structurally valid `ReviewOutcome` envelope
> with no P0 in residual or pre_existing, write a `Skipped` verification row
> and let the close proceed without dispatching `task-verifier`.

That path was exercised when workers ran `cas-code-review` at close (the
v2.12.0 `[code_review] owner = "worker"` default). The envelope they submitted
satisfied both gates:

1. **MCP auth gate** (`mod.rs::authorize_agent_action` →
   `check_pending_verification`): finds the `Skipped` row → unblocks.
2. **`close_ops` verification gate** (lines 308+): the worker_owns_verification
   predicate fires → writes the `Skipped` row.

Under v2.13.0 (cas-b51a / cas-865b, default flipped to `owner = "supervisor"`),
workers no longer dispatch `cas-code-review` at close and therefore submit no
envelope. Without an envelope:

- The `worker_owns_verification` predicate is false →
  `cas_task_close` arms `pending_verification=true` and returns
  `VERIFICATION REQUIRED`.
- On every subsequent close attempt, the MCP auth gate sees a leased
  in-progress task with no `Approved`/`Skipped` verification row → returns
  `VERIFICATION_JAIL_BLOCKED` before `cas_task_close` even runs.

The `supervisor_review_mode` branch in `cas_task_close` (lines 1078–1160) that
*was* supposed to handle this case is positioned **after** the verification
gate. The gate fires first and short-circuits with an error, so the
supervisor_review_mode branch is never reached on a worker close that arrives
without an envelope. (Tip-of-iceberg observation: the existing v2.12.0 +
v2.13.0 unit tests covered each gate in isolation but no integration test
exercised "fresh worker close under default config" — the regression hid in
the seam.)

## Repro (deterministic, before the fix)

```bash
cd <any project with default config — no [code_review] section needed>
# Factory worker session, owner=supervisor by default
mcp__cas__task action=create title="..." task_type=task
mcp__cas__task action=start id=<id>
mcp__cas__task action=close id=<id> reason="..."
# → VERIFICATION_JAIL_BLOCKED on the FIRST close attempt
```

This was observed today (2026-05-12) on `cas 2.14.0-dirty` / commit `982b6d8`
across at least 4 closes (cas-22e9 diagnostic, cas-219d perf, cas-e0b9
hardening, cas-a368 test tightening) for two workers (sharp-cardinal-48,
solid-cobra-88). Supervisor `true-gazelle-21` had to close every one on the
worker's behalf.

## How self-cert is supposed to work

The post-cas-8edb model is:

| `[code_review] owner` | Worker close path | Verification gate |
|---|---|---|
| `"supervisor"` (default, cas-865b) | (1) MCP auth gate exempts workers from jail; (2) `cas_task_close` skips the legacy verification gate; (3) `supervisor_review_mode` block at line 1084 runs the lightweight structural lint and transitions the task to `PendingSupervisorReview` (for reviewable diffs) — or falls through to the normal close path for additive-only / docs-only / zero-diff. | Replaced by the supervisor-review queue. Supervisor invokes `cas-code-review` at cherry-pick + EPIC merge time. |
| `"worker"` (opt-in legacy) | Unchanged: worker dispatches `cas-code-review` at close, submits `ReviewOutcome` envelope on `task.close`. Clean envelope short-circuits via the worker-owned self-cert path; P0 envelope still blocks. | Verification gate fires; `Skipped` row written on clean envelope; `pending_verification` arms only on no/dirty envelope. |

Both modes converge on the same downstream gates after the verification
question is settled: factory-branch merge-state (cas-95ce), epic-close
merge-state (cas-8f8f), additive-only enforcement, `run_code_review_gate`.

## Fix (cas-8edb)

Two surgical changes:

1. **`cas-cli/src/mcp/server/mod.rs` — `authorize_agent_action`** (lines
   662–699 area): under `is_factory_worker && tool == "task" && action == "close"`,
   if `config.code_review.supervisor_owned()` returns true, return `Ok(())`.
   The verification jail's purpose — preventing workers from accumulating
   unverified work — is replaced by the supervisor-review queue under
   `owner=supervisor`, so the jail's lever has no target.

2. **`cas-cli/src/mcp/tools/core/task/lifecycle/close_ops.rs` — `cas_task_close`**
   (lines ~285–308): compute `worker_under_supervisor_review = is_factory_worker
   && task.task_type != Epic && config.code_review.supervisor_owned()` near
   the top of the function (alongside the existing `is_factory_worker` and
   `verification_enabled` calculations). Then change the verification gate
   guard from `if verification_enabled && !skip_verification {` to
   `if verification_enabled && !skip_verification && !worker_under_supervisor_review {`.
   This routes the close around the verification gate so the
   `supervisor_review_mode` block at line 1084 can run normally — for
   reviewable diffs it transitions to `PendingSupervisorReview`; for
   additive-only / docs-only / zero-diff it falls through to the rest of the
   close pipeline.

**Supervisor-driven paths are unaffected.** `is_factory_worker` is `false`
for supervisors, so `worker_under_supervisor_review` is `false` and the
existing verification gate (with its supervisor exemptions) runs unchanged.
The MCP auth gate also keeps the existing supervisor exemption.

**Legacy `owner = "worker"` is unaffected.** When the config explicitly opts
out via `[code_review] owner = "worker"`, both new exemptions short-circuit
to false and the gate paths run exactly as in v2.12.0. The legacy worker-owned
self-cert (clean envelope → `Skipped` row → proceed) still fires.

## Regression coverage

Added to `cas-cli/tests/mcp_tools_test/task_tools/verification_flow.rs`:

1. `test_worker_close_zero_diff_passes_jail_under_supervisor_owned_review_cas_8edb`
   — clean zero-diff worker close on default config succeeds (no
   `VERIFICATION_JAIL_BLOCKED`, no `VERIFICATION REQUIRED`).
2. `test_worker_close_additive_only_passes_jail_under_supervisor_owned_review_cas_8edb`
   — additive-only worker close on default config succeeds.
3. `test_legacy_owner_worker_still_jails_clean_close_without_envelope_cas_8edb`
   — explicit `owner = "worker"` config still jails a worker close that
   doesn't submit an envelope (legacy contract preserved).

Existing legacy-mode tests
(`test_factory_worker_close_hits_narrowed_jail`,
`test_worker_close_with_clean_review_envelope_proceeds`,
`test_worker_close_with_p0_residual_still_blocked`,
`test_worker_close_with_p0_residual_pre_existing_true_still_blocked`,
`test_worker_close_with_malformed_envelope_still_blocked`,
`test_worker_self_cert_blocked_when_fresh_dispatch_row_exists`) were updated
to write `[code_review] owner = "worker"` config explicitly — they pin
legacy-mode contracts and must keep doing so as the default flipped.

All 150 `mcp_tools_test` tests pass.

## Why this is debuggable from the test failure alone next time

The new tests pin three orthogonal axes:

- **Default config + worker + zero diff → close succeeds.** Any future change
  that re-jails this shape (whether through the MCP auth gate, the
  verification gate, or a new gate inserted earlier) flips this test.
- **Default config + worker + additive-only → close succeeds.** Catches
  additive-only-specific regressions.
- **Explicit `owner = "worker"` + worker + no envelope → jail fires.** Catches
  the opposite regression — accidentally removing the legacy jail lever.

The two existing seams (verification gate vs MCP auth gate) are both now
exercised by the same surface (close return value or error) so a regression
in either layer fails the same test for the same reason.

## Compensating control (already in place, no longer needed)

`cas-cli/src/builtins/skills/cas-worker/references/recovery.md` documents the
"forward once, trust the DB" pattern. Supervisors closed with
`bypass_code_review=true` and a citation note while the regression was live.
After the fix lands and supervisors rebuild + restart `cas serve`, that
compensating control is unnecessary for clean worker closes.

## Status

Fixed on `factory/solid-cobra-88` in commit **TBD-merge-SHA** (the fourth+
fifth commit on this branch, stacked on cas-219d / cas-e0b9 / cas-a368).
Supervisor `true-gazelle-21` to verify, merge, and close cas-8edb on the
worker's behalf (same flow as the other three tasks this session).
