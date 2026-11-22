# review-tool

A macOS desktop app for reviewing GitHub pull requests with real code intelligence:
it checks out the PR and runs **gopls** / **clangd** against it, so the diff view has
hover types, go-to-definition, peek and find-references — powered by VS Code's own
editor services (`@codingame/monaco-vscode-api` + `monaco-languageclient`).

## Requirements

- `gh` logged in (`gh auth status`) — the app reuses its token
- `git`
- Go PRs: `go` and `gopls` (`go install golang.org/x/tools/gopls@latest`)
- C++ PRs: `clangd` (ships with Xcode Command Line Tools; Homebrew `llvm` is preferred if installed)

## Run

```sh
npm install
npm run dev
```

## How it works

| Piece | Where |
|---|---|
| GitHub API (review queue, PR files) | `src/main/github.ts` |
| Bare clone per repo + `git worktree` per PR, diff against merge-base | `src/main/git.ts` |
| Language servers spawned per PR, LSP relayed to the renderer over a MessagePort | `src/main/lsp.ts` |
| Read-only file access for the editor (worktrees, Go stdlib/modules, system headers) | `src/main/fsaccess.ts` |
| VS Code services init + disk-backed `file://` provider | `src/renderer/src/editor/services.ts`, `diskfs.ts` |
| Diff / file editor, cross-file navigation, back stack | `src/renderer/src/editor/EditorHost.ts` |

Data lives in `~/Library/Application Support/review-tool/` (`repos/` bare clones, `worktrees/` PR checkouts).

### C++ flags

clangd uses `compile_commands.json` if found at the worktree root or in `build/`, `out/`,
`cmake-build-debug/` or `builddir/`. Otherwise it falls back to `-std=c++20` plus `-I` for the
repo root, `include/`, `src/`, `lib/`, `third_party/`, `external/`.

### Safety

PR code is untrusted. The app never runs the repo's build. gopls runs with `GOTOOLCHAIN=local`
so a PR's `go.mod` can't make it download and run a different Go toolchain. Checkouts are read-only in the editor.

## Navigation

- Cmd+click / F12 — go to definition (a changed file opens as its diff)
- Alt+F12 — peek definition; Shift+F12 — references
- Ctrl+- or the mouse back button — back

## Reviewing

- Existing review threads show inline under their lines; outdated threads are listed via the
  "Show N outdated threads" toggle above the editor.
- Hover a diff line and click the blue **+** in the gutter (or ⌘⌥C / right-click → Add Review Comment)
  to comment. Select several lines first for a multi-line comment.
- Comments and replies go into your **pending review** (visible only to you) until you click
  **Finish review** and pick Comment / Approve / Request changes.
- Only lines inside the diff's hunks can be commented on (a GitHub restriction).
