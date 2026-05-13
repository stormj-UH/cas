---
from: Petra Stella Cloud team
date: 2026-05-13
priority: P1
---

# `cas cloud sync` pull returns 0 of every entity type for a new team member, even when team data exists in the active project

## Problem

Onboarded `daniel.l@petrastella.io` (plan=`unlimited`, role=`member` of team `Petra Stella` / UUID `2a57bec9-5dfa-4a8f-b711-31f9aeb8d6cb`) and walked through `cas-login` → `cas cloud team set <uuid>` → `cas cloud sync`. All three commands report success, push lands correctly in team scope, but pull always reports `0 synced` for every non-event type, even when there are thousands of team-scoped rows that match the active project.

### Observed (terminal output, edited for brevity)

```
~/cas-src $ cas cloud team set 2a57bec9-5dfa-4a8f-b711-31f9aeb8d6cb
✓ Active team set
  UUID: 2a57bec9-5dfa-4a8f-b711-31f9aeb8d6cb
  Slug resolution deferred — see `cas cloud team show`

~/cas-src $ cas cloud sync
✓ Push complete
    Events: 0 inserted, 6 updated
✓ Pull complete
    0 entries synced
    0 tasks synced
    0 rules synced
    0 skills synced
    0 specs synced
    0 events synced
    0 prompts synced
    0 file changes synced
    0 commit links synced
```

### What's actually in the database

For team `Petra Stella`, project `cas-src`:

| entity_type | rows | oldest updated_at         | newest updated_at         |
|-------------|------|---------------------------|---------------------------|
| entry       | 166  | 2026-04-22T18:38:50.143Z  | 2026-05-13T13:59:00.932Z  |
| task        | 1300 | 2026-04-23T14:45:43.011Z  | 2026-05-13T13:19:55.690Z  |
| rule        | 19   | 2026-05-04T16:52:33.004Z  | 2026-05-04T22:32:38.796Z  |
| skill       | 3    | 2026-04-29T13:08:13.983Z  | 2026-04-29T13:08:13.983Z  |

`daniel.l`'s pushes from this same shell land in the same scope (`team_id = 2a57bec9…`, `project_id = cas-src`), so the team-push path is working correctly. Only pull is empty.

## Server-side analysis (cloud team — this is *not* a server bug)

Two relevant routes:

- **Personal pull** `app/api/sync/pull/route.ts` filters `userId = me AND team_id IS NULL` (plus optional `project_id`, optional `since`).
- **Team pull** `app/api/teams/[teamId]/sync/pull/route.ts` filters `team_id = teamId` (plus optional `project_id`, optional `since`). **No user filter.** Membership is gated by `validateTeamMembership(user.id, teamId)`.

Verified manually: hitting team pull directly with `daniel.l`'s API key, the right team UUID, and `project_id=cas-src` returns the expected ~1,488 rows. So the server is fine — the CLI is either:

1. **Calling the personal pull endpoint** on `cas cloud sync` even when a team is active. Personal pull's `team_id IS NULL` clause would return 0 for him because, post-team-set, none of his rows are personal. This is the simplest explanation and matches the `0 of everything` signature.
2. **Calling the team pull endpoint but sending a stale `since` timestamp** persisted from a previous personal-scope sync. The team's newest row is `2026-05-13T13:59`, the user's first team sync ran at `~14:30`; if the local `last_pulled_at` is reused across scopes, `gt(updated_at, since)` excludes the historical backfill.
3. **Calling team pull without `since`, but with a `project_id` value the CLI derived incorrectly** (see "Related" below — first sync went out with `project_id=cas` because the working dir was named `cas`, not `cas-src`).

Push works because the push payload carries `team_id` / `project_canonical_id` explicitly. Pull is GET-only, so the CLI has to *choose* the right URL and querystring up front — that's where the asymmetry likely lives.

## What we need from cas-src

1. **Route `cas cloud sync` pull through `/api/teams/{active_team_id}/sync/pull` whenever a team is active**, not `/api/sync/pull`. If you already do this, confirm — then hypothesis (2) or (3) is the live one and we need a repro trace.
2. **Track `last_pulled_at` per `(team_id, project_id)` scope**, not globally. On first sync into a new scope, send no `since` (or `since=0`) so historical team data backfills.
3. **Resolve project slug before first push/pull**, not deferred. The `Slug resolution deferred — see `cas cloud team show`` warning from `team set` is what let daniel.l's first sync go out as `project_id=cas` (his local dir name) instead of canonical `cas-src`. Rename was a manual workaround. A `cas cloud project set <canonical-id>` (or eager resolution at `team set` time, anchored by git remote / `.cas/config.toml`) would close this gap.
4. **First-sync UX hint.** When `cas cloud sync` returns 0 of everything *and* the server's per-scope count is non-zero, the CLI could print a hint ("expected N team-scoped rows in this scope, got 0 — try `cas cloud sync --full` / check `cas cloud team show`"). The cloud already knows the totals; we'd be happy to add a `/sync/status` shape that returns per-scope row counts if you want to lean on that.

## Reproduction

1. Create a fresh user, add them to a team that has ≥1 entry / task / rule / skill scoped to some project `P`.
2. On a fresh machine, clone repo `P` into a directory named exactly `P` (so slug derivation lands correctly — or use any name and watch hypothesis #3 fire).
3. `cas-login` → `cas cloud team set <team_uuid>` → `cas cloud sync`.
4. Expected: backfill of all team-scoped rows for project `P`.
5. Actual: `0 entries / 0 tasks / 0 rules / 0 skills` synced.

## Touchpoints (best guesses — please correct)

- `cas-cli/src/cloud/syncer/pull.rs` — endpoint selection & `since` handling
- `cas-cli/src/cloud/team.rs` (or wherever `cloud team set` lives) — slug resolution deferral
- `cas-cli/src/cloud/config.rs` — `last_pulled_at` storage shape; if globally keyed, needs to become per-scope

## Server-side, for reference (no changes requested unless coordination needs it)

- `petra-stella-cloud/app/api/teams/[teamId]/sync/pull/route.ts` — team pull, correct
- `petra-stella-cloud/app/api/sync/pull/route.ts` — personal pull, correct
- `petra-stella-cloud/lib/teams.ts` — `validateTeamMembership`

## Related

- `FEATURE-cloud-sync-pull-team-memories.md` (already in this dir) — likely overlaps; this bug is the "still broken after that change" follow-up if so.
- `FEATURE-mandatory-project-id-on-pull.md` (filed from us recently) — mandates `project_id` on pull. Aligns with hypothesis #3.
- `team-memories-filter-policy.md` — policy doc for which team rows a member can read; we believe the policy is satisfied here (he is `role=member`), so this bug is purely a CLI wiring issue.
