//! Colors and sizes, taken from the web version's `src/renderer/src/styles.css`
//! (the CSS variables at its top) so both apps look the same.
use gpui::{rgb, rgba, pattern_slash, App, Background, Hsla, Pixels, px};
use gpui_component::{Theme, ThemeMode};

pub const BG: u32 = 0x1f1f1f;
pub const PANEL: u32 = 0x181818;
pub const BORDER: u32 = 0x2b2b2b;
pub const FG: u32 = 0xcccccc;
pub const MUTED: u32 = 0x8b8b8b;
pub const ACCENT: u32 = 0x0078d4;
pub const HOVER: u32 = 0x2a2d2e;
pub const ACTIVE: u32 = 0x04395e;
pub const ADD: u32 = 0x3fb950;
pub const DEL: u32 = 0xf85149;
pub const WARN: u32 = 0xd29922;
pub const LINK: u32 = 0x4daafc;
/// `.button` background and its hover.
pub const BUTTON: u32 = 0x2b2b2b;
pub const BUTTON_HOVER: u32 = 0x333333;
/// Thread headers inside the editor.
pub const THREAD_HEADER: u32 = 0x202020;

/// Body text: 13px system UI font, like `body { font: 13px/1.4 -apple-system }`.
pub const TEXT: Pixels = px(13.);
pub const TEXT_SMALL: Pixels = px(12.);
pub const TEXT_TINY: Pixels = px(11.);
/// Height of the title-bar strip that holds the macOS traffic lights.
pub const TITLEBAR: Pixels = px(38.);
pub const SIDEBAR_WIDTH: Pixels = px(300.);
/// Editor font, as in the web version's Monaco settings (Menlo 13).
pub const MONO_FONT: &str = "Menlo";
pub const MONO_SIZE: Pixels = px(13.);

pub fn hsla(hex: u32) -> Hsla {
    rgb(hex).into()
}

/// Dark theme for gpui-component (inputs, buttons, menus, popovers) in our colors.
pub fn apply(cx: &mut App) {
    Theme::change(ThemeMode::Dark, None, cx);
    let theme = Theme::global_mut(cx);
    theme.font_size = TEXT;
    theme.mono_font_family = MONO_FONT.into();
    theme.mono_font_size = MONO_SIZE;
    theme.radius = px(6.);
    theme.radius_lg = px(8.);

    let c = &mut theme.colors;
    c.background = hsla(BG);
    c.foreground = hsla(FG);
    c.border = hsla(BORDER);
    c.input = hsla(BORDER);
    c.ring = hsla(ACCENT);
    c.caret = hsla(FG);
    c.selection = hsla(0x264f78);
    c.muted = hsla(PANEL);
    c.muted_foreground = hsla(MUTED);
    c.link = hsla(LINK);
    c.link_hover = hsla(LINK);
    c.link_active = hsla(LINK);
    c.primary = hsla(ACCENT);
    c.primary_hover = hsla(0x1a85dd);
    c.primary_active = hsla(0x006cbe);
    c.primary_foreground = hsla(0xffffff);
    c.button_primary = hsla(ACCENT);
    c.button_primary_hover = hsla(0x1a85dd);
    c.button_primary_active = hsla(0x006cbe);
    c.button_primary_foreground = hsla(0xffffff);
    c.secondary = hsla(BUTTON);
    c.secondary_hover = hsla(BUTTON_HOVER);
    c.secondary_active = hsla(BUTTON_HOVER);
    c.secondary_foreground = hsla(FG);
    c.button = hsla(BUTTON);
    c.button_hover = hsla(BUTTON_HOVER);
    c.button_active = hsla(BUTTON_HOVER);
    c.button_foreground = hsla(FG);
    c.list = hsla(PANEL);
    c.list_hover = hsla(HOVER);
    c.list_active = hsla(ACTIVE);
    c.list_active_border = hsla(ACTIVE);
    c.popover = hsla(PANEL);
    c.popover_foreground = hsla(FG);
    c.sidebar = hsla(PANEL);
    c.sidebar_border = hsla(BORDER);
    c.sidebar_foreground = hsla(FG);
    c.title_bar = hsla(PANEL);
    c.title_bar_border = hsla(BORDER);
    c.scrollbar_thumb = hsla(0x424242);
    c.scrollbar_thumb_hover = hsla(0x4f4f4f);
    c.danger = hsla(DEL);
    c.success = hsla(ADD);
    c.warning = hsla(WARN);
    // Push the edited colors to Base (Markdown views' link color, scrollbars).
    Theme::sync_base(cx);
}

// ---- Diff colors (VS Code Dark Modern defaults) ----

/// diffEditor.removedLineBackground / insertedLineBackground.
pub fn diff_line_bg(removed: bool) -> Hsla {
    if removed { rgba(0xff000033).into() } else { rgba(0x9bb95533).into() }
}

/// diffEditor.removedTextBackground / insertedTextBackground (changed words, on top of the line).
pub fn diff_text_bg(removed: bool) -> Hsla {
    if removed { rgba(0xff000033).into() } else { rgba(0x9ccc2c33).into() }
}

/// diffEditor.diagonalFill: hatching where the other side has lines and this one has none.
pub fn diff_filler() -> Background {
    pattern_slash(rgba(0xcccccc40), 2., 6.)
}

/// Monaco's line height for a font size on macOS (1.5×, rounded).
pub fn code_row_height(font_size: f32) -> f32 {
    (font_size * 1.5).round()
}
