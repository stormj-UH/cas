---
from: gabber-studio (pippenz @ /home/pippenz/Petrastella/gabber-studio)
date: 2026-05-01
priority: P1
---

# BUG: verification-jail-recovery close pattern allows factory-branch commits to strand off-epic

## Summary

Supervisor close-via-bypass (`mcp__cas__task action=close bypass_code_review=true`) used during the verification-jail-recovery protocol updates the task DB to `Closed` and releases the lease, but **does NOT verify or trigger the worker's factory-branch merge into the epic branch**. Result: tasks marked "Closed" can have their commits silently stranded on `factory/<worker>` branches, invisible to the epic.

Observed live in epic `cas-6e07` (SMS conversion funnel hardening): **5 of the 11 tasks I closed via verification-jail-recovery had stranded factory commits that never reached the epic branch**, including the architectural anchor (T5 funnel-session, ~520 LOC) and the regression-guard capstone (T7 e2e Playwright, ~430 LOC). Both nearly disappeared from the epic.

The supervisor and worker both saw "Closed" and assumed the work was on the epic. Neither side ran `git push origin factory/<name>` + PR-merge after the close. The factory branches just sat unreferenced on local clones.

## Concrete failure mode (cas-6e07, 2026-05-01)

After the supervisor (me) closed 5 tasks via `bypass_code_review=true`:
- `cas-76a0` (T4 atomic claim) — 4 commits stranded on `factory/happy-jay-14`
- `cas-4e48` (T9 sweeper) — 3 commits stranded on `factory/happy-jay-14`
- `cas-7625` (T5 funnel-session) — 5 commits stranded on `factory/noble-sparrow-57`
- `cas-1036` (T7 e2e capstone) — 3 commits stranded on `factory/noble-sparrow-57`
- `cas-99c8` (T10 auth middleware) — 2 commits stranded on `factory/subtle-marten-74`
- `cas-f4f9` (SSR config move) — 2 commits stranded on `factory/subtle-marten-74`
- `cas-afe1` (T3 ownership guard) — 2 commits stranded on `factory/noble-sparrow-57`

A worker (`subtle-marten-74`) caught it accidentally during a rebase preparing a different PR — they discovered that `epic/sms-conversion-funnel` at HEAD did NOT contain the cas-99c8 middleware additions even though the task showed Closed.

Net: ~3000 lines of approved, reviewed, tested work was structurally invisible to the epic until manually flagged. **This was caught mid-stream by accident; the supervisor checklist's "Verify all worker branches are merged into the epic branch" step at epic-close time would have caught it eventually, but only after the work appeared lost.**

Worse, the recovery surfaced a **second-order race**: while the workers were rebasing their stranded branches onto the current epic tip, the epic kept moving (other PRs landing). subtle-marten-74's branch went stale during their first rebase; they re-rebased; in the 26-second window between their second push and supervisor merge, another PR landed that obliterated their base again. Required THREE rebase cycles before the merge could complete safely.

## Why it happens

`mcp__cas__task action=close bypass_code_review=true` mutates the task row but takes no action on the working tree or git refs. Workers in the verification-jail state (which is itself a different bug — workers can't always close their own task even after passing all gates locally) commit to `factory/<worker>`, hit jail, signal supervisor; supervisor closes via bypass; both sides assume "Closed" → "merged."

The lifecycle is:
1. Worker commits to `factory/<worker>` ✅
2. Worker runs `mcp__cas__task action=close` → **VERIFICATION_JAIL_BLOCKED**
3. Worker forwards close to supervisor per recovery.md
4. Supervisor runs `task action=close bypass_code_review=true` → SUCCESS
5. ⚠ **No git operation has occurred. `factory/<worker>` is stranded.**

The supervisor checklist's "Before Closing an EPIC" gate (`Verify all worker branches are merged into the epic branch`) runs only at *epic* close, not per-task. By then, several tasks may have stranded refs that no one is actively tracking.

## Proposed fix

**Option A (preferred, hard-block):** `task action=close` (with or without bypass) should fail if the worker's factory branch has commits not present in the parent epic's HEAD. Error message:

```
task close rejected: factory/<worker> has 7 commits not on epic/<id>.
Push the branch and merge a PR before closing, OR pass --skip-merge-check
with a justification for the audit log.
```

**Option B (soft-warn):** close succeeds but the response surface includes a strong warning + auto-creates a follow-up "MERGE: <task-id>" reminder visible in `coordination action=worker_status` until the merge happens.

**Option C (epic-status command):** add `coordination action=epic_status epic_id=<id>` that scans every child task's assignee → factory branch and reports unmerged commits. Run-on-demand surface; doesn't block close but makes the gap discoverable in seconds.

Recommend **A + C combined**: hard-block by default with `--skip-merge-check` escape hatch; epic_status command for ongoing audit + at-epic-close verification.

## What we did to recover (gabber-studio)

- Surveyed each worker's factory branch via `git log --oneline factory/<name> ^epic/...` to enumerate stranded commits
- Coordinated PR creation per factory branch (`gh pr create --base epic/sms-conversion-funnel --head factory/<name>`)
- Resolved 3 rebase race-cycles caused by other PRs landing during the recovery
- Filed `cas-9a0a` (P2) in the gabber-studio CAS for tracking from the project side

The recovery added ~45 minutes of supervisor coordination that would have been zero if the close-time check existed.

## Related observations

- Skill `cas-supervisor-checklist` says "Before Closing an EPIC: verify all worker branches are merged into the epic branch" — currently this is a manual mental step. Could be automated as `coordination action=epic_status` per Option C.
- The lease-tracking layer also has stale-state issues: `task action=reset` returned `lease_released: true` on first call but the lease still appeared locked from the worker's view. This is a separate bug but the same coordination-layer family.
- `mcp__cas__verification action=add` accepts task IDs from supervisors *sometimes* (worked for cas-8d9a, cas-7625, cas-99c8, cas-f4f9, cas-cf61, cas-1036, cas-4e48, cas-76a0) but rejected cas-e152 with "Supervisors can only verify epics, not individual tasks." The acceptance rule appears to depend on whether the task has an assignee at the time of the call. Inconsistent behavior; worth a separate cleanup.

## Reproducer (synthetic)

1. Create a CAS task in factory mode.
2. Worker claims, commits to `factory/<worker>`, runs `task action=close`.
3. Worker hits VERIFICATION_JAIL_BLOCKED, forwards to supervisor.
4. Supervisor runs `task action=close <id> bypass_code_review=true reason="recovery"`.
5. Run `git log --oneline factory/<worker> ^<epic-branch>` — observe stranded commits despite task DB showing Closed.

## Severity

**P1.** Real silent data-loss vector. The work isn't actually lost (it's still in the worker's worktree git history), but the system reports completion without delivering it. In a multi-day epic with workers cycling out, the gap could grow until the next epic-close manual audit — by which point factory branches may have been auto-cleaned (`gc_report` mentions a pruning policy for stale worktrees).

For epics with parallel workers and frequent jail-recovery closes, this is the most dangerous failure mode I've encountered in CAS to date.
