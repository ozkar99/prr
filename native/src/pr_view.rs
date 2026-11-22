//! A PR under review (web: PRView.tsx): header, sidebar (conversation + changed
//! files), and the main column (diff / conversation, agent panel below).
use crate::backend::{self, git, types::*};
use crate::highlight;
use crate::theme::{self, hsla};
use crate::ui;
use gpui::prelude::FluentBuilder as _;
use gpui::*;
use crate::diffview::{Diff, DiffChangeIndicator, DiffFile, DiffHunkSeparator, DiffLinePosition, DiffMode, DiffSide, DiffState, TokenHit};
use crate::lsp::{self, LspClient};
use gpui_component::text::TextView;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

actions!(review_tool, [GoBack, GoToDefinition]);

pub fn init(cx: &mut App) {
    cx.bind_keys([KeyBinding::new("ctrl--", GoBack, None), KeyBinding::new("f12", GoToDefinition, None)]);
}

pub enum PrViewEvent {
    /// "← Pull requests": back to the home screen.
    Close,
}

/// What the main column is showing: a changed file's diff, or a plain file
/// (opened by go-to-definition) by absolute path.
#[derive(Clone, PartialEq)]
enum Shown {
    File(usize),
    Plain(String),
}

/// A place to come back to with Ctrl+-.
#[derive(Clone)]
struct NavEntry {
    shown: Shown,
    line: Option<(DiffSide, usize)>,
}

enum LspState {
    Starting,
    Running(Arc<LspClient>),
    Failed(String),
}

struct Hover {
    point: Point<Pixels>,
    markdown: SharedString,
}

/// What the main column shows for the selected file.
enum FileView {
    Loading,
    Diff(Entity<DiffState>),
    /// No textual change to show (binary file, mode change, …), or an error.
    Message(String),
}

pub struct PrView {
    pub pr: PRDetails,
    pub checkout: Checkout,
    /// What the main column shows (a changed file's index, or a plain file).
    shown: Option<Shown>,
    file_view: FileView,
    /// Ignores results of loads that were superseded by a later selection.
    load_seq: usize,
    back_stack: Vec<NavEntry>,
    /// The line last scrolled to, recorded for the back stack.
    current_line: Option<(DiffSide, usize)>,
    lsp: HashMap<&'static str, LspState>,
    hover: Option<Hover>,
    /// Identifies the latest hover request; older ones are dropped.
    hover_seq: usize,
    hover_over_popup: bool,
    /// The symbol last clicked, for F12.
    last_token: Option<TokenHit>,
    /// Source lines of files used for LSP column conversion, by absolute path.
    file_lines: HashMap<String, Arc<Vec<String>>>,
    focus: FocusHandle,
}

impl EventEmitter<PrViewEvent> for PrView {}

fn status_letter(status: &str) -> (&'static str, u32) {
    match status {
        "added" => ("A", theme::ADD),
        "removed" => ("D", theme::DEL),
        "renamed" => ("R", theme::LINK),
        "copied" => ("C", theme::LINK),
        "modified" | "changed" => ("M", theme::WARN),
        _ => ("?", theme::MUTED),
    }
}

impl PrView {
    pub fn new(pr: PRDetails, checkout: Checkout, _window: &mut Window, cx: &mut Context<Self>) -> Self {
        let first = pr.files.iter().position(|f| f.status != "removed").or(if pr.files.is_empty() { None } else { Some(0) });
        let mut view = Self {
            pr,
            checkout,
            shown: None,
            file_view: FileView::Message(String::new()),
            load_seq: 0,
            back_stack: Vec::new(),
            current_line: None,
            lsp: HashMap::new(),
            hover: None,
            hover_seq: 0,
            hover_over_popup: false,
            last_token: None,
            file_lines: HashMap::new(),
            focus: cx.focus_handle(),
        };
        view.start_language_servers(cx);
        if let Some(ix) = first {
            view.show(Shown::File(ix), None, false, cx);
        }
        view
    }

    /// Index of the changed file shown, if a diff is shown (for the sidebar).
    fn selected(&self) -> Option<usize> {
        match &self.shown {
            Some(Shown::File(ix)) => Some(*ix),
            _ => None,
        }
    }

    pub fn select_file(&mut self, ix: usize, cx: &mut Context<Self>) {
        self.show(Shown::File(ix), None, true, cx);
    }

    /// Shows a changed file (whole-file side-by-side diff) or a plain file, at
    /// `line` or, for diffs, at the first change. `remember` pushes the current
    /// place onto the back stack.
    fn show(&mut self, shown: Shown, line: Option<(DiffSide, usize)>, remember: bool, cx: &mut Context<Self>) {
        if remember && let Some(current) = self.shown.clone() {
            self.back_stack.push(NavEntry { shown: current, line: self.current_line });
            if self.back_stack.len() > 100 {
                self.back_stack.remove(0);
            }
        }
        self.hover = None;
        // Same file: just scroll.
        if self.shown.as_ref() == Some(&shown) && let (Some((side, n)), FileView::Diff(state)) = (line, &self.file_view) {
            let path = self.diff_path(&shown);
            state.update(cx, |s, cx| s.scroll_to_line(DiffLinePosition::new(path, side, n), cx));
            self.current_line = line;
            cx.notify();
            return;
        }
        self.shown = Some(shown.clone());
        self.current_line = line;
        self.file_view = FileView::Loading;
        self.load_seq += 1;
        let seq = self.load_seq;
        cx.notify();

        let (root, base, head) = (self.checkout.root.clone(), self.checkout.merge_base.clone(), self.checkout.head_sha.clone());
        let (patch_path, fut): (String, std::pin::Pin<Box<dyn std::future::Future<Output = Result<String, String>> + Send>>) = match &shown {
            Shown::File(ix) => {
                let Some(file) = self.pr.files.get(*ix).cloned() else { return };
                let (path, previous) = (file.path.clone(), file.previous_path.clone());
                (file.path.clone(), Box::pin(backend::task(async move { git::file_patch(&root, &base, &head, &path, previous.as_deref()).await })))
            }
            Shown::Plain(abs) => {
                let abs = abs.clone();
                (abs.clone(), Box::pin(backend::task(async move { plain_patch(&abs).await })))
            }
        };
        let is_plain = matches!(shown, Shown::Plain(_));
        cx.spawn(async move |this, cx| {
            let patch = fut.await;
            this.update(cx, |this, cx| {
                if this.load_seq != seq {
                    return;
                }
                let first = if is_plain { None } else { patch.as_ref().ok().and_then(|p| first_change(p, &patch_path)) };
                let target = line.map(|(side, n)| DiffLinePosition::new(patch_path.clone(), side, n)).or(first);
                this.file_view = match patch.and_then(|p| DiffFile::parse(&p).map_err(|e| e.message().to_string())) {
                    Ok(files) if !files.is_empty() => {
                        let language = highlight::language_for(&patch_path);
                        let files: Vec<DiffFile> = files.into_iter().map(|f| f.with_language(language)).collect();
                        let state = cx.new(|cx| {
                            let mut state = DiffState::new(files, cx)
                                .with_mode(if is_plain { DiffMode::Unified } else { DiffMode::Split })
                                .with_context_lines(None)
                                .with_syntax_highlighter(highlight::highlight);
                            if let Some(pos) = target {
                                state.scroll_to_line(pos, cx);
                            }
                            state
                        });
                        FileView::Diff(state)
                    }
                    Ok(_) => FileView::Message("No textual changes to show for this file.".into()),
                    Err(e) => FileView::Message(format!("Couldn't load the file: {e}")),
                };
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// The path the diff uses for positions: repo-relative for changed files, absolute for plain ones.
    fn diff_path(&self, shown: &Shown) -> String {
        match shown {
            Shown::File(ix) => self.pr.files.get(*ix).map(|f| f.path.clone()).unwrap_or_default(),
            Shown::Plain(abs) => abs.clone(),
        }
    }

    fn go_back(&mut self, _: &GoBack, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(entry) = self.back_stack.pop() {
            self.show(entry.shown, entry.line, false, cx);
        }
    }

    /// Path relative to the checkout, if inside it.
    fn relative(&self, abs: &str) -> Option<String> {
        abs.strip_prefix(&format!("{}/", self.checkout.root)).map(String::from)
    }

    // ---- Language servers ----

    fn start_language_servers(&mut self, cx: &mut Context<Self>) {
        let mut langs: Vec<&'static str> = self
            .pr
            .files
            .iter()
            .filter_map(|f| lsp::server_for_language(highlight::language_for(&f.path)))
            .collect();
        langs.sort();
        langs.dedup();
        for lang in langs {
            self.lsp.insert(lang, LspState::Starting);
            let root = self.checkout.root.clone();
            let fut = backend::task(LspClient::start(lang, root));
            cx.spawn(async move |this, cx| {
                let result = fut.await;
                this.update(cx, |this, cx| {
                    this.lsp.insert(lang, match result {
                        Ok(client) => LspState::Running(client),
                        Err(e) => LspState::Failed(e),
                    });
                    cx.notify();
                })
                .ok();
            })
            .detach();
        }
    }

    pub fn stop_language_servers(&mut self) {
        for (_, state) in self.lsp.drain() {
            if let LspState::Running(client) = state {
                let _ = backend::task(async move { client.stop().await });
            }
        }
    }

    fn lines_of(&mut self, abs: &str) -> Option<Arc<Vec<String>>> {
        if let Some(lines) = self.file_lines.get(abs) {
            return Some(lines.clone());
        }
        let text = std::fs::read_to_string(abs).ok()?;
        let lines = Arc::new(text.lines().map(String::from).collect::<Vec<_>>());
        self.file_lines.insert(abs.to_string(), lines.clone());
        Some(lines)
    }

    /// Where a hit is for the language server: absolute path, language, 0-based
    /// line, UTF-16 column, and the client. The old side of a PR diff isn't on
    /// disk, so it has no language features (as in the web version).
    fn resolve(&mut self, hit: &TokenHit) -> Option<(String, &'static str, u32, u32, Arc<LspClient>)> {
        let shown = self.shown.clone()?;
        if matches!(shown, Shown::File(_)) && hit.position.side() == DiffSide::Original {
            return None;
        }
        let path = hit.position.path().to_string();
        let abs = if path.starts_with('/') { path.clone() } else { format!("{}/{}", self.checkout.root, path) };
        let language = highlight::language_for(&abs);
        let server = lsp::server_for_language(language)?;
        let client = match self.lsp.get(server)? {
            LspState::Running(c) => c.clone(),
            _ => return None,
        };
        let lines = self.lines_of(&abs)?;
        let line_ix = hit.position.line().checked_sub(1)?;
        let text = lines.get(line_ix)?;
        Some((abs, language, line_ix as u32, lsp::utf16_column(text, hit.column), client))
    }

    fn on_token_hover(&mut self, hit: Option<TokenHit>, cx: &mut Context<Self>) {
        self.hover_seq += 1;
        let seq = self.hover_seq;
        let Some(hit) = hit else {
            // Leave the popup up briefly so the pointer can move into it.
            cx.spawn(async move |this, cx| {
                cx.background_executor().timer(Duration::from_millis(300)).await;
                this.update(cx, |this, cx| {
                    if this.hover_seq == seq && !this.hover_over_popup {
                        this.hover = None;
                        cx.notify();
                    }
                })
                .ok();
            })
            .detach();
            return;
        };
        let Some((abs, language, line, character, client)) = self.resolve(&hit) else { return };
        let point = hit.point;
        cx.spawn(async move |this, cx| {
            // VS Code's hover delay.
            cx.background_executor().timer(Duration::from_millis(250)).await;
            if this.read_with(cx, |this, _| this.hover_seq != seq).unwrap_or(true) {
                return;
            }
            let result = backend::task(async move { client.hover(&abs, language, line, character).await }).await;
            this.update(cx, |this, cx| {
                if this.hover_seq != seq {
                    return;
                }
                this.hover = match result {
                    Ok(Some(md)) => Some(Hover { point, markdown: md.into() }),
                    _ => None,
                };
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn go_to_definition(&mut self, hit: TokenHit, cx: &mut Context<Self>) {
        let Some((abs, language, line, character, client)) = self.resolve(&hit) else { return };
        self.hover = None;
        let fut = backend::task(async move { client.definition(&abs, language, line, character).await });
        cx.spawn(async move |this, cx| {
            let Ok(locations) = fut.await else { return };
            let Some(loc) = locations.into_iter().next() else { return };
            this.update(cx, |this, cx| this.open_location(&loc.path, loc.line as usize + 1, cx)).ok();
        })
        .detach();
    }

    /// Opens a file position: a changed file opens as its diff, anything else as a plain file.
    fn open_location(&mut self, abs: &str, line: usize, cx: &mut Context<Self>) {
        let changed = self
            .relative(abs)
            .and_then(|rel| self.pr.files.iter().position(|f| f.path == rel && f.status != "removed"));
        match changed {
            Some(ix) => self.show(Shown::File(ix), Some((DiffSide::Modified, line)), true, cx),
            None => self.show(Shown::Plain(abs.to_string()), Some((DiffSide::Modified, line)), true, cx),
        }
    }

    fn on_go_to_definition(&mut self, _: &GoToDefinition, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(hit) = self.last_token.clone() {
            self.go_to_definition(hit, cx);
        }
    }

    fn render_header(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let pr = &self.pr;
        div()
            .h(theme::TITLEBAR)
            .flex_shrink_0()
            .flex()
            .items_center()
            .gap(px(12.))
            .pl(px(84.)) // traffic lights
            .pr(px(12.))
            .bg(hsla(theme::PANEL))
            .border_b_1()
            .border_color(hsla(theme::BORDER))
            .child(ui::link("back", "← Pull requests").on_click(cx.listener(|_, _, _, cx| cx.emit(PrViewEvent::Close))))
            .child(
                div()
                    .flex()
                    .gap(px(4.))
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .font_weight(FontWeight::MEDIUM)
                    .child(ui::muted(format!("{}/{}#{}", pr.owner, pr.repo, pr.number)))
                    .child(div().truncate().child(pr.title.clone())),
            )
            .child(ui::spacer())
            .when(self.checkout.local.is_some(), |el| el.child(ui::chip("your clone", theme::FG)))
            .children(lsp::LANGUAGES.iter().filter_map(|lang| {
                let (text, color, tip) = match self.lsp.get(lang)? {
                    LspState::Starting => ("starting".to_string(), theme::WARN, None),
                    LspState::Running(_) => ("running".to_string(), theme::ADD, None),
                    LspState::Failed(e) if e.contains("not installed") => ("not installed".to_string(), theme::DEL, Some(e.clone())),
                    LspState::Failed(e) => ("failed".to_string(), theme::DEL, Some(e.clone())),
                };
                let chip = ui::chip(format!("{}: {text}", lsp::server_name(lang)), color);
                Some(match tip {
                    Some(tip) => div().id(SharedString::from(format!("lsp-{lang}"))).child(chip).tooltip(move |window, cx| gpui_component::tooltip::Tooltip::new(tip.clone()).build(window, cx)).into_any_element(),
                    None => chip.into_any_element(),
                })
            }))
            .child({
                let url = pr.url.clone();
                ui::link("open-github", "Open on GitHub").on_click(move |_, _, cx| cx.open_url(&url))
            })
            .child(ui::button("agent", "✦ Claude Code"))
            .child(ui::primary_button("review", "Review"))
    }

    fn render_sidebar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let pr = &self.pr;
        let snippet = crate::markdown::snippet(&pr.body);
        div()
            .id("sidebar")
            .w(theme::SIDEBAR_WIDTH)
            .flex_shrink_0()
            .h_full()
            .overflow_y_scroll()
            .bg(hsla(theme::PANEL))
            .border_r_1()
            .border_color(hsla(theme::BORDER))
            .child(div().px(px(12.)).pt(px(10.)).pb(px(6.)).child(ui::section_title("Conversation")))
            .child(
                div()
                    .id("conv-description")
                    .flex()
                    .items_baseline()
                    .gap(px(6.))
                    .px(px(12.))
                    .py(px(3.))
                    .hover(|s| s.bg(hsla(theme::HOVER)))
                    .cursor_pointer()
                    .child(div().w(px(16.)).text_color(hsla(theme::MUTED)).text_size(theme::TEXT_TINY).child("≡"))
                    .child("Description")
                    .child(div().flex_1().truncate().text_color(hsla(theme::MUTED)).child(if snippet.is_empty() { "No description".to_string() } else { snippet })),
            )
            .child(div().h(px(6.)).border_b_1().border_color(hsla(theme::BORDER)))
            .child(
                div()
                    .px(px(12.))
                    .pt(px(10.))
                    .pb(px(6.))
                    .flex()
                    .gap(px(4.))
                    .child(ui::section_title("Changed files"))
                    .child(div().text_size(theme::TEXT_TINY).text_color(hsla(theme::MUTED)).child(format!("({})", pr.files.len()))),
            )
            .child(div().flex().flex_col().pb(px(12.)).children(pr.files.iter().enumerate().map(|(i, f)| {
                let (letter, color) = status_letter(&f.status);
                let (dir, name) = match f.path.rfind('/') {
                    Some(slash) => (f.path[..=slash].to_string(), f.path[slash + 1..].to_string()),
                    None => (String::new(), f.path.clone()),
                };
                let active = self.selected() == Some(i);
                div()
                    .id(SharedString::from(format!("file-{i}")))
                    .flex()
                    .items_baseline()
                    .gap(px(6.))
                    .px(px(12.))
                    .py(px(3.))
                    .whitespace_nowrap()
                    .cursor_pointer()
                    .when(active, |el| el.bg(hsla(theme::ACTIVE)))
                    .when(!active, |el| el.hover(|s| s.bg(hsla(theme::HOVER))))
                    .on_click(cx.listener(move |this, _, _, cx| this.select_file(i, cx)))
                    .child(div().w(px(10.)).flex_shrink_0())
                    .child(div().w(px(12.)).flex_shrink_0().text_size(theme::TEXT_TINY).font_weight(FontWeight::SEMIBOLD).text_color(hsla(color)).child(letter))
                    .child(div().flex_shrink_0().child(name))
                    .child(div().flex_1().min_w_0().truncate().text_size(theme::TEXT_TINY).text_color(hsla(theme::MUTED)).child(dir))
                    .child(
                        div()
                            .flex_shrink_0()
                            .flex()
                            .gap(px(4.))
                            .text_size(theme::TEXT_TINY)
                            .child(div().text_color(hsla(theme::ADD)).child(format!("+{}", f.additions)))
                            .child(div().text_color(hsla(theme::DEL)).child(format!("−{}", f.deletions))),
                    )
            })))
    }

    fn render_main(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let (path, plain) = match &self.shown {
            Some(Shown::File(ix)) => (self.pr.files.get(*ix).map(|f| f.path.clone()).unwrap_or_default(), false),
            Some(Shown::Plain(abs)) => (self.relative(abs).unwrap_or_else(|| abs.clone()), true),
            None => (String::new(), false),
        };
        let can_go_back = !self.back_stack.is_empty();
        let weak = cx.entity().downgrade();
        div()
            .flex_1()
            .min_w_0()
            .h_full()
            .flex()
            .flex_col()
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(10.))
                    .px(px(12.))
                    .py(px(4.))
                    .min_h(px(28.))
                    .border_b_1()
                    .border_color(hsla(theme::BORDER))
                    .child(
                        div()
                            .id("nav-back")
                            .text_color(hsla(if can_go_back { theme::LINK } else { theme::MUTED }))
                            .when(can_go_back, |el| {
                                el.cursor_pointer().on_click(cx.listener(|this, _, window, cx| this.go_back(&GoBack, window, cx)))
                            })
                            .tooltip(|window, cx| gpui_component::tooltip::Tooltip::new("Go back (Ctrl+-)").build(window, cx))
                            .child("←"),
                    )
                    .child(div().font_family(theme::MONO_FONT).text_size(theme::TEXT_SMALL).truncate().child(path))
                    .when(plain, |el| el.child(ui::chip("not changed in this PR", theme::MUTED))),
            )
            .child(match &self.file_view {
                FileView::Diff(state) => {
                    let (hover_view, click_view) = (weak.clone(), weak.clone());
                    div().flex_1().min_h_0().child(
                        Diff::new(state)
                            .size_full()
                            .line_number(true)
                            .syntax_highlight(true)
                            .header_visible(false)
                            .hunk_separator(DiffHunkSeparator::Simple)
                            .change_indicator(DiffChangeIndicator::Signs)
                            .on_token_hover(move |hit, _, cx| {
                                let hit = hit.cloned();
                                hover_view.update(cx, |this, cx| this.on_token_hover(hit, cx)).ok();
                            })
                            .on_token_click(move |hit, event, _, cx| {
                                let (hit, cmd) = (hit.clone(), event.modifiers().platform);
                                click_view
                                    .update(cx, |this, cx| {
                                        this.last_token = Some(hit.clone());
                                        if cmd {
                                            this.go_to_definition(hit, cx);
                                        }
                                    })
                                    .ok();
                            }),
                    )
                }
                FileView::Loading => div().flex_1(),
                FileView::Message(m) => div().flex_1().flex().items_center().justify_center().child(ui::muted(m.clone())),
            })
    }

    /// VS Code-style hover widget with the server's Markdown.
    fn render_hover(&self, cx: &mut Context<Self>) -> Option<impl IntoElement> {
        let hover = self.hover.as_ref()?;
        let at = point(hover.point.x, hover.point.y + px(10.));
        Some(deferred(
            anchored().position(at).snap_to_window_with_margin(px(8.)).child(
                div()
                    .id("lsp-hover")
                    .occlude()
                    .max_w(px(600.))
                    .max_h(px(360.))
                    .overflow_y_scroll()
                    .p(px(8.))
                    .bg(hsla(0x202020))
                    .border_1()
                    .border_color(hsla(0x454545))
                    .rounded(px(3.))
                    .shadow_md()
                    .text_size(theme::TEXT)
                    .on_hover(cx.listener(|this, hovered: &bool, _, cx| {
                        this.hover_over_popup = *hovered;
                        if !*hovered {
                            this.hover = None;
                            cx.notify();
                        }
                    }))
                    .child(TextView::markdown("lsp-hover-md", hover.markdown.clone()).selectable(true)),
            ),
        ))
    }
}

/// Where the first change in a file's patch is, for scrolling there on open.
fn first_change(patch: &str, path: &str) -> Option<DiffLinePosition> {
    let (mut old_line, mut new_line, mut in_hunk) = (0usize, 0usize, false);
    for line in patch.lines() {
        if let Some(header) = line.strip_prefix("@@ -") {
            // @@ -a[,b] +c[,d] @@
            let mut parts = header.split(' ');
            old_line = parts.next()?.split(',').next()?.parse().ok()?;
            new_line = parts.next()?.trim_start_matches('+').split(',').next()?.parse().ok()?;
            in_hunk = true;
            continue;
        }
        if !in_hunk {
            continue;
        }
        match line.as_bytes().first() {
            Some(b'+') => return Some(DiffLinePosition::new(path.to_string(), DiffSide::Modified, new_line)),
            Some(b'-') => return Some(DiffLinePosition::new(path.to_string(), DiffSide::Original, old_line)),
            Some(b' ') => {
                old_line += 1;
                new_line += 1;
            }
            _ => {}
        }
    }
    None
}

impl Render for PrView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(hsla(theme::BG))
            .text_color(hsla(theme::FG))
            .text_size(theme::TEXT)
            .key_context("PrView")
            .track_focus(&self.focus)
            .on_action(cx.listener(Self::go_back))
            .on_action(cx.listener(Self::on_go_to_definition))
            .child(self.render_header(cx))
            .child(div().flex_1().min_h_0().flex().child(self.render_sidebar(cx)).child(self.render_main(cx)))
            .children(self.render_hover(cx))
    }
}

/// A plain file as a patch with no changes, so the diff view shows it as one
/// unchanged column (go-to-definition into files the PR didn't touch).
async fn plain_patch(abs: &str) -> Result<String, String> {
    let text = tokio::fs::read_to_string(abs).await.map_err(|e| format!("{abs}: {e}"))?;
    let lines: Vec<&str> = text.lines().collect();
    if lines.is_empty() {
        return Ok(String::new());
    }
    let n = lines.len();
    // No `diff --git` header and no a/ b/ prefixes: the parser keeps the absolute path as is.
    let mut out = format!("--- {abs}\n+++ {abs}\n@@ -1,{n} +1,{n} @@\n");
    for line in lines {
        out.push(' ');
        out.push_str(line);
        out.push('\n');
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    #[tokio::test]
    async fn plain_patch_keeps_absolute_path() {
        let abs = concat!(env!("CARGO_MANIFEST_DIR"), "/Cargo.toml");
        let patch = super::plain_patch(abs).await.unwrap();
        let files = crate::diffview::DiffFile::parse(&patch).unwrap();
        assert_eq!(files[0].path().as_ref(), abs);
    }
}
