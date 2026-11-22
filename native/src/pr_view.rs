//! A PR under review (web: PRView.tsx): header, sidebar (conversation + changed
//! files), and the main column (diff / conversation, agent panel below).
use crate::backend::types::*;
use crate::theme::{self, hsla};
use crate::ui;
use gpui::prelude::FluentBuilder as _;
use gpui::*;

pub enum PrViewEvent {
    /// "← Pull requests": back to the home screen.
    Close,
}

pub struct PrView {
    pub pr: PRDetails,
    pub checkout: Checkout,
    /// Index into `pr.files` of the file shown in the main column.
    selected: Option<usize>,
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
    pub fn new(pr: PRDetails, checkout: Checkout, _window: &mut Window, _cx: &mut Context<Self>) -> Self {
        let selected = pr.files.iter().position(|f| f.status != "removed").or(if pr.files.is_empty() { None } else { Some(0) });
        Self { pr, checkout, selected }
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
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.selected = Some(i);
                        cx.notify();
                    }))
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
            .child(div().flex_1().flex().items_center().justify_center().child(ui::muted("Diff view: next checkpoint")))
    }
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
