---
name: codemap
description: Generate or update `.claude/CODEMAP.md` — a tight structural map of the repo (top-level layout, workspace members, key modules, entrypoints, where domain logic lives). Use when the user asks for a "codemap", "/codemap", "update codemap", "regenerate codemap", "the codemap is stale", or when SessionStart/PreToolUse warns that `.claude/CODEMAP.md` is missing or stale. This is the remediation skill for the codemap freshness gate.
managed_by: cas
---

# Codemap

Produce a **short, structural map** of the repo at `.claude/CODEMAP.md`. The goal is to replace blind glob/grep exploration with a 100–150 line index that names every directory worth opening and one-lines what lives there. File-structure facts only — domain content belongs in `docs/PRODUCT_OVERVIEW.md` (the `project-overview` skill).

**IMPORTANT: All file references use repo-relative paths** (e.g., `crates/cas-core/src/lib.rs`), never absolute paths.

## What this skill is (and isn't)

- **IS:** a navigational index of the repo. Names directories, workspace members, key entrypoints, where each subsystem lives.
- **IS NOT:** a product overview, a domain doc, an architecture deep-dive, or a README. `project-overview` covers product/domain; READMEs cover human onboarding.

If the project also has `docs/PRODUCT_OVERVIEW.md`, assume the reader has it. Don't restage product/domain content here.

## Read order (highest signal first)

Read only what's needed to map the structure. Stop once every top-level directory has a one-liner — do not exhaustively skim files.

1. **Workspace / package roots** — `Cargo.toml` `[workspace.members]`, `package.json` `workspaces`, `pnpm-workspace.yaml`, `pyproject.toml` — these enumerate every first-class package.
2. **Top-level directory listing** — name and purpose of every entry under repo root.
3. **Crate / package entrypoints** — `src/lib.rs`, `src/main.rs`, `src/index.ts`, `__init__.py`. Read enough to know what each crate exports.
4. **Module roots inside large crates** — `mod.rs` files, `src/` subdirectories that group cohesive subsystems.
5. **Route / handler files** — `cli/mod.rs`, `routes/`, `app/`, `handlers/`, `pages/`. These reveal user-facing surface area.
6. **Tests directory layout** — note the convention (`tests/`, `__tests__/`, inline `mod tests`), don't enumerate every test file.

**Skip** framework chrome and noise: `target/`, `node_modules/`, `dist/`, `build/`, lockfiles, `vendor/`, generated clients, snapshot directories, fixture trees, CI YAML, ESLint/Prettier configs, `.git/`.

## Output structure (fixed)

Write to `.claude/CODEMAP.md`. Target **100–150 lines**. Hard cap 200 lines. The file is grep-bait — short lines, lots of paths, one-liner per entry.

```markdown
# <Project Name> — Codemap
> Auto-generated structural map. Regenerate with `/codemap` when the layout drifts (modules added, removed, or renamed).

## Top-level layout
- `<dir>/` — one-line purpose
- `<dir>/` — ...
(every entry under repo root that isn't junk)

## Workspace / packages
- `<member-path>` — one-line purpose, language/framework hint
- ...
(only if the repo is a workspace; otherwise omit this section)

## <Member or top-level dir name>
Brief sentence (one line) on what this subsystem does.
- `path/to/module/` — purpose
- `path/to/entrypoint.ext` — purpose
- ...
(repeat per major subsystem; aim for 5–15 entries each, not exhaustive)

## Cross-cutting
- **Tests:** convention + where they live
- **Docs:** `docs/`, `README.md`, `CLAUDE.md`, planning dirs
- **Tooling / scripts:** `scripts/`, `.github/`, `Makefile`, etc.
- **Config:** `.claude/`, `.cas/`, env files, root-level configs

## Entrypoints
- CLI: `<path>` (binary name)
- Library: `<path>` (crate/package name)
- Service: `<path>` (binary/server name)
- Tests: `<command>` (e.g., `cargo test`, `pnpm test`)
```

## Quality bar — every line earns its place

Every line in the codemap must answer: *"if I'm hunting for X, does this line tell me where to look?"*

If yes, keep it. If it just restates the directory name, cut it.

- ❌ `src/` — source code
- ❌ `tests/` — tests
- ❌ `lib.rs` — library entrypoint
- ✅ `crates/cas-core/src/hooks/` — hook input schema, dispatcher types, handler trait
- ✅ `cas-cli/src/cli/codemap_cmd.rs` — `cas codemap status|pending|clear` subcommands
- ✅ `apps/web/src/routes/api/` — public REST endpoints (one file per resource)

When in doubt, **name a concrete module or filename** that lives there.

## Preserving hand-edited sections

If `.claude/CODEMAP.md` already exists:

1. **Read it first.**
2. **Preserve any `<!-- keep -->` … `<!-- /keep -->` blocks verbatim.** These are user-owned; do not rewrite, reflow, or even re-whitespace them. Place them back in the same section they appeared in.
3. Everything outside keep-blocks is regenerated.
4. If a section header has `<!-- keep -->` on the line directly below it, preserve that entire section including the header.

Example:

```markdown
## Cross-cutting
<!-- keep -->
- **Hot paths:** request handling lives entirely under `src/server/middleware/` — touch with care
- **Migration gotcha:** `prisma/seed.ts` runs in CI; never put dev-only fixtures there
<!-- /keep -->
- **Tests:** ...
```

The two bulleted lines and the `keep` markers survive re-runs.

## After writing the doc

### 1. Write a thin memory pointer

Invoke `mcp__cas__memory` with `action=remember` to create/update a pointer memory.

- **Name / title:** `project_<slug>_codemap.md` (slug = lowercase kebab-case of project name)
- **Body:** ONE line only. A repo-relative link to the doc plus a single-sentence hook.
- **No content duplication.** Do not inline the layout. The whole point is that search surfaces the pointer and the reader opens the doc.

Example:

```
See [.claude/CODEMAP.md](.claude/CODEMAP.md) — Rust workspace + TS frontend; CLI lives in `cas-cli/`, hooks in `crates/cas-core/`.
```

If a pointer already exists with the same name, update it. Do not create duplicates.

### 2. Commit CODEMAP.md to reset the staleness signal

The freshness gate (SessionStart hook + `cas codemap status`) uses **git history** as the sole authority. Once you commit `.claude/CODEMAP.md`, its git timestamp advances past all prior structural changes and both signals automatically report "up to date" in the next session.

```bash
git add .claude/CODEMAP.md
git commit -m "docs: regenerate CODEMAP.md"
```

Then verify:

```bash
cas codemap status
```

Should report `Status: up to date`. No manual `cas codemap clear` is required.

### 3. Report back

Print two things to the user:

1. The file path that was written.
2. A 3-bullet summary: (a) total line count, (b) how many top-level subsystems are mapped, (c) anything notable about the layout (workspace? monorepo? polyglot?).

## When to run

- **Missing:** `.claude/CODEMAP.md` does not exist → SessionStart fires a `severity="high"` banner, PreToolUse blocks worker dispatch. Generate from scratch.
- **Stale:** SessionStart/PreToolUse banner reports structural changes since CODEMAP.md was last updated → regenerate, keep-blocks survive.
- **Manual:** user invokes `/codemap` or asks to refresh the codemap.
- **After refactors:** modules were added, removed, or renamed across more than a handful of files.

## Anti-patterns

- Listing every file in the repo. This is a map, not an inventory. If a directory has 50 files, name the directory and 1–3 representative files.
- Drifting into product/domain content (personas, journeys, business concepts). That's `project-overview`'s job.
- Generic one-liners that just restate the path (`tests/ — tests`). Cut the line or write a real one.
- Skipping the keep-block check on regeneration. Destroying hand-edits is a trust breaker.
- Forgetting to commit `.claude/CODEMAP.md`. Freshness is computed from git history — committing resets the staleness signal for the next session.
- Forgetting to write the memory pointer.
- Including `target/`, `node_modules/`, `dist/`, `vendor/` as if they were source. They aren't — skip them.
