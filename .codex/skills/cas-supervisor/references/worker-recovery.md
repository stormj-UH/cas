# Worker Recovery — Triage and Failure Modes

## Is the worker actually dead? (cas-4513 triage)

Before you run `shutdown_workers` on a pane that *looks* broken, spend 60 seconds on triage. The supervisor TUI is not ground truth for worker liveness — the most common false-positive failure mode is a worker that's mid-way through a long tool call or showing Claude Code's Bun/React-Ink crash screen (which leaves the process alive with an unresponsive UI). Destructive recovery on a live worker rips its worktree out from under itself and turns a recoverable hang into a real crash.

**Step 1: classify.** `cas factory is-wedged <worker>` returns one of four states plus evidence and exits with a differentiated code:

| Exit | State | What it means | Recovery |
|---|---|---|---|
| 0 | `alive` | PID up, transcript fresh, no crash signature — worker is running. | Wait. |
| 1 | `wedged` | PID up, transcript fresh, Bun/React-Ink crash signature matched. | `cas factory kill` + respawn. |
| 2 | `starved` | PID up, transcript cold (>60s since last write). Likely scheduler-starved or hung on a tool call. | Wait another 2 minutes, then re-classify. |
| 3 | `dead` | PID gone. | Cleanup only — no kill needed. |

The Bun/React-Ink crash signature is the visual fingerprint captured in the cas-4513 discovery note 2026-04-23 15:11 UTC: the pane fills with minified source paths like `/$bunfs/root/src/entrypoints/cli.js`, React-Ink `createElement("ink-box", ...)` enumerations, and a JS stack trace. The Bun event loop does NOT exit on unhandled rejection, so the PID stays alive and a daemon-faked heartbeat stays fresh — without the transcript grep you cannot distinguish this from a live worker mid-call.

**Step 2: read the transcript tail.** `cas factory debug <worker> --tail 20` prints the last N JSONL entries from `~/.claude/projects/*/<session>.jsonl` without touching the TUI. This is the canonical "what did the worker just do" signal — use it to decide whether the wedged state has salvageable in-flight work before killing.

**Step 3: recovery.** Only after `is-wedged` reports `wedged` or `dead`:

- **Wedged:** `cas factory kill <worker>` — SIGKILL (SIGTERM is observed not to exit cleanly on the Bun wedge) and reset any leased tasks (release lease + status→Open + clear assignee, same semantics as `mcp__cas__task action=reset`). Idempotent on an already-dead process. Then respawn.
- **Starved:** do not kill. Come back in 2 minutes; if it re-classifies as `wedged`, proceed to the kill path.
- **Dead:** no kill needed. The `kill` verb is still safe to run (`skipping SIGKILL` + task reset runs); or manually `mcp__cas__task action=reset id=<task>`.

**PID-recycling guard.** `cas factory kill` refuses to SIGKILL unless the `/proc/<pid>/stat` starttime fingerprint recorded at agent registration (cas-ea46 / cas-b157) still matches the process at that PID. On a busy host the kernel can recycle a PID between registration and kill, so without this guard we could SIGKILL an unrelated process. If the fingerprint mismatches, the summary says `pid N SKIPPED: starttime fingerprint mismatch (PID recycled). Pass --force to override.` — investigate before using `--force`. Legacy agents (registered before cas-ea46) have no fingerprint and also require `--force`.

**Anti-pattern:** "pane looks broken → `shutdown_workers`". That pathway has destroyed in-progress work multiple times (silent-owl-56 2026-04-23 shipped cas-4181 through what looked like a crashed pane). The `is-wedged` / `debug` / `kill` triad replaces it.

## Worker Failure Recovery

Workers fail in production. These are the three observed failure modes and their recovery procedures. All three have occurred in real factory sessions.

### Dead or Silent Worker

**Signature:** Worker stops responding to messages. No progress notes, no commits, no heartbeat updates. Task stays `in_progress` indefinitely.

**Diagnosis:**
1. Check worker status: `mcp__cas__coordination action=worker_status`
2. Look for stale heartbeat (last activity timestamp far in the past) or missing entry
3. Check worker activity log: `mcp__cas__coordination action=worker_activity`

**Recovery:**
1. Check the worker's worktree for partial work: `git -C .cas/worktrees/<worker> log --oneline main..HEAD`
2. If commits exist, cherry-pick salvageable work to the base branch before cleanup
3. Release the dead worker's lease: `mcp__cas__task action=release id=<task-id>`
4. Shut down the dead worker: `mcp__cas__coordination action=shutdown_workers count=0` (then respawn the count you need)
5. Spawn a fresh worker: `mcp__cas__coordination action=spawn_workers count=1 isolate=true`
6. Reassign the task to the new worker. If partial work was cherry-picked, include that context in the assignment message so the new worker builds on it rather than redoing it.

### Garbage Output (Context Exhaustion)

**Signature:** Worker output degrades into garbled multi-language text (Russian/Chinese characters mixed with English, repeating pseudo-words like "updofficial/action/official", BPE fragment nonsense). May be followed by a generic "violates Usage Policy" API error. This is token sampling collapse from an exhausted context window, not a real policy violation.

**Triggering conditions:** Long iterative fix-test-rerun loops, heavy stack trace volume in tool results, extended sessions with rapid context churn (20+ file edits in a short window).

**Recovery:**
1. **Do NOT send revision instructions.** The worker's context is poisoned — any further messages make it worse, not better.
2. Shut down the affected worker immediately. Do not attempt to salvage the session.
3. Check the worker's worktree for any commits made before degradation: `git -C .cas/worktrees/<worker> log --oneline main..HEAD`
4. Cherry-pick any good commits. Discard anything committed after degradation began (inspect diffs carefully — degraded output may have produced syntactically plausible but semantically wrong code).
5. Spawn a fresh worker with a clean context.
6. Reassign the task. If the task involves iterative test-fix loops, add guidance to the assignment: "periodically commit working state" so partial progress survives if degradation recurs.

### Verification Jail Deadlock

**Signature:** Worker reports `VERIFICATION_JAIL_BLOCKED` and cannot close tasks or use tools. The jail check fires agent-wide — one task's pending verification blocks ALL tool usage across all tasks for that worker.

**Note:** Factory workers are exempt from verification jail as of commit `bba6fbf`. If this failure mode appears, the running CAS binary is older than that fix.

**Diagnosis:**
1. Confirm the worker is actually jailed (not just reporting a stale error)
2. Check whether the running `cas` binary includes the jail exemption fix: verify the binary was rebuilt after `bba6fbf` landed

**Recovery (binary is current — exemption should apply):**
1. Rebuild CAS: `~/.cargo/bin/cargo build --release` and restart the `cas serve` process
2. Respawn workers — they will pick up the new binary

**Recovery (binary is outdated or rebuild is not feasible mid-session):**
1. Close the jailed task with an audit trail: `mcp__cas__task action=close id=<task-id> reason="Supervisor close — verification jail deadlock. Work verified at <commit-sha>. Worker jailed, CAS binary predates bba6fbf exemption fix."`
2. If `close` is also blocked, use direct sqlite as last resort:
   ```sql
   UPDATE tasks SET status='closed', pending_verification=0 WHERE id='cas-XXXX';
   UPDATE task_leases SET status='released' WHERE task_id='cas-XXXX' AND status='active';
   ```
3. After clearing the jail, message the worker that they can proceed with remaining tasks.
4. File a note on the epic that the binary needs rebuilding before the next session.

### Resource-Contention Worker Crashes (cas-0bf4)

**Signature:** Multiple workers wedge around the same time in the Claude Code JS crash-screen state (Bun/React Ink render exception). Host shows `uptime` load avg well above CPU count (5-min avg > 1.0 × num_cpus on a 16-thread box = saturated). Memory is NOT under pressure — this is CPU scheduler starvation, not OOM.

**Root cause:** Each worker's `cargo` builds a per-worktree `target/` with rustc fanning out to `num_cpus` parallel jobs. 4 workers × 16 rustc threads × an autofix pass = scheduler storm → Claude Code event loop starves → Ink render exception → worker wedged in crash-screen state. See task `cas-0bf4` and discovery in `cas-4513`.

**Built-in mitigation (on by default):** Factory mode exports `CARGO_BUILD_JOBS` into each worker's env at spawn and wraps the worker command with `nice -n 10` so cargo runs at a lower priority than the supervisor. Controlled by two config knobs in `.cas/config.toml`:

```toml
[factory]
# Cap on CARGO_BUILD_JOBS exported into workers.
# "auto" (default) = max(2, num_cpus / 4).
# Any numeric string like "4" is exported verbatim.
cargo_build_jobs = "auto"

# When true, prefix each worker spawn with `nice -n 10`.
# Default true. Flip false for single-worker or benchmarking.
nice_cargo = true
```

Shell-level overrides (win over config): `CAS_FACTORY_CARGO_BUILD_JOBS=<N>`, `CAS_FACTORY_NICE_WORKER=1`, `CAS_FACTORY_NICE_LEVEL=<N>`.

**When the defaults are wrong:**
- Running more than 4 workers on a 16-thread host → set `cargo_build_jobs = "2"` (÷4 assumption no longer holds).
- Host has 4–8 cores → the auto-cap floors at 2, which is still `workers × 2` rustc threads; on a 4-worker factory with 4 cores consider `cargo_build_jobs = "1"` manually.
- Host has 32+ threads → `"auto"` is fine; can push higher if wall-time matters.
- CPU-bound but not crashing → flip `nice_cargo = false` to let workers and supervisor compete on equal terms.

**Repro runbook (for verifying the cap works on a given host):** spawn 4 workers on this repo, trigger simultaneous cargo builds in all of them (`cargo test` in each worktree), watch `uptime` over 60 s. 5-min load avg should stay below CPU count. If it still saturates, drop `cargo_build_jobs` one step (e.g. `"auto"` → `"2"`) and re-check.

**If workers still wedge under these caps:** the scheduler storm is not the bottleneck. Likely candidates, in order of follow-up cost: (1) `sccache` shared across workers (cas-0bf4 Phase 2), (2) review-persona concurrency cap (cas-0bf4 Phase 3), (3) operational — spawn fewer workers.
