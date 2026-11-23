//! A terminal for the agent panel (web: xterm.js in AgentPanel.tsx), built on
//! alacritty_terminal (Apache-2.0) for the PTY and VT state; drawn with gpui.
use alacritty_terminal::event::{Event as TermEvent, EventListener, Notify, WindowSize};
use alacritty_terminal::event_loop::{EventLoop, Msg, Notifier};
use alacritty_terminal::grid::{Dimensions, Scroll};
use alacritty_terminal::index::{Column, Line, Point as TPoint, Side};
use alacritty_terminal::selection::{Selection, SelectionType};
use alacritty_terminal::sync::FairMutex;
use alacritty_terminal::term::cell::Flags;
use alacritty_terminal::term::{Config, Term, TermMode};
use alacritty_terminal::tty;
use alacritty_terminal::vte::ansi::{Color, CursorShape, NamedColor};
use futures::StreamExt;
use futures::channel::mpsc::{UnboundedSender, unbounded};
use gpui::prelude::FluentBuilder as _;
use gpui::*;
use std::borrow::Cow;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// The web version's xterm.js settings.
pub const FONT_SIZE: f32 = 12.;
pub const LINE_HEIGHT: f32 = 1.15;
const FONT: &str = "Menlo";
pub const BG: u32 = 0x181818;
const FG: u32 = 0xcccccc;
const SELECTION: u32 = 0x264f78;
const ANSI: [u32; 16] = [
    0x000000, 0xf14c4c, 0x23d18b, 0xf5f543, 0x3b8eea, 0xd670d6, 0x29b8db, 0xe5e5e5, //
    0x666666, 0xf14c4c, 0x23d18b, 0xf5f543, 0x3b8eea, 0xd670d6, 0x29b8db, 0xe5e5e5,
];

#[derive(Clone)]
struct Listener(UnboundedSender<TermEvent>);

impl EventListener for Listener {
    fn send_event(&self, event: TermEvent) {
        let _ = self.0.unbounded_send(event);
    }
}

#[derive(Clone, Copy, PartialEq)]
struct GridSize {
    cols: usize,
    rows: usize,
    cell_w: Pixels,
    cell_h: Pixels,
}

impl Dimensions for GridSize {
    fn total_lines(&self) -> usize {
        self.rows
    }
    fn screen_lines(&self) -> usize {
        self.rows
    }
    fn columns(&self) -> usize {
        self.cols
    }
}

impl GridSize {
    fn window_size(&self) -> WindowSize {
        WindowSize {
            num_lines: self.rows as u16,
            num_cols: self.cols as u16,
            cell_width: f32::from(self.cell_w) as u16,
            cell_height: f32::from(self.cell_h) as u16,
        }
    }
}

/// A running PTY and terminal state, before its view exists.
pub struct Started {
    term: Arc<FairMutex<Term<Listener>>>,
    notifier: Notifier,
    size: GridSize,
    rx: futures::channel::mpsc::UnboundedReceiver<TermEvent>,
}

pub enum TerminalEvent {
    Exited,
}

pub struct TerminalView {
    term: Arc<FairMutex<Term<Listener>>>,
    notifier: Notifier,
    size: GridSize,
    focus: FocusHandle,
    pub exited: bool,
    pub started_at: Instant,
    pub last_output: Option<Instant>,
    /// Where the left button went down (for drag selection), in grid points.
    selecting: bool,
    /// Scroll movement smaller than a line, kept between events (trackpads send tiny deltas).
    scroll_rest: f32,
    /// Dragging the scrollbar thumb.
    scrollbar_drag: bool,
    /// Where the grid is drawn (for the scrollbar).
    grid_bounds: std::rc::Rc<std::cell::Cell<Bounds<Pixels>>>,
}

/// Width kept free on the right for the scrollbar.
const SCROLLBAR_W: f32 = 10.;

impl EventEmitter<TerminalEvent> for TerminalView {}

impl Focusable for TerminalView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl TerminalView {
    /// Starts `program` in a new PTY at `cwd`; the view is made with [`Self::new`].
    pub fn start(program: String, args: Vec<String>, env: Vec<(String, String)>, cwd: String) -> Result<Started, String> {
        let size = GridSize { cols: 120, rows: 20, cell_w: px(7.), cell_h: px(14.) };
        let (tx, rx) = unbounded();
        let listener = Listener(tx);
        let config = Config { scrolling_history: 5000, ..Config::default() };
        let term = Arc::new(FairMutex::new(Term::new(config, &size, listener.clone())));
        let mut env: HashMap<String, String> = env.into_iter().collect();
        env.insert("TERM".into(), "xterm-256color".into());
        env.insert("COLORTERM".into(), "truecolor".into());
        let options = tty::Options {
            shell: Some(tty::Shell::new(program, args)),
            working_directory: Some(cwd.into()),
            drain_on_exit: true,
            env,
        };
        let pty = tty::new(&options, size.window_size(), 0).map_err(|e| e.to_string())?;
        let event_loop = EventLoop::new(term.clone(), listener, pty, true, false).map_err(|e| e.to_string())?;
        let notifier = Notifier(event_loop.channel());
        event_loop.spawn();
        Ok(Started { term, notifier, size, rx })
    }

    pub fn new(started: Started, cx: &mut Context<Self>) -> Self {
        let Started { term, notifier, size, mut rx } = started;
        cx.spawn(async move |this, cx| {
            while let Some(event) = rx.next().await {
                let alive = this
                    .update(cx, |this, cx| {
                        match event {
                            TermEvent::Wakeup => {
                                this.last_output = Some(Instant::now());
                                cx.notify();
                            }
                            TermEvent::PtyWrite(text) => this.write(text.into_bytes()),
                            TermEvent::ClipboardStore(_, text) => cx.write_to_clipboard(ClipboardItem::new_string(text)),
                            TermEvent::ClipboardLoad(_, format) => {
                                let text = cx.read_from_clipboard().and_then(|c| c.text()).unwrap_or_default();
                                this.write(format(&text).into_bytes());
                            }
                            TermEvent::ColorRequest(ix, format) => {
                                let rgb = color_rgb(ix);
                                this.write(format(alacritty_terminal::vte::ansi::Rgb { r: (rgb >> 16) as u8, g: (rgb >> 8) as u8, b: rgb as u8 }).into_bytes());
                            }
                            TermEvent::TextAreaSizeRequest(format) => this.write(format(this.size.window_size()).into_bytes()),
                            TermEvent::Exit | TermEvent::ChildExit(_) => {
                                if !this.exited {
                                    this.exited = true;
                                    cx.emit(TerminalEvent::Exited);
                                    cx.notify();
                                }
                            }
                            _ => {}
                        }
                    })
                    .is_ok();
                if !alive {
                    break;
                }
            }
        })
        .detach();

        Self { term, notifier, size, focus: cx.focus_handle(), exited: false, started_at: Instant::now(), last_output: None, selecting: false, scroll_rest: 0., scrollbar_drag: false, grid_bounds: Default::default() }
    }

    pub fn write(&self, bytes: impl Into<Cow<'static, [u8]>>) {
        self.notifier.notify(bytes);
    }

    pub fn focus(&self, window: &mut Window, cx: &mut App) {
        window.focus(&self.focus, cx);
    }

    pub fn kill(&mut self) {
        let _ = self.notifier.0.send(Msg::Shutdown);
    }

    /// The last `n` lines of the screen as text (for prompt detection).
    pub fn tail_text(&self, n: usize) -> String {
        let term = self.term.lock();
        let grid = term.grid();
        let rows = grid.screen_lines();
        let mut out = String::new();
        for row in rows.saturating_sub(n)..rows {
            let line = &grid[Line(row as i32)];
            let text: String = (0..grid.columns()).map(|c| line[Column(c)].c).collect();
            out.push_str(text.trim_end());
            out.push('\n');
        }
        out
    }

    /// Whether the agent is asking a yes/no or trust question (never type into those).
    pub fn shows_confirmation_prompt(&self) -> bool {
        let text = self.tail_text(20).to_lowercase();
        ["trust this folder", "trust the files in this folder", "enter to confirm", "do you want to proceed", "(y/n)", "[y/n]"].iter().any(|p| text.contains(p))
    }

    fn resize(&mut self, size: GridSize) {
        if size == self.size || size.cols < 2 || size.rows < 1 {
            return;
        }
        self.size = size;
        self.term.lock().resize(size);
        let _ = self.notifier.0.send(Msg::Resize(size.window_size()));
    }

    fn mode(&self) -> TermMode {
        *self.term.lock().mode()
    }

    fn paste(&self, text: &str) {
        let text = text.replace("\r\n", "\r").replace('\n', "\r");
        if self.mode().contains(TermMode::BRACKETED_PASTE) {
            self.write(format!("\x1b[200~{}\x1b[201~", text.replace('\x1b', "")).into_bytes());
        } else {
            self.write(text.into_bytes());
        }
    }

    fn on_key(&mut self, event: &KeyDownEvent, cx: &mut Context<Self>) {
        let k = &event.keystroke;
        let m = k.modifiers;
        if m.platform {
            match k.key.as_str() {
                "v" => {
                    if let Some(text) = cx.read_from_clipboard().and_then(|c| c.text()) {
                        self.paste(&text);
                    }
                    cx.stop_propagation();
                }
                "c" => {
                    if let Some(text) = self.term.lock().selection_to_string() {
                        cx.write_to_clipboard(ClipboardItem::new_string(text));
                    }
                    cx.stop_propagation();
                }
                "k" => {
                    // Clear scrollback, like Terminal.app.
                    self.write(&b"\x0c"[..]);
                    cx.stop_propagation();
                }
                // Other ⌘ shortcuts belong to the app (⌘J, ⌘W, …).
                _ => {}
            }
            return;
        }
        if let Some(bytes) = key_bytes(k, self.mode()) {
            // Typing scrolls back to the prompt and drops the selection.
            {
                let mut term = self.term.lock();
                term.scroll_display(Scroll::Bottom);
                term.selection = None;
            }
            self.write(bytes);
            cx.stop_propagation();
            cx.notify();
        }
    }

    fn grid_point(&self, position: Point<Pixels>, bounds: Bounds<Pixels>) -> (TPoint, Side) {
        let x = (position.x - bounds.left()).max(px(0.));
        let y = (position.y - bounds.top()).max(px(0.));
        let col = ((x / self.size.cell_w) as usize).min(self.size.cols.saturating_sub(1));
        let row = ((y / self.size.cell_h) as usize).min(self.size.rows.saturating_sub(1));
        let side = if (x / self.size.cell_w).fract() < 0.5 { Side::Left } else { Side::Right };
        let offset = self.term.lock().grid().display_offset() as i32;
        (TPoint::new(Line(row as i32 - offset), Column(col)), side)
    }

    fn on_scroll(&mut self, event: &ScrollWheelEvent, cx: &mut Context<Self>) {
        let lines = match event.delta {
            ScrollDelta::Lines(p) => p.y,
            ScrollDelta::Pixels(p) => f32::from(p.y) / f32::from(self.size.cell_h),
        };
        // Add up small trackpad movements until they make whole lines.
        let total = self.scroll_rest + lines;
        let n = total.trunc() as i32;
        self.scroll_rest = total - n as f32;
        if n == 0 {
            return;
        }
        let mode = self.mode();
        if mode.intersects(TermMode::MOUSE_MODE) {
            // The app asked for mouse events (e.g. Claude Code's full-screen UI):
            // report the wheel, like xterm, as buttons 64 (up) / 65 (down).
            let (col, row) = self.cell_at(event.position);
            let button: u8 = if n > 0 { 64 } else { 65 };
            for _ in 0..n.abs() {
                let report = if mode.contains(TermMode::SGR_MOUSE) {
                    format!("\x1b[<{button};{};{}M", col + 1, row + 1).into_bytes()
                } else {
                    // Legacy X10 encoding: values offset by 32, coordinates capped at 223.
                    vec![0x1b, b'[', b'M', 32 + button, 32 + (col + 1).min(223) as u8, 32 + (row + 1).min(223) as u8]
                };
                self.write(report);
            }
        } else if mode.contains(TermMode::ALT_SCREEN) && mode.contains(TermMode::ALTERNATE_SCROLL) {
            let seq: &[u8] = if n > 0 { b"\x1bOA" } else { b"\x1bOB" };
            for _ in 0..n.abs() {
                self.write(seq.to_vec());
            }
        } else {
            self.term.lock().scroll_display(Scroll::Delta(n));
        }
        cx.notify();
    }

    /// The screen cell (column, row; 0-based) under a window position.
    fn cell_at(&self, position: Point<Pixels>) -> (usize, usize) {
        let b = self.grid_bounds.get();
        let x = (position.x - b.left()).max(px(0.));
        let y = (position.y - b.top()).max(px(0.));
        let col = ((x / self.size.cell_w) as usize).min(self.size.cols.saturating_sub(1));
        let row = ((y / self.size.cell_h) as usize).min(self.size.rows.saturating_sub(1));
        (col, row)
    }

    /// (lines of history above the screen, lines scrolled up, screen lines).
    fn scroll_state(&self) -> (usize, usize, usize) {
        let term = self.term.lock();
        let grid = term.grid();
        (grid.history_size(), grid.display_offset(), grid.screen_lines())
    }

    /// Scrolls so the scrollbar thumb is centered at window y.
    fn scroll_to_y(&mut self, y: Pixels, cx: &mut Context<Self>) {
        let b = self.grid_bounds.get();
        let (history, offset, screen) = self.scroll_state();
        if history == 0 || b.size.height <= px(0.) {
            return;
        }
        let total = (history + screen) as f32;
        let fraction = (f32::from(y - b.top()) / f32::from(b.size.height)).clamp(0., 1.);
        // Top of the screen, in lines from the top of the history.
        let top = (fraction * total - screen as f32 / 2.).clamp(0., history as f32);
        let target_offset = history as i32 - top.round() as i32;
        self.term.lock().scroll_display(Scroll::Delta(target_offset - offset as i32));
        cx.notify();
    }

    /// VS Code-style scrollbar over the right edge: where the screen is in the history.
    fn render_scrollbar(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let (history, offset, screen) = self.scroll_state();
        if history == 0 {
            return None;
        }
        let total = (history + screen) as f32;
        let height = (screen as f32 / total).max(0.05);
        let top = ((history - offset.min(history)) as f32 / total).min(1. - height);
        let dragging = self.scrollbar_drag;
        Some(
            div()
                .id("terminal-scrollbar")
                .absolute()
                .top_0()
                .bottom_0()
                .right_0()
                .w(px(SCROLLBAR_W))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, ev: &MouseDownEvent, _, cx| {
                        cx.stop_propagation();
                        this.scrollbar_drag = true;
                        this.scroll_to_y(ev.position.y, cx);
                    }),
                )
                .child(
                    div()
                        .absolute()
                        .left_0()
                        .right_0()
                        .top(relative(top))
                        .h(relative(height))
                        .bg(gpui::rgba(if dragging { 0xbfbfbf66 } else { 0x79797966 }))
                        .hover(|el| el.bg(gpui::rgba(0x646464b3))),
                )
                .into_any_element(),
        )
    }
}

/// Bytes a key sends, like xterm (with Option as Meta).
fn key_bytes(k: &Keystroke, mode: TermMode) -> Option<Vec<u8>> {
    let m = k.modifiers;
    let app_cursor = mode.contains(TermMode::APP_CURSOR);
    let arrow = |c: char| -> Vec<u8> {
        let modifier = 1 + m.shift as u8 + 2 * m.alt as u8 + 4 * m.control as u8;
        if modifier > 1 {
            format!("\x1b[1;{modifier}{c}").into_bytes()
        } else if app_cursor {
            format!("\x1bO{c}").into_bytes()
        } else {
            format!("\x1b[{c}").into_bytes()
        }
    };
    let bytes = match k.key.as_str() {
        "enter" => {
            if m.alt || m.shift {
                b"\x1b\r".to_vec()
            } else {
                b"\r".to_vec()
            }
        }
        "backspace" => {
            if m.alt {
                b"\x1b\x7f".to_vec()
            } else if m.control {
                b"\x08".to_vec()
            } else {
                b"\x7f".to_vec()
            }
        }
        "tab" => {
            if m.shift {
                b"\x1b[Z".to_vec()
            } else {
                b"\t".to_vec()
            }
        }
        "escape" => b"\x1b".to_vec(),
        "up" => arrow('A'),
        "down" => arrow('B'),
        "right" => arrow('C'),
        "left" => arrow('D'),
        "home" => arrow('H'),
        "end" => arrow('F'),
        "pageup" => b"\x1b[5~".to_vec(),
        "pagedown" => b"\x1b[6~".to_vec(),
        "delete" => b"\x1b[3~".to_vec(),
        "space" if m.control => vec![0],
        key => {
            if m.control && key.chars().count() == 1 {
                let c = key.chars().next()?.to_ascii_lowercase();
                let code = match c {
                    'a'..='z' => c as u8 - b'a' + 1,
                    '[' => 0x1b,
                    '\\' => 0x1c,
                    ']' => 0x1d,
                    '^' | '6' => 0x1e,
                    '_' | '-' => 0x1f,
                    '@' | '2' => 0,
                    _ => return None,
                };
                let mut v = Vec::new();
                if m.alt {
                    v.push(0x1b);
                }
                v.push(code);
                v
            } else if m.alt {
                // Option as Meta: ESC + the unmodified key.
                let mut v = vec![0x1b];
                v.extend_from_slice(if k.key == "space" { b" " } else { k.key.as_bytes() });
                v
            } else {
                let text = k.key_char.clone().or_else(|| (k.key == "space").then(|| " ".to_string()))?;
                text.into_bytes()
            }
        }
    };
    Some(bytes)
}

fn color_rgb(ix: usize) -> u32 {
    match ix {
        0..=15 => ANSI[ix],
        16..=231 => {
            let i = ix - 16;
            let level = |v: usize| if v == 0 { 0 } else { (55 + v * 40) as u32 };
            (level(i / 36) << 16) | (level((i / 6) % 6) << 8) | level(i % 6)
        }
        232..=255 => {
            let v = (8 + (ix - 232) * 10) as u32;
            (v << 16) | (v << 8) | v
        }
        257 => BG,
        _ => FG,
    }
}

fn resolve(color: Color, colors: &alacritty_terminal::term::color::Colors, bold: bool) -> Hsla {
    let rgb = match color {
        Color::Spec(c) => ((c.r as u32) << 16) | ((c.g as u32) << 8) | c.b as u32,
        Color::Indexed(i) => {
            let i = if bold && i < 8 { i + 8 } else { i } as usize;
            match colors[i] {
                Some(c) => ((c.r as u32) << 16) | ((c.g as u32) << 8) | c.b as u32,
                None => color_rgb(i),
            }
        }
        Color::Named(n) => {
            if let Some(c) = colors[n] {
                ((c.r as u32) << 16) | ((c.g as u32) << 8) | c.b as u32
            } else {
                match n {
                    NamedColor::Foreground | NamedColor::BrightForeground | NamedColor::Cursor => FG,
                    NamedColor::Background => BG,
                    NamedColor::DimForeground => 0x999999,
                    n => {
                        let ix = n as usize;
                        if ix < 16 {
                            color_rgb(if bold && ix < 8 { ix + 8 } else { ix })
                        } else if (NamedColor::DimBlack as usize..=NamedColor::DimWhite as usize).contains(&ix) {
                            color_rgb(ix - NamedColor::DimBlack as usize)
                        } else {
                            FG
                        }
                    }
                }
            }
        }
    };
    rgb_hsla(rgb)
}

fn rgb_hsla(rgb: u32) -> Hsla {
    gpui::rgb(rgb).into()
}

impl Render for TerminalView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let focused = self.focus.is_focused(window);
        let term = self.term.clone();
        let this = cx.entity().downgrade();
        let font = Font { family: FONT.into(), ..font(FONT) };
        let size = crate::settings::get(cx).terminal_font_size;
        let font_size = px(size);
        let font_id = window.text_system().resolve_font(&font);
        let cell_w = window.text_system().advance(font_id, font_size, 'm').map(|s| s.width).unwrap_or(px(7.2));
        let cell_h = px((size * LINE_HEIGHT).ceil());
        let bounds_cell = std::rc::Rc::new(std::cell::Cell::new(Bounds::default()));
        let grid_bounds = self.grid_bounds.clone();
        let scrollbar = self.render_scrollbar(cx);
        let bounds_for_mouse = bounds_cell.clone();
        let bounds_for_drag = bounds_cell.clone();

        div()
            .id("terminal")
            .key_context("Terminal")
            .track_focus(&self.focus)
            .size_full()
            .relative()
            .bg(rgb_hsla(BG))
            .cursor_text()
            .on_key_down(cx.listener(|this, ev: &KeyDownEvent, _, cx| this.on_key(ev, cx)))
            .on_scroll_wheel(cx.listener(|this, ev: &ScrollWheelEvent, _, cx| this.on_scroll(ev, cx)))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, ev: &MouseDownEvent, window, cx| {
                    window.focus(&this.focus, cx);
                    let (point, side) = this.grid_point(ev.position, bounds_for_mouse.get());
                    let kind = match ev.click_count {
                        2 => SelectionType::Semantic,
                        3 => SelectionType::Lines,
                        _ => SelectionType::Simple,
                    };
                    if ev.modifiers.platform {
                        // ⌘-click opens a URL under the pointer (web: WebLinksAddon).
                        if let Some(url) = this.url_at(point) {
                            cx.open_url(&url);
                        }
                        return;
                    }
                    this.term.lock().selection = (ev.click_count > 1).then(|| Selection::new(kind, point, side));
                    this.selecting = ev.click_count == 1;
                    if ev.click_count == 1 {
                        this.term.lock().selection = Some(Selection::new(SelectionType::Simple, point, side));
                    }
                    cx.notify();
                }),
            )
            .on_mouse_move(cx.listener(move |this, ev: &MouseMoveEvent, _, cx| {
                if this.scrollbar_drag {
                    if ev.pressed_button == Some(MouseButton::Left) {
                        this.scroll_to_y(ev.position.y, cx);
                    } else {
                        this.scrollbar_drag = false;
                    }
                    return;
                }
                if this.selecting && ev.pressed_button == Some(MouseButton::Left) {
                    let (point, side) = this.grid_point(ev.position, bounds_for_drag.get());
                    if let Some(sel) = this.term.lock().selection.as_mut() {
                        sel.update(point, side);
                    }
                    cx.notify();
                }
            }))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _: &MouseUpEvent, _, cx| {
                    this.scrollbar_drag = false;
                    this.selecting = false;
                    // A click without a drag leaves no selection.
                    let empty = this.term.lock().selection_to_string().is_none_or(|s| s.is_empty());
                    if empty {
                        this.term.lock().selection = None;
                    }
                    cx.notify();
                }),
            )
            .child(
                canvas(
                    move |bounds, _, cx| {
                        bounds_cell.set(bounds);
                        grid_bounds.set(bounds);
                        let cols = (((bounds.size.width - px(SCROLLBAR_W)) / cell_w).floor() as usize).max(2);
                        let rows = ((bounds.size.height / cell_h).floor() as usize).max(1);
                        this.update(cx, |this, _| this.resize(GridSize { cols, rows, cell_w, cell_h })).ok();
                    },
                    move |bounds, _, window, cx| {
                        let term = term.lock();
                        paint_grid(&term, bounds, &font, font_size, cell_w, cell_h, focused, window, cx);
                    },
                )
                .size_full(),
            )
            .children(scrollbar)
    }
}

impl TerminalView {
    /// The http(s) URL around a grid point, if any.
    fn url_at(&self, point: TPoint) -> Option<String> {
        let term = self.term.lock();
        let grid = term.grid();
        let row = &grid[point.line];
        let text: String = (0..grid.columns()).map(|c| row[Column(c)].c).collect();
        let col = point.column.0;
        let re = regex::Regex::new(r#"https?://[^\s<>"'`]+"#).ok()?;
        re.find_iter(&text)
            .find(|m| {
                let start = text[..m.start()].chars().count();
                let end = start + m.as_str().chars().count();
                (start..end).contains(&col)
            })
            .map(|m| m.as_str().trim_end_matches(['.', ',', ')', ']', ';', ':']).to_string())
    }
}

#[allow(clippy::too_many_arguments)]
fn paint_grid(term: &Term<Listener>, bounds: Bounds<Pixels>, font: &Font, font_size: Pixels, cell_w: Pixels, cell_h: Pixels, focused: bool, window: &mut Window, cx: &mut App) {
    let content = term.renderable_content();
    let offset = content.display_offset as i32;
    let selection = content.selection;
    let colors = content.colors;
    let cursor = content.cursor;
    let rows = term.screen_lines();
    let cols = term.columns();

    // Collect cells per screen row.
    let mut lines: Vec<Vec<(usize, char, Hsla, Option<Hsla>, Flags)>> = vec![Vec::new(); rows];
    for indexed in content.display_iter {
        let row = (indexed.point.line.0 + offset) as usize;
        if row >= rows {
            continue;
        }
        let cell = indexed.cell;
        if cell.flags.contains(Flags::WIDE_CHAR_SPACER) || cell.flags.contains(Flags::LEADING_WIDE_CHAR_SPACER) {
            continue;
        }
        let bold = cell.flags.contains(Flags::BOLD);
        let mut fg = resolve(cell.fg, colors, bold);
        let mut bg = match cell.bg {
            Color::Named(NamedColor::Background) => None,
            other => Some(resolve(other, colors, false)),
        };
        if cell.flags.contains(Flags::INVERSE) {
            let old_fg = fg;
            fg = bg.unwrap_or(rgb_hsla(BG));
            bg = Some(old_fg);
        }
        if cell.flags.contains(Flags::DIM) {
            fg = fg.opacity(0.66);
        }
        if selection.is_some_and(|s| s.contains(indexed.point)) {
            bg = Some(rgb_hsla(SELECTION));
        }
        let c = if cell.flags.contains(Flags::HIDDEN) { ' ' } else { cell.c };
        lines[row].push((indexed.point.column.0, c, fg, bg, cell.flags));
    }

    for (row, cells) in lines.iter().enumerate() {
        let y = bounds.top() + cell_h * row as f32;
        // Backgrounds, merged per run of equal color.
        let mut i = 0;
        while i < cells.len() {
            if let Some(bg) = cells[i].3 {
                let start = cells[i].0;
                let mut end = start + if cells[i].4.contains(Flags::WIDE_CHAR) { 2 } else { 1 };
                let mut j = i + 1;
                while j < cells.len() && cells[j].3 == Some(bg) {
                    end = cells[j].0 + if cells[j].4.contains(Flags::WIDE_CHAR) { 2 } else { 1 };
                    j += 1;
                }
                window.paint_quad(fill(
                    Bounds::new(point(bounds.left() + cell_w * start as f32, y), size(cell_w * (end - start) as f32, cell_h)),
                    bg,
                ));
                i = j;
            } else {
                i += 1;
            }
        }
        // Text: one shaped run per cell keeps glyphs on the grid; group plain
        // single-width cells of the same style into one line shape.
        let mut i = 0;
        while i < cells.len() {
            let (col, _, fg, _, flags) = cells[i];
            let wide = flags.contains(Flags::WIDE_CHAR);
            let style = |f: Flags| (f.contains(Flags::BOLD), f.contains(Flags::ITALIC), f.intersects(Flags::ALL_UNDERLINES), f.contains(Flags::STRIKEOUT));
            let mut text = String::new();
            let mut j = i;
            while j < cells.len() {
                let (c2, ch, fg2, _, f2) = cells[j];
                if c2 != col + (j - i) || fg2 != fg || style(f2) != style(flags) || f2.contains(Flags::WIDE_CHAR) != wide || (wide && j > i) {
                    break;
                }
                text.push(ch);
                j += 1;
            }
            if text.trim().is_empty() && !flags.intersects(Flags::ALL_UNDERLINES) {
                i = j;
                continue;
            }
            let run_font = Font {
                weight: if flags.contains(Flags::BOLD) { FontWeight::BOLD } else { FontWeight::NORMAL },
                style: if flags.contains(Flags::ITALIC) { FontStyle::Italic } else { FontStyle::Normal },
                ..font.clone()
            };
            let run = TextRun {
                len: text.len(),
                font: run_font,
                color: fg,
                background_color: None,
                underline: flags.intersects(Flags::ALL_UNDERLINES).then(|| UnderlineStyle { thickness: px(1.), color: Some(fg), wavy: false }),
                strikethrough: flags.contains(Flags::STRIKEOUT).then(|| StrikethroughStyle { thickness: px(1.), color: Some(fg) }),
            };
            let width = if wide { cell_w * 2. } else { cell_w };
            let shaped = window.text_system().shape_line(text.into(), font_size, &[run], Some(width));
            let _ = shaped.paint(point(bounds.left() + cell_w * col as f32, y), cell_h, TextAlign::Left, None, window, cx);
            i = j;
        }
    }

    // Cursor.
    let crow = cursor.point.line.0 + offset;
    if cursor.shape != CursorShape::Hidden && crow >= 0 && (crow as usize) < rows && cursor.point.column.0 < cols {
        let origin = point(bounds.left() + cell_w * cursor.point.column.0 as f32, bounds.top() + cell_h * crow as f32);
        let color = rgb_hsla(FG);
        match (focused, cursor.shape) {
            (true, CursorShape::Beam) => window.paint_quad(fill(Bounds::new(origin, size(px(1.), cell_h)), color)),
            (true, CursorShape::Underline) => window.paint_quad(fill(Bounds::new(point(origin.x, origin.y + cell_h - px(1.)), size(cell_w, px(1.))), color)),
            (true, _) => {
                window.paint_quad(fill(Bounds::new(origin, size(cell_w, cell_h)), color));
                // The character under a block cursor, in the background color.
                let line = &term.grid()[cursor.point.line];
                let ch = line[cursor.point.column].c;
                if ch != ' ' {
                    let run = TextRun { len: ch.len_utf8(), font: font.clone(), color: rgb_hsla(BG), background_color: None, underline: None, strikethrough: None };
                    let shaped = window.text_system().shape_line(ch.to_string().into(), font_size, &[run], Some(cell_w));
                    let _ = shaped.paint(origin, cell_h, TextAlign::Left, None, window, cx);
                }
            }
            (false, _) => window.paint_quad(outline(Bounds::new(origin, size(cell_w, cell_h)), color, BorderStyle::Solid)),
        }
    }
}

/// Kept for callers that want to wait for output to settle.
pub fn settled(view: &TerminalView, now: Instant) -> bool {
    view.last_output.is_some_and(|t| now.duration_since(t) > Duration::from_millis(700)) && now.duration_since(view.started_at) > Duration::from_millis(1500)
}

pub fn status_line_hint() -> &'static str {
    "in this PR's checkout · ⌘L: about this code · ⌘⇧L: explain this code"
}

