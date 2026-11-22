//! A PR under review (web: PRView.tsx): header, sidebar (conversation + changed
//! files), and the main column (diff / conversation, agent panel below).
use crate::backend::{self, git, types::*};
use crate::highlight;
use crate::theme::{self, hsla};
use crate::ui;
use gpui::prelude::FluentBuilder as _;
use gpui::*;
use gpui_component::diff::{Diff, DiffChangeIndicator, DiffFile, DiffHunkSeparator, DiffLinePosition, DiffMode, DiffSide, DiffState};

pub enum PrViewEvent {
    /// "← Pull requests": back to the home screen.
    Close,
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
    /// Index into `pr.files` of the file shown in the main column.
    selected: Option<usize>,
    file_view: FileView,
    /// Ignores results of loads that were superseded by a later selection.
    load_seq: usize,
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
        let mut view = Self { pr, checkout, selected: None, file_view: FileView::Message(String::new()), load_seq: 0 };
        if let Some(ix) = first {
            view.select_file(ix, cx);
        }
        view
    }

    /// Shows a changed file as a side-by-side diff of the whole file, scrolled to its first change.
    pub fn select_file(&mut self, ix: usize, cx: &mut Context<Self>) {
        let Some(file) = self.pr.files.get(ix).cloned() else { return };
        self.selected = Some(ix);
        self.file_view = FileView::Loading;
        self.load_seq += 1;
        let seq = self.load_seq;
        cx.notify();

        let (root, base, head) = (self.checkout.root.clone(), self.checkout.merge_base.clone(), self.checkout.head_sha.clone());
        let (path, previous) = (file.path.clone(), file.previous_path.clone());
        let fut = backend::task(async move { git::file_patch(&root, &base, &head, &path, previous.as_deref()).await });
        cx.spawn(async move |this, cx| {
            let patch = fut.await;
            this.update(cx, |this, cx| {
                if this.load_seq != seq {
                    return;
                }
                let first = patch.as_ref().ok().and_then(|p| first_change(p, &file.path));
                this.file_view = match patch.and_then(|p| DiffFile::parse(&p).map_err(|e| e.message().to_string())) {
                    Ok(files) if !files.is_empty() => {
                        let language = highlight::language_for(&file.path);
                        let files: Vec<DiffFile> = files.into_iter().map(|f| f.with_language(language)).collect();
                        let state = cx.new(|cx| {
                            let mut state = DiffState::new(files, cx)
                                .with_mode(DiffMode::Split)
                                .with_context_lines(None)
                                .with_syntax_highlighter(highlight::highlight);
                            if let Some(pos) = first {
                                state.scroll_to_line(pos, cx);
                            }
                            state
                        });
                        FileView::Diff(state)
                    }
                    Ok(_) => FileView::Message("No textual changes to show for this file.".into()),
                    Err(e) => FileView::Message(format!("Couldn't load the diff: {e}")),
                };
                cx.notify();
            })
            .ok();
        })
        .detach();
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
                let active = self.selected == Some(i);
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

    fn render_main(&self) -> impl IntoElement {
        let path = self.selected.and_then(|i| self.pr.files.get(i)).map(|f| f.path.clone()).unwrap_or_default();
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
                    .child(div().text_color(hsla(theme::LINK)).child("←"))
                    .child(div().font_family(theme::MONO_FONT).text_size(theme::TEXT_SMALL).truncate().child(path)),
            )
            .child(match &self.file_view {
                FileView::Diff(state) => div().flex_1().min_h_0().child(
                    Diff::new(state)
                        .size_full()
                        .line_number(true)
                        .syntax_highlight(true)
                        .header_visible(false)
                        .hunk_separator(DiffHunkSeparator::Simple)
                        .change_indicator(DiffChangeIndicator::Signs),
                ),
                FileView::Loading => div().flex_1(),
                FileView::Message(m) => div().flex_1().flex().items_center().justify_center().child(ui::muted(m.clone())),
            })
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
            .child(self.render_header(cx))
            .child(div().flex_1().min_h_0().flex().child(self.render_sidebar(cx)).child(self.render_main()))
    }
}
