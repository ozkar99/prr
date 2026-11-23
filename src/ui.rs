//! Small shared building blocks, mirroring the web version's CSS classes
//! (`.link`, `.button`, `.chip`, `.muted`, section titles).
use crate::theme::{self, hsla};
use gpui::*;

/// `button.link`: blue text, no chrome.
pub fn link(id: impl Into<ElementId>, label: impl Into<SharedString>) -> Stateful<Div> {
    div()
        .id(id)
        .text_color(hsla(theme::LINK))
        .cursor_pointer()
        .whitespace_nowrap()
        .child(label.into())
}

/// A `.link` that may be disabled (dimmed, no pointer).
pub fn link_enabled(id: impl Into<ElementId>, label: impl Into<SharedString>, enabled: bool) -> Stateful<Div> {
    let el = link(id, label);
    if enabled { el } else { el.opacity(0.5).cursor_default() }
}

/// `.button`: bordered, dark, rounded.
pub fn button(id: impl Into<ElementId>, label: impl Into<SharedString>) -> Stateful<Div> {
    div()
        .id(id)
        .px(px(12.))
        .py(px(4.))
        .rounded(px(6.))
        .border_1()
        .border_color(hsla(theme::BORDER))
        .bg(hsla(theme::BUTTON))
        .hover(|s| s.bg(hsla(theme::BUTTON_HOVER)))
        .cursor_pointer()
        .whitespace_nowrap()
        .child(label.into())
}

/// `.button.primary`.
pub fn primary_button(id: impl Into<ElementId>, label: impl Into<SharedString>) -> Stateful<Div> {
    div()
        .id(id)
        .px(px(12.))
        .py(px(4.))
        .rounded(px(6.))
        .border_1()
        .border_color(hsla(theme::ACCENT))
        .bg(hsla(theme::ACCENT))
        .text_color(gpui::white())
        .hover(|s| s.bg(hsla(0x1a85dd)))
        .cursor_pointer()
        .whitespace_nowrap()
        .child(label.into())
}

/// `.chip`: small rounded status pill.
pub fn chip(label: impl Into<SharedString>, color: u32) -> Div {
    div()
        .text_size(theme::TEXT_TINY)
        .px(px(8.))
        .py(px(1.))
        .rounded(px(10.))
        .border_1()
        .border_color(hsla(theme::BORDER))
        .text_color(hsla(color))
        .whitespace_nowrap()
        .child(label.into())
}

/// `.sidebar-title` / home `h3`: small uppercase heading.
pub fn section_title(label: impl Into<SharedString>) -> Div {
    div()
        .text_size(theme::TEXT_TINY)
        .text_color(hsla(theme::FG))
        .child(label.into().to_uppercase())
}

pub fn muted(text: impl Into<SharedString>) -> Div {
    div().text_color(hsla(theme::MUTED)).child(text.into())
}

pub fn error_text(text: impl Into<SharedString>) -> Div {
    div().text_color(hsla(theme::DEL)).child(text.into())
}

/// `code` inline: monospace on a dark pill.
pub fn code(text: impl Into<SharedString>) -> Div {
    div()
        .font_family(theme::MONO_FONT)
        .text_size(theme::TEXT_SMALL)
        .bg(hsla(theme::BG))
        .px(px(4.))
        .rounded(px(4.))
        .child(text.into())
}

/// A horizontal spacer that takes the remaining room (`.spacer`).
pub fn spacer() -> Div {
    div().flex_1()
}

/// "2016-03-06T…" → "3/6/2016", like `toLocaleDateString()` in en-US.
pub fn short_date(iso: &str) -> String {
    let mut parts = iso.get(..10).unwrap_or("").split('-');
    match (parts.next(), parts.next(), parts.next()) {
        (Some(y), Some(m), Some(d)) => format!("{}/{}/{}", m.trim_start_matches('0'), d.trim_start_matches('0'), y),
        _ => iso.to_string(),
    }
}

/// "2026-11-03T21:23:01Z" → "11/3/2026, 1:23:01 PM" in local time, like `toLocaleString()`.
pub fn short_datetime(iso: &str) -> String {
    let num = |r: std::ops::Range<usize>| iso.get(r).and_then(|v| v.parse::<i32>().ok());
    let (Some(y), Some(mo), Some(d), Some(h), Some(mi), Some(sec)) = (num(0..4), num(5..7), num(8..10), num(11..13), num(14..16), num(17..19)) else {
        return short_date(iso);
    };
    // SAFETY: plain C time conversions on stack values.
    let local = unsafe {
        let mut tm: libc::tm = std::mem::zeroed();
        tm.tm_year = y - 1900;
        tm.tm_mon = mo - 1;
        tm.tm_mday = d;
        tm.tm_hour = h;
        tm.tm_min = mi;
        tm.tm_sec = sec;
        let t = libc::timegm(&mut tm);
        let mut out: libc::tm = std::mem::zeroed();
        libc::localtime_r(&t, &mut out);
        out
    };
    let hour = local.tm_hour;
    let (h12, ampm) = match hour {
        0 => (12, "AM"),
        1..=11 => (hour, "AM"),
        12 => (12, "PM"),
        _ => (hour - 12, "PM"),
    };
    format!("{}/{}/{}, {h12}:{:02}:{:02} {ampm}", local.tm_mon + 1, local.tm_mday, local.tm_year + 1900, local.tm_min, local.tm_sec)
}

/// Human-readable size: "39 MB", "1.2 GB".
pub fn size(bytes: u64) -> String {
    let b = bytes as f64;
    if b >= 1e9 {
        format!("{:.1} GB", b / 1e9)
    } else if b >= 1e6 {
        format!("{} MB", (b / 1e6).round())
    } else {
        format!("{} KB", (b / 1e3).round())
    }
}

pub fn plural(n: usize, word: &str) -> String {
    format!("{n} {word}{}", if n == 1 { "" } else { "s" })
}


/// Text with a small "Copy" link after it, which puts the text on the
/// clipboard and says "Copied" for a moment: for install commands and errors.
#[derive(IntoElement)]
pub struct Copyable {
    id: ElementId,
    text: SharedString,
    command: bool,
}

/// A command to run (monospace pill), copyable.
pub fn copy_command(id: impl Into<ElementId>, text: impl Into<SharedString>) -> Copyable {
    Copyable { id: id.into(), text: text.into(), command: true }
}

/// An error message (red), copyable.
pub fn copy_error(id: impl Into<ElementId>, text: impl Into<SharedString>) -> Copyable {
    Copyable { id: id.into(), text: text.into(), command: false }
}

impl RenderOnce for Copyable {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let copied = window.use_keyed_state(self.id.clone(), cx, |_, _| false);
        let is_copied = *copied.read(cx);
        let text = self.text.clone();
        let body = if self.command {
            div()
                .font_family(theme::MONO_FONT)
                .text_size(theme::TEXT_SMALL)
                .bg(hsla(theme::BG))
                .border_1()
                .border_color(hsla(theme::BORDER))
                .rounded(px(4.))
                .px(px(6.))
                .py(px(1.))
                .child(self.text)
        } else {
            div().text_color(hsla(theme::DEL)).child(self.text)
        };
        div()
            .flex()
            .items_baseline()
            .gap(px(8.))
            .min_w_0()
            .child(body.min_w_0())
            .child(
                div()
                    .id(self.id)
                    .flex_shrink_0()
                    .text_size(theme::TEXT_SMALL)
                    .text_color(hsla(if is_copied { theme::ADD } else { theme::LINK }))
                    .cursor_pointer()
                    .whitespace_nowrap()
                    .child(if is_copied { "Copied ✓" } else { "Copy" })
                    .on_click(move |_, _, cx| {
                        cx.write_to_clipboard(ClipboardItem::new_string(text.to_string()));
                        copied.update(cx, |c, cx| {
                            *c = true;
                            cx.notify();
                            cx.spawn(async move |this, cx| {
                                cx.background_executor().timer(std::time::Duration::from_millis(1500)).await;
                                this.update(cx, |c, cx| {
                                    *c = false;
                                    cx.notify();
                                })
                                .ok();
                            })
                            .detach();
                        });
                    }),
            )
    }
}
