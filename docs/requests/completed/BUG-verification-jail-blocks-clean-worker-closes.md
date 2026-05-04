---
from: Gabber Studio (pippenz @ /home/pippenz/Petrastella/gabber-studio)
date: 2026-05-03
priority: P1
---

# BUG: VERIFICATION_JAIL_BLOCKED forces supervisor toil on every clean worker close

## Summary

On a healthy factory cycle (worker writes code → tests pass → cas-code-review autofix has no P0 → worker calls `mcp__cas__task action=close` with a complete ReviewOutcome envelope), the close is hard-blocked with:

```
MCP error -32600: VERIFICATION_JAIL_BLOCKED: Mutating operation task.close blocked.
Task <id> requires verification before any mutations are allowed. Use the Task tool
to spawn a task-verifier subagent: Task(subagent_type="task-verifier", prompt="Verify task <id>").
```

The cas-worker recovery guidance explicitly tells the worker NOT to spawn a task-verifier and instead to "forward once and trust the DB". The supervisor then has to:

1. Receive the worker's forwarded message
2. Run their own verification (or bypass it)
3. Manually close + merge
4. Reply to the worker confirming the close

This is **fully repeatable** and creates supervisor toil that the worker-side close should be able to handle on its own. The pattern occurred **13 times across all 4 workers in a single ~3.5-hour factory session** (gabber-studio, 2026-05-03 PM, EPICs cas-2dfa + cas-0c8b):

| Worker | Task | Epic | Outcome |
|---|---|---|---|
| ready-finch-99 | cas-c52c (I6 — Whitelist mechanism) | cas-2dfa | jail-blocked → forwarded → supervisor verified+closed+merged |
| ready-finch-99 | cas-e531 (B5 — Pricing card clickability) | cas-0c8b | jail-blocked → forwarded → supervisor verified+closed+merged |
| ready-finch-99 | cas-6532 (B8 — Dashboard card clickability) | cas-0c8b | jail-blocked → forwarded → supervisor verified+closed+merged |
| ready-finch-99 | cas-db7a (B12 — keyboard-scroll sweep) | cas-0c8b | jail-blocked → forwarded → supervisor verified+closed+merged |
| ready-finch-99 | cas-894f (B7 — UX polish) | cas-0c8b | jail-blocked → forwarded → supervisor verified+closed+merged |
| ready-finch-99 | cas-1b03 (B9 — 5 screenshot fixes) | cas-0c8b | jail-blocked → forwarded → supervisor verified+closed+merged |
| noble-falcon-40 | cas-5556 (I4 — public audit viewer) | cas-2dfa | jail-blocked → forwarded → supervisor verified+closed+merged |
| noble-falcon-40 | cas-789a (I5 — completion SMS) | cas-2dfa | jail-blocked → forwarded → supervisor verified+closed+merged |
| noble-falcon-40 | cas-f598 (I7 — Make It Better gate) | cas-2dfa | jail-blocked → forwarded → supervisor verified+closed+merged |
| noble-newt-22 | cas-8f6a (I8 — credit gate on capture) | cas-2dfa | jail-blocked → forwarded → supervisor verified+closed+merged |
| noble-newt-22 | cas-e2b0 (I9 — sms-responder constants) | cas-2dfa | jail-blocked → forwarded → supervisor verified+closed+merged |
| witty-marten-42 | cas-9b87 | cas-0c8b | jail-blocked → forwarded → supervisor verified+closed+merged |
| witty-marten-42 | cas-35fd | cas-0c8b | jail-blocked → forwarded → supervisor verified+closed+merged |

**Per-worker breakdown:** ready-finch-99 ×6, noble-falcon-40 ×3, noble-newt-22 ×2, witty-marten-42 ×2.

**Every close required supervisor intervention.** The work was clean each time — the verification step itself approved it without any P0 corrections needed. **Zero of the 13 closes had a legitimate gating reason.** The jail is firing as a default-on policy on tasks that should never have hit it.

The toil scales linearly with worker output: a 4-worker session produces N×forwards + N×manual verifies + N×ack messages where N is the supervisor's task throughput, multiplying the supervisor's time-to-close per task by ~5×. With 13 closes in this session, that's ≈35 minutes of supervisor toil that didn't need to happen.

### Sibling concern (out of scope for this report)

Two other tasks in the same session (cas-9791 invoice.paid grant fix, cas-17de free_trial grant fix — both witty-marten-42) hit a **different** close gate: `MERGE_REQUIRED: factory/<branch> has N commits not on epic/<branch>`. That guard explicitly says "cannot be bypassed" — different mechanism from verification-jail (which CAN be bypassed via `bypass_code_review=true`). The merge gate is arguably acting more correctly (it forces actual data-state convergence) while the verification-jail blocks on a self-fulfilling prophecy (the close requires verification but won't accept the worker's own ReviewOutcome JSON). **This report is scoped to the verification-jail behavior only; the merge-gate may warrant a separate report or may turn out to be a positive design choice we just don't appreciate yet.**

## Environment

- Installed binary: `cas` (version installed at `/home/pippenz/.local/bin/cas`)
- Project: `gabber-studio`
- Factory mode: `CAS_FACTORY_MODE=1`
- Workers in session: `ready-finch-99`, `noble-falcon-40`, `noble-newt-22`, `witty-marten-42`
- Supervisor: `mighty-puma-25` (Primary, sole supervisor)
- Reporter session ID: `911c8e0c-2b11-4b4b-b797-d5568d37c757` (ready-finch-99)
- EPICs in flight at the time: `cas-2dfa` (core onboarding flow remediation), `cas-0c8b` (bossman testing-session feedback)

## Repro (deterministic)

1. Spawn a factory worker session under a supervisor.
2. Assign the worker a normal task with `mcp__cas__task action=create` + supervisor message.
3. Worker runs the standard close-gate workflow:
   - Implements the work
   - All tests pass locally (jest + tsc)
   - Worker dispatches `cas-code-review` skill in `mode=autofix`
   - Skill runs N personas in parallel, returns merged findings with no P0
   - Worker applies any safe_auto fixes, commits, and calls:
     ```
     mcp__cas__task action=close id=<task> reason="..." code_review_findings='<ReviewOutcome JSON>'
     ```
4. Close returns `VERIFICATION_JAIL_BLOCKED` — every time, regardless of:
   - whether `code_review_findings` is empty `{"residual":[],"pre_existing":[]}` or fully populated
   - whether the work is a 3-line a11y micro-fix or a 580-line multi-file feature
   - whether all upstream verification (test pass, type-check pass) succeeded

## Expected behavior

When a worker submits a close with a structurally valid `ReviewOutcome` and no P0 findings, the close should either:

- **Option A:** auto-verify the close on the worker side (the same way `task-verifier` would when invoked manually), then commit
- **Option B:** route the verification request to the supervisor's queue without rejecting the close call — keep the close call's lease alive while the supervisor's verifier confirms in the background

The current behavior — hard-rejecting the worker's call with an error message that *also* tells the worker "do not spawn the verifier yourself" — produces a flow where the worker has no path to forward progress without supervisor manual action.

## Worker-side workaround currently in use

Per `cas-worker/references/recovery.md`:

> VERIFICATION_JAIL_BLOCKED → see references/recovery.md. Forward once, then trust the DB.

So workers:

1. Add a `note_type=blocker` task note documenting the jail block.
2. Send a single `mcp__cas__coordination action=message` to the supervisor with the close payload contents.
3. Stop work on this task.
4. Wait for the supervisor to verify, close, and merge.

This works but is wasteful — the supervisor does in 30 seconds what the worker just spent 5 minutes assembling.

## Suggested fix

Three options ordered by effort:

### Low: improve the error message + workflow doc

Rename the error to something that makes the worker's correct action obvious: `VERIFICATION_REQUIRED_SUPERVISOR_OWNED` with body text like "This task requires supervisor verification. Your work is staged. Send `mcp__cas__coordination action=message target=supervisor summary=... message=<close payload>`. Do not retry close." Stop suggesting `subagent_type="task-verifier"` — workers can't call it per the cas-worker skill.

### Medium: worker auto-bypass when ReviewOutcome has no P0

If `code_review_findings` arrives with `residual.findings.severity ∉ {P0}`, treat it as worker-owned verification and commit the close directly. Reserve VERIFICATION_JAIL_BLOCKED for the genuine P0 / supervisor-override case.

### High: split the close call into stage + commit

`task action=stage_close` (worker) → record the close payload, transition status to `pending_supervisor_verification`. `task action=commit_close` (supervisor) → flip to closed. The supervisor TUI can then show a queue of pending closes and approve in batch.

## Severity rationale

**Bumped to P1 (was P2 in initial filing) on the strength of the 13-instance dataset.** Not data-loss, not blocking work indefinitely (workaround exists), but the failure rate is **100% on clean closes** in a 4-worker, ~3.5-hour session — the workaround IS the workflow at this point. The supervisor toil:

- **Linear scaling:** 1 forward + 1 manual verify + 1 ack message per worker close. 13 closes in this session = ≈39 messages exchanged, ≈35 min of supervisor wall-clock that should have been zero.
- **Stalls parallelism:** while supervisor is verifying worker A's close, workers B/C/D are sitting on their own jail-blocked closes waiting their turn.
- **Confidence cost:** workers re-receive their own task assignments via the director (message-lag pattern, see msg-lag entries in this session) because the system thinks the task is still in-progress. Twice in this session a worker had to rebuild context to confirm "yes, I already shipped this; the close is just stuck."
- **Misleading error guidance:** the error message tells workers to spawn `task-verifier` themselves, but the cas-worker skill documentation explicitly bars that path. New workers will burn time trying to follow the error literally before discovering the recovery doc.

## References

- Worker recovery doc: `cas-worker/references/recovery.md` (the "Forward once, then trust the DB" guidance)
- Sister `task-verifier` agent definition: marked "Internal agent ... Do not invoke directly" — confirms the error message's suggested workaround is wrong
- Affected session messages from `ready-finch-99`: 1106 (cas-c52c), 1118 (cas-e531), 1131 (cas-6532), 1140 (cas-db7a), 1166 (cas-894f), 1192 (cas-1b03) — all forwarded close blockers
- Cross-worker close blocks confirmed by supervisor `mighty-puma-25` for: `noble-falcon-40` (cas-5556, cas-789a, cas-f598), `noble-newt-22` (cas-8f6a, cas-e2b0), `witty-marten-42` (cas-9b87, cas-35fd) — same session, same pattern, every worker

---
completed: 2026-05-04
completed_by: cas-778a
commit: 12dea48
---
