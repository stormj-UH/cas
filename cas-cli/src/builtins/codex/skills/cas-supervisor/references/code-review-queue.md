# Supervisor-Owned Code Review Queue (cas-b51a / cas-865b)

When the project config has `[code_review] owner = "supervisor"` (the default as of cas-865b), workers skip the full multi-persona review at close and instead transition their tasks to `pending_supervisor_review`. This eliminates the ~14-minute per-close blocking cost on the worker side.

## The queue is a visibility tool — not the trigger

**The full review fires at cherry-pick time** (see workflow.md Phase 3, step 5), not at queue intake. Use this queue page to see what is awaiting cherry-pick, not to decide when to run the review.

```
mcp__cas__task action=list status=pending_supervisor_review
```

This shows you which tasks have been closed by workers and are waiting for you to cherry-pick and review.

## Review workflow

See **workflow.md Phase 3, step 5** for the exact review invocation sequence:
- Capture pre-cherry-pick HEAD: `git rev-parse HEAD@{1}`
- Invoke: `/cas-code-review mode=interactive base_sha=<pre_cp> task_id=<task-id>`
- Address P0 findings before notifying other workers to sync

## After review

1. **Deliver the verdict** — Send the worker a coordination message with the findings summary and any P0/P1 issues to address. If clean, confirm they can consider the task complete.
2. **Record the verification** (optional) — `mcp__cas__verification action=add task_id=<id> status=approved summary="..."` to create an audit trail.

## Config

Default as of cas-865b is `owner = "supervisor"` — no config entry is needed for new projects. To opt out to the legacy inline worker dispatch, add:
```toml
[code_review]
owner = "worker"
```
