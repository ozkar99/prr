//! The root view: the home screen, or the PR being reviewed.
use crate::backend::{self, git};
use crate::home::{Home, HomeEvent};
use crate::pr_view::{PrView, PrViewEvent};
use crate::settings::{OpenSettings, SettingsEvent, SettingsView};
use gpui::*;

pub struct AppView {
    home: Entity<Home>,
    pr: Option<Entity<PrView>>,
    settings: Option<Entity<SettingsView>>,
    focus: FocusHandle,
    /// The PR this window was asked to open (owner, repo, number; lowercase), while loading or open.
    opening: Option<(String, String, u64)>,
    _subscriptions: Vec<Subscription>,
}

impl AppView {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let home = cx.new(|cx| Home::new(window, cx));
        let sub = cx.subscribe_in(&home, window, |this, _, event: &HomeEvent, window, cx| match event {
            HomeEvent::Opened { pr, checkout } => this.show_pr(pr.clone(), checkout.clone(), window, cx),
        });
        let focus = cx.focus_handle();
        // App-wide shortcuts (⌘,) need something focused from the start.
        window.focus(&focus, cx);
        Self { home, pr: None, settings: None, focus, opening: None, _subscriptions: vec![sub] }
    }

    /// Opens a PR (from the command line or another launch).
    pub fn open(&mut self, pr: crate::backend::types::PRRef, window: &mut Window, cx: &mut Context<Self>) {
        self.close_pr(cx);
        self.opening = Some((pr.owner.to_lowercase(), pr.repo.to_lowercase(), pr.number));
        self.home.update(cx, |home, cx| home.open(pr.owner, pr.repo, pr.number, pr.local_repo, pr.local_worktree, window, cx));
    }

    /// Shows the home screen, loading its tool check and PR lists.
    pub fn show_home(&mut self, cx: &mut Context<Self>) {
        self.opening = None;
        self.home.update(cx, |home, cx| home.refresh(cx));
        cx.notify();
    }

    /// While a PR is opening (and nothing needs answering on the home
    /// screen): a plain progress screen, so the home screen's PR lists
    /// aren't loaded or shown on the way.
    fn render_opening(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let (owner, repo, number) = self.opening.clone()?;
        let home = self.home.read(cx);
        if home.needs_answer() {
            return None;
        }
        let (busy, error) = home.opening_status();
        if busy.is_none() && error.is_none() {
            return None;
        }
        use crate::theme::{self, hsla};
        use crate::ui;
        Some(
            div()
                .size_full()
                .flex()
                .flex_col()
                .items_center()
                .justify_center()
                .gap(px(12.))
                .px(px(40.))
                .bg(hsla(theme::BG))
                .text_color(hsla(theme::FG))
                .child(div().text_size(px(16.)).font_weight(FontWeight::SEMIBOLD).child(format!("{owner}/{repo}#{number}")))
                .children(busy.map(|b| ui::muted(b)))
                .children(error.map(|e| {
                    div()
                        .flex()
                        .flex_col()
                        .items_center()
                        .gap(px(10.))
                        .max_w(px(800.))
                        .child(ui::copy_error("opening-error", e))
                        .child(ui::link("opening-back", "← Pull requests").on_click(cx.listener(|this, _, _, cx| this.show_home(cx))))
                }))
                .into_any_element(),
        )
    }

    pub fn pr_view(&self) -> Option<Entity<PrView>> {
        self.pr.clone()
    }

    /// The PR this window shows or is loading.
    pub fn showing(&self, cx: &App) -> Option<(String, String, u64)> {
        match &self.pr {
            Some(pr) => {
                let pr = &pr.read(cx).pr;
                Some((pr.owner.to_lowercase(), pr.repo.to_lowercase(), pr.number))
            }
            None => self.opening.clone().filter(|_| self.home.read(cx).is_busy()),
        }
    }

    /// Your own clone this window's PR is reviewed in, if any.
    pub fn local_clone(&self, cx: &App) -> Option<String> {
        self.pr.as_ref()?.read(cx).checkout.local.as_ref().map(|l| l.path.clone())
    }

    /// On the home screen and not opening anything: a later PR can use this window.
    pub fn is_free(&self, cx: &App) -> bool {
        self.pr.is_none() && !self.home.read(cx).is_busy() && self.settings.is_none()
    }

    fn show_pr(&mut self, pr: crate::backend::types::PRDetails, checkout: crate::backend::types::Checkout, window: &mut Window, cx: &mut Context<Self>) {
        let view = cx.new(|cx| PrView::new(pr, checkout, window, cx));
        let sub = cx.subscribe_in(&view, window, |this, _, event: &PrViewEvent, _, cx| match event {
            PrViewEvent::Close => this.close_pr(cx),
        });
        self._subscriptions.push(sub);
        self.pr = Some(view);
        cx.notify();
    }

    /// Leaves the open PR; if it switched the user's own clone, switches it back.
    pub fn close_pr(&mut self, cx: &mut Context<Self>) {
        self.opening = None;
        let Some(view) = self.pr.take() else { return };
        view.update(cx, |v, cx| v.close(cx));
        let (local, number) = {
            let v = view.read(cx);
            (v.checkout.local.clone(), v.pr.number)
        };
        // A copy-on-write worktree of the user's clone goes away with its window.
        if let Some(clone) = view.read(cx).checkout.worktree_of.clone() {
            let root = view.read(cx).checkout.root.clone();
            let _ = backend::task(async move { git::remove_clone_worktree(&root, &clone).await });
        }
        self.home.update(cx, |home, cx| home.refresh(cx));
        if let Some(local) = local {
            if let (Some(from), Some(pr_branch)) = (local.switched_from.clone(), local.pr_branch.clone()) {
                let path = local.path.clone();
                let fut = backend::task({
                    let (path, from, pr_branch) = (path.clone(), from.clone(), pr_branch.clone());
                    async move { git::restore_local(&path, &from, &pr_branch).await }
                });
                let home = self.home.clone();
                cx.spawn(async move |_, cx| {
                    let notice = match fut.await {
                        Ok(true) => format!("Switched your clone back to {from} (the PR stays on branch {pr_branch})."),
                        Ok(false) => format!("Left your clone at {path} as it is: it's no longer on {pr_branch} with no changes. It was on {from} before PR #{number}."),
                        Err(e) => format!("Couldn't switch your clone back to {from}: {e}"),
                    };
                    home.update(cx, |home, cx| home.set_notice(Some(notice), cx));
                })
                .detach();
            }
        }
        cx.notify();
    }
}

impl AppView {
    fn open_settings(&mut self, _: &OpenSettings, window: &mut Window, cx: &mut Context<Self>) {
        if self.settings.is_some() {
            return;
        }
        let view = cx.new(|cx| SettingsView::new(window, cx));
        let sub = cx.subscribe_in(&view, window, |this, _, ev: &SettingsEvent, window, cx| match ev {
            SettingsEvent::Close => {
                this.settings = None;
                // Back to the screen underneath (keyboard shortcuts need focus there).
                match &this.pr {
                    Some(pr) => {
                        let handle = pr.read(cx).focus_handle();
                        window.focus(&handle, cx);
                    }
                    None => window.focus(&this.focus, cx),
                }
                cx.notify();
            }
        });
        self._subscriptions.push(sub);
        self.settings = Some(view);
        cx.notify();
    }
}

impl Render for AppView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .relative()
            .track_focus(&self.focus)
            .on_action(cx.listener(Self::open_settings))
            .on_action(cx.listener(|this, _: &crate::windows::CloseWindow, window, cx| {
                this.close_pr(cx);
                window.remove_window();
            }))
            .on_action(|_: &crate::windows::MinimizeWindow, window, _| window.minimize_window())
            .child(match &self.pr {
                Some(pr) => pr.clone().into_any_element(),
                None => self.render_opening(cx).unwrap_or_else(|| self.home.clone().into_any_element()),
            })
            .children(self.settings.clone())
    }
}
