# MacBook from zero to CAS

End-to-end setup for a new user installing CAS on a Mac. Assumes the user is comfortable with a terminal but has never installed CAS or Claude Code before.

> **Hard requirement:** Apple Silicon Mac (M1 / M2 / M3 / M4). The official `install.sh` and Homebrew formula both reject Intel Macs — they error with `"Intel macOS is not currently supported."` If you're on an Intel Mac, build from source (Step 5C) or use Linux.

---

## What you'll end up with

- `cas` binary installed to your `$PATH`
- Claude Code installed and configured to talk to `cas` over MCP
- A project initialized with `cas init`
- The CAS factory TUI launching successfully

Total time: **~20 minutes** if you already have Homebrew + Node, **~40 minutes** from a truly fresh machine.

---

## Step 1 — Confirm your Mac

```bash
# Should print "arm64". If it prints "x86_64", you're on Intel — see "Intel Mac" note below.
uname -m

# macOS Sonoma (14) is the supported baseline. Older versions may work but are untested.
sw_vers
```

If `uname -m` prints `x86_64`, you have two choices: **(a)** build CAS from source on Linux instead, or **(b)** skip to Step 5C (build from source on Intel Mac). The pre-built binary path will not work for you.

---

## Step 2 — Install Homebrew (if you don't have it)

```bash
# Skip if `brew --version` already prints something.
/bin/bash -c "$(curl -fsSL https://raw.githubusercontent.com/Homebrew/install/HEAD/install.sh)"
```

After install, **add Homebrew to your shell** — the installer prints these two lines but they are easy to miss, and without them `brew` (and everything you install) won't be on your `$PATH`:

```zsh
echo 'eval "$(/opt/homebrew/bin/brew shellenv)"' >> ~/.zprofile
eval "$(/opt/homebrew/bin/brew shellenv)"
```

Then verify:

```bash
brew doctor   # expect: "Your system is ready to brew."
which brew    # expect: /opt/homebrew/bin/brew
```

> **Why `~/.zprofile` and not `~/.zshrc`:** `~/.zprofile` runs once per login, which is the right place for `$PATH` exports. `~/.zshrc` runs every interactive shell — putting `$PATH` setup there works but adds startup overhead. zsh has been the default macOS shell since Catalina (10.15), so this is what you want by default.

---

## Step 3 — Install Git and Node

```bash
brew install git node
```

`git` for cloning repos and the factory's worktree workflow. `node` for installing Claude Code (it's an npm package).

---

## Step 4 — Install Claude Code

```bash
npm install -g @anthropic-ai/claude-code@2.1.116
```

> **Why pin to 2.1.116:** versions 2.1.117 through at least 2.1.124 ship a React-Ink rendering regression that crashes Claude Code with `<Box> can't be nested inside <Text>` in agent-teams (factory) mode. 2.1.116 is the **last known-good**. Verify with `claude --version` after install. Before upgrading later, check whether [anthropics/claude-code#53838](https://github.com/anthropics/claude-code/issues/53838) is closed AND a release after 2.1.124 mentions Box/Text/Ink/render fixes in its changelog.

If npm complains about permissions when writing to a global path, prefer one of:

If you already used `brew install node` from Step 3, you generally won't hit permission errors — Homebrew owns its own prefix. Skip this. The workaround below is for users who installed `node` from the Node.js installer or another non-Homebrew source.

```bash
# Option A: use a user-local npm global prefix (only if you didn't use Homebrew node).
mkdir -p ~/.npm-global
npm config set prefix ~/.npm-global
echo 'export PATH=$HOME/.npm-global/bin:$PATH' >> ~/.zprofile
exec zsh -l   # re-login the shell so ~/.zprofile re-runs (sourcing it doesn't help new tabs)
npm install -g @anthropic-ai/claude-code@2.1.116 --save-exact

# Option B: switch to Homebrew Node.
brew install node   # if not already done in Step 3
# then re-run the npm install
```

> **`--save-exact`:** pins the version in your local npm cache so a stray `npm update -g` won't yank you onto a 2.1.117+ build that has the React-Ink crash.

Verify:

```bash
claude --version    # expect: 2.1.116 (Claude Code)
```

---

## Step 5 — Install CAS

Three paths in order of recommendation. Pick **one**.

### 5A — `install.sh` (recommended for most users)

```bash
curl -fsSL https://cas.dev/install.sh | sh
```

The script:
- Detects your platform (rejects Intel Mac and ARM Linux explicitly)
- Downloads the latest GitHub release (`cas-aarch64-apple-darwin.tar.gz`) — currently **v1.0**
- Installs to `/usr/local/bin/cas` if writable, otherwise `~/.local/bin/cas`. If you set `CAS_INSTALL_DIR` to a path that needs root (or `/usr/local/bin` exists but isn't writable in your case), the script will prompt for `sudo` to move the binary.
- Does NOT trigger Gatekeeper because `curl` doesn't set the `com.apple.quarantine` extended attribute on downloaded files
- Is **not sha256-verified** end-to-end. If supply-chain integrity matters, prefer Step 5B (Homebrew formula pins sha256s — note the irony that the older Homebrew path is more verifiable than `install.sh`).
- Is hosted at `cas.dev`. The script source is **not** mirrored in the cas-src repo at the time of writing; if you want to read it before piping into `sh`, fetch it first: `curl -fsSL https://cas.dev/install.sh -o /tmp/cas-install.sh && less /tmp/cas-install.sh`.

After install, the script may print a PATH suggestion using `~/.zshrc`. **Prefer `~/.zprofile`** (see Step 2 rationale). If it landed in `~/.local/bin/`, make sure that's on your `$PATH`:

```bash
echo 'export PATH=$HOME/.local/bin:$PATH' >> ~/.zprofile
exec zsh -l   # re-login so ~/.zprofile re-runs in this shell
```

### 5B — Homebrew

```bash
brew install codingagentsystem/cas/cas
```

> **Caveat:** the Homebrew formula is currently pinned to **v0.2.1** — much older than the v1.0 you'd get from `install.sh`. Use 5A instead unless you specifically need Homebrew for upgrade management.

### 5C — Build from source

Required if you're on an Intel Mac, or want a version newer than v1.0 (the latest tagged release).

> **What you'll get:** a `git clone` of the default branch (`main`) lands you on whatever HEAD is at clone time. The latest *tagged release* is **v1.0**; the development tip is **v2.10.1+**. If you want the v1.0 release specifically, append `--branch v1.0` to the clone. If you want the bleeding edge, use the default. Use `git log -1 --pretty=format:'%h %s' && cas --version` after build to know what you have.

```bash
# Install Rust toolchain (skip if `rustc --version` works).
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
source "$HOME/.cargo/env"

# Clone and build.
git clone https://github.com/codingagentsystem/cas.git
cd cas
cargo build -p cas --profile release-fast
install -m 0755 target/release-fast/cas ~/.local/bin/cas

# Make sure ~/.local/bin is on PATH.
echo 'export PATH=$HOME/.local/bin:$PATH' >> ~/.zprofile
exec zsh -l   # re-login so ~/.zprofile re-runs in this shell
```

This takes **5–15 minutes** on first build (downloading + compiling ~600 crate dependencies). Subsequent rebuilds are seconds.

---

## Step 6 — Verify the install

```bash
cas --version       # expect: cas X.Y.Z (...)
cas doctor          # runs diagnostics; should report all green
which cas           # confirm it's where you expect (/usr/local/bin/cas, /opt/homebrew/bin/cas, or ~/.local/bin/cas)
```

If `cas` is not found, your `$PATH` doesn't include the install directory. Re-run the relevant `echo ... >> ~/.zprofile` line from Step 5, then `exec zsh -l` to re-login the shell (or open a new terminal tab).

> **Gatekeeper note:** if you double-clicked a downloaded tarball in Finder and copied the binary out manually, macOS may block it with `"cas" cannot be opened because the developer cannot be verified`. Fix:
> ```bash
> xattr -d com.apple.quarantine $(which cas) 2>/dev/null || true
> ```
> The `|| true` keeps it copy-paste-safe whether or not the attribute is actually present. Avoid the issue entirely by sticking to `curl | sh`, `brew install`, or `cargo build` — none of those tag the binary with quarantine.

---

## Step 7 — Wire CAS into Claude Code (MCP)

CAS exposes its memory/task/skill APIs to Claude Code via MCP. There are two scopes; pick one:

- **Project scope** (recommended for getting started): write the config to `<your-project>/.mcp.json`. Claude Code picks it up only when run from that project. Travels with the repo if checked in.
- **User scope** (cross-project): write it to `~/.claude.json` (Claude Code's per-user config file). The exact path may vary between Claude Code versions; if `~/.claude.json` doesn't exist, run `claude mcp list` once to let Claude Code scaffold it. The two scopes are NOT interchangeable — settings.json schemas differ.

Either way, the `mcpServers` block looks the same:

```json
{
  "mcpServers": {
    "cas": {
      "command": "cas",
      "args": ["serve"]
    }
  }
}
```

Then in Claude Code, run `/mcp` — `cas` should appear and connect within ~1 second. If it's red ("disconnected"), the most likely cause is `cas` not being on the `$PATH` of the shell that Claude Code spawns; fix Step 6, or use an absolute command path in the config (e.g., `"command": "/opt/homebrew/bin/cas"` from `which cas`).

---

## Step 8 — Initialize CAS in a project

```bash
cd /path/to/your/repo
cas init
```

This creates a `.cas/` directory with `cas.db` (SQLite for memories/tasks/rules/skills) and a `config.toml`. It also writes a starter `.claude/` directory with built-in skills, agents, and settings.

```bash
ls .cas/
# expect: cas.db  config.toml  cache/  backup/

ls .claude/
# expect: skills/  agents/  rules/  settings.json
```

---

## Step 9 — First factory session

```bash
cas        # launches the factory TUI
# or
cas -w 3   # launches with 3 worker panes pre-allocated
```

You should see the factory UI: a supervisor pane on the left, worker panes on the right. Type a task into the supervisor input ("look around the codebase and tell me what's here") and the supervisor will spawn a worker to do it.

To exit cleanly: `Ctrl-Q` from the supervisor pane.

---

## Where things live (Mac edition)

| What | Path |
|---|---|
| `cas` binary | `/usr/local/bin/cas` *(install.sh, system writable)* or `/opt/homebrew/bin/cas` *(Homebrew)* or `~/.local/bin/cas` *(install.sh fallback / cargo)* |
| Global CAS data | `~/.config/cas/` (XDG-style, even on Mac — CAS doesn't use `~/Library/Application Support`) |
| Per-project CAS data | `<project>/.cas/` |
| Per-project Claude config | `<project>/.claude/` |
| MCP config (project) | `<project>/.mcp.json` |
| MCP config (user) | `~/.claude.json` (Claude Code's per-user config; scaffold with `claude mcp list` if absent) |
| Claude Code itself | `<your npm prefix>/bin/claude` (typically `/opt/homebrew/bin/claude` if installed via brew, or `~/.npm-global/bin/claude` with the user-prefix workaround) |

---

## Troubleshooting

### `command not found: cas` after install

`$PATH` is missing the install directory. Run:
```bash
which cas || echo 'not in $PATH'
echo $PATH | tr ':' '\n' | grep -E '/(usr/local|opt/homebrew|\.local)/bin'
```
Add whichever of `/usr/local/bin`, `/opt/homebrew/bin`, or `$HOME/.local/bin` is missing — see Step 5 / Step 6 fixes.

### `cas: bad CPU type in executable` on Intel Mac

You're on Intel Mac and got the Apple Silicon binary somehow (the install script *should* refuse, but if you grabbed the tarball manually). Build from source (Step 5C).

### `<Box> can't be nested inside <Text>` crash in Claude Code

You're on Claude Code 2.1.117 or newer and using factory / agent-teams mode. Roll back:
```bash
npm install -g @anthropic-ai/claude-code@2.1.116
```

### `cas serve` works in terminal but Claude Code shows MCP disconnected

The shell Claude Code spawns has a different `$PATH` than your interactive zsh. Either:
- Use the absolute path in `.mcp.json` (`"command": "/opt/homebrew/bin/cas"` or wherever `which cas` reports), or
- Move your `$PATH` exports from `~/.zprofile` to `~/.zshenv` so they apply to non-interactive shells too.

### Homebrew install gave me v0.2.1, install.sh gave me v1.0, but I want the latest features

Both binary distribution channels are well behind the development branch. Build from source (Step 5C) for current features. This is a known distribution gap — see `docs/ideation/2026-04-30-cas-shell-helpers-distribution-ideation.md`.

### Gatekeeper popup: `"cas" cannot be opened because the developer cannot be verified`

You downloaded the binary in a way that set the quarantine attribute (Safari, Mail, AirDrop). Strip it:
```bash
xattr -d com.apple.quarantine $(which cas) 2>/dev/null || true
```
Then retry. Alternatively, control-click the binary in Finder → Open → confirm.

---

## What this guide does NOT cover

- CAS Cloud sync setup beyond the basics — `cas login` + `cas cloud sync` are packaged commands; see the [README Team Memories section](../../README.md#team-memories-optional) for the full team-scope flow (`cas cloud team default <slug>`).
- The `cas-update` / `cas-refresh` orchestrator scripts — those live in `~/.local/bin/` and are author-specific. See `docs/ideation/2026-04-30-cas-shell-helpers-distribution-ideation.md`.
- Multi-user setups, shared cas.db, team collaboration patterns.
- Custom skill / agent authoring.
