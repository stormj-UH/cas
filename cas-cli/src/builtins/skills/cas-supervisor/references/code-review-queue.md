# Supervisor-Owned Code Review Queue (cas-b51a)

When the project config has `[code_review] owner = "supervisor"`, workers skip the full multi-persona review at close and instead transition their tasks to `pending_supervisor_review`. This eliminates the ~14-minute per-close blocking cost on the worker side.

## Your responsibilities in this mode

1. **Monitor the review queue** — Tasks in `pending_supervisor_review` are waiting for you:
   ```
   mcp__cas__task action=list status=pending_supervisor_review
   ```
2. **Run the full review** — For each queued task, invoke `/cas-code-review mode=interactive task_id=<id>` or batch-review all pending tasks.
3. **Deliver the verdict** — After review, send the worker a coordination message with the findings summary and any P0/P1 issues to address. If clean, confirm they can consider the task complete.
4. **Record the verification** (optional) — `mcp__cas__verification action=add task_id=<id> status=approved summary="..."` to create an audit trail.

## Config to enable

Add to `.cas/config.toml`:
```toml
[code_review]
owner = "supervisor"
```

Default (`owner = "worker"`) preserves the existing behavior where each worker runs the full review inline. Stage 2 (flip the default to `supervisor`) is a follow-on task.
