# review-tool

A macOS desktop app for reviewing GitHub pull requests with real code intelligence.
It checks out the PR, runs the language's own server against it (gopls, clangd, pyright,
starpls), and shows the diff in VS Code's editor, so you get hover types, go-to-definition,
peek and find-references while you review. Comments, replies and reviews go straight to GitHub.

**You don't need VS Code installed.** The app bundles VS Code's editor code (editor, hovers,
peek views, Markdown renderer) as npm packages via
[`@codingame/monaco-vscode-api`](https://github.com/CodinGame/monaco-vscode-api). The app is built with
[Tauri](https://tauri.app): a Rust backend and macOS's built-in WebView, so the app is ~30 MB.

## Setup

### 1. Prerequisites

| Tool | Why | Install |
|---|---|---|
| Homebrew | installs and updates the app | [brew.sh](https://brew.sh) |
| Xcode Command Line Tools | git (checking out PRs) and clangd (C/C++) | `xcode-select --install` |

Homebrew also installs the GitHub CLI (`gh`) and Node.js with the app. The app uses `gh`'s login,
so log in once after installing: `gh auth login`.

Language servers. Syntax highlighting works without them; they add hover types and navigation.
Install only the ones for languages you review:

| Language | Files | Server | Install |
|---|---|---|---|
| Go | `.go` | gopls | `brew install go`, then `go install golang.org/x/tools/gopls@latest` |
| C/C++ | `.c` `.cc` `.cpp` `.h` `.hpp` … | clangd | included with Xcode Command Line Tools (Homebrew `llvm` is used if installed) |
| Python | `.py` `.pyi` | pyright | nothing: bundled with the app (runs on Node.js) |
| Starlark / Bazel | `BUILD` `.bazel` `.bzl` `WORKSPACE` `MODULE.bazel` | starpls | download from [starpls releases](https://github.com/withered-magic/starpls/releases) and put it on your `PATH` |

A missing server shows as **not installed** in the app's header; hover over it for the install command.

### 2. Install

```sh
brew tap ozkar99/review-tool https://github.com/ozkar99/review-tool
brew install review-tool
```

Homebrew builds the app on your Mac (the first install takes a few minutes and pulls in Rust and
Node as build tools). Because it's built locally, macOS opens it without any security prompt. This
also puts the `review-tool` command on your `PATH`. To open the app from Launchpad or Spotlight
too:

```sh
ln -sf "$(brew --prefix review-tool)/Review Tool.app" ~/Applications/
```

Update with `brew upgrade review-tool`.

On first launch the home screen checks for the tools above and shows the install command for
anything missing.

## Usage

### Opening a PR

From a terminal:

```sh
review-tool https://github.com/owner/repo/pull/123   # a PR URL
review-tool owner/repo#123                           # a PR reference
review-tool 123                                      # inside a clone: PR #123 of that repo
review-tool                                          # inside a clone: the PR for your current branch
```

If the app is already open, the PR opens in the existing window.

You can also run `review-tool` with no PR (outside a repo) and pick one in the app: from
**Review requested** / **Your pull requests**, or by pasting a URL or `owner/repo#123`.

The first PR from a repo clones it, once per repo; big repos take a while. Later PRs from the
same repo only fetch what's new.

### Reading the code

- Files open as a side-by-side diff of the **whole file**, scrolled to the first change.
- **Hover** a symbol for its type and docs.
- **⌘-click** or **F12**: go to definition. If it's in another file the PR changes, that file
  opens as a diff; otherwise it opens read-only. **⌥F12** peeks instead; **⇧F12** finds references.
- **Ctrl+-**, the mouse back button, or **←** above the editor: go back.

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
agent in the PR's checkout: Claude Code by default, using your existing Claude Code login. Select
some code and press **⌘L** (or right-click → *Ask Agent About Selection*): the panel opens with
`About <file> lines A-B: ` typed in, and you finish the question. Drag the divider to resize; the
session keeps running while the panel is hidden.

The agent knows what you're looking at through the app's MCP server, which has three tools:
`get_selection` (file, side of the diff, lines, selected text), `get_pr` (title, description,
changed files) and `get_file_diff` (one file's diff against the merge-base).

Claude Code is started so that the PR can't configure it: the repo's `.mcp.json` and
`.claude/settings.json` (hooks, permissions) are ignored, and Edit/Write are disabled since this
is review, not editing.

#### Using another agent (Codex, Gemini CLI, …)

Put the command to run in `~/.review-tool-agent-command` (one shell command; blank lines and `#`
comments are ignored). It runs in your login shell, in the PR checkout. Delete the file to go back
to Claude Code. Click **Restart** in the panel, or reopen the PR, after changing it.

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

## Troubleshooting

| Problem | Fix |
|---|---|
| "Couldn't load pull requests" | Check `gh auth status`; run `gh auth refresh -h github.com` if the token is invalid. |
| A server chip says **not installed** | Install it (hover the chip for the command), then reopen the PR. |
| A server chip says **failed** | Hover it to see the server's error output. |
| C++ types are wrong or headers don't resolve | clangd needs `compile_commands.json` (see below). |
| `review-tool` says the app isn't built | Run `npm run build` in the checkout. |
| The agent panel says the agent exited | Its output is in the panel. Check the command in `~/.review-tool-agent-command`, then **Restart**. |

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
- Markdown in comments and descriptions goes through VS Code's sanitizer, with command links disabled.

## Development

### Building from source

Needs Rust ([rustup](https://rustup.rs)) and Node.js 20+.

```sh
git clone git@github.com:ozkar99/review-tool.git
cd review-tool
npm install
npm run build
```

The first build compiles the Rust dependencies and takes a few minutes. The result is
`src-tauri/target/release/bundle/macos/Review Tool.app`.

#### The `review-tool` command

The app's own binary is the command. Link it onto your `PATH`:

```sh
ln -sf "$PWD/src-tauri/target/release/bundle/macos/Review Tool.app/Contents/MacOS/review-tool" /opt/homebrew/bin/review-tool
```

(Any directory on your `PATH` works; `/opt/homebrew/bin` is there on Apple Silicon Macs with
Homebrew.) After pulling new changes, run `npm run build` again.


```sh
npm run dev        # app with hot reload (frontend) and auto-rebuild (Rust)
npm run typecheck
npm run build      # release .app
```

While `npm run dev` is running, `review-tool <pr>` opens PRs in the dev window (the app allows
a single instance). `npm run dev` sets `REVIEW_TOOL_APP=1` so the binary starts as the app even
though it's launched from a terminal.

For scripted testing, a development build started with `REVIEW_TOOL_DEBUG_EVAL=<port>` runs
JavaScript POSTed to `http://127.0.0.1:<port>/` in the window and returns the JSON result
(WebKit has no Chrome-style remote protocol). This hook isn't compiled into release builds.

Data lives in `~/Library/Application Support/review-tool/`: `repos/` has one bare clone per repo,
`worktrees/` one checkout per PR. Delete either to free space; they're re-created on demand.

### Releasing

1. Bump `version` in `src-tauri/tauri.conf.json`, `src-tauri/Cargo.toml` and `package.json`, commit, and push.
2. Tag it: `git tag -a v0.2.0 -m "Review Tool v0.2.0" && git push origin v0.2.0`
3. Point the formula at the new tag: in `Formula/review-tool.rb`, update `url` to the new tag and
   `sha256` to `curl -sL <url> | shasum -a 256`, then commit and push. Teammates get it with
   `brew upgrade review-tool`.

The tag also triggers a CI build of a universal app (unsigned unless the Apple secrets below are
set) attached to a draft GitHub Release. Push a version tag to build a universal (Apple Silicon + Intel) app and attach it to a draft
GitHub Release:

Review the draft on GitHub and publish it if you want a downloadable `.dmg` (not needed for
Homebrew installs). **Actions → Release → Run workflow** builds without
releasing (the app is attached to the run as an artifact).

Without Apple signing the app is unsigned, and macOS blocks it when downloaded. To sign and
notarize, add these repository secrets (Settings → Secrets and variables → Actions):

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
| `review-tool` command: resolves the PR with `gh`, launches or reuses the app | `src-tauri/src/cli.rs` |
| Setup check (git, gh login, language servers, agent) | `src-tauri/src/deps.rs`, `src/renderer/src/SetupCheck.tsx` |
| App setup, commands, single instance, window | `src-tauri/src/lib.rs` |
| GitHub API: review queue, PR files, threads, comments, submitting reviews | `src-tauri/src/github.rs` |
| Bare clone per repo + `git worktree` per PR, diff against the merge-base | `src-tauri/src/git.rs` |
| Language servers spawned per PR; LSP messages relayed to the page over a Tauri Channel | `src-tauri/src/lsp.rs` |
| Read-only file access for the editor | `src-tauri/src/fsaccess.rs` |
| `window.api`: the page's interface to the Rust commands | `src/renderer/src/api.ts` |
| VS Code services, grammars (incl. Starlark), disk-backed `file://` provider | `src/renderer/src/editor/` |
| Diff / file editor, cross-file navigation, back stack | `src/renderer/src/editor/EditorHost.ts` |
| Review threads in the editor, sidebar, conversation, review panel | `src/renderer/src/review/` |
| Agent panel: launch command, MCP context server, terminal | `src-tauri/src/agent.rs`, `mcp.rs`, `pty.rs`, `src/renderer/src/agent/` |
