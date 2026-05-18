---
from: Petra Stella Cloud team
date: 2026-04-23
priority: P2
cas_task: cas-4244
---

# Cloud client 404 on every factory session start — phone-home effectively dead

## Problem

The cloud client hits HTTP `404` immediately on factory session start, retries 10x over ~4 min, then gives up permanently. Observed in **every single factory session** across `domdms` and `gabber-studio` (12+ "giving up" messages in logs).

Net effect: cloud phone-home / sync is effectively dead for all factory sessions.

## Likely cause

A route change on petra-stella-cloud that the client doesn't match (or a route the client still expects that we never shipped). We need you to identify which endpoint the client is hitting on session start so we can confirm whether the fix is on the server (restore the route) or the client (update to the current path).

## Asks

1. Surface the exact URL the client is requesting (path + method) from the first failing log line. Post it as a comment on cas-4244 or in a reply file in our inbox.
2. Once the route is identified, either:
   - Update the client to match the current server routes (listed below), OR
   - Tell us which route is missing and we'll add or redirect it on the server side.

## Current server routes (for reference)

Personal sync:

- `POST /api/sync/push`
- `POST /api/sync/pull`
- `GET  /api/sync/status`
- CRUD under `/api/sync/entities/...`

Team sync (scoped by teamId):

- `POST /api/teams/[teamId]/sync/push`
- `POST /api/teams/[teamId]/sync/pull`
- `GET  /api/teams/[teamId]/sync/status`

Patterns / device auth:

- `GET  /api/patterns` (read-only rules-as-patterns)
- `POST /device/code`, `POST /device/token`, `POST /device/authorize`

Account:

- `GET  /api/account/usage` (added in commit `c489d9a`)

If the client is hitting something outside this set (e.g., a `/phone-home`, `/sync/session`, `/heartbeat`, or old `/v1/...` prefix) — that's the smoking gun.

## Acceptance criteria

- Fresh factory session starts with zero `404`s from the cloud client
- No `"giving up"` messages in factory logs tied to cloud phone-home
- Verified across at least two different project factory sessions (domdms + gabber-studio are the easiest reproductions)
