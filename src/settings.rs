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
    /// The gh account prr uses for GitHub; empty: gh's active account.
    pub github_account: String,
    /// The gh command prr runs; empty: `gh`, the way your shell runs it.
    pub gh_command: String,
    /// The folder gh runs in; empty: the clone prr last opened a PR in.
    pub gh_folder: String,
    /// The clone prr last opened a PR in (remembered automatically).
    pub last_clone: String,
}

impl Default for Settings {
    fn default() -> Self {
        Self { agent_command: String::new(), editor_font_size: EDITOR_FONT.0, terminal_font_size: TERMINAL_FONT.0, agent_panel_height: 300., sidebar_width: 300., split_ratio: 0.5, trust_claude_checkouts: false, github_account: String::new(), gh_command: String::new(), gh_folder: String::new(), last_clone: String::new() }
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
    /// The folder gh runs in: the one set, or else the last clone.
    pub fn gh_folder_in_use(&self) -> &str {
        if self.gh_folder.trim().is_empty() { &self.last_clone } else { &self.gh_folder }
    }

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
    crate::backend::auth::set_account(&get(cx).github_account);
    crate::backend::auth::set_gh_command(&get(cx).gh_command);
    crate::backend::auth::set_folder(get(cx).gh_folder_in_use());
    let size = get(cx).editor_font_size;
    gpui_component::Theme::global_mut(cx).mono_font_size = px(size);
    gpui_component::Theme::sync_base(cx);
    cx.refresh_windows();
}

// ---------------------------------------------------------------- screen

pub enum SettingsEvent {
    Close,
    /// Another GitHub account was picked: check GitHub access again.
    GitHubAccountChanged,
}

pub struct SettingsView {
    agent_command: Entity<InputState>,
    gh_command: Entity<InputState>,
    gh_folder: Entity<InputState>,
    /// The gh command the account list was loaded with.
    checked_gh: String,
    /// gh's github.com accounts (None: still loading).
    accounts: Option<Vec<crate::backend::auth::Account>>,
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
        let current_gh = get(cx).gh_command.clone();
        let gh_command = cx.new(|cx| {
            let mut state = InputState::new(window, cx).placeholder("gh (run like in your terminal)");
            state.set_value(current_gh, window, cx);
            state
        });
        let gh_sub = cx.subscribe(&gh_command, |this: &mut Self, input, ev: &InputEvent, cx| match ev {
            InputEvent::Change => {
                let value = input.read(cx).value().trim().to_string();
                update(cx, |s| s.gh_command = value);
            }
            // Check the new command once it's typed.
            InputEvent::PressEnter { .. } | InputEvent::Blur => this.gh_changed(cx),
            _ => {}
        });
        let (current_folder, last_clone) = (get(cx).gh_folder.clone(), get(cx).last_clone.clone());
        let gh_folder = cx.new(|cx| {
            let placeholder = if last_clone.is_empty() { "Automatic: the clone you last ran prr in".to_string() } else { format!("Automatic: {last_clone}") };
            let mut state = InputState::new(window, cx).placeholder(placeholder);
            state.set_value(current_folder, window, cx);
            state
        });
        let folder_sub = cx.subscribe(&gh_folder, |this: &mut Self, input, ev: &InputEvent, cx| match ev {
            InputEvent::Change => {
                let value = input.read(cx).value().trim().to_string();
                update(cx, |s| s.gh_folder = value);
            }
            InputEvent::PressEnter { .. } | InputEvent::Blur => this.gh_changed(cx),
            _ => {}
        });
        let mut this = Self { agent_command, gh_command, gh_folder, accounts: None, checked_gh: String::new(), focus: cx.focus_handle(), _subs: vec![sub, gh_sub, folder_sub] };
        this.checked_gh = gh_key(get(cx));
        this.load_accounts(cx);
        this
    }

    fn load_accounts(&mut self, cx: &mut Context<Self>) {
        self.accounts = None;
        let fut = crate::backend::task(crate::backend::auth::accounts());
        cx.spawn(async move |this, cx| {
            let accounts = fut.await;
            this.update(cx, |this, cx| {
                this.accounts = Some(accounts);
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn gh_changed(&mut self, cx: &mut Context<Self>) {
        let current = gh_key(get(cx));
        if current == self.checked_gh {
            return;
        }
        self.checked_gh = current;
        self.load_accounts(cx);
        cx.emit(SettingsEvent::GitHubAccountChanged);
        cx.notify();
    }

    fn pick_account(&mut self, login: String, cx: &mut Context<Self>) {
        if get(cx).github_account == login {
            return;
        }
        update(cx, |s| s.github_account = login);
        cx.emit(SettingsEvent::GitHubAccountChanged);
    }

    fn render_accounts(&self, picked: &str, cx: &mut Context<Self>) -> AnyElement {
        let Some(accounts) = &self.accounts else { return ui::muted("Asking gh…").text_size(theme::TEXT_SMALL).into_any_element() };
        if accounts.is_empty() {
            return ui::muted("gh isn't logged in to github.com, isn't found, or is too old to list accounts. Run: gh auth login").text_size(theme::TEXT_SMALL).into_any_element();
        }
        let active = accounts.iter().find(|a| a.active).map(|a| format!(" (now @{})", a.login)).unwrap_or_default();
        let mut options: Vec<(String, String, Option<&'static str>)> = vec![(String::new(), format!("gh's active account{active}"), None)];
        for a in accounts {
            options.push((a.login.clone(), format!("@{}", a.login), (!a.ok).then_some("gh's token for it has expired: gh auth refresh")));
        }
        // An account picked earlier that gh is no longer logged in to.
        if !picked.is_empty() && !accounts.iter().any(|a| a.login == picked) {
            options.push((picked.to_string(), format!("@{picked}"), Some("gh isn't logged in to it any more")));
        }
        div()
            .flex()
            .flex_col()
            .gap(px(6.))
            .children(options.into_iter().enumerate().map(|(i, (login, label, warning))| {
                let selected = login == picked;
                div()
                    .id(("gh-account", i))
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .cursor_pointer()
                    .on_click(cx.listener(move |this, _, _, cx| this.pick_account(login.clone(), cx)))
                    .child(
                        div()
                            .size(px(14.))
                            .rounded_full()
                            .border_1()
                            .border_color(hsla(if selected { theme::ACCENT } else { theme::MUTED }))
                            .flex()
                            .items_center()
                            .justify_center()
                            .when(selected, |el| el.child(div().size(px(6.)).rounded_full().bg(hsla(theme::ACCENT)))),
                    )
                    .child(label)
                    .children(warning.map(|w| div().text_size(theme::TEXT_SMALL).text_color(hsla(theme::WARN)).child(w)))
            }))
            .into_any_element()
    }
}

/// What decides which gh runs, and where: a change means checking GitHub access again.
fn gh_key(s: &Settings) -> String {
    format!("{}\n{}", s.gh_command, s.gh_folder_in_use())
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
            && !self.gh_command.read(cx).focus_handle(cx).is_focused(window)
            && !self.gh_folder.read(cx).focus_handle(cx).is_focused(window)
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
                        "GitHub account",
                        Some("prr uses the GitHub CLI's login. Log in to more accounts with gh auth login."),
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(6.))
                            .child(self.render_accounts(&s.github_account, cx))
                            .child(ui::muted("Used for GitHub and for prr's own clones. Your gh and git settings don't change.").text_size(theme::TEXT_SMALL)),
                    ))
                    .child(row(
                        "gh command",
                        Some("Leave empty to run gh the way your terminal does."),
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(6.))
                            .child(Input::new(&self.gh_command))
                            .child(ui::muted("prr runs gh through your login shell, so a wrapper function or alias around gh (one that sets a company token, say) is used too. Set a path (/usr/local/bin/corp-gh) or another command name to run that instead. Press Enter to check it.").text_size(theme::TEXT_SMALL)),
                    ))
                    .child(row(
                        "Run gh in",
                        Some("A clone of the repo you review."),
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(6.))
                            .child(Input::new(&self.gh_folder))
                            .child(ui::muted("gh runs as if you had cd'd into this folder in a terminal, so whatever sets its environment there (direnv, mise, your company's per-repo settings) applies. Leave empty to use the clone you last ran prr in. Press Enter to check it.").text_size(theme::TEXT_SMALL)),
                    ))
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
                    .child(row("Editor font size", Some("Code in diffs and files."), stepper("editor-font", s.editor_font_size, EDITOR_FONT, |s, v| s.editor_font_size = v, cx)))
                    .child(row("Terminal font size", Some("The agent panel."), stepper("terminal-font", s.terminal_font_size, TERMINAL_FONT, |s, v| s.terminal_font_size = v, cx)))
                    .child(div().pt(px(10.)).text_size(theme::TEXT_SMALL).text_color(hsla(theme::MUTED)).child(format!("Saved in {}", path().display()))),
            )
    }
}
