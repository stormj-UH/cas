# Workflow — Worker Modes, Phases, Blockers

## Worker Modes

Workers can run in two modes:

- **Isolated** (`isolate=true`): Each worker gets its own git worktree and branch. Use when workers will modify overlapping files or when you need clean branch-based merging.
- **Shared** (`isolate=false` or omitted): Workers share the main working directory. Simpler setup, but workers must coordinate to avoid editing the same files simultaneously.

## Worker Count Strategy

Spawn workers based on independent file groups, not task count.

1. Map which files each task will modify
2. Group tasks touching the same files into one lane (prevents conflicts)
3. Workers needed = number of parallel lanes

```
# 8 tasks, but only 2 independent file groups → 2 workers, not 8
workers = min(tasks_without_file_overlap, tasks_at_same_dependency_level)
```

In shared mode, file-overlap analysis is even more critical — two workers editing the same file simultaneously will cause problems.

## Phase 1: Plan

1. Search before planning — check all three sources for prior art:
   ```
   # Similar past EPICs (patterns, sizing, what worked)
   mcp__cas__task action=list task_type=epic status=closed

   # CAS memories for learnings, bugfixes, architectural decisions
   mcp__cas__search action=search query="<keywords>" doc_type=entry limit=10

   # Codebase for existing implementations you might duplicate or conflict with
   Grep pattern="<feature-name>" or mcp__cas__search action=search query="<keywords>" scope=code
   ```
2. Create EPIC: `mcp__cas__task action=create task_type=epic title="..." description="..."`
3. Gather spec with `/epic-spec`, break down with `/epic-breakdown`
4. Review task scope and dependencies

## Phase 2: Coordinate

1. Spawn workers:
   ```
   mcp__cas__coordination action=spawn_workers count=N isolate=true
   ```
   Omit `isolate` for shared mode.
2. Verify workers appear in TUI before assigning (stale DB records are not real workers)
3. Assign tasks: `mcp__cas__task action=update id=<id> assignee=<worker>`
4. Search for relevant context and send assignment message:
   ```
   mcp__cas__coordination action=message target=<worker> message="Task <id>: <description>. Context: <findings>. Run mcp__cas__task action=mine to see your tasks."
   ```
5. **End your turn immediately.** Stop here. Do not monitor, poll, or run any commands. Workers will push a message to you when done or blocked. Your next action is triggered by their message, not by checking.

### Resuming an Existing EPIC

Workers from previous sessions are gone. Stale DB records are not live processes.

1. **Check for binary/source drift** — fixes merged to main since last session don't take effect until rebuild. Run `~/.cargo/bin/cargo build --release` if CAS source changed, then restart `cas serve`. If a "fixed" bug reappears, this is the first thing to check.
2. Spawn fresh workers
3. Verify they appear in TUI
4. Assign open tasks to the new workers

## Phase 3: Merge and Sync (Isolated Mode)

When workers have isolated worktrees, merge their work into the epic branch after each completion, then tell other workers to sync.

```
base branch ────────────────────► (stays clean)
          \                    /
           └─ epic/feature ───►
              \          \     /
               ├─ factory/fox ┤
               └─ factory/owl ┘
```

**Worker completes a task:**
1. Worker closes their own task
2. Review changes in the worker worktree: `git -C .cas/worktrees/<worker> log --oneline main..HEAD`
3. Cherry-pick to base branch: `git cherry-pick <commit-sha>` (one per commit)
   - **If conflicts arise:** (a) non-overlapping additions (e.g., both workers added to Cargo.toml) — keep both entries, (b) semantic conflicts — review both changes and pick the correct merge, (c) if unsure — message the worker who committed for context before resolving
4. Verify build after cherry-pick: `~/.cargo/bin/cargo build --quiet`
5. Message other active workers to sync onto the **local** branch (not `origin/`):
   ```
   mcp__cas__coordination action=message target=<other-worker> message="Branch updated after cherry-pick. Sync: git stash && git rebase <base-branch> && git stash pop"
   ```
6. Clear completed worker's context: `mcp__cas__coordination action=clear_context target=<worker>`
7. Assign next task

## Phase 3: Review (Shared Mode)

When workers share the main directory, there's no branch merging — workers commit directly.

**Worker completes a task:**
1. Worker closes their own task
2. Review their commits
3. Clear worker context and assign next task

## Handling Blockers

- Workers set status to blocked and add a blocker note
- Help resolve or reassign the task
- **Race condition warning:** Task state updates are not atomic across supervisor and worker. After closing a task (especially via the escape hatch), verify it stayed closed before proceeding — a worker's stale `status=blocked` update can overwrite the close. If a worker resurrects a closed task, re-close with an audit trail noting the race.
- **Stale outbox replays:** Workers may send duplicate stale messages due to outbox replay. Before acting on a blocker notification or status change, check the task's current state with `mcp__cas__task action=show` — the message may be outdated.

**Multiple workers complete simultaneously:**
- Run verification calls in parallel (single response turn)
- Close approved tasks in a second parallel pass
- Reassign workers immediately

## Phase 4: Complete

1. Verify all tasks closed: `mcp__cas__task action=list status=open epic=<epic-id>`
2. Run tests
3. **Isolated mode only**: Merge epic to base branch and cleanup worktrees (can be 10GB+ each):
   ```bash
   git checkout <base-branch> && git merge epic/<slug>
   mcp__cas__coordination action=shutdown_workers count=0
   git worktree remove <path>  # for each worker worktree
   git branch -d epic/<slug>
   ```
4. Shutdown workers: `mcp__cas__coordination action=shutdown_workers count=0`
