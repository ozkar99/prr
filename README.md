# review-tool

A macOS desktop app for reviewing GitHub pull requests with real code intelligence.
It checks out the PR, runs the language's own server against it (gopls, clangd, pyright,
starpls), and shows a VS Code-style side-by-side diff, so you get hover types, go-to-definition,
peek and find-references while you review. Comments, replies and reviews go straight to GitHub.

![Review Tool reviewing a Go PR: side-by-side diff with hover type info from gopls, and Claude Code answering a question about the selected lines in the bottom panel](docs/screenshots/review-tool.png)

*Reviewing a Go PR: the diff with hover docs from gopls (top), and Claude Code explaining the
selected lines (bottom, ⌘L).*

**It's a single native binary.** The whole app is Rust, with a GPU-rendered UI built on
[GPUI](https://www.gpui.rs) (the framework behind the Zed editor) and
[GPUI Kit](https://github.com/longbridge/gpui-component); no web view, no Electron. The binary is
about 30 MB. The only extra pieces are two language servers that run on Node.js (pyright and
bash-language-server), which ship inside the app.

## Setup

### 1. Prerequisites

| Tool | Why | Install |
|---|---|---|
| macOS 11 or later, Apple Silicon or Intel | | |
| Homebrew | installs and updates the app | [brew.sh](https://brew.sh) |
| Xcode Command Line Tools | git (checking out PRs) and clangd (C/C++) | `xcode-select --install` |

Homebrew also installs the GitHub CLI (`gh`) and Node.js with the app.

Language servers. Syntax highlighting works without them; they add hover types and navigation.
Install only the ones for languages you review:

| Language | Files | Server | Install |
|---|---|---|---|
| Go | `.go` | gopls | `brew install go`, then `go install golang.org/x/tools/gopls@latest` |
| C/C++ | `.c` `.cc` `.cpp` `.h` `.hpp` … | clangd | included with Xcode Command Line Tools (Homebrew `llvm` is used if installed) |
| Python | `.py` `.pyi` | pyright | nothing: bundled with the app (runs on Node.js) |
| Shell | `.sh` `.bash` `.zsh` `.ksh`, `.bashrc`, `.zshrc`, … | bash-language-server | nothing: bundled with the app (runs on Node.js) |
| Starlark / Bazel | `BUILD` `.bazel` `.bzl` `WORKSPACE` `MODULE.bazel` | starpls | download from [starpls releases](https://github.com/withered-magic/starpls/releases) and put it on your `PATH` |

A missing server shows as **not installed** in the app's header; hover over it for the install command.

### 2. Install

```sh
brew tap ozkar99/review-tool https://github.com/ozkar99/review-tool
brew trust --cask ozkar99/review-tool/review-tool   # Homebrew asks before using a third-party tap
brew install --cask review-tool
```

This installs **Review Tool** into Applications and the `review-tool` command onto your `PATH`,
along with the GitHub CLI and Node.js if you don't have them. Then log in to GitHub once (the app
uses `gh`'s login):

```sh
gh auth login
```

Update with `brew upgrade --cask review-tool`; uninstall with `brew uninstall --cask review-tool`
(add `--zap` to also remove its checkouts and settings).

On first launch the home screen checks for the tools the app uses and shows the install command
for anything missing.

## Usage

### Opening a PR

From a terminal:

```sh
review-tool https://github.com/owner/repo/pull/123   # a PR URL
review-tool owner/repo#123                           # a PR reference
review-tool 123                                      # inside a clone: PR #123 of that repo
review-tool                                          # inside a clone: the PR for your current branch
```

Each PR gets its own window, so you can review several at once. If the PR is already open, its
window comes to the front; a window on the home screen is reused. Each window has its own
language servers, agent session and back stack, and closing it (**⌘W**) cleans up just that PR.
**⌘N** opens a new window, **⌘`** switches between them, and the app quits with its last window.

You can also run `review-tool` with no PR (outside a repo) and pick one in the app: from
**Review requested** / **Your pull requests**, or by pasting a URL or `owner/repo#123`.

#### Where the code comes from

- **Inside your own clone of the PR's repo**, `review-tool` reviews the PR right there: no second
  copy, which matters for big monorepos, and your language-server caches are reused. The PR's
  commits are fetched into your clone without adding any branches or refs. If your clone isn't
  already at the PR (e.g. after `gh pr checkout`), the app asks before switching it, and switches
  it back to your branch when you close the PR. It won't switch a clone with uncommitted changes.
- **More PRs from the same clone** (another window already uses it), or a clone with uncommitted
  changes: the PR gets a `git worktree` of your clone in the app's folder, filled with
  copy-on-write copies of your checkout (APFS clones). It shares your clone's objects and, on disk,
  costs only the files that differ, so several PRs of a large monorepo stay cheap. It appears in
  your clone's `git worktree list` while the window is open and is removed when you close it.
- **Anywhere else** (or from the app's PR list), the app uses its own copy: one clone per repo,
  shared by its PRs, plus a checkout per PR. The clone is blobless (history without file
  contents), so even a large repo's first PR opens quickly; later PRs only fetch what's new.

The app's checkouts live in `~/Library/Application Support/review-tool/`. Ones you haven't opened
for 14 days are removed automatically, and the **Storage** section at the bottom of the home
screen shows how much space they take, with buttons to clear them. Your own clones are never
touched by cleanup.

### Reading the code

- Files open as a side-by-side diff of the **whole file**, scrolled to the first change.
- **Hover** a symbol for its type and docs.
- **⌘-click** or **F12**: go to definition. If it's in another file the PR changes, that file
  opens as a diff; otherwise it opens read-only. **⌥F12** peeks instead; **⇧F12** finds references.
- **Ctrl+-**, the mouse back button, or **←** above the editor: go back.
- Several definitions or references show as a list; click one to open it, **Esc** closes it.

### Reviewing

The left sidebar has two sections:

- **Conversation**: the PR description, general comments, and review summaries (✓ approved,
  ± changes requested). Click one to read it. The bottom of that page has a box for a general comment.
- **Changed files**: each file lists its comment threads underneath (line, author, first line).
  Click a thread to jump to it.

In the diff:

- **Add a comment**: hover a line and click the blue **+** in the gutter, press **⌘⌥C**, or
  right-click → *Add Review Comment*. Select several lines first to comment on a range.
  Only lines inside the diff can be commented on (a GitHub rule).
- **⌘↩** submits a comment; **Esc** cancels.
- Threads show inline under their line. You can reply, resolve or unresolve, and delete your own
  pending comments. Threads whose code has since changed are under **Show N outdated threads**
  above the editor.

Comments and replies go into your **pending review**, which only you can see, like on GitHub.
When you're done, click **Finish review** (top right), add an optional summary, and choose
**Comment**, **Approve** or **Request changes**. General comments on the Conversation page are
the exception: they post immediately.

### Asking an agent about the code

**⌘J** (or **✦ Claude Code** in the header) opens a terminal panel at the bottom running a coding
agent in the PR's checkout: Claude Code by default, using your existing Claude Code login. It
starts the first time you open the panel and keeps running while the panel is hidden.

To ask about code, select some lines (or just click one), then use the shortcut or the
**Agent** items at the top of the code's right-click menu:

| Command | Shortcut | What happens |
|---|---|---|
| **Agent: About this code** | ⌘L | The panel opens with `About <file> lines A-B: ` typed in; you finish the question. |
| **Agent: Explain this code** | ⌘⇧L | Sends `Explain <file> lines A-B.` right away. |

Lines on the left side of a diff are marked as the old version. If the agent is just starting, the
text is typed once its prompt is ready. It's never typed into a question the agent is asking (like
Claude Code's "trust this folder?" on a new checkout): answer that yourself first.

In the panel:

- It's a full terminal: **⌘C** / **⌘V** copy and paste, **⌘K** clears, scroll for history,
  **⌘-click** opens a link.
- Drag the divider to resize; the height is remembered.
- **Restart** appears if the agent exits.
- **⌘J** hides it again; **Ctrl+-** and **⌘L** go to the agent while you're typing there, not to the app.
- The terminal font size is in **Settings**.

If you start `review-tool` from inside a Claude Code session, the panel's agent still starts as a
separate session of its own.

The agent knows what you're looking at through the app's MCP server, which has three tools:
`get_selection` (file, side of the diff, lines, selected text), `get_pr` (title, description,
changed files) and `get_file_diff` (one file's diff against the merge-base).

Claude Code is started so that the PR can't configure it: the repo's `.mcp.json` and
`.claude/settings.json` (hooks, permissions) are ignored, and Edit/Write are disabled since this
is review, not editing.

#### Using another agent (Codex, Gemini CLI, …)

Open **Settings** (**⌘,**, **Review Tool → Settings…** in the menu bar, the **Settings** link on
the home screen, or **⚙** in a PR) and enter
the command under **Agent command**. It runs in your login shell, in the PR checkout; clear it to go
back to Claude Code. It applies the next time the agent starts (**Restart** in the panel, or reopen
the PR). A command in `~/.review-tool-agent-command`, as older versions used, still works when the
setting is empty.

Every agent gets these environment variables:

| Variable | Contents |
|---|---|
| `REVIEW_TOOL_MCP_STDIO` | A stdio MCP server command (no arguments) that forwards to the app; for agents that only run stdio MCP servers |
| `REVIEW_TOOL_MCP_URL`, `REVIEW_TOOL_MCP_TOKEN` | The HTTP MCP endpoint and its bearer token |
| `REVIEW_TOOL_MCP_CONFIG` | `{"mcpServers": {...}}` JSON for the HTTP endpoint, the shape Claude Code's `--mcp-config` takes |
| `REVIEW_TOOL_SELECTION_FILE` | A JSON file with the PR and current selection, kept up to date; for agents without MCP |
| `REVIEW_TOOL_INSTRUCTIONS` | A paragraph explaining the review setup, to pass as the agent's system prompt or first message |
| `REVIEW_TOOL_PR`, `REVIEW_TOOL_ROOT`, `REVIEW_TOOL_MERGE_BASE` | `owner/repo#N ("title")`, the checkout path, and the diff base commit |

Examples (Codex and Gemini CLI weren't available to test these with; check their docs for the
exact flags):

```sh
# Codex, with the review-tool MCP server over stdio
codex -c "mcp_servers.review_tool.command=\"$REVIEW_TOOL_MCP_STDIO\""

# Claude Code with a different model, keeping the app's context
claude --model sonnet --mcp-config "$REVIEW_TOOL_MCP_CONFIG" --strict-mcp-config --setting-sources user --append-system-prompt "$REVIEW_TOOL_INSTRUCTIONS"
```

An agent configured this way runs with whatever permissions and project settings it normally
uses; the protections above apply only to the built-in Claude Code default.

### Settings

**⌘,** (or **Review Tool → Settings…** in the menu bar) opens Settings: the agent command, the editor and terminal font sizes (applied right
away), and the agent panel's height is remembered. They're saved in
`~/Library/Application Support/review-tool/settings.json`.

## Troubleshooting

| Problem | Fix |
|---|---|
| "Couldn't load pull requests" | Check `gh auth status`; run `gh auth refresh -h github.com` if the token is invalid. |
| A server chip says **not installed** | Install it (hover the chip for the command), then reopen the PR. |
| A server chip says **failed** | Hover it to see the server's error output. |
| C++ types are wrong or headers don't resolve | clangd needs `compile_commands.json` (see below). |
| The agent panel says the agent exited | Its output is in the panel. Check the agent command in **Settings**, then **Restart**. |

### C++ compile flags

clangd uses `compile_commands.json` if it's at the repo root or in `build/`, `out/`,
`cmake-build-debug/` or `builddir/`. Otherwise it guesses: `-std=c++20` plus `-I` for the repo
root, `include/`, `src/`, `lib/`, `third_party/` and `external/`. That resolves most headers but
can pick the wrong overload. The app doesn't generate the file for you, because that means running
the PR's build system.

## Safety

PR code is untrusted, so the app never builds or runs it. In particular:

- gopls runs with `GOTOOLCHAIN=local`, so a PR's `go.mod` can't make it download and run another Go toolchain.
- The editor is read-only and can only read PR checkouts plus the places go-to-definition leads:
  the Go stdlib and module cache, system and Homebrew headers, Python's stdlib and packages,
  and Bazel's external repo cache.
- Markdown in comments and descriptions is rendered as text: no HTML, scripts or remote content other than avatars.

## Development

### Building from source

Needs Rust ([rustup](https://rustup.rs)); Node.js only to bundle the Python and shell language
servers.

```sh
git clone git@github.com:ozkar99/review-tool.git
cd review-tool
cargo run --release -- --open-pr=https://github.com/owner/repo/pull/123
```

The first build compiles the dependencies and takes a few minutes. No Xcode is needed: shaders are
compiled at run time.

For Python and shell language features in development builds, install their servers once:

```sh
npm install --prefix target/language-servers pyright@1.1.414 bash-language-server@5.8.1
```

To build the app bundle and DMG (`dist/`), as releases do:

```sh
scripts/bundle-macos.sh             # this Mac's architecture
scripts/bundle-macos.sh universal   # Apple Silicon + Intel
```

#### The `review-tool` command

The app's own binary is the command. Link it onto your `PATH`:

```sh
ln -sf "$PWD/dist/review-tool" /opt/homebrew/bin/review-tool
```

(`dist/review-tool` is a symlink into the app bundle made by `scripts/bundle-macos.sh`; the
command needs the bundle next to it.)

The app allows a single instance: `review-tool <pr>` hands the PR to the open window. Development
builds use a separate instance, so they don't interfere with the installed app.

For scripted UI testing, a development build started with `REVIEW_TOOL_DEBUG_PORT=<port>` accepts
JSON commands POSTed to `http://127.0.0.1:<port>/` (pointer moves, clicks, keys, typing,
screenshots of its window; see `src/devdriver.rs`). This isn't compiled into release builds.

Data lives in `~/Library/Application Support/review-tool/`: `repos/` has one blobless bare clone
per repo, `worktrees/` one checkout per PR (see `src/backend/git.rs`; cleanup in
`src/backend/cleanup.rs`).

### Releasing

1. Bump `version` in `Cargo.toml`; commit and push.
2. Tag it: `git tag -a v0.4.0 -m "Review Tool v0.4.0" && git push origin v0.4.0`. CI
   (`.github/workflows/release.yml`) builds a universal (Apple Silicon + Intel) app and publishes
   it as a GitHub Release.
3. Point the cask at it: in `Casks/review-tool.rb`, set `version` and set `sha256` to the
   checksum of the release's `.dmg`
   (`curl -sL <dmg url> | shasum -a 256`), then commit and push. Teammates get it with
   `brew upgrade --cask review-tool`.

**Actions → Release → Run workflow** builds without releasing (the app is attached to the run).

The app is ad-hoc signed; the cask clears macOS's download quarantine so it opens without a
prompt. To sign and notarize with an Apple Developer ID instead, add these repository secrets
(Settings → Secrets and variables → Actions) and the workflow uses them automatically:

| Secret | Value |
|---|---|
| `APPLE_CERTIFICATE` | The "Developer ID Application" certificate exported as `.p12`, base64-encoded (`base64 -i cert.p12 \| pbcopy`) |
| `APPLE_CERTIFICATE_PASSWORD` | The `.p12` export password |
| `APPLE_SIGNING_IDENTITY` | e.g. `Developer ID Application: Your Name (TEAMID)` |
| `APPLE_ID` | The Apple ID email used for notarization |
| `APPLE_PASSWORD` | An app-specific password for that Apple ID (appleid.apple.com → Sign-In and Security) |
| `APPLE_TEAM_ID` | The 10-character team ID |

### How it works

| Piece | Where |
|---|---|
| `review-tool` command: resolves the PR with `gh`, launches or reuses the app | `src/cli.rs`, `src/instance.rs` |
| Homebrew cask (this repo is also the tap) | `Casks/review-tool.rb` |
| Window, home screen, setup check, storage | `src/main.rs`, `src/app.rs`, `src/home.rs`, `src/backend/deps.rs` |
| GitHub API: review queue, PR files, threads, comments, submitting reviews | `src/backend/github.rs` |
| Bare clone per repo + `git worktree` per PR, diff against the merge-base | `src/backend/git.rs` |
| Side-by-side diff (vendored from GPUI Kit, with token hover/click and annotations) | `src/diffview/` |
| Syntax highlighting (tree-sitter, VS Code Dark Modern colors) | `src/highlight.rs` |
| Language servers per PR: hover, definition, references | `src/lsp.rs` |
| PR view: sidebar, navigation, back stack, hover and peek popups | `src/pr_view.rs` |
| Review threads, composer, conversation, review panel | `src/review.rs` |
| Agent panel: terminal (alacritty_terminal), launch command, MCP context server | `src/terminal.rs`, `src/backend/agent.rs`, `src/backend/mcp.rs` |
| Settings | `src/settings.rs` |
