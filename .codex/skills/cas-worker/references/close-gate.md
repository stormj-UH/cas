# Close Gate — Self-Verification + Code Review

Both must run before `mcp__cas__task action=close`. The gate is the same regardless of task type. Skip and you eat a verifier rejection round-trip.

## Pre-Close Self-Verification

### 1. No shortcut markers
```bash
# Must return zero results in your changed files
rg 'TODO|FIXME|XXX|HACK' <changed_files>
rg 'for now|temporarily|placeholder|stub|workaround' <changed_files>
```

Also check for language-specific incomplete markers:
- **TypeScript**: `throw new Error('Not implemented')`
- **Rust**: `unimplemented!()`, `todo!()`
- **Python**: `raise NotImplementedError`

### 2. All new code is wired up
For every new function, class, module, route, or handler you created:
```bash
# Verify it's actually called/imported somewhere outside its definition
rg 'your_new_symbol' src/
```
If zero external references → you built it but didn't wire it in. Fix before closing.

Registration checklist (varies by framework):
- New CLI command → added to command registry?
- New API route/endpoint → added to router or module?
- New migration → listed in migration runner?
- New service/provider → registered in DI container?
- New config field → has a default, is read somewhere?

### 3. Changed signatures don't break callers
```bash
# If you changed a function signature, verify all call sites
rg 'changed_function' src/
```

### 4. Tests pass
```bash
# Run the project's test suite
# Examples: cargo test, pnpm test, pytest, npm test
```

If tests fail in code you didn't modify:
1. Re-run to check if flaky (transient failures happen).
2. If consistent, report as blocker with the specific test name and error output.
3. Do NOT try to fix other people's tests — that's out of scope.

### 5. No dead code left behind
Check for language-specific dead code markers on your new code:
- **TypeScript**: `// @ts-ignore` without justification
- **Rust**: `#[allow(dead_code)]`
- **Python**: `# type: ignore` without justification

### 6. System-wide test check

For every non-trivial change, trace **2 levels out** from the edited code — callers of the edited symbols, observers/middleware, hook subscribers, anything that imports the edited module. For each touched boundary:

- Confirm integration tests exist for that boundary, with **real objects** (not mocks) at the crossing point.
- **Run those integration tests** — not just the file you edited. `cargo test <crate>::<integration-test>` or equivalent. Presence of a test file is weak signal; an executed test is evidence.

"2 levels out" is LLM-judgment — do not over-engineer this into a call-graph analysis. Read the code, identify the obvious boundaries, test them.

**Skip allowed for**: pure additive helpers with no callers yet, pure styling changes, pure documentation changes. If you skip, record *why* in a task note (`note_type=decision`) before close. Don't skip silently.

Only close after all checks pass. The verifier will catch what you miss — but rejections cost time.

## Close-time Code Review Gate

Before closing any task with code changes, run the `cas-code-review` skill and pass its output to close:

1. **Run the review:**
   ```
   Skill(cas-code-review, mode=autofix, task_id=<your task id>)
   ```

2. **Pass the result to close.** The skill returns a `ReviewOutcome` JSON envelope. Pass it to close:
   ```
   mcp__cas__task action=close id=<task id> reason=<...> \
     code_review_findings='<ReviewOutcome JSON>'
   ```

3. **Skipped automatically** for `execution_note=additive-only` tasks and pure docs/test-only diffs. Calling close without findings on other tasks returns `CODE_REVIEW_REQUIRED`.

### If close is blocked on P0

1. Read every P0 finding — they are code-grounded, not speculative.
2. Fix the finding, commit, retry close. Do not spam-retry without fixing.
3. If you cannot fix it (pre-existing code, out-of-scope), forward the block to supervisor via `note_type=blocker` and wait. `bypass_code_review=true` is supervisor-only.

Non-P0 findings become follow-up tasks automatically — they don't block your close.

### What NOT to do

- Do not invoke the legacy `code-reviewer` agent — it's deprecated.
- Do not edit `close_ops.rs` or gate policy to let your diff through.
- Do not skip pre-close self-verification — the gate supplements your own checks.

**Latency:** the multi-persona review adds noticeable time to close. Do not assume it's hung or bypass the gate to dodge latency.

## Simplify-As-You-Go

After closing your **third** task in the current EPIC — and again after the 6th, 9th, 12th, etc. — invoke the `simplify` skill on your own recent work in that EPIC before picking up the next task.

- **Counter is per-worker-per-EPIC.** It resets when you move to a different EPIC.
- **Counter is stateless** — derive it at close time by querying `mcp__cas__task action=list assignee=<self> epic=<current-epic> status=closed` and checking whether `(count + 1) % 3 == 0` (the `+1` is for the task you're about to close).
- **Scope of simplification** = your own committed and staged work within the current EPIC only. Not cross-worker. Not cross-EPIC. Not code you haven't touched.
- **If the EPIC has fewer than 3 of your tasks total**, simplify-as-you-go never fires for you in that EPIC. That is intentional — the trigger exists to catch pattern accumulation, and <3 tasks is below the accumulation threshold.

The simplify pass should produce visible output — a commit, a task note, or an explicit "nothing to simplify" decision note. Do not run it silently.
