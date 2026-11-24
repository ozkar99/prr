//! User settings (⌘,): the agent command, font sizes, and remembered layout.
//! Stored in ~/Library/Application Support/review-tool/settings.json. The
//! agent command used to live in ~/.review-tool-agent-command, which still
//! works when no command is set here.
use crate::backend::exec::data_dir;
use crate::theme::{self, hsla};
use crate::ui;
use gpui::prelude::FluentBuilder as _;
use gpui::*;
use gpui_component::input::{Input, InputEvent, InputState};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

actions!(review_tool, [OpenSettings]);

pub const EDITOR_FONT: (f32, f32, f32) = (12., 9., 24.);
pub const TERMINAL_FONT: (f32, f32, f32) = (12., 9., 24.);

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(default, rename_all = "camelCase")]
pub struct Settings {
    /// Command line for the agent panel; empty means Claude Code.
    pub agent_command: String,
    pub editor_font_size: f32,
    pub terminal_font_size: f32,
    pub agent_panel_height: f32,
    /// The PR sidebar's width, in px.
    pub sidebar_width: f32,
    /// The old side's share of a side-by-side diff.
    pub split_ratio: f32,
    /// Mark PR checkouts as trusted in Claude Code, so it doesn't ask. Off by default.
    pub trust_claude_checkouts: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self { agent_command: String::new(), editor_font_size: EDITOR_FONT.0, terminal_font_size: TERMINAL_FONT.0, agent_panel_height: 300., sidebar_width: 300., split_ratio: 0.5, trust_claude_checkouts: false }
    }
}

impl Global for Settings {}

pub fn path() -> PathBuf {
    data_dir().join("settings.json")
}

/// Reads the settings file (defaults for anything missing or unreadable).
pub fn load() -> Settings {
    std::fs::read(path()).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
}

impl Settings {
    fn save(&self) {
        let _ = std::fs::create_dir_all(data_dir());
        let tmp = path().with_extension("json.tmp");
        if std::fs::write(&tmp, serde_json::to_vec_pretty(self).unwrap_or_default()).is_ok() {
            let _ = std::fs::rename(&tmp, path());
        }
    }
}

pub fn init(cx: &mut App) {
    cx.set_global(load());
    cx.bind_keys([KeyBinding::new("cmd-,", OpenSettings, None)]);
    apply(cx);
}

pub fn get(cx: &App) -> &Settings {
    cx.global::<Settings>()
}

/// Changes settings, saves them, and applies them to open windows.
pub fn update(cx: &mut App, f: impl FnOnce(&mut Settings)) {
    let mut settings = cx.global::<Settings>().clone();
    f(&mut settings);
    if &settings == cx.global::<Settings>() {
        return;
    }
    settings.save();
    cx.set_global(settings);
    apply(cx);
}

fn apply(cx: &mut App) {
    let size = get(cx).editor_font_size;
    gpui_component::Theme::global_mut(cx).mono_font_size = px(size);
    gpui_component::Theme::sync_base(cx);
    cx.refresh_windows();
}

// ---------------------------------------------------------------- screen

pub enum SettingsEvent {
    Close,
}

pub struct SettingsView {
    agent_command: Entity<InputState>,
    /// Disk used by prr's own copies (None: still measuring).
    usage: Option<crate::backend::cleanup::DiskUsage>,
    clearing: bool,
    focus: FocusHandle,
    _subs: Vec<Subscription>,
}

impl EventEmitter<SettingsEvent> for SettingsView {}

impl SettingsView {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let current = get(cx).agent_command.clone();
        let agent_command = cx.new(|cx| {
            let mut state = InputState::new(window, cx).placeholder("claude (Claude Code, the default)");
            state.set_value(current, window, cx);
            state
        });
        let sub = cx.subscribe(&agent_command, |_, input, ev: &InputEvent, cx| {
            if let InputEvent::Change = ev {
                let value = input.read(cx).value().trim().to_string();
                update(cx, |s| s.agent_command = value);
            }
        });
        let mut this = Self { agent_command, usage: None, clearing: false, focus: cx.focus_handle(), _subs: vec![sub] };
        this.load_usage(cx);
        this
    }

    fn load_usage(&mut self, cx: &mut Context<Self>) {
        let fut = crate::backend::blocking(crate::backend::cleanup::usage);
        cx.spawn(async move |this, cx| {
            let usage = fut.await;
            this.update(cx, |this, cx| {
                this.usage = Some(usage);
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn clear(&mut self, include_clones: bool, cx: &mut Context<Self>) {
        if self.clearing {
            return;
        }
        self.clearing = true;
        let fut = crate::backend::task(crate::backend::cleanup::clear(include_clones));
        cx.spawn(async move |this, cx| {
            fut.await;
            this.update(cx, |this, cx| {
                this.clearing = false;
                this.load_usage(cx);
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    fn render_storage(&self, cx: &mut Context<Self>) -> AnyElement {
        let Some(u) = &self.usage else { return ui::muted("Measuring…").text_size(theme::TEXT_SMALL).into_any_element() };
        let clearing = self.clearing;
        div()
            .flex()
            .flex_col()
            .gap(px(8.))
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap(px(4.))
                    .child(format!("{} · {}", ui::plural(u.worktrees, "PR checkout"), ui::size(u.worktree_bytes)))
                    .child(ui::muted(format!("· {} · {}", ui::plural(u.repos, "cloned repo"), ui::size(u.repo_bytes)))),
            )
            .child(
                div()
                    .flex()
                    .gap(px(8.))
                    .child(
                        ui::button("clear-checkouts", if clearing { "Clearing…" } else { "Clear PR checkouts" })
                            .when(clearing || u.worktrees == 0, |el| el.opacity(0.5))
                            .on_click(cx.listener(|this, _, _, cx| this.clear(false, cx))),
                    )
                    .child(
                        ui::button("clear-everything", "Clear everything")
                            .when(clearing, |el| el.opacity(0.5))
                            .on_click(cx.listener(|this, _, _, cx| this.clear(true, cx))),
                    ),
            )
            .child(
                ui::muted("Clear everything also removes the repositories prr downloaded itself. Your own clones are never touched. Checkouts not opened for 14 days are removed automatically.")
                    .text_size(theme::TEXT_SMALL),
            )
            .into_any_element()
    }
}

fn stepper(id: &'static str, value: f32, (default, min, max): (f32, f32, f32), set: fn(&mut Settings, f32), cx: &mut Context<SettingsView>) -> impl IntoElement {
    let step = move |delta: f32| move |_: &ClickEvent, _: &mut Window, cx: &mut App| update(cx, |s| set(s, (value + delta).clamp(min, max)));
    div()
        .flex()
        .items_center()
        .gap(px(6.))
        .child(ui::button(SharedString::from(format!("{id}-dec")), "−").when(value > min, |el| el.on_click(step(-1.))).when(value <= min, |el| el.opacity(0.5)))
        .child(div().w(px(44.)).text_center().child(format!("{value} px")))
        .child(ui::button(SharedString::from(format!("{id}-inc")), "+").when(value < max, |el| el.on_click(step(1.))).when(value >= max, |el| el.opacity(0.5)))
        .when(value != default, |el| {
            el.child(ui::link(SharedString::from(format!("{id}-reset")), "Reset").on_click(move |_, _, cx| update(cx, |s| set(s, default))))
        })
        .map(|el| {
            let _ = cx;
            el
        })
}

fn row(label: &str, help: Option<&str>, control: impl IntoElement) -> impl IntoElement {
    div()
        .flex()
        .gap(px(16.))
        .py(px(10.))
        .border_b_1()
        .border_color(hsla(theme::BORDER))
        .child(
            div()
                .w(px(200.))
                .flex_shrink_0()
                .child(div().child(label.to_string()))
                .children(help.map(|h| div().mt(px(2.)).text_size(theme::TEXT_SMALL).text_color(hsla(theme::MUTED)).child(h.to_string()))),
        )
        .child(div().flex_1().min_w_0().child(control))
}

impl Render for SettingsView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if !self.focus.contains_focused(window, cx)
            && !self.agent_command.read(cx).focus_handle(cx).is_focused(window)
        {
            window.focus(&self.focus, cx);
        }
        let s = get(cx).clone();
        let legacy = crate::backend::agent::config_path();
        let legacy_note = if s.agent_command.is_empty() && legacy.exists() {
            Some(format!("Empty here, so the command in {} is used.", legacy.display()))
        } else {
            None
        };
        // Backdrop + centered dialog.
        div()
            .id("settings-backdrop")
            .absolute()
            .inset_0()
            .flex()
            .items_center()
            .justify_center()
            .bg(gpui::rgba(0x00000080))
            .occlude()
            .on_click(cx.listener(|_, _, _, cx| cx.emit(SettingsEvent::Close)))
            .child(
                div()
                    .id("settings")
                    .track_focus(&self.focus)
                    .key_context("Settings")
                    .on_key_down(cx.listener(|_, ev: &KeyDownEvent, _, cx| {
                        if ev.keystroke.key == "escape" {
                            cx.emit(SettingsEvent::Close);
                        }
                    }))
                    .on_click(|_, _, cx| cx.stop_propagation())
                    .w(px(640.))
                    .max_h(relative(0.85))
                    .overflow_y_scroll()
                    .p(px(20.))
                    .bg(hsla(theme::PANEL))
                    .border_1()
                    .border_color(hsla(theme::BORDER))
                    .rounded(px(8.))
                    .shadow_lg()
                    .text_size(theme::TEXT)
                    .text_color(hsla(theme::FG))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .mb(px(6.))
                            .child(div().flex_1().text_size(px(16.)).font_weight(FontWeight::SEMIBOLD).child("Settings"))
                            .child(ui::link("settings-close", "✕").on_click(cx.listener(|_, _, _, cx| cx.emit(SettingsEvent::Close)))),
                    )
                    .child(row(
                        "Agent command",
                        Some("Runs in the agent panel, in the PR's checkout. Leave empty for Claude Code."),
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(6.))
                            .child(Input::new(&self.agent_command))
                            .child(ui::muted("For example: codex, or gemini. It runs through your login shell and gets the review context as REVIEW_TOOL_* environment variables and an MCP server ($REVIEW_TOOL_MCP_CONFIG, $REVIEW_TOOL_MCP_STDIO). Applies the next time the agent starts.").text_size(theme::TEXT_SMALL))
                            .children(legacy_note.map(|n| ui::muted(n).text_size(theme::TEXT_SMALL))),
                    ))
                    .child(row(
                        "Trust PR checkouts in Claude Code",
                        Some("Skips Claude Code's \"trust this folder?\" for the agent panel."),
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(6.))
                            .child(
                                div()
                                    .id("trust-claude")
                                    .flex()
                                    .items_center()
                                    .gap(px(8.))
                                    .cursor_pointer()
                                    .on_click(|_, _, cx| update(cx, |s| s.trust_claude_checkouts = !s.trust_claude_checkouts))
                                    .child(
                                        div()
                                            .size(px(14.))
                                            .rounded(px(3.))
                                            .border_1()
                                            .border_color(hsla(if s.trust_claude_checkouts { theme::ACCENT } else { theme::MUTED }))
                                            .when(s.trust_claude_checkouts, |el| el.bg(hsla(theme::ACCENT)))
                                            .flex()
                                            .items_center()
                                            .justify_center()
                                            .text_size(px(10.))
                                            .text_color(gpui::white())
                                            .when(s.trust_claude_checkouts, |el| el.child("✓")),
                                    )
                                    .child(if s.trust_claude_checkouts { "On" } else { "Off" }),
                            )
                            .child(ui::muted("The panel's Claude Code is read-only and ignores the repository's own settings, hooks and MCP servers, which is what that prompt protects against. When on, the app marks the PR's repository as trusted in ~/.claude.json before starting it.").text_size(theme::TEXT_SMALL)),
                    ))
                    .child(row("Storage", Some("prr's own copies of repositories and PR checkouts."), self.render_storage(cx)))
                    .child(row("Editor font size", Some("Code in diffs and files."), stepper("editor-font", s.editor_font_size, EDITOR_FONT, |s, v| s.editor_font_size = v, cx)))
                    .child(row("Terminal font size", Some("The agent panel."), stepper("terminal-font", s.terminal_font_size, TERMINAL_FONT, |s, v| s.terminal_font_size = v, cx)))
                    .child(div().pt(px(10.)).text_size(theme::TEXT_SMALL).text_color(hsla(theme::MUTED)).child(format!("Saved in {}", path().display()))),
            )
    }
}
