//! The root view: the home screen, or the PR being reviewed.
use crate::backend::{self, git};
use crate::home::{Home, HomeEvent};
use crate::pr_view::{PrView, PrViewEvent};
use gpui::*;

pub struct AppView {
    home: Entity<Home>,
    pr: Option<Entity<PrView>>,
    _subscriptions: Vec<Subscription>,
}

impl AppView {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let home = cx.new(|cx| Home::new(window, cx));
        let sub = cx.subscribe_in(&home, window, |this, _, event: &HomeEvent, window, cx| match event {
            HomeEvent::Opened { pr, checkout } => this.show_pr(pr.clone(), checkout.clone(), window, cx),
        });
        Self { home, pr: None, _subscriptions: vec![sub] }
    }

    /// Opens a PR (from the command line or another launch).
    pub fn open(&mut self, pr: crate::backend::types::PRRef, window: &mut Window, cx: &mut Context<Self>) {
        self.close_pr(cx);
        self.home.update(cx, |home, cx| home.open(pr.owner, pr.repo, pr.number, pr.local_repo, window, cx));
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
    fn close_pr(&mut self, cx: &mut Context<Self>) {
        let Some(view) = self.pr.take() else { return };
        view.update(cx, |v, _| v.stop_language_servers());
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

impl Render for AppView {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        match &self.pr {
            Some(pr) => div().size_full().child(pr.clone()),
            None => div().size_full().child(self.home.clone()),
        }
    }
}
