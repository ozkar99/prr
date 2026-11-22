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
| Rust | builds the app | `curl https://sh.rustup.rs -sSf \| sh` ([rustup](https://rustup.rs)) |
| Node.js 20+ | builds the frontend; runs pyright for Python | `brew install node` |
| git | checks out PRs | Xcode Command Line Tools (`xcode-select --install`) |
| GitHub CLI, logged in | the app reuses its login, so there's no separate sign-in | `brew install gh`, then `gh auth login` |

Language servers. Syntax highlighting works without them; they add hover types and navigation.
Install only the ones for languages you review:

| Language | Files | Server | Install |
|---|---|---|---|
| Go | `.go` | gopls | `brew install go`, then `go install golang.org/x/tools/gopls@latest` |
| C/C++ | `.c` `.cc` `.cpp` `.h` `.hpp` … | clangd | included with Xcode Command Line Tools (Homebrew `llvm` is used if installed) |
| Python | `.py` `.pyi` | pyright | nothing: bundled with the app (runs on Node.js) |
| Starlark / Bazel | `BUILD` `.bazel` `.bzl` `WORKSPACE` `MODULE.bazel` | starpls | download from [starpls releases](https://github.com/withered-magic/starpls/releases) and put it on your `PATH` |

A missing server shows as **not installed** in the app's header; hover over it for the install command.

### 2. Install and build

```sh
git clone git@github.com:ozkar99/review-tool.git
cd review-tool
npm install
npm run build
```

The first build compiles the Rust dependencies and takes a few minutes. The result is
`src-tauri/target/release/bundle/macos/Review Tool.app`.

### 3. Add the `review-tool` command

```sh
npm link
```

This puts `review-tool` on your `PATH`, pointing at this checkout. It launches the app built in
step 2; after pulling new changes, run `npm run build` again.

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

## Troubleshooting

| Problem | Fix |
|---|---|
| "Couldn't load pull requests" | Check `gh auth status`; run `gh auth refresh -h github.com` if the token is invalid. |
| A server chip says **not installed** | Install it (hover the chip for the command), then reopen the PR. |
| A server chip says **failed** | Hover it to see the server's error output. |
| C++ types are wrong or headers don't resolve | clangd needs `compile_commands.json` (see below). |
| `review-tool` says the app isn't built | Run `npm run build` in the checkout. |

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

```sh
npm run dev        # app with hot reload (frontend) and auto-rebuild (Rust)
npm run typecheck
npm run build      # release .app
```

While `npm run dev` is running, `review-tool <pr>` opens PRs in the dev window (the app allows
a single instance).

For scripted testing, a development build started with `REVIEW_TOOL_DEBUG_EVAL=<port>` runs
JavaScript POSTed to `http://127.0.0.1:<port>/` in the window and returns the JSON result
(WebKit has no Chrome-style remote protocol). This hook isn't compiled into release builds.

Data lives in `~/Library/Application Support/review-tool/`: `repos/` has one bare clone per repo,
`worktrees/` one checkout per PR. Delete either to free space; they're re-created on demand.

### How it works

| Piece | Where |
|---|---|
| `review-tool` command: resolves the PR with `gh`, launches or reuses the app | `bin/review-tool.mjs` |
| App setup, commands, single instance, window | `src-tauri/src/lib.rs` |
| GitHub API: review queue, PR files, threads, comments, submitting reviews | `src-tauri/src/github.rs` |
| Bare clone per repo + `git worktree` per PR, diff against the merge-base | `src-tauri/src/git.rs` |
| Language servers spawned per PR; LSP messages relayed to the page over a Tauri Channel | `src-tauri/src/lsp.rs` |
| Read-only file access for the editor | `src-tauri/src/fsaccess.rs` |
| `window.api`: the page's interface to the Rust commands | `src/renderer/src/api.ts` |
| VS Code services, grammars (incl. Starlark), disk-backed `file://` provider | `src/renderer/src/editor/` |
| Diff / file editor, cross-file navigation, back stack | `src/renderer/src/editor/EditorHost.ts` |
| Review threads in the editor, sidebar, conversation, review panel | `src/renderer/src/review/` |
