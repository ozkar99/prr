//! Review threads, the comment composer, the conversation and the "Finish your
//! review" panel (web: review/ReviewStore.ts, ThreadZone.tsx, Conversation.tsx,
//! ReviewPanel.tsx). GitHub stays the single source of truth: every change
//! reloads the review state afterwards.
use crate::avatar;
use crate::backend::{self, github, types::*};
use crate::markdown;
use crate::theme::{self, hsla};
use crate::ui;
use gpui::prelude::FluentBuilder as _;
use gpui::*;
use gpui_component::input::{InputEvent, Textarea, TextareaState};
use gpui_component::text::{TextView, TextViewStyle};
use std::collections::HashMap;
use std::future::Future;

// ---------------------------------------------------------------- store

pub struct Review {
    pub owner: String,
    pub repo: String,
    pub number: u64,
    pub head_sha: String,
    pub state: Option<ReviewState>,
    pub loading: bool,
    pub error: Option<String>,
}

impl Review {
    pub fn new(pr: &PRDetails, cx: &mut Context<Self>) -> Self {
        let mut review = Self {
            owner: pr.owner.clone(),
            repo: pr.repo.clone(),
            number: pr.number,
            head_sha: pr.head_sha.clone(),
            state: None,
            loading: true,
            error: None,
        };
        review.load(cx);
        review
    }

    pub fn load(&mut self, cx: &mut Context<Self>) {
        self.loading = true;
        let (owner, repo, number) = (self.owner.clone(), self.repo.clone(), self.number);
        let fut = backend::task(async move { github::review_state(&owner, &repo, number).await });
        cx.spawn(async move |this, cx| {
            let result = fut.await;
            this.update(cx, |this, cx| {
                this.loading = false;
                match result {
                    Ok(state) => {
                        this.state = Some(state);
                        this.error = None;
                    }
                    Err(e) => this.error = Some(e),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn handle(&self) -> Result<ReviewHandle, String> {
        let s = self.state.as_ref().ok_or("Review state not loaded")?;
        Ok(ReviewHandle { pr_node_id: s.pr_node_id.clone(), pending_review_id: s.pending_review_id.clone() })
    }

    /// Runs a change, then reloads; resolves with the change's result.
    fn mutate<F, Fut>(&mut self, cx: &mut Context<Self>, op: F) -> Task<Result<(), String>>
    where
        F: FnOnce(ReviewHandle, String) -> Fut,
        Fut: Future<Output = Result<(), String>> + Send + 'static,
    {
        let handle = match self.handle() {
            Ok(h) => h,
            Err(e) => return Task::ready(Err(e)),
        };
        let (owner, repo) = (self.owner.clone(), self.repo.clone());
        let change = op(handle, self.head_sha.clone());
        // gh runs from this repo's clone.
        let fut = backend::task(async move { backend::gh::in_repo(&owner, &repo, change).await });
        cx.spawn(async move |this, cx| {
            let result = fut.await;
            this.update(cx, |this, cx| this.load(cx)).ok();
            // Wait for the reload so callers see the new state when they continue.
            loop {
                cx.background_executor().timer(std::time::Duration::from_millis(50)).await;
                if !this.read_with(cx, |this, _| this.loading).unwrap_or(false) {
                    break;
                }
            }
            result
        })
    }

    pub fn add_thread(&mut self, path: String, side: String, line: u64, start_line: Option<u64>, body: String, cx: &mut Context<Self>) -> Task<Result<(), String>> {
        self.mutate(cx, move |h, sha| async move { github::add_thread(h, &sha, NewThread { path, side, line, start_line, body }).await })
    }

    pub fn reply(&mut self, thread_id: String, body: String, cx: &mut Context<Self>) -> Task<Result<(), String>> {
        self.mutate(cx, move |h, sha| async move { github::reply(h, &sha, &thread_id, &body).await })
    }

    pub fn add_pr_comment(&mut self, body: String, cx: &mut Context<Self>) -> Task<Result<(), String>> {
        self.mutate(cx, move |h, _| async move { github::add_pr_comment(&h.pr_node_id, &body).await })
    }

    pub fn delete_comment(&mut self, id: String, cx: &mut Context<Self>) -> Task<Result<(), String>> {
        self.mutate(cx, move |_, _| async move { github::delete_comment(&id).await })
    }

    pub fn set_resolved(&mut self, thread_id: String, resolved: bool, cx: &mut Context<Self>) -> Task<Result<(), String>> {
        self.mutate(cx, move |_, _| async move { github::set_resolved(&thread_id, resolved).await })
    }

    pub fn submit(&mut self, event: &'static str, body: String, cx: &mut Context<Self>) -> Task<Result<(), String>> {
        self.mutate(cx, move |h, sha| async move { github::submit_review(h, &sha, event, &body).await })
    }

    pub fn pending_count(&self) -> usize {
        self.state.as_ref().map(|s| s.threads.iter().map(|t| t.comments.iter().filter(|c| c.pending).count()).sum()).unwrap_or(0)
    }

    pub fn threads_at(&self, path: &str, side: &str, line: u64) -> Vec<ReviewThread> {
        self.state
            .as_ref()
            .map(|s| s.threads.iter().filter(|t| t.path == path && t.side == side && t.line == Some(line)).cloned().collect())
            .unwrap_or_default()
    }
}

// ---------------------------------------------------------------- composer

pub enum ComposerEvent {
    Submit(String),
    Cancel,
}

/// A comment box: Markdown textarea, ⌘↩ to submit, Esc to cancel.
pub struct Composer {
    input: Entity<TextareaState>,
    submit_label: SharedString,
    cancelable: bool,
    allow_empty: bool,
    pub busy: bool,
    pub error: Option<String>,
    _sub: Subscription,
}

impl EventEmitter<ComposerEvent> for Composer {}

impl Composer {
    pub fn new(submit_label: impl Into<SharedString>, cancelable: bool, focus: bool, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let input = cx.new(|cx| {
            TextareaState::new(window, cx)
                .auto_grow(3, 16)
                .placeholder("Leave a comment (Markdown supported). ⌘↩ to submit, Esc to cancel")
        });
        if focus {
            input.update(cx, |i, cx| i.focus(window, cx));
        }
        let _sub = cx.subscribe(&input, |this: &mut Self, _, ev: &InputEvent, cx| {
            if let InputEvent::PressEnter { secondary: true, .. } = ev {
                this.submit(cx);
            } else if let InputEvent::Change = ev {
                cx.notify();
            }
        });
        Self { input, submit_label: submit_label.into(), cancelable, allow_empty: false, busy: false, error: None, _sub }
    }

    pub fn set_allow_empty(&mut self, allow: bool) {
        self.allow_empty = allow;
    }

    pub fn set_submit_label(&mut self, label: impl Into<SharedString>) {
        self.submit_label = label.into();
    }

    fn body(&self, cx: &App) -> String {
        self.input.read(cx).value().trim().to_string()
    }

    fn submit(&mut self, cx: &mut Context<Self>) {
        let body = self.body(cx);
        if self.busy || (body.is_empty() && !self.allow_empty) {
            return;
        }
        cx.emit(ComposerEvent::Submit(body));
    }

    /// Owner calls this with the outcome of a submit.
    pub fn finish(&mut self, result: Result<(), String>, window: &mut Window, cx: &mut Context<Self>) {
        self.busy = false;
        match result {
            Ok(()) => {
                self.error = None;
                self.input.update(cx, |i, cx| i.set_value("", window, cx));
            }
            Err(e) => self.error = Some(e),
        }
        cx.notify();
    }
}

impl Render for Composer {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let empty = self.body(cx).is_empty() && !self.allow_empty;
        let busy = self.busy;
        div()
            .flex()
            .flex_col()
            .gap(px(6.))
            .capture_key_down(cx.listener(|this, ev: &KeyDownEvent, _, cx| {
                if ev.keystroke.key == "escape" && this.cancelable {
                    cx.stop_propagation();
                    cx.emit(ComposerEvent::Cancel);
                }
            }))
            .child(Textarea::new(&self.input).disabled(busy))
            .children(self.error.clone().map(|e| ui::copy_error("review-error", e)))
            .child(
                div()
                    .flex()
                    .justify_end()
                    .gap(px(8.))
                    .when(self.cancelable, |el| {
                        el.child(ui::button("composer-cancel", "Cancel").on_click(cx.listener(|_, _, _, cx| cx.emit(ComposerEvent::Cancel))))
                    })
                    .child(
                        ui::primary_button("composer-submit", if busy { SharedString::from("Saving…") } else { self.submit_label.clone() })
                            .when(empty || busy, |el| el.opacity(0.5))
                            .when(!empty && !busy, |el| el.on_click(cx.listener(|this, _, _, cx| this.submit(cx)))),
                    ),
            )
    }
}

// ---------------------------------------------------------------- thread

/// One review thread: header (range, resolved, collapse), comments, reply.
pub struct ThreadView {
    review: Entity<Review>,
    pub thread_id: String,
    collapsed: bool,
    reply: Option<Entity<Composer>>,
    error: Option<String>,
    deleting: Option<String>,
    _subs: Vec<Subscription>,
}

impl ThreadView {
    pub fn new(review: Entity<Review>, thread: &ReviewThread, cx: &mut Context<Self>) -> Self {
        let sub = cx.observe(&review, |_, _, cx| cx.notify());
        Self { review, thread_id: thread.id.clone(), collapsed: thread.is_resolved, reply: None, error: None, deleting: None, _subs: vec![sub] }
    }

    fn thread(&self, cx: &App) -> Option<ReviewThread> {
        self.review.read(cx).state.as_ref()?.threads.iter().find(|t| t.id == self.thread_id).cloned()
    }

    fn open_reply(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let composer = cx.new(|cx| Composer::new("Reply", true, true, window, cx));
        let sub = cx.subscribe_in(&composer, window, |this, composer, ev: &ComposerEvent, window, cx| match ev {
            ComposerEvent::Cancel => {
                this.reply = None;
                cx.notify();
            }
            ComposerEvent::Submit(body) => {
                composer.update(cx, |c, _| c.busy = true);
                let task = this.review.update(cx, |r, cx| r.reply(this.thread_id.clone(), body.clone(), cx));
                let composer = composer.clone();
                cx.spawn_in(window, async move |this, cx| {
                    let result = task.await;
                    this.update_in(cx, |this, window, cx| {
                        if result.is_ok() {
                            this.reply = None;
                        } else {
                            composer.update(cx, |c, cx| c.finish(result, window, cx));
                        }
                        cx.notify();
                    })
                    .ok();
                })
                .detach();
            }
        });
        self._subs.push(sub);
        self.reply = Some(composer);
        cx.notify();
    }

    fn toggle_resolved(&mut self, resolved: bool, cx: &mut Context<Self>) {
        self.error = None;
        let task = self.review.update(cx, |r, cx| r.set_resolved(self.thread_id.clone(), !resolved, cx));
        cx.spawn(async move |this, cx| {
            let result = task.await;
            this.update(cx, |this, cx| {
                match result {
                    Ok(()) => this.collapsed = !resolved,
                    Err(e) => this.error = Some(e),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn delete(&mut self, id: String, cx: &mut Context<Self>) {
        self.deleting = Some(id.clone());
        let task = self.review.update(cx, |r, cx| r.delete_comment(id, cx));
        cx.spawn(async move |this, cx| {
            let result = task.await;
            this.update(cx, |this, cx| {
                this.deleting = None;
                if let Err(e) = result {
                    this.error = Some(e);
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }
}

pub fn badge(text: &str, color: u32) -> Div {
    div()
        .flex_shrink_0()
        .text_size(theme::TEXT_TINY)
        .px(px(6.))
        .border_1()
        .border_color(hsla(color))
        .text_color(hsla(color))
        .rounded(px(10.))
        .child(text.to_string())
}

/// Comment author line: avatar, name, date, extras.
pub fn comment_header(author: &str, avatar_url: &str, created_at: &str, cx: &mut App) -> Div {
    div()
        .flex()
        .items_center()
        .gap(px(8.))
        .text_size(theme::TEXT_SMALL)
        .mb(px(4.))
        .child(avatar::view(avatar_url, 18., cx))
        .child(div().font_weight(FontWeight::BOLD).child(author.to_string()))
        .child(ui::muted(ui::short_datetime(created_at)))
}

/// The web version's Markdown look: 1.5 line height, 8px paragraph gaps,
/// 18/16/14px headings, gray inline code.
pub fn markdown_style(cx: &App) -> TextViewStyle {
    use gpui_component::ActiveTheme as _;
    TextViewStyle {
        paragraph_gap: rems(0.5),
        heading_font_size: Some(std::sync::Arc::new(|level, _| px(match level {
            1 => 18.,
            2 => 16.,
            _ => 14.,
        }))),
        highlight_theme: cx.theme().highlight_theme.clone(),
        inline_code: HighlightStyle { background_color: Some(rgba(0x6e768140).into()), ..Default::default() },
        is_dark: true,
        ..Default::default()
    }
}

pub fn markdown_body(id: impl Into<ElementId>, body: &str, cx: &App) -> impl IntoElement {
    div()
        .text_size(theme::TEXT)
        .line_height(px(19.5))
        .cursor_text()
        .child(TextView::markdown(id, body.to_string()).style(markdown_style(cx)).selectable(true))
}

impl Render for ThreadView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let Some(t) = self.thread(cx) else { return div() };
        let range = if t.is_outdated || t.line.is_none() {
            "Outdated".to_string()
        } else if let (Some(s), Some(l)) = (t.start_line, t.line)
            && s != l
        {
            format!("Lines {s}–{l}")
        } else {
            format!("Line {}", t.line.unwrap_or(0))
        };
        let can_toggle = if t.is_resolved { t.viewer_can_unresolve } else { t.viewer_can_resolve };
        let first = t.comments.first();
        let summary = first.map(|c| {
            let more = if t.comments.len() > 1 { format!(" (+{})", t.comments.len() - 1) } else { String::new() };
            format!("{}: {}{more}", c.author, markdown::snippet(&c.body))
        });
        let resolved = t.is_resolved;
        let collapsed = self.collapsed;
        let header = div()
            .flex()
            .items_center()
            .gap(px(8.))
            .px(px(10.))
            .py(px(6.))
            .text_size(theme::TEXT_SMALL)
            .bg(hsla(0x202020))
            .when(!collapsed, |el| el.border_b_1().border_color(hsla(theme::BORDER)))
            .child(
                ui::link("thread-twisty", if collapsed { "▸" } else { "▾" }).on_click(cx.listener(|this, _, _, cx| {
                    this.collapsed = !this.collapsed;
                    cx.notify();
                })),
            )
            .child(div().flex_shrink_0().child(range))
            .when(resolved, |el| el.child(badge("Resolved", theme::MUTED)))
            .when(collapsed, |el| el.children(summary.map(|s| div().flex_1().min_w_0().truncate().text_color(hsla(theme::MUTED)).child(s))))
            .when(!collapsed, |el| el.child(div().flex_1()))
            .when(can_toggle, |el| {
                el.child(
                    ui::link("thread-resolve", if resolved { "Unresolve" } else { "Resolve" })
                        .on_click(cx.listener(move |this, _, _, cx| this.toggle_resolved(resolved, cx))),
                )
            });
        let mut body = div().flex().flex_col();
        if !collapsed {
            for (i, c) in t.comments.iter().enumerate() {
                let id = c.id.clone();
                let deleting = self.deleting.as_deref() == Some(c.id.as_str());
                body = body.child(
                    div()
                        .px(px(10.))
                        .py(px(8.))
                        .when(i > 0, |el| el.border_t_1().border_color(hsla(theme::BORDER)))
                        .child(
                            comment_header(&c.author, &c.author_avatar, &c.created_at, cx)
                                .when(c.pending, |el| el.child(badge("Pending", theme::WARN)))
                                .child(div().flex_1())
                                .when(c.pending && c.viewer_can_delete, |el| {
                                    el.child(
                                        ui::link_enabled(SharedString::from(format!("del-{}", c.id)), "Delete", !deleting)
                                            .when(!deleting, |l| l.on_click(cx.listener(move |this, _, _, cx| this.delete(id.clone(), cx)))),
                                    )
                                }),
                        )
                        .child(markdown_body(SharedString::from(format!("md-{}", c.id)), &c.body, cx)),
                );
            }
            body = body.children(self.error.clone().map(|e| div().px(px(10.)).child(ui::copy_error("review-panel-error", e))));
            body = body.child(match &self.reply {
                Some(composer) => div().px(px(10.)).pt(px(8.)).pb(px(10.)).child(composer.clone()).into_any_element(),
                None => div()
                    .id("reply-placeholder")
                    .mx(px(10.))
                    .mb(px(10.))
                    .mt(px(2.))
                    .px(px(8.))
                    .py(px(5.))
                    .border_1()
                    .border_color(hsla(theme::BORDER))
                    .rounded(px(4.))
                    .bg(hsla(theme::BG))
                    .text_color(hsla(theme::MUTED))
                    .cursor_text()
                    .on_click(cx.listener(|this, _, window, cx| this.open_reply(window, cx)))
                    .child("Reply…")
                    .into_any_element(),
            });
        }
        div()
            .border_1()
            .border_color(hsla(theme::BORDER))
            .rounded(px(6.))
            .bg(hsla(theme::PANEL))
            .overflow_hidden()
            .when(resolved, |el| el.opacity(0.85))
            .child(header)
            .child(body)
    }
}

// ---------------------------------------------------------------- zone

/// Everything under one diff line: its threads, plus a new-comment box if open.
pub struct ThreadZone {
    review: Entity<Review>,
    path: String,
    side: String,
    line: u64,
    threads: HashMap<String, Entity<ThreadView>>,
    composer: Option<(Option<u64>, Entity<Composer>)>,
    _subs: Vec<Subscription>,
}

pub enum ZoneEvent {
    ComposerClosed,
}

impl EventEmitter<ZoneEvent> for ThreadZone {}

impl ThreadZone {
    pub fn new(review: Entity<Review>, path: String, side: String, line: u64, cx: &mut Context<Self>) -> Self {
        let sub = cx.observe(&review, |_, _, cx| cx.notify());
        Self { review, path, side, line, threads: HashMap::new(), composer: None, _subs: vec![sub] }
    }

    pub fn has_composer(&self) -> bool {
        self.composer.is_some()
    }

    pub fn open_composer(&mut self, start_line: Option<u64>, window: &mut Window, cx: &mut Context<Self>) {
        if let Some((_, c)) = &self.composer {
            c.update(cx, |c, cx| c.input.update(cx, |i, cx| i.focus(window, cx)));
            return;
        }
        let label = if self.review.read(cx).state.as_ref().and_then(|s| s.pending_review_id.as_ref()).is_some() { "Add review comment" } else { "Start a review" };
        let composer = cx.new(|cx| Composer::new(label, true, true, window, cx));
        let sub = cx.subscribe_in(&composer, window, |this, composer, ev: &ComposerEvent, window, cx| match ev {
            ComposerEvent::Cancel => this.close_composer(cx),
            ComposerEvent::Submit(body) => {
                composer.update(cx, |c, _| c.busy = true);
                let start = this.composer.as_ref().and_then(|(s, _)| *s);
                let (path, side, line) = (this.path.clone(), this.side.clone(), this.line);
                let task = this.review.update(cx, |r, cx| r.add_thread(path, side, line, start, body.clone(), cx));
                let composer = composer.clone();
                cx.spawn_in(window, async move |this, cx| {
                    let result = task.await;
                    this.update_in(cx, |this, window, cx| {
                        if result.is_ok() {
                            this.close_composer(cx);
                        } else {
                            composer.update(cx, |c, cx| c.finish(result, window, cx));
                        }
                    })
                    .ok();
                })
                .detach();
            }
        });
        self._subs.push(sub);
        self.composer = Some((start_line, composer));
        cx.notify();
    }

    fn close_composer(&mut self, cx: &mut Context<Self>) {
        self.composer = None;
        cx.emit(ZoneEvent::ComposerClosed);
        cx.notify();
    }
}

impl Render for ThreadZone {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let threads = self.review.read(cx).threads_at(&self.path, &self.side, self.line);
        let review = self.review.clone();
        let views: Vec<Entity<ThreadView>> = threads
            .iter()
            .map(|t| self.threads.entry(t.id.clone()).or_insert_with(|| cx.new(|cx| ThreadView::new(review.clone(), t, cx))).clone())
            .collect();
        let line = self.line;
        div()
            .w_full()
            .max_w(px(900.))
            .pl(px(8.))
            .pr(px(16.))
            .py(px(4.))
            .flex()
            .flex_col()
            .gap(px(6.))
            .font_family(".SystemUIFont")
            .text_size(theme::TEXT)
            .line_height(rems(1.45))
            .text_color(hsla(theme::FG))
            .whitespace_normal()
            .children(views)
            .children(self.composer.as_ref().map(|(start, composer)| {
                let what = match start {
                    Some(s) => format!("New comment on lines {s}–{line}"),
                    None => format!("New comment on line {line}"),
                };
                div()
                    .border_1()
                    .border_color(hsla(theme::BORDER))
                    .rounded(px(6.))
                    .bg(hsla(theme::PANEL))
                    .overflow_hidden()
                    .child(
                        div()
                            .flex()
                            .gap(px(4.))
                            .px(px(10.))
                            .py(px(6.))
                            .text_size(theme::TEXT_SMALL)
                            .bg(hsla(0x202020))
                            .border_b_1()
                            .border_color(hsla(theme::BORDER))
                            .child(what)
                            .when(self.side == "LEFT", |el| el.child(ui::muted("(old version)"))),
                    )
                    .child(div().px(px(10.)).pt(px(8.)).pb(px(10.)).child(composer.clone()))
            }))
    }
}

// ---------------------------------------------------------------- conversation

/// PR description plus everything not attached to a file.
pub struct Conversation {
    review: Entity<Review>,
    pr: PRDetails,
    composer: Option<Entity<Composer>>,
    scroll: ScrollHandle,
    /// Item to scroll to and flash.
    pub target: Option<String>,
    _subs: Vec<Subscription>,
}

impl Conversation {
    pub fn new(review: Entity<Review>, pr: PRDetails, cx: &mut Context<Self>) -> Self {
        let sub = cx.observe(&review, |_, _, cx| cx.notify());
        Self { review, pr, composer: None, scroll: ScrollHandle::new(), target: None, _subs: vec![sub] }
    }

    pub fn show(&mut self, target: Option<String>, cx: &mut Context<Self>) {
        self.target = target;
        match &self.target {
            None => self.scroll.set_offset(point(px(0.), px(0.))),
            Some(id) => {
                let ix = self.review.read(cx).state.as_ref().and_then(|s| s.conversation.iter().position(|c| &c.id == id));
                // Children: title, meta, description, then the items.
                if let Some(ix) = ix {
                    self.scroll.scroll_to_item(ix + 3);
                }
            }
        }
        cx.notify();
    }

    fn ensure_composer(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.composer.is_some() {
            return;
        }
        let composer = cx.new(|cx| Composer::new("Comment", false, false, window, cx));
        let sub = cx.subscribe_in(&composer, window, |this, composer, ev: &ComposerEvent, window, cx| {
            if let ComposerEvent::Submit(body) = ev {
                composer.update(cx, |c, _| c.busy = true);
                let task = this.review.update(cx, |r, cx| r.add_pr_comment(body.clone(), cx));
                let composer = composer.clone();
                cx.spawn_in(window, async move |_, cx| {
                    let result = task.await;
                    composer.update_in(cx, |c, window, cx| c.finish(result, window, cx)).ok();
                })
                .detach();
            }
        });
        self._subs.push(sub);
        self.composer = Some(composer);
    }
}

fn verdict(state: &str) -> &'static str {
    match state {
        "APPROVED" => "approved these changes",
        "CHANGES_REQUESTED" => "requested changes",
        "DISMISSED" => "reviewed (dismissed)",
        _ => "reviewed",
    }
}

fn verdict_color(state: Option<&str>) -> u32 {
    match state {
        Some("APPROVED") => theme::ADD,
        Some("CHANGES_REQUESTED") => theme::DEL,
        _ => theme::BORDER,
    }
}

fn card() -> Div {
    div().border_1().border_color(hsla(theme::BORDER)).rounded(px(6.)).bg(hsla(theme::PANEL)).px(px(12.)).py(px(10.))
}

impl Render for Conversation {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let review = self.review.read(cx);
        let (state, loading, error) = (review.state.clone(), review.loading, review.error.clone());
        if state.is_some() {
            self.ensure_composer(window, cx);
        }
        let pr = &self.pr;
        let mut col = div()
            .w_full()
            .max_w(px(860.))
            .mx_auto()
            .pt(px(20.))
            .px(px(24.))
            .pb(px(48.))
            .flex()
            .flex_col()
            .gap(px(12.))
            .child({
                let text = format!("{} #{}", pr.title, pr.number);
                let muted = HighlightStyle { color: Some(hsla(theme::MUTED)), ..Default::default() };
                div()
                    .text_size(px(20.))
                    .line_height(px(25.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(StyledText::new(text.clone()).with_highlights([(pr.title.len() + 1..text.len(), muted)]))
            })
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(4.))
                    .text_color(hsla(theme::MUTED))
                    .child(format!("{} wants to merge into", pr.author))
                    .child(ui::code(pr.base_ref.clone())),
            )
            .child(
                card()
                    .child(
                        div()
                            .flex()
                            .gap(px(8.))
                            .text_size(theme::TEXT_SMALL)
                            .mb(px(4.))
                            .child(div().font_weight(FontWeight::BOLD).child(pr.author.clone()))
                            .child(ui::muted("description")),
                    )
                    .child(if pr.body.trim().is_empty() {
                        ui::muted("No description provided.").into_any_element()
                    } else {
                        markdown_body("pr-description", &pr.body, cx).into_any_element()
                    }),
            );
        if let Some(e) = error {
            col = col.child(ui::copy_error("conversation-error", format!("Couldn't load the conversation: {e}")));
        }
        if loading && state.is_none() {
            col = col.child(ui::muted("Loading…"));
        }
        if let Some(state) = &state {
            for item in &state.conversation {
                let flash = self.target.as_deref() == Some(item.id.as_str());
                col = col.child(
                    card()
                        .when(item.review_state.is_some(), |el| el.border_l_2().border_color(hsla(verdict_color(item.review_state.as_deref()))))
                        .when(flash, |el| el.border_color(hsla(theme::ACCENT)))
                        .child(
                            comment_header(&item.author, &item.author_avatar, &item.created_at, cx)
                                .children(item.review_state.as_deref().map(|s| div().child(verdict(s)))),
                        )
                        .when(!item.body.trim().is_empty(), |el| el.child(markdown_body(SharedString::from(format!("conv-{}", item.id)), &item.body, cx))),
                );
            }
            if let Some(composer) = &self.composer {
                col = col.child(
                    card()
                        .child(
                            div()
                                .flex()
                                .gap(px(8.))
                                .text_size(theme::TEXT_SMALL)
                                .mb(px(6.))
                                .child(div().font_weight(FontWeight::BOLD).child("Add a comment"))
                                .child(ui::muted("posts immediately, separate from your review")),
                        )
                        .child(composer.clone()),
                );
            }
        }
        div().id("conversation").size_full().overflow_y_scroll().track_scroll(&self.scroll).child(col)
    }
}

fn span_muted(text: String) -> impl IntoElement {
    div().flex_none().text_color(hsla(theme::MUTED)).child(text)
}

// ---------------------------------------------------------------- finish review

const EVENTS: [(&str, &str, &str); 3] = [
    ("COMMENT", "Comment", "Submit general feedback without explicit approval."),
    ("APPROVE", "Approve", "Submit feedback and approve merging these changes."),
    ("REQUEST_CHANGES", "Request changes", "Submit feedback that must be addressed before merging."),
];

pub enum ReviewPanelEvent {
    Close,
}

/// "Finish your review" popover: summary, event choice, submit.
pub struct ReviewPanel {
    review: Entity<Review>,
    pr_author: String,
    event: &'static str,
    done: bool,
    composer: Entity<Composer>,
    _subs: Vec<Subscription>,
}

impl EventEmitter<ReviewPanelEvent> for ReviewPanel {}

impl ReviewPanel {
    pub fn new(review: Entity<Review>, pr_author: String, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let composer = cx.new(|cx| Composer::new("Submit review", true, true, window, cx));
        let pending = review.read(cx).pending_count();
        composer.update(cx, |c, _| c.set_allow_empty(pending > 0));
        let sub = cx.subscribe_in(&composer, window, |this: &mut Self, composer, ev: &ComposerEvent, window, cx| match ev {
            ComposerEvent::Cancel => cx.emit(ReviewPanelEvent::Close),
            ComposerEvent::Submit(body) => {
                let pending = this.review.read(cx).pending_count();
                if this.event == "REQUEST_CHANGES" && body.is_empty() && pending == 0 {
                    composer.update(cx, |c, cx| c.finish(Err("Requesting changes needs a comment.".into()), window, cx));
                    return;
                }
                composer.update(cx, |c, _| c.busy = true);
                let task = this.review.update(cx, |r, cx| r.submit(this.event, body.clone(), cx));
                let composer = composer.clone();
                cx.spawn_in(window, async move |this, cx| {
                    let result = task.await;
                    this.update_in(cx, |this, window, cx| {
                        if result.is_ok() {
                            this.done = true;
                        }
                        composer.update(cx, |c, cx| c.finish(result, window, cx));
                        cx.notify();
                    })
                    .ok();
                })
                .detach();
            }
        });
        let obs = cx.observe(&review, |_, _, cx| cx.notify());
        Self { review, pr_author, event: "COMMENT", done: false, composer, _subs: vec![sub, obs] }
    }

    fn set_event(&mut self, event: &'static str, cx: &mut Context<Self>) {
        self.event = event;
        let pending = self.review.read(cx).pending_count();
        self.composer.update(cx, |c, cx| {
            c.set_allow_empty(event == "APPROVE" || pending > 0);
            cx.notify();
        });
        cx.notify();
    }
}

impl Render for ReviewPanel {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let review = self.review.read(cx);
        let pending = review.pending_count();
        let is_own = review.state.as_ref().is_some_and(|s| s.viewer == self.pr_author);
        let panel = div()
            .id("review-panel")
            .occlude()
            .w(px(440.))
            .p(px(12.))
            .flex()
            .flex_col()
            .gap(px(10.))
            .bg(hsla(theme::PANEL))
            .border_1()
            .border_color(hsla(theme::BORDER))
            .rounded(px(6.))
            .shadow_lg()
            .text_size(theme::TEXT)
            .text_color(hsla(theme::FG));
        if self.done {
            return panel
                .child(div().font_weight(FontWeight::SEMIBOLD).child("Review submitted"))
                .child(div().child(ui::button("review-close", "Close").on_click(cx.listener(|_, _, _, cx| cx.emit(ReviewPanelEvent::Close)))));
        }
        let current = self.event;
        panel
            .child(div().font_weight(FontWeight::SEMIBOLD).child("Finish your review"))
            .child(ui::muted(if pending > 0 {
                format!("{pending} pending comment{} will be published.", if pending == 1 { "" } else { "s" })
            } else {
                "No pending comments.".to_string()
            }))
            .child(self.composer.clone())
            .child(div().flex().flex_col().gap(px(6.)).text_size(theme::TEXT_SMALL).children(EVENTS.iter().map(|&(value, label, help)| {
                let disabled = is_own && value != "COMMENT";
                let checked = current == value;
                div()
                    .id(value)
                    .flex()
                    .gap(px(8.))
                    .items_start()
                    .when(disabled, |el| el.opacity(0.5))
                    .when(!disabled, |el| el.cursor_pointer().on_click(cx.listener(move |this, _, _, cx| this.set_event(value, cx))))
                    .child(
                        div()
                            .mt(px(2.))
                            .size(px(13.))
                            .flex_shrink_0()
                            .rounded_full()
                            .border_1()
                            .border_color(hsla(theme::MUTED))
                            .flex()
                            .items_center()
                            .justify_center()
                            .when(checked, |el| el.child(div().size(px(5.)).rounded_full().bg(hsla(theme::FG)))),
                    )
                    .child({
                        // "Label — help" as one wrapping paragraph, the label bold.
                        let text = format!("{label} — {help}");
                        let bold = HighlightStyle { color: Some(hsla(theme::FG)), font_weight: Some(FontWeight::BOLD), ..Default::default() };
                        div()
                            .flex_1()
                            .min_w_0()
                            .text_color(hsla(theme::MUTED))
                            .child(StyledText::new(text).with_highlights([(0..label.len(), bold)]))
                    })
            })))
    }
}
