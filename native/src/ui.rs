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

/// "2026-11-03T13:23:01Z" → "11/3/2026, 1:23:01 PM" (UTC), like `toLocaleString()`.
pub fn short_datetime(iso: &str) -> String {
    let date = short_date(iso);
    let time = iso.get(11..19).unwrap_or("");
    let mut t = time.split(':');
    match (t.next().and_then(|h| h.parse::<u32>().ok()), t.next(), t.next()) {
        (Some(h), Some(m), Some(s)) => {
            let (h12, ampm) = if h == 0 { (12, "AM") } else if h < 12 { (h, "AM") } else if h == 12 { (12, "PM") } else { (h - 12, "PM") };
            format!("{date}, {h12}:{m}:{s} {ampm}")
        }
        _ => date,
    }
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

