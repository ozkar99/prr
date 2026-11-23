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
        self.home.update(cx, |home, cx| home.open(pr.owner, pr.repo, pr.number, pr.local_repo, window, cx));
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
        let (local, head_sha, number) = {
            let v = view.read(cx);
            (v.checkout.local.clone(), v.checkout.head_sha.clone(), v.pr.number)
        };
        self.home.update(cx, |home, cx| home.refresh(cx));
        if let Some(local) = local {
            if let Some(from) = local.switched_from.clone() {
                let path = local.path.clone();
                let fut = backend::task({
                    let (path, from, head_sha) = (path.clone(), from.clone(), head_sha.clone());
                    async move { git::restore_local(&path, &from, &head_sha).await }
                });
                let home = self.home.clone();
                cx.spawn(async move |_, cx| {
                    let notice = match fut.await {
                        Ok(true) => format!("Switched your clone back to {from}."),
                        Ok(false) => format!("Left your clone at {path} as it is: it changed after switching to PR #{number} (it was on {from})."),
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
                None => self.home.clone().into_any_element(),
            })
            .children(self.settings.clone())
    }
}
