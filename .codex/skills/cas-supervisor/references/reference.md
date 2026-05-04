# Reference — Action Names, Field Names, Dispatch Pattern

Wrong field names and invalid actions waste dispatch cycles. This section covers exact valid actions and field names.

**Valid `mcp__cas__task` actions** (do not invent others): `create`, `show`, `update`, `start`, `close`, `reopen`, `delete`, `list`, `ready`, `blocked`, `notes`, `dep_add`, `dep_remove`, `dep_list`, `claim`, `release`, `transfer`, `available`, `mine`.

**Valid `mcp__cas__coordination` actions** (do not invent others):
- *Agent*: `register`, `unregister`, `whoami`, `heartbeat`, `agent_list`, `agent_cleanup`, `session_start`, `session_end`, `loop_start`, `loop_cancel`, `loop_status`, `lease_history`, `queue_notify`, `queue_poll`, `queue_peek`, `queue_ack`, `message`, `message_ack`, `message_status`
- *Factory*: `spawn_workers`, `shutdown_workers`, `worker_status`, `worker_activity`, `clear_context`, `my_context`, `sync_all_workers`, `gc_report`, `gc_cleanup`, `remind`, `remind_list`, `remind_cancel`
- *Worktree*: `worktree_create`, `worktree_list`, `worktree_show`, `worktree_cleanup`, `worktree_merge`, `worktree_status`

**Task ID is always `id`** — not `task_id`, `taskId`, or `_id`.

**Priority** is `0=Critical, 1=High, 2=Medium (default), 3=Low, 4=Backlog`. Accepts numeric OR named alias: `priority=1` ≡ `priority="high"`. Other aliases: `critical`, `medium`, `low`, `backlog`, `p0`-`p4`.

**Initial assignment uses `update`, NOT `transfer`:**

```
# CORRECT — initial assignment of an unclaimed task
mcp__cas__task action=update id=cas-abc1 assignee=<worker-name>

# WRONG — transfer requires an ALREADY-CLAIMED lease, otherwise errors
# with "No active lease found". Use transfer only to reassign between
# workers after one has claimed.
mcp__cas__task action=transfer id=cas-abc1 to_agent=<worker>
```

The `transfer` action's target field is `to_agent` (not `assignee`). The `update` action's target field is `assignee` (not `to_agent`). Yes, they disagree. Remember: `update assignee=...` for initial assignment; `transfer to_agent=...` only when reassigning a claimed task.

**Dispatching tasks is a two-step operation.** Sending a coordination message telling a worker to "claim tasks X and Y" does not actually dispatch work — workers react to `assignee` changes on the task, not to message content. Full pattern:

```
# 1. Create
mcp__cas__task action=create title="Fix login bug" priority=high \
  description="..." acceptance_criteria="..."

# 2. Assign (this is what causes the worker to pick it up)
mcp__cas__task action=update id=cas-abc1 assignee=<worker>

# 3. (optional) Provide extra context as a separate message
mcp__cas__coordination action=message target=<worker> \
  summary="cas-abc1 briefing" \
  message="Extra context for cas-abc1: ..."
```

Skipping step 2 leaves the task unassigned — the worker will go idle regardless of how clear the message in step 3 was.

**Coordination messages require BOTH `message` and `summary`:**

```
mcp__cas__coordination action=message target=worker-1 \
  summary="c29a ready for review" \
  message="Please verify cas-c29a. Commit dfe824b on main."
```

Missing either field is a rejection. `summary` is the one-line UI preview; `message` is the full body.

**Task notes** parameter is `notes` (plural), not `note`:

```
mcp__cas__task action=notes id=cas-abc1 notes="Progress update" note_type=progress
```

**Booleans** accept native bool, string `"true"`/`"false"`, or numeric `1`/`0`.
