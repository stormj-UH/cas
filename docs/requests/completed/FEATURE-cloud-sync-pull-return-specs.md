# FEATURE: Extend `/api/sync/pull` response to include `specs` array

**Status:** open
**From:** cas-src (sharp-cardinal-48 / EPIC cas-2eb3)
**To:** petra-stella-cloud team
**Filed:** 2026-05-12

## What's needed

Add a top-level `specs: Spec[]` array to the response from
`GET /api/sync/pull?project_id=<canonical_id>`, scoped by `project_canonical_id`
in the same way `tasks` / `rules` / `skills` already are. Empty array when
the project has no specs.

## Why

cas-bba4 (subtask of EPIC cas-2eb3) re-adds scoped pull support in
`CloudSyncer::pull` for the 5 entity kinds that cas-ed15 dropped from
`cas cloud pull` to close the cross-project leak:

- `events` ✅ already returned (9595 rows for cas-src today)
- `prompts` ✅ already returned
- `file_changes` ✅ already returned
- `commit_links` ✅ already returned
- `specs` ❌ **not returned**

cas-side will defensively `.unwrap_or_default()` on `body.specs`, so the
syncer extension can land regardless — but until cloud ships this, every
`cas cloud pull` reports `0 specs synced` even when the cloud's spec store
has rows for the project.

## Sniff-test that confirmed the gap

```bash
curl 'https://petra-stella-cloud.vercel.app/api/sync/pull?project_id=cas-src' \
  -H "Authorization: Bearer $TOKEN" \
  | jq 'keys'
```

Returned: `["agents", "commit_links", "entries", "events", "file_changes",
"prompts", "pulled_at", "rules", "sessions", "skills", "tasks",
"verifications", "worktrees"]`

No `specs`.

## Suggested shape

Mirror the existing `tasks` / `rules` / `skills` blocks in
`app/api/sync/pull/route.ts`:

```ts
const specs = await db
  .select()
  .from(syncEntities)
  .where(
    and(
      eq(syncEntities.userId, userId),
      isNull(syncEntities.teamId),
      eq(syncEntities.entityType, 'spec'),
      eq(syncEntities.projectCanonicalId, projectId),
      gt(syncEntities.updatedAt, since),
    ),
  );

// In response:
{ ..., specs: specs.map(s => s.payload), ... }
```

Each row's payload should already carry the `project_canonical_id` field
(per the cas-d656 schema migration). cas-side `entity_matches_project`
runs the same filter client-side as belt-and-suspenders.

## Acceptance

`curl '.../api/sync/pull?project_id=cas-src' | jq '.specs | length'` returns
a non-negative integer (0 is fine when no specs exist), and the array
contains only specs whose `project_canonical_id == "cas-src"`.

## Not blocking on this

cas-bba4 ships independently. Once the cloud change lands, the existing
cas-side code will pick up specs without further changes.
