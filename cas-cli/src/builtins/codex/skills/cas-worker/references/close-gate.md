---
name: close-gate
description: Worker pre-close self-verification gate.
managed_by: cas
---

# Close Gate — Self-Verification

Run all 6 self-verification checks before `mcp__cas__task action=close`. The gate is the same regardless of task type. Skip and you eat a verifier rejection round-trip.

**Code review is not your job at close** under the v2.13.0+ default `[code_review] owner = "supervisor"`. The supervisor runs `/cas-code-review` at cherry-pick + EPIC merge time. Do not invoke the multi-persona review yourself unless your supervisor explicitly tells you to, or your project has opted in to legacy `owner = "worker"` in `.cas/config.toml` — the worker-inline path adds ~14 min and ~100K tokens per close, which is exactly what the v2.13.0 flip was designed to eliminate.

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

**Rust public-type additions:** adding a field to a `pub struct` is a silent breaking change for downstream crates that construct the struct by listing every field (no `..Default::default()`). Those crates compile fine in isolation but fail with `E0063` at workspace scope. Always verify with check 4 scope rules when touching public types.

### 4. Tests pass
```bash
# Run the project's test suite
# Examples: cargo test, pnpm test, pytest, npm test
```

If tests fail in code you didn't modify:
1. Re-run to check if flaky (transient failures happen).
2. If consistent, report as blocker with the specific test name and error output.
3. Do NOT try to fix other people's tests — that's out of scope.

#### Rust: per-crate vs workspace-wide test scope

| What you changed | Minimum test run |
|---|---|
| Internal logic, private functions only | `cargo test -p <crate>` |
| Public type in `crates/*/src/lib.rs` — new/removed field, changed signature | **`cargo test --workspace`** |
| Anything in `crates/cas-mux`, `crates/cas-factory`, `crates/cas-types` | **`cargo test --workspace`** (consumed by `cas-cli`) |
| Test files only (`tests/**/*.rs`) with no API change | `cargo build --workspace --tests` at minimum |

**Why per-crate isn't enough:** when you add a field to a `pub struct` in a shared crate, that crate's own tests pass (the new field has a `Default`). But downstream crates that name every field in a struct literal fail with `E0063`. `cargo test -p <crate>` never sees this — only `cargo test --workspace` or `cargo build --workspace --tests` catches it.

**Historical note (cas-c0e0):** two fields were added to `FactoryConfig` (cas-factory). Per-crate tests passed. `cas-cli` constructors failed E0063 at workspace scope. The regression shipped to main as commit `3dc7488` and was caught only during manual merge.

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

## Simplify-As-You-Go

After closing your **third** task in the current EPIC — and again after the 6th, 9th, 12th, etc. — invoke the `simplify` skill on your own recent work in that EPIC before picking up the next task.

- **Counter is per-worker-per-EPIC.** It resets when you move to a different EPIC.
- **Counter is stateless** — derive it at close time by querying `mcp__cas__task action=list assignee=<self> epic=<current-epic> status=closed` and checking whether `(count + 1) % 3 == 0` (the `+1` is for the task you're about to close).
- **Scope of simplification** = your own committed and staged work within the current EPIC only. Not cross-worker. Not cross-EPIC. Not code you haven't touched.
- **If the EPIC has fewer than 3 of your tasks total**, simplify-as-you-go never fires for you in that EPIC. That is intentional — the trigger exists to catch pattern accumulation, and <3 tasks is below the accumulation threshold.

The simplify pass should produce visible output — a commit, a task note, or an explicit "nothing to simplify" decision note. Do not run it silently.
