# Native Rust rewrite (branch `rust-native`)

Goal: the same app as `main` (Tauri + web UI) rebuilt with **no web tech**: one Rust
binary, native GPU-rendered UI, functionally and visually as close to the current app as
possible. `main` keeps shipping meanwhile; this branch is checkpointed as it goes.

## Stack (decided)

| Piece | Choice | Notes |
|---|---|---|
| UI framework | `gpui` (Zed's), as **`gpui-pre` =0.3.12** | gpui-component pins this exact gpui snapshot. |
| Platform layer | `gpui-pre-platform` =0.3.12 with `runtime_shaders` | Compiles Metal shaders at launch, so builds don't need Xcode's `metal` compiler (this Mac only has Command Line Tools). Start with `gpui_platform::application().run(...)`. |
| Components | `gpui-component` =0.10.0 | Inputs (single/multi-line), buttons, lists, popovers, context menus, tooltips, resizable panels, markdown view. |
| Syntax highlighting | tree-sitter grammars, colors mapped to VS Code **Dark Modern** | |
| Diff | `similar` (line diff + inline word diff) | Side-by-side with aligned fillers, like Monaco's diff editor. |
| LSP | own client over stdio (`lsp-types`), reusing the framing from src-tauri/src/lsp.rs | hover, definition, references/peek, semantic tokens later. |
| Markdown | gpui-component's markdown view (or pulldown-cmark → gpui elements) | comments, PR description, hovers. |
| Terminal (agent panel) | `alacritty_terminal` for VT state + pty, own gpui renderer | Zed's terminal_view is GPL; don't copy it. |
| Async | tokio runtime on background threads for backend (reqwest, processes); results handed to gpui via `cx.spawn` awaiting tokio JoinHandles | |

## Reused from src-tauri (copy into native/src/backend, drop Tauri types)

exec.rs, types.rs, github.rs, git.rs, cleanup.rs, deps.rs, cli.rs, agent.rs, mcp.rs.
Replaced: lsp.rs relay (becomes an in-process client), pty.rs (alacritty_terminal),
fsaccess.rs (no web sandbox; reads are in-process), devtools.rs (test hook: rethink),
single-instance plugin (own unix-socket handoff).

## Checkpoints

- [x] **CP0** spike: gpui + gpui-component window renders on macOS without Xcode.
- [x] **CP1** backend port + app shell: theme tokens (from styles.css), Home (setup check,
      open form, PR queue, storage), open PR → PR view shell (header, sidebar file list).
- [ ] **CP2** diff view: highlighting, side-by-side alignment + hatched fillers, gutters,
      change colors + inline word diff, scrolling, reveal first change, plain file view.
- [ ] **CP3** LSP: hover popup (markdown), go-to-definition (⌘-click/F12) + back stack,
      peek, selection tracking.
- [ ] **CP4** reviews: inline threads, composer, gutter "+", reply/resolve/delete,
      outdated threads, sidebar thread list, conversation view, finish-review panel.
- [ ] **CP5** agent panel: terminal, MCP server + agent launch, ⌘L/⌘⇧L + context menu.
- [ ] **CP6** CLI + single instance, local clone flow + switch dialog, cleanup, packaging
      (.app, cask, release workflow), remove web/Node pieces, README.

## Visual reference

Colors and spacing come from `src/renderer/src/styles.css` on `main` (CSS variables at the
top) and VS Code's Dark Modern theme for code. Compare screenshots of both apps per screen.
