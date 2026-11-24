//! The home screen (web: App.tsx + SetupCheck.tsx + Storage.tsx): setup check,
//! "open a PR" field, the PR queue, storage, and the "switch your clone?" dialog.
use crate::backend::{self, cleanup::DiskUsage, deps::ToolStatus, git, github, types::*};
use crate::theme::{self, hsla};
use crate::ui;
use futures::StreamExt as _;
use gpui::prelude::FluentBuilder as _;
use gpui::*;
use gpui_component::input::{Input, InputEvent, InputState};

pub enum HomeEvent {
    Opened { pr: PRDetails, checkout: Checkout },
}

/// Waiting for the user to OK switching their own clone to the PR.
#[derive(Clone)]
struct PendingSwitch {
    pr: PRDetails,
    path: String,
    current: String,
    dirty: bool,
}

pub struct Home {
    input: Entity<InputState>,
    queue: Option<Result<Vec<PRSummary>, String>>,
    tools: Option<Vec<ToolStatus>>,
    show_optional: bool,
    checking: bool,
    usage: Option<DiskUsage>,
    clearing: bool,
    busy: Option<String>,
    error: Option<String>,
    notice: Option<String>,
    pending_switch: Option<PendingSwitch>,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<HomeEvent> for Home {}

impl Home {
    /// Whether it's busy opening a PR.
    pub fn is_busy(&self) -> bool {
        self.busy.is_some()
    }

    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let input = cx.new(|cx| InputState::new(window, cx).placeholder("Paste a PR URL or owner/repo#123"));
        let subscription = cx.subscribe_in(&input, window, |this, _, event: &InputEvent, window, cx| {
            if let InputEvent::PressEnter { .. } = event {
                this.submit(window, cx);
            }
        });
        let mut home = Self {
            input,
            queue: None,
            tools: None,
            show_optional: false,
            checking: false,
            usage: None,
            clearing: false,
            busy: None,
            error: None,
            notice: None,
            pending_switch: None,
            _subscriptions: vec![subscription],
        };
        home.check_tools(cx);
        home.load_usage(cx);
        home
    }

    /// Shown on the home screen after leaving a PR (e.g. "Switched your clone back to main.").
    pub fn set_notice(&mut self, notice: Option<String>, cx: &mut Context<Self>) {
        self.notice = notice;
        cx.notify();
    }

    pub fn refresh(&mut self, cx: &mut Context<Self>) {
        self.check_tools(cx);
        self.load_usage(cx);
    }

    // ---- Setup check (git + a logged-in gh are required; the queue waits for them) ----

    fn check_tools(&mut self, cx: &mut Context<Self>) {
        self.checking = true;
        let fut = backend::task(backend::deps::check());
        cx.spawn(async move |this, cx| {
            let tools = fut.await;
            let required_ok = tools.iter().all(|t| !t.required || t.ok);
            this.update(cx, |this, cx| {
                this.tools = Some(tools);
                this.checking = false;
                if required_ok {
                    this.load_queue(cx);
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    fn load_queue(&mut self, cx: &mut Context<Self>) {
        let fut = backend::task(github::review_queue());
        cx.spawn(async move |this, cx| {
            let result = fut.await;
            this.update(cx, |this, cx| {
                this.queue = Some(result);
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn load_usage(&mut self, cx: &mut Context<Self>) {
        let fut = backend::blocking(backend::cleanup::usage);
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
        self.clearing = true;
        let fut = backend::task(backend::cleanup::clear(include_clones));
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

    // ---- Opening a PR ----

    fn submit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let value = self.input.read(cx).value().to_string();
        match parse_pr_ref(&value) {
            Some(r) => self.open(r.owner, r.repo, r.number, None, false, window, cx),
            None => {
                self.error = Some("Enter a PR URL or owner/repo#number".into());
                cx.notify();
            }
        }
    }

    pub fn open(&mut self, owner: String, repo: String, number: u64, local_repo: Option<String>, local_worktree: bool, _window: &mut Window, cx: &mut Context<Self>) {
        // Remember the clone: gh runs there for this repo from now on.
        if let Some(path) = local_repo.clone() {
            crate::backend::gh::remember_clone(&owner, &repo, &path);
        }
        self.pending_switch = None;
        self.notice = None;
        self.error = None;
        self.busy = Some(format!("Loading {owner}/{repo}#{number}…"));
        cx.notify();
        let fut = backend::task(async move { github::get_pr(&owner, &repo, number).await });
        cx.spawn(async move |this, cx| match fut.await {
            Ok(pr) => {
                this.update(cx, |this, cx| this.checkout(pr, local_repo, local_worktree, false, cx)).ok();
            }
            Err(e) => {
                this.update(cx, |this, cx| {
                    this.busy = None;
                    this.error = Some(e);
                    cx.notify();
                })
                .ok();
            }
        })
        .detach();
    }

    fn checkout(&mut self, pr: PRDetails, local_repo: Option<String>, local_worktree: bool, allow_switch: bool, cx: &mut Context<Self>) {
        self.pending_switch = None;
        self.error = None;
        self.busy = Some(format!("Loading {}/{}#{}…", pr.owner, pr.repo, pr.number));
        cx.notify();

        // Progress messages ("Cloning…", "Fetching PR #12…") stream back while it runs.
        let (progress_tx, mut progress_rx) = futures::channel::mpsc::unbounded::<String>();
        let pr_for_task = pr.clone();
        let fut = backend::task(async move {
            git::checkout(&pr_for_task, local_repo.as_deref(), local_worktree, allow_switch, move |msg| {
                let _ = progress_tx.unbounded_send(msg);
            })
            .await
        });
        cx.spawn(async move |this, cx| {
            while let Some(msg) = progress_rx.next().await {
                if this.update(cx, |this, cx| { this.busy = Some(msg); cx.notify(); }).is_err() {
                    break;
                }
            }
        })
        .detach();
        cx.spawn(async move |this, cx| {
            let result = fut.await;
            this.update(cx, |this, cx| {
                this.busy = None;
                match result {
                    Ok(CheckoutResult::Ready(checkout)) => {
                        cx.emit(HomeEvent::Opened { pr, checkout });
                    }
                    Ok(CheckoutResult::NeedsSwitch { path, current, dirty }) => {
                        this.pending_switch = Some(PendingSwitch { pr, path, current, dirty });
                    }
                    Err(e) => this.error = Some(e),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    // ---- Rendering ----

    fn render_setup(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let tools = self.tools.as_ref()?;
        let required: Vec<&ToolStatus> = tools.iter().filter(|t| t.required && !t.ok).collect();
        let optional: Vec<&ToolStatus> = tools.iter().filter(|t| !t.required).collect();
        let missing: Vec<&&ToolStatus> = optional.iter().filter(|t| !t.ok).collect();
        let checking = self.checking;

        let tool_list = |items: Vec<&ToolStatus>| {
            div().flex().flex_col().gap(px(6.)).w_full().children(items.into_iter().map(|t| {
                let (mark, color) = if t.ok { ("✓", theme::ADD) } else if t.required { ("✗", theme::DEL) } else { ("✗", theme::MUTED) };
                div()
                    .flex()
                    .flex_wrap()
                    .items_baseline()
                    .gap(px(8.))
                    .child(div().text_color(hsla(color)).child(mark))
                    .child(div().font_weight(FontWeight::BOLD).child(t.name.clone()))
                    .child(ui::muted(t.purpose.clone()))
                    .when_some(t.problem.clone().filter(|_| !t.ok), |el, p| {
                        el.child(div().text_color(hsla(theme::WARN)).text_size(theme::TEXT_SMALL).child(p))
                    })
                    .when_some(t.fix.clone(), |el, fix| el.child(ui::copy_command(SharedString::from(format!("fix-{}", t.name)), fix)))
            }))
        };
        let card = || {
            div()
                .flex()
                .flex_col()
                .gap(px(10.))
                .items_start()
                .p(px(12.))
                .mt(px(8.))
                .rounded(px(8.))
                .border_1()
                .border_color(hsla(theme::BORDER))
                .bg(hsla(theme::PANEL))
        };

        let el = div().mb(px(14.));
        if !required.is_empty() {
            return Some(
                el.child(
                    card()
                        .border_color(hsla(theme::DEL))
                        .child(div().font_weight(FontWeight::SEMIBOLD).child("prr needs a couple of things first"))
                        .child(tool_list(required))
                        .child(
                            ui::primary_button("check-again", if checking { "Checking…" } else { "Check again" })
                                .on_click(cx.listener(|this, _, _, cx| this.check_tools(cx))),
                        ),
                )
                .into_any_element(),
            );
        }
        let summary = if missing.is_empty() {
            "All tools found.".to_string()
        } else {
            format!(
                "{} optional tool{} not installed: {}.",
                missing.len(),
                if missing.len() == 1 { "" } else { "s" },
                missing.iter().map(|t| t.name.as_str()).collect::<Vec<_>>().join(", ")
            )
        };
        Some(
            el.child(
                div()
                    .flex()
                    .gap(px(12.))
                    .items_baseline()
                    .text_size(theme::TEXT_SMALL)
                    .child(ui::muted(summary))
                    .child(
                        ui::link("tools-details", if self.show_optional { "Hide" } else { "Details" })
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.show_optional = !this.show_optional;
                                cx.notify();
                            })),
                    )
                    .child(
                        ui::link_enabled("tools-recheck", if checking { "Checking…" } else { "Check again" }, !checking)
                            .on_click(cx.listener(|this, _, _, cx| this.check_tools(cx))),
                    ),
            )
            .when(self.show_optional, |el| el.child(card().child(tool_list(optional))))
            .into_any_element(),
        )
    }

    fn render_switch(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let p = self.pending_switch.clone()?;
        let number = p.pr.number;
        let text = if p.dirty {
            div()
                .flex()
                .flex_wrap()
                .gap(px(4.))
                .child("Your clone at")
                .child(ui::code(p.path.clone()))
                .child(format!("has uncommitted changes, so it can't be switched to PR #{number}. Commit or stash them, or review it in a separate checkout."))
        } else {
            div()
                .flex()
                .flex_wrap()
                .gap(px(4.))
                .child("Your clone at")
                .child(ui::code(p.path.clone()))
                .child("is on")
                .child(ui::code(p.current.clone()))
                .child(format!(". Switch it to a new branch"))
                .child(ui::code(format!("prr/pr-{number}")))
                .child(format!("at PR #{number} to review it there? It switches back to"))
                .child(ui::code(p.current.clone()))
                .child("when you close the PR; the branch stays.")
        };
        let (p1, p2, p3) = (p.clone(), p.clone(), p.clone());
        Some(
            div()
                .flex()
                .flex_col()
                .gap(px(10.))
                .p(px(12.))
                .my(px(10.))
                .rounded(px(8.))
                .border_1()
                .border_color(hsla(theme::BORDER))
                .bg(hsla(theme::PANEL))
                .child(text)
                .child(
                    div()
                        .flex()
                        .gap(px(8.))
                        .items_center()
                        .when(!p.dirty, |el| {
                            el.child(ui::primary_button("switch-yes", "Switch my clone").on_click(cx.listener(move |this, _, _, cx| {
                                this.checkout(p1.pr.clone(), Some(p1.path.clone()), false, true, cx)
                            })))
                        })
                        .child(ui::button("switch-separate", "Use a separate checkout").on_click(cx.listener(move |this, _, _, cx| {
                            this.checkout(p2.pr.clone(), Some(p2.path.clone()), true, false, cx)
                        })))
                        .when(p.dirty, |el| {
                            el.child(ui::button("switch-recheck", "Check again").on_click(cx.listener(move |this, _, _, cx| {
                                this.checkout(p3.pr.clone(), Some(p3.path.clone()), false, false, cx)
                            })))
                        })
                        .child(ui::link("switch-cancel", "Cancel").on_click(cx.listener(|this, _, _, cx| {
                            this.pending_switch = None;
                            cx.notify();
                        }))),
                )
                .into_any_element(),
        )
    }

    fn render_queue(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let busy = self.busy.is_some();
        let mut col = div().flex().flex_col();
        let Some(queue) = &self.queue else {
            return col.child(ui::muted("Loading…"));
        };
        let items = match queue {
            Ok(items) => items.clone(),
            Err(e) => return col.child(ui::copy_error("queue-error", format!("Couldn't load pull requests: {e}"))),
        };
        for (label, reason) in [("Review requested", "review-requested"), ("Your pull requests", "authored")] {
            let group: Vec<PRSummary> = items.iter().filter(|p| p.reason == reason).cloned().collect();
            col = col.child(
                div()
                    .mt(px(20.))
                    .mb(px(6.))
                    .text_size(theme::TEXT_SMALL)
                    .text_color(hsla(theme::MUTED))
                    .child(label.to_uppercase()),
            );
            if group.is_empty() {
                col = col.child(ui::muted("None"));
                continue;
            }
            let count = group.len();
            col = col.child(
                div()
                    .flex()
                    .flex_col()
                    .rounded(px(6.))
                    .border_1()
                    .border_color(hsla(theme::BORDER))
                    .children(group.into_iter().enumerate().map(|(i, p)| {
                        let id = SharedString::from(format!("pr-{}-{}-{}", p.owner, p.repo, p.number));
                        let (owner, repo, number) = (p.owner.clone(), p.repo.clone(), p.number);
                        div()
                            .id(id)
                            .flex()
                            .items_center()
                            .gap(px(12.))
                            .px(px(12.))
                            .py(px(10.))
                            .when(i + 1 < count, |el| el.border_b_1().border_color(hsla(theme::BORDER)))
                            .when(!busy, |el| el.cursor_pointer().hover(|s| s.bg(hsla(theme::HOVER))))
                            .on_click(cx.listener(move |this, _, window, cx| {
                                if this.busy.is_none() {
                                    this.open(owner.clone(), repo.clone(), number, None, false, window, cx);
                                }
                            }))
                            .child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .flex_1()
                                    .gap(px(2.))
                                    .child(
                                        div()
                                            .text_size(theme::TEXT_SMALL)
                                            .text_color(hsla(theme::MUTED))
                                            .child(format!("{}/{}#{}", p.owner, p.repo, p.number)),
                                    )
                                    .child(
                                        div()
                                            .flex()
                                            .gap(px(6.))
                                            .font_weight(FontWeight::MEDIUM)
                                            .when(p.draft, |el| {
                                                el.child(
                                                    div()
                                                        .text_size(theme::TEXT_TINY)
                                                        .text_color(hsla(theme::MUTED))
                                                        .border_1()
                                                        .border_color(hsla(theme::MUTED))
                                                        .rounded(px(10.))
                                                        .px(px(6.))
                                                        .child("Draft"),
                                                )
                                            })
                                            .child(p.title.clone()),
                                    ),
                            )
                            .child(
                                div()
                                    .text_size(theme::TEXT_SMALL)
                                    .text_color(hsla(theme::MUTED))
                                    .child(format!("{} · {}", p.author, ui::short_date(&p.updated_at))),
                            )
                    })),
            );
        }
        col
    }

    fn render_storage(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let u = self.usage.as_ref()?;
        if u.worktrees == 0 && u.repos == 0 {
            return None;
        }
        let clearing = self.clearing;
        Some(
            div()
                .mt(px(28.))
                .flex()
                .flex_col()
                .child(
                    div()
                        .mb(px(6.))
                        .text_size(theme::TEXT_SMALL)
                        .text_color(hsla(theme::MUTED))
                        .child("STORAGE"),
                )
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(8.))
                        .child(
                            div()
                                .flex()
                                .gap(px(4.))
                                .child(format!("{} · {}", ui::plural(u.worktrees, "PR checkout"), ui::size(u.worktree_bytes)))
                                .child(ui::muted(format!("· {} · {}", ui::plural(u.repos, "cloned repo"), ui::size(u.repo_bytes)))),
                        )
                        .child(ui::spacer())
                        .child(
                            ui::button("clear-checkouts", "Clear PR checkouts")
                                .when(clearing || u.worktrees == 0, |el| el.opacity(0.5))
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    if !this.clearing {
                                        this.clear(false, cx)
                                    }
                                })),
                        )
                        .child(
                            ui::button("clear-everything", "Clear everything")
                                .when(clearing, |el| el.opacity(0.5))
                                .on_click(cx.listener(|this, _, _, cx| {
                                    if !this.clearing {
                                        this.clear(true, cx)
                                    }
                                })),
                        ),
                )
                .child(
                    div()
                        .mt(px(6.))
                        .text_size(theme::TEXT_SMALL)
                        .text_color(hsla(theme::MUTED))
                        .flex()
                        .flex_wrap()
                        .gap(px(4.))
                        .child("Checkouts not opened for 14 days are removed automatically. Run")
                        .child(ui::code("prr"))
                        .child("inside your own clone of a repo to review its PRs there, with no extra copy."),
                )
                .into_any_element(),
        )
    }
}

impl Render for Home {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let busy = self.busy.is_some();
        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(hsla(theme::BG))
            .text_color(hsla(theme::FG))
            .text_size(theme::TEXT)
            // Room for the traffic lights; the transparent title bar keeps it draggable.
            .child(
                div().h(theme::TITLEBAR).flex_shrink_0().flex().items_center().justify_end().px(px(16.)).child(
                    ui::link("open-settings", "Settings")
                        .on_click(|_, window, cx| window.dispatch_action(Box::new(crate::settings::OpenSettings), cx)),
                ),
            )
            .child(
                div()
                    .id("home-scroll")
                    .flex_1()
                    .overflow_y_scroll()
                    .child(
                        div()
                            .w_full()
                            .max_w(px(980.))
                            .mx_auto()
                            .px(px(32.))
                            .pt(px(8.))
                            .pb(px(32.))
                            .flex()
                            .flex_col()
                            .children(self.render_setup(cx))
                            .child(
                                div()
                                    .flex()
                                    .gap(px(8.))
                                    .child(div().flex_1().child(Input::new(&self.input).disabled(busy)))
                                    .child(
                                        ui::primary_button("open", "Open")
                                            .px(px(16.))
                                            .flex()
                                            .items_center()
                                            .when(busy, |el| el.opacity(0.5))
                                            .on_click(cx.listener(|this, _, window, cx| {
                                                if this.busy.is_none() {
                                                    this.submit(window, cx)
                                                }
                                            })),
                                    ),
                            )
                            .when_some(self.busy.clone(), |el, b| el.child(ui::muted(b).my(px(8.))))
                            .when_some(self.error.clone(), |el, e| el.child(div().my(px(8.)).child(ui::copy_error("open-error", e))))
                            .when_some(self.notice.clone(), |el, n| el.child(ui::muted(n).my(px(8.))))
                            .children(self.render_switch(cx))
                            .child(
                                div()
                                    .mt(px(24.))
                                    .flex()
                                    .items_baseline()
                                    .gap(px(12.))
                                    .child(div().text_size(px(16.)).font_weight(FontWeight::BOLD).child("Pull requests"))
                                    .child(ui::link("refresh", "Refresh").on_click(cx.listener(|this, _, _, cx| {
                                        this.queue = None;
                                        this.load_queue(cx);
                                        cx.notify();
                                    }))),
                            )
                            .child(self.render_queue(cx))
                            .children(self.render_storage(cx)),
                    ),
            )
    }
}
