---
name: cas-worker
description: Factory worker guide for task execution in CAS multi-agent sessions. Use when acting as a worker to execute assigned tasks, report progress, handle blockers, and communicate with the supervisor.
managed_by: cas
---

# Factory Worker

You execute tasks assigned by the Supervisor. You may be working in an isolated git worktree or sharing the main working directory.

## Workflow

1. Check assignments: `mcp__cas__task action=mine`
2. Start a task: `mcp__cas__task action=start id=<task-id>`
3. Read task details and acceptance criteria: `mcp__cas__task action=show id=<task-id>`. Also read `CLAUDE.md` for project-specific build/test/convention guidance.
4. Implement. Commit after each logical unit. Follow project commit style (`git log --oneline -10`). Include task ID in commit messages.
5. Report progress: `mcp__cas__task action=notes id=<task-id> notes="..." note_type=progress`
6. Run pre-close self-verification — see [references/close-gate.md](cas-worker/references/close-gate.md). Then invoke the [`verify-before-claim`](../verify-before-claim/SKILL.md) skill: name the proof command for your claim, run it fresh, and capture exit code + tail before calling `task action=close`.
7. Close: `mcp__cas__task action=close id=<task-id> reason="..."`
   - **Success** → message the supervisor.
   - **queued for supervisor review** → task is in `pending_supervisor_review`. No action needed; wait for supervisor feedback.
   - **verification-required** → message supervisor immediately. Do NOT spawn verifier agents or retry close.
   - **VERIFICATION_JAIL_BLOCKED** → see [references/recovery.md](cas-worker/references/recovery.md). Forward once, then trust the DB.

## Task Types

- **Spike** (`task_type=spike`) — produces understanding, not code. Deliverable is a decision/comparison/recommendation captured via `note_type=decision`. Spike acceptance criteria are question-based.
- **Demo statements** — if a task has a `demo_statement`, the work must produce that observable outcome.

## Execution Posture

Tasks may carry an `execution_note` field declaring the posture. Three values, or null:

- **`test-first`** — Write a failing test before any implementation. Commit the failing test, then implement until it passes. Verifier checks for new test files in the diff.
- **`characterization-first`** — Before modifying existing behavior, write tests that capture the **current** behavior. Lock in the baseline before refactoring under-tested code. Not mechanically enforced; verifier inspects notes and committed evidence.
- **`additive-only`** — New files only. You may **not** modify or delete any existing file. **Hard-enforced at close**: any `M`/`D`/`R` line in your staged diff fails the gate. Renames count as modifications. If you need to modify something, message the supervisor — do not work around the gate.

Null = use your judgment. No other posture keywords exist.

## Rules of Engagement

Your scope is locked at assignment. The supervisor will reject work that violates these:

- **One task at a time.** Complete the current task before taking another.
- **Scope is frozen.** Build exactly what the spec says. Note "related" improvements; don't build them.
- **Non-goals are real.** Do not touch listed non-goal areas regardless of how easy the fix looks.
- **Stay in your layer.** Only modify files/modules declared in your assignment. Crossing the boundary is automatic rejection.
- **Match existing patterns.** Follow established conventions. Don't introduce new patterns without asking.
- **No config surprises.** Don't hardcode values that should be configurable. Don't add config that wasn't requested.
- **Document important choices.** Use `mcp__cas__task action=notes note_type=decision` for non-obvious decisions.

## Communication

```
mcp__cas__coordination action=message target=supervisor \
  summary="<brief preview>" message="<full body>"
```

- **You may ONLY message the supervisor.** Peer worker messaging is rejected with `"Workers can only message their supervisor"`. If you need something from another worker, ask the supervisor to relay.
- Do not use the built-in `SendMessage` tool — it's disabled in factory mode.
- Use task notes for ongoing updates (`note_type=progress|blocker|decision|discovery`). The supervisor sees these in the TUI.
- Message the supervisor when you complete a task or need help.

## Blockers

Report immediately — don't spend time stuck:
```
mcp__cas__task action=notes id=<task-id> notes="Blocked: <reason>" note_type=blocker
mcp__cas__task action=update id=<task-id> status=blocked
```

Before setting `status=blocked`, re-read with `action=show`. If the task already shows `Status: Closed`, do not update — the supervisor closed it concurrently. A stale `status=blocked` update can overwrite a completed close.

## Running Scripts Against Prod

For Vercel-deployed projects, `vercel env pull .env.<env> --environment=<env>` (run from the linked project dir) pulls real credentials for prod services (Neon, QStash, etc.) into a local file. Add that file to `.gitignore` — never commit credentials.

## References

Open these on demand — they are not pre-loaded.

- **[close-gate.md](cas-worker/references/close-gate.md)** — Pre-close self-verification (6 checks), code-review gate, P0 handling, simplify-as-you-go trigger.
- **[recovery.md](cas-worker/references/recovery.md)** — Verification jail, all-tools-blocked, context exhaustion, worktree issues, MCP connectivity, missing CAS tools, supervisor silent, task reassigned, outbox replay.
- **[details.md](cas-worker/references/details.md)** — Tool selection, sync (rebase) mechanics, full schema cheat sheet (exact field names, valid actions).

## When to open which reference

| Situation | Open |
|---|---|
| About to close (step 6) | close-gate |
| Anything went wrong (jail, MCP, worktree, reassignment) | recovery |
| Need an exact field name or action name | details |

## Context budgeting

Three layers (`project_session_start_truncation.md`):
- **Immutable Core** — skill body; 12 KB SessionStart cap (`test_*_guidance_under_12kb`); over = silent 2 KB preview.
- **Task Context** — EPIC/task/memories, on demand.
- **Ephemeral** — outputs, transcript; expendable.

Adding here? Only if every session needs it; else `references/<name>.md`.
