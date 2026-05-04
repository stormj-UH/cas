# Details — Tools, Sync, Schema

## Tool Selection Guide

Pick the right tool for the job:

| Need | Tool | Example |
|------|------|---------|
| Conceptual/exploratory query | `mcp__cas__search action=search` | "how does auth work?", "where is X handled?" |
| Exact symbol or string match | `Grep` | find all callers of `process_task()` |
| Complex codebase investigation | `Agent` with `subagent_type=Explore` | tracing a data flow across multiple modules |
| Record a learning or bugfix | `mcp__cas__memory action=remember` | root cause found, pattern discovered |
| Find files by name/pattern | `Glob` | `**/*.rs`, `src/**/mod.rs` |

See the `cas-search` skill for detailed search guidance including code symbol search and hybrid queries.

## Syncing (Isolated Mode)

If the supervisor asks you to sync, safely rebase without losing WIP:

```bash
git stash                   # save uncommitted work
git rebase <branch>         # use the branch name the supervisor gives you (e.g. master, epic/<slug>)
git stash pop               # restore WIP
```

**Important:** Use the **local** branch name the supervisor specifies (e.g. `master`, `epic/<slug>`), NOT `origin/master`. In factory mode, the supervisor merges into the local branch directly, so `origin/master` is stale.

If the rebase has conflicts, resolve them before popping the stash. Message the supervisor if you're stuck.

## Schema Cheat Sheet (exact field names and valid actions)

Wrong field names are rejected. These are the **exact** names for the calls workers make most often.

**`mcp__cas__task`** — the task ID field is always `id` (NOT `task_id`, `taskId`, `_id`). Notes parameter is `notes` (plural, NOT `note`).

```
# Start / show / close
mcp__cas__task action=start id=cas-abc1
mcp__cas__task action=show id=cas-abc1
mcp__cas__task action=close id=cas-abc1 reason="Implemented X, tests pass"

# Progress notes (note_type ∈ progress|blocker|decision|discovery|question)
mcp__cas__task action=notes id=cas-abc1 notes="Found root cause in Y" note_type=progress

# Mark blocked
mcp__cas__task action=update id=cas-abc1 status=blocked
mcp__cas__task action=notes id=cas-abc1 notes="Blocked: <reason>" note_type=blocker
```

**Priority** accepts numeric (0–4) OR named alias: `critical`/`high`/`medium`/`low`/`backlog`. `priority="high"` is the same as `priority=1`.

**Booleans** on `with_deps`, etc. accept `true`/`false`, `"true"`/`"false"`, or `1`/`0`.

**`mcp__cas__coordination action=message`** requires BOTH `message` and `summary`:

```
mcp__cas__coordination action=message target=supervisor \
  summary="task blocked on verification" \
  message="cas-abc1 needs schema review before I can proceed"
```

Sending `message` alone without `summary` is rejected. `summary` is the one-line preview shown in the UI.

**Valid `mcp__cas__task` actions** (do not invent others): `create`, `show`, `update`, `start`, `close`, `reopen`, `delete`, `list`, `ready`, `blocked`, `notes`, `dep_add`, `dep_remove`, `dep_list`, `claim`, `release`, `transfer`, `available`, `mine`.

**Valid `mcp__cas__coordination` actions for workers**: `message`, `message_ack`, `message_status`, `whoami`, `heartbeat`, `queue_poll`, `queue_ack`. Factory/worktree/spawn actions are supervisor-only.
