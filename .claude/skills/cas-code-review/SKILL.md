---
name: cas-code-review
description: Multi-persona code review orchestrator. Runs as the pre-close quality gate for CAS factory workers and also on manual invocation. Dispatches 5 always-on reviewer personas (correctness, testing, maintainability, project-standards, fallow) plus any activated conditional personas (security, performance, adversarial) in parallel against the current diff, merges their structured findings through a deterministic pipeline, and routes results according to the invocation mode. Trigger automatically at `task.close` in `autofix` mode (the primary path), or manually for interactive review, report-only scans, or headless skill-to-skill calls.
managed_by: cas
---

# cas-code-review — Multi-persona code review orchestrator

This skill is the orchestrator for CAS's Phase 1 multi-persona code review pipeline. It does **not** perform the review itself — each reviewer persona is a separate Sonnet sub-agent with its own prompt under `references/personas/`. Your job in this skill is to:

1. Figure out **what** is being reviewed (the diff + the author's intent).
2. Decide **who** reviews it (the 5 always-on personas plus any activated conditional ones).
3. Run them in **parallel**, collect their structured output envelopes.
4. Hand the merged findings to the merge pipeline (Unit 5) and then route them according to the invocation mode.

Everything in this document is the orchestrator's responsibility. The personas themselves are authoritative on *what counts as a finding in their lane* — you do not second-guess their judgment here, you just marshal their inputs and outputs.

**Model tier:** The orchestrator and merge logic run on Opus. All 8 reviewer personas run on Sonnet. The fixer sub-agent (Unit 7) also runs on Opus. Do not inherit model choice from the caller — these are fixed per R13.

## Purpose

The primary trigger for this skill is **automatic invocation at CAS factory worker `task.close`** in `autofix` mode. The review runs *before* close completes and its outcome gates the close (P0 findings hard-block; see the mode table below). This is the tightest feedback loop the brainstorm identified and it is the path most invocations will take.

Three secondary modes exist and are described in the Mode reference table:

- `interactive` — a human is driving; show findings in a full UX, support a bounded 2-round fix-and-rereview loop.
- `report-only` — read-only scan, safe to run in parallel (never writes, never mutates tasks).
- `headless` — called from another skill, returns the merged envelope as structured text.

Do **not** invoke this skill manually as a substitute for `mcp__cas__verification` — that is a separate system, still owned by the task-verifier path. cas-code-review is a pre-close *quality gate*, not a verification record.

## Inputs

| Name | Required | Description |
|---|---|---|
| `base_sha` | optional | Commit to diff against. If absent, fall through to the Unit 3 helper at `crates/cas-store/src/code_review/base_sha.rs` (`code_review::base_sha::resolve`). That helper tries caller override → `GITHUB_BASE_REF` → `origin/HEAD` → `gh` default branch → common branches (`main`/`master`/`develop`/`trunk`) → `HEAD~1` in order and returns a full commit SHA. Never hand-roll base resolution here. |
| `changed_files` | computed | The set of files changed between `base_sha` and `HEAD`, computed via `git diff --name-only <base_sha>...HEAD`. You pass the full diff text to each persona, not just the file list. |
| `task_id` | optional | The CAS task ID being closed. Required in `autofix` mode (used for review-to-task linking, supervisor override path, and the task-note trail). Optional in other modes. |
| `mode` | default `autofix` | One of `autofix`, `interactive`, `report-only`, `headless`. See the Mode reference table below. |

If you receive none of these (bare manual invocation), default `mode=interactive`, resolve `base_sha` via the Unit 3 helper, and proceed.

## Step 1: Intent extraction

Before dispatching any reviewer, write a **2–3 line intent summary** describing *what the author was trying to accomplish*. This becomes an input to every persona so their calibration matches the author's stated goal — a "refactor only" change is judged differently from a "new security-sensitive endpoint".

Sources to mine, in order of priority:

1. The associated CAS task (`mcp__cas__task action=show id=<task_id>`) — its title, description, acceptance criteria, and notes are the strongest signal.
2. The commit messages on the diff (`git log --format=%B <base_sha>..HEAD`).
3. Any linked PR description if one is discoverable without shelling out to `gh` (Phase 1 does not make `gh` a hard dependency).
4. The actual diff as a last resort — only if the above are silent.

The intent summary is your own synthesis, not a quote. It must capture:

- **Goal:** one line — what behavior should exist after this change that did not exist before.
- **Scope marker:** one line — "refactor / no behavior change", "new feature", "bug fix", "dependency bump", etc.
- **Non-goals (optional):** one line — anything the author explicitly said they were *not* doing.

Keep it tight. Personas use this to calibrate severity, not to learn the problem domain from scratch.

## Step 2: Conditional persona selection (LLM-judged, not path pattern matching)

The **5 always-on personas always run**: `correctness`, `testing`, `maintainability`, `project-standards`, `fallow`. No conditions, no exceptions. The `fallow` persona is a thin wrapper around the deterministic `fallow audit` CLI — it self-skips on non-JS/TS repos / diffs and on missing-binary; it is "always-on" in the sense that the orchestrator always dispatches it and lets the persona's internal skip rules decide whether to emit findings. See `references/personas/fallow.md` for the skip contract.

The **3 conditional personas** — `security`, `performance`, `adversarial` — activate based on the R2 heuristics below. **Critically: you activate them by reading the diff and judging whether the heuristic applies. This is an LLM judgment, LLM-judged, not path pattern matching.** Do not grep for `/auth/` in paths and call it security activation. Do not count lines with `regex`. Read the diff, understand what it does, decide whether the heuristic fires.

Why the stridency: past iterations of automated review have drifted into "activate security if file name contains login" — which is both false-positive noisy (renaming a file does not make it security-relevant) and false-negative dangerous (a SQL-injection fix in a file called `util.rs` is security-relevant regardless of its path). The whole point of running an LLM orchestrator here is to apply *judgment*. Use it.

**Activation heuristics (from brainstorm R2):**

- **`security`** — activate when the diff touches authentication boundaries, user input handling, or permission surfaces. Concretely: session/token issuance, signature checks, role/permission gates, anywhere external input is parsed or deserialized, anywhere a privilege decision is made. Changes that merely *pass through* auth-adjacent code without touching its logic do not require security review.
- **`performance`** — activate when the diff touches DB queries, data transforms on potentially large inputs, caching, or async code paths. Concretely: SQL/Prisma query construction, sort/group/aggregate logic, cache read/write/invalidation, new `async`/`await` usage in hot paths, anything touching a loop whose bound comes from untrusted data. Changes to one-shot, small-N, non-hot-path code usually do not.
- **`adversarial`** — activate when **either** (a) the diff is 50+ changed non-test lines, **or** (b) the diff touches any CAS high-stakes module: task verification flow (`close_ops`, `verify_ops`), factory coordination (spawn / message / queue / lifecycle), SQLite store mutations, hook system (`pre_tool`, `post_tool`), MCP tool dispatch. Either condition is sufficient. This is the persona that stress-tests for "what could go wrong under concurrent factory sessions", "what happens when this lease expires mid-operation", "what's the cascade when this assertion fires in production" — exactly the failure modes the Phase 0 debugging history documents.

Record your activation decision explicitly in the output envelope so a reader can see *which* personas ran and *why* the conditional ones were included. Example: `"activated": ["security", "adversarial"], "activation_reason": {"security": "diff adds a new MCP tool handler that parses untrusted input", "adversarial": "diff is 83 non-test lines and touches close_ops.rs"}`.

## Step 3: Parallel dispatch

Spawn all activated personas in parallel via the Task tool. This is a single message with one Task tool call per persona — **not** a sequential loop. Opus can issue all 5–8 Task calls in a single response; do so.

For each persona, the Task tool call passes:

- `subagent_type` — a Sonnet-tier generic subagent (the persona's prompt file *is* the prompt; persona files are not registered as first-class agents).
- **Persona prompt body** — loaded verbatim from `references/personas/<persona>.md`.
- **Intent summary** — the 2–3 line output of Step 1.
- **Full diff text** — `git diff <base_sha>..HEAD`, not just a file list. Personas need the actual hunks to ground findings.
- **Changed files list** — `git diff --name-only <base_sha>..HEAD` as a convenience index.
- **`base_sha`** — so the persona can spot-check history or ancestry if needed.
- **Findings schema reference** — point each persona at `references/findings-schema.md` and remind them that output must be a single `ReviewerOutput` JSON envelope conforming to that schema (and to `crates/cas-types/src/code_review.rs` at runtime).
- **Reviewer name** — so the persona knows which `reviewer` field to set in the envelope.
- **Model tier** — Sonnet, per R13.

Each persona returns exactly one `ReviewerOutput` JSON object. Collect all of them in a vector. Any persona that fails to produce valid JSON (parse error, schema violation, unknown field, invalid enum) is a **reviewer error**, not a finding — log the error to the output envelope's per-persona status field and continue with the rest. Do not retry a failed persona within a single review pass; the merge pipeline will surface the error.

**Parallelism is not optional.** Serializing personas burns latency budget for no benefit — they do not share state and Opus is already the orchestrator; running one at a time would just double or quadruple the review wall-clock. One message, N Task calls.

## Step 4: Hand off to the merge pipeline (Unit 5)

Once every persona has returned (or errored), pass the full vector of `ReviewerOutput` envelopes to the merge pipeline. Unit 5 implements this and defines the deterministic merge ordering from R4:

1. Schema validation — drop unparseable envelopes as reviewer errors.
2. Confidence gate — suppress findings with `confidence < 0.60`, except P0 findings which pass at `confidence >= 0.50`.
3. Fingerprint deduplication — collapse findings with the same normalized (`file`, `line bucket ±3`, normalized `title`).
4. Cross-reviewer agreement boost — when two or more reviewers agree on a fingerprint, add `+0.10` to the merged confidence (cap at `1.0`).
5. Pre-existing separation — split findings with `pre_existing: true` into a separate bucket so they can be reported without gating the close.
6. Conservative route resolution — when reviewers disagree on `owner`, keep the more restrictive owner (`human` > `downstream-resolver` > `review-fixer`).
7. Partition + severity-sorted presentation — group by severity descending.

**Unit 4 does not re-implement any of this.** Call Unit 5. Unit 5 is the source of truth for merge semantics, and any drift in that ordering belongs in Unit 5's module, not here.

## Step 5: Mode-specific output

With merged findings in hand, branch on `mode`:

- **`autofix`** — the primary path. Feed the merged output to Unit 7 (the autofix loop). Unit 7 runs at most 2 rounds: it applies `safe_auto` findings via the fixer sub-agent, re-runs this orchestrator on the patched tree, and exits after round 2 regardless of residual findings. After Unit 7 returns, any residual non-`safe_auto` findings are routed to CAS tasks via the Phase 1 review-to-task subsystem (Unit 8), with priority mapping `P0→0`, `P1→1`, `P2→2`, `P3→3`. `advisory` findings never become tasks. Any surviving `P0` finding hard-blocks the close; the worker must either fix it and retry, or record a downgrade note and request supervisor override (R9).
- **`interactive`** — render the merged output to the user in a readable format (severity-sorted, grouped by reviewer, file+line anchored), offer the bounded 2-round fix loop as an explicit choice, and wait for the human to decide.
- **`report-only`** — write the merged envelope to a file under `docs/reviews/<YYYY-MM-DD>-<short-ref>.md` (or a caller-provided path) and exit. No edits. No task creation. No `task.close` side effects. Safe to run in parallel with other reviews.
- **`headless`** — return the merged envelope as a single structured text blob to the caller. No side effects beyond that return value. The caller decides what to do with the findings.

In every mode, the output envelope includes the activation decision from Step 2 and a per-persona status table so the caller can tell which reviewers ran, which succeeded, and which errored.

## Mode reference

The four invocation modes, per brainstorm R8 + R5 + R9–R11:

| Mode | Trigger | Edits files? | Creates tasks? | Gates close? | Fix loop | Notes |
|---|---|---|---|---|---|---|
| `autofix` | Automatic at factory worker `task.close` (primary) | Yes, via fixer sub-agent on `safe_auto` | Yes, residual non-`safe_auto` → CAS tasks with `P0→0…P3→3` | Yes — any P0 hard-blocks; supervisor override required to downgrade | Bounded `max_rounds=2` | R5, R9, R10, R11. This is the primary path; everything else is secondary. |
| `interactive` | Manual human invocation | Only via fixer if user accepts the offered loop | Only if user accepts | No | Bounded 2-round on user consent | R8. Full UX; show findings, let the human drive. |
| `report-only` | Manual or scheduled | No | No | No | None | R8. Safe for parallel runs; strictly read-only. |
| `headless` | Skill-to-skill call | No (orchestrator itself does not edit) | No | No | None | R8. Returns merged envelope as structured text; caller decides next steps. |

## Inputs from upstream units

This skill consumes, but does not implement:

- **Unit 1 (`cas-cfb5`)** — findings schema at `crates/cas-types/src/code_review.rs` + the human-readable doc at `references/findings-schema.md`. The personas and the merge pipeline both validate against this.
- **Unit 2 (`cas-1e98`)** — the 7 persona prompt files under `references/personas/`. You load these verbatim and hand them to the Task tool.
- **Unit 3 (`cas-c663`)** — the base-SHA resolution helper at `crates/cas-store/src/code_review/base_sha.rs`. You call this when the caller did not supply `base_sha`.

Downstream units this skill hands off to (not implemented here):

- **Unit 5** — merge pipeline. See Step 4.
- **Unit 6** — distribution (BuiltinFile registration and legacy `code-reviewer` cutover).
- **Unit 7** — autofix fixer sub-agent loop.
- **Unit 8** — review-to-task routing.

## Failure modes and how to handle them

- **A persona returns invalid JSON.** Record as a reviewer error in the envelope and continue. Do not retry. Do not fabricate findings.
- **Every persona returns no findings.** This is a clean pass — report it honestly; do not invent noise to justify the latency.
- **The Unit 3 helper returns `AllStrategiesFailed`.** Surface the error to the caller; do not fall back to a made-up base. A review without a base is worse than no review.
- **The diff is empty (no changed files).** Return an empty merged envelope with a clear note; do not run personas against nothing.
- **The activation judgment is genuinely uncertain** (e.g., "is this async code in a hot path?"). Prefer activation — false positives cost wall-clock; false negatives cost correctness.
