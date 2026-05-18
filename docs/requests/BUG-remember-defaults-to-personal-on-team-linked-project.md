---
from: Petra Stella Cloud team
date: 2026-05-15
priority: P1
---

# `cas remember` defaults to personal scope on team-linked projects

## Problem

Surfaced while debugging "why can't Daniel L pull Daniel V's Ozer memories" on the cloud. The Ozer project (`canonical_id='ozer'`) is registered server-side with `team_id = 2a57bec9-...` (Petra Stella). All three Petra Stella members have access. Yet Daniel L's team-scoped pull returns almost nothing for Ozer.

Server-side check on `sync_entities` for `entity_type='entry' AND project_id='ozer'`:

| author | scope | count | window |
|---|---|---|---|
| Daniel V | `team_id IS NULL`, `data->>'scope' = 'project'` | **478** | 2026-03-23 → ongoing |
| Daniel V | `team_id = <PS team>`, team-scoped | 3 | one window on 2026-04-22 |
| Ben | `team_id = <PS team>`, team-scoped | 39 | single bulk push 2026-04-23 |

The team-pull route correctly filters `WHERE team_id = teamId` (`petra-stella-cloud/app/api/teams/[teamId]/sync/pull/route.ts:27`), so the 478 personal rows are invisible to Daniel L by design.

**Root cause is on the CLI write-side.** Even though the active project has a server-registered `team_id`, `cas remember` is writing entries with `scope=project` (personal) and pushing them via the personal sync push, not the team push. The 3 + 39 team-scoped entries from April 22-23 look like one-off explicit team pushes; after that, the CLI reverted to personal scope and stayed there for the next 478 entries.

## Design

**Decision needed: should team-linked projects auto-team-scope `remember`?**

Two reasonable shapes:

1. **Auto-promote** — if the active project's local registration carries a `team_id`, `cas remember` defaults to team scope. Add `--personal` / `--private` flag to opt out for a specific entry. This matches the principle of least surprise for shared projects.

2. **Explicit opt-in** — keep personal as the default but warn when a user remembers something in a team-linked project, prompting them to `--team` or pin the default in project config.

Recommend option 1 for Petrastella's own usage pattern (small team, shared projects).

Either way, the local-only "personal note in a shared project" path needs to remain available.

## Backfill / migration

The 478 stranded entries are being repaired in the cloud DB directly by flipping `team_id` from NULL to the Petra Stella team id (Daniel V is the team owner; the project is team-owned; the entries pertain to work the team shares). After the repair, Daniel L's next team pull will hydrate them.

A `cas memories promote --to-team` / `--to-personal` UX would let users do this themselves in the future instead of needing a server-side patch.

## Touchpoints (cas-src)

- Wherever `cas remember` builds the entity payload and decides which sync queue it lands in (personal vs team push)
- Project registration / local config that holds `team_id` for the active project
- `cas memories` / `cas remember` CLI surface — add `--team` / `--personal` flags
- New `promote` subcommand for retroactive scope changes

## Acceptance criteria

- In a team-linked project, `cas remember "..."` (no flags) lands the entry on the team-pull path so other team members receive it on their next sync
- A user can override with `--personal` for a one-off private note in the same project
- There is some mechanism (flag, command, or migration tool) to convert existing personal-scoped project entries to team scope without manual SQL

## Coordination

No server-side schema changes required for the auto-scoping decision. If a `promote` API is wanted, that's a small endpoint on petra-stella-cloud that flips `team_id` for `(user_id, entity_type, id)`; ping us with the desired shape.
