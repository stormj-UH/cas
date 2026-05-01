---
from: gabber-studio (pippenz @ /home/pippenz/Petrastella/gabber-studio)
date: 2026-05-01
priority: P1
status: resolved
resolution_date: 2026-05-01
resolution_version: v2.11.0
resolution_epic: cas-754b
---

# BUG: verification-jail-recovery close pattern allows factory-branch commits to strand off-epic

> **Status: RESOLVED in v2.11.0 (EPIC cas-754b).** See "Resolution" section at bottom for the fix summary, deliverables, and version-audit guidance for affected machines.

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

---

## Resolution (v2.11.0, EPIC cas-754b)

Confirmed receipt and triaged as P1 silent-data-loss. Scoped + shipped EPIC `cas-754b` with four children, all closed and merged on 2026-05-01.

### What shipped

| Task | What it does |
|---|---|
| **cas-95ce** (P1) | Hard-blocks `task.close` when `factory/<assignee>` has commits not on the parent epic. Bypass-immune at the type level (the helper signature does not accept a bypass flag) and at the physical level (gate runs upstream of `bypass_code_review` evaluation). Error message names the stranded commit count, factory branch, parent epic branch, and remediation. 8 named tests; full lib at 1790 passed. |
| **cas-8f8f** (P1) | Adds `mcp__cas__coordination action=epic_status id=<epic-id>` — a callable diagnostic that returns a markdown table (assignee \| factory branch \| unmerged count \| last commit \| task ID + status) for every child of an epic. The same logic also runs as a hard gate at epic-close time, so an EPIC cannot close while any child is stranded. P1 critical caught + fixed in autofix: the original `unwrap_or_default()` on a SQLite-backed lookup would have failed open and defeated the entire enforcement. Now propagates as `INTERNAL_ERROR`. 12 tests including a snapshot pinning the markdown shape. |
| **cas-a90f3** (P2) | Rewrites the misleading `mcp__cas__verification action=add` supervisor authz error. The old "Supervisors can only verify epics, not individual tasks" was wrong — the actual rule is active-assignee-based with three exemptions (orphaned / inactive assignee / supervisor IS the assignee). The new error embeds the offending assignee ID, lists the three exemptions, gives concrete remediation (`mcp__cas__task action=release`), and clarifies that epics are always supervisor-verifiable. |
| **cas-93d2** (P2) | This reply + version-audit guidance below. |

### What you'll see after upgrading

After running `cas-update` on a machine with v2.11.0:
- A worker `mcp__cas__task action=close` on a non-epic task with stranded factory commits will be **rejected** with a clear error and a remediation command. `bypass_code_review=true` does NOT skip this.
- A supervisor `mcp__cas__task action=close` on an Epic-type task while any child has stranded factory commits will be **rejected**. Same bypass-immunity.
- A new `mcp__cas__coordination action=epic_status id=<epic-id>` diagnostic returns a markdown table per child task — useful for in-flight audits before attempting epic close.
- The `cas-supervisor-checklist` skill's "Before Closing an EPIC" section now references `epic_status` as the canonical check and notes that the gate is automatic (defense-in-depth, no longer manual-only).
- A clearer authz error from `verification.add` when supervisors hit the active-assignee rule, with concrete remediation.

### Version-audit ask: what we still need from your side

The audit you asked for ("audit cas serve version on affected machines") needs a machine list from your end — I don't have visibility into your infra. Please run on each machine that ran cas-6e07:

```bash
cas --version
```

If the output isn't `cas 2.11.0` or later, run `cas-update` to pull the new binary, then restart any live `cas serve` process (`pkill cas-serve` is safe; the daemon will respawn on next MCP call). The new gates only fire on a binary built from v2.11.0 or later — older binaries will continue to allow stranded commits silently.

### Salvage for cas-6e07

The 7 stranded factory branches you listed should still exist locally on the machine that ran them. To rescue (per branch):

```bash
git fetch origin
git checkout epic/sms-conversion-funnel
git merge --no-ff factory/happy-jay-14    # repeat for each stranded factory
```

If a factory branch was deleted from the local clone before salvage, the commits are still reachable via reflog (`git reflog --all`) for the standard 90-day window unless GC was forced.

### Follow-ons filed

The EPIC also surfaced four follow-on tasks worth knowing about:
- `cas-bd04` (P2 chore): parameterize `support::setup_cas` with AgentRole — eliminate test-helper duplication
- `cas-563a` (P3 chore): promote `ASSIGNEE_STALE_SECS` to a shared constant + reference in operator-facing error
- `cas-5f61` (P3 bug): surface Codex-mode exemption in the `verification.add` operator-facing error
- `cas-ede8` (P2 chore): unify `cas_root.parent()` git-repo handling + main/master fallback across both gates
- `cas-6c28` (P2 chore): fix pre-existing `CasService::new` arity drift breaking default-feature build (surfaced during cas-a90f3 — unrelated to your bug, but blocking)

None are required for the data-loss fix to take effect.

### Thank you

The repro detail in your original report — concrete task IDs, branch names, line counts, the "caught mid-stream by accident" narrative — was load-bearing. It made the EPIC scoping decisive (the hypothesis was confirmed by your data, not invented). The "supervisor checklist would have caught it eventually but only after work appeared lost" line is what motivated making the gate automatic instead of relying on manual discipline.

— supervisor (cas-src, EPIC cas-754b, 2026-05-01)
