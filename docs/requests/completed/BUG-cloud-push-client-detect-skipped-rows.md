---
from: Petra Stella Cloud team
date: 2026-04-23
priority: P1
cas_task: cas-f645
---

# Cloud push client must detect server-side skipped rows

## Problem

Surfaced during cas-0bdc code review (adversarial + security personas).

After cas-0bdc, the server's push route silently skips conflict updates when the incoming `project_canonical_id` doesn't match an existing row's project_id. Postgres `ON CONFLICT DO UPDATE ... WHERE false ... RETURNING` excludes skipped rows entirely, so the client sees a `200` response with **fewer rows in the result than it sent**.

The client in `cas-cli/src/cloud/syncer/push.rs` calls `self.queue.mark_synced(item.id)` for every item in the sub-batch as soon as `push_sub_batch` returns `Ok(())` — it never inspects the response body to verify which rows actually landed.

Consequences:

1. A client pushing under the wrong `project_canonical_id` marks its local queue entries as synced even though the server rejected them.
2. The user gets no signal anything went wrong.
3. On a fresh machine clone or local DB wipe, the un-synced data is gone (server-of-record holds the other project's version).

This is the client-side complement to server-side cas-0bdc and to sibling request `BUG-reject-null-project-id-on-push.md` (cas-d656, server-side, already filed in cloud inbox).

## Design

**Server change (petra-stella-cloud — we will own this).** Extend `PushResult` to include a `skipped` count per entity type. Compare `upsertResults.length` vs `allValues.length` per batch and categorize the difference. At minimum, log `reqLog.warn` when any rows are skipped so ops has a signal. Ideally return a per-entity `{inserted, updated, skipped}` object.

**Client change (cas-src — you).** In `push_batch` / `push_sub_batch`, inspect the `PushResult` response and only call `mark_synced(item.id)` for items whose corresponding entity-type count in the response implies the row was accepted. If the server reports any skipped rows, surface a warning via `tracing::warn!` and leave the items in the queue for manual inspection. Consider a new `CloudError::PartialPushConflict` variant.

## Touchpoints

Client (cas-src):

- `cas-cli/src/cloud/syncer/push.rs:286-304` — `push_batch` `mark_synced` loop
- `cas-cli/src/cloud/syncer/push.rs:435-448` — `push_sub_batch` response handling
- `cas-cli/src/cloud/types.rs` (or wherever `PushResult` lives) — add `skipped: Option<HashMap<String, usize>>`

Server (already scoped on our side):

- `petra-stella-cloud/app/api/sync/push/route.ts` — emit `skipped` count
- `petra-stella-cloud/app/api/teams/[teamId]/sync/push/route.ts` — same

## Acceptance criteria

- A cross-project push surfaces a warning in both server logs and client output, and the affected local queue items remain un-marked-synced (retryable)
- Existing happy-path pushes are unchanged
- Test asserts that when the server returns a response with `skipped > 0`, the client does not call `mark_synced` for the skipped items

## Coordination

Server-side `skipped` field shape — please align on the JSON schema before the client PR lands. Propose a shape in a comment on this file or on cas-f645 notes, and we'll match on the cloud side.
