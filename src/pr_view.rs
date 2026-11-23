//! A PR under review (web: PRView.tsx): header, sidebar (conversation + changed
//! files), and the main column (diff / conversation, agent panel below).
use crate::backend::{self, git, types::*};
use crate::highlight;
use crate::theme::{self, hsla};
use crate::ui;
use gpui::prelude::FluentBuilder as _;
use gpui::*;
use crate::diffview::{CodeSelection, Diff, DiffChangeIndicator, DiffEvent, DiffFile, DiffHunkSeparator, DiffLinePosition, DiffMode, DiffSide, DiffState, TokenHit};
use crate::backend::mcp;
use crate::language::{self, Language, ServerStatus};
use crate::lsp::{self, LspClient};
use crate::terminal::{self, TerminalEvent, TerminalView};
use crate::review::{Conversation, Review, ReviewPanel, ReviewPanelEvent, ThreadView, ThreadZone};
use crate::diffview::{DiffAnnotation, DiffLineRange};
use std::cell::RefCell;
use std::collections::HashSet;
use std::rc::Rc;
use serde_json::{Value, json};
use gpui_component::menu::ContextMenuExt as _;
use gpui_component::text::TextView;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

actions!(review_tool, [GoBack, GoToDefinition, PeekDefinition, FindReferences, ClosePopup, ToggleAgent, AskAgent, ExplainAgent, AddComment]);

pub fn init(cx: &mut App) {
    cx.bind_keys([
        // Inside the agent's terminal these keys go to the agent (as in the web version).
        KeyBinding::new("ctrl--", GoBack, Some("PrView && !Terminal")),
        KeyBinding::new("cmd-j", ToggleAgent, Some("PrView")),
        KeyBinding::new("cmd-l", AskAgent, Some("PrView && !Terminal")),
        KeyBinding::new("cmd-shift-l", ExplainAgent, Some("PrView && !Terminal")),
        KeyBinding::new("cmd-alt-c", AddComment, Some("PrView && !Terminal")),
        KeyBinding::new("f12", GoToDefinition, None),
        KeyBinding::new("shift-f12", FindReferences, None),
        KeyBinding::new("alt-f12", PeekDefinition, None),
        KeyBinding::new("escape", ClosePopup, Some("PrView")),
    ]);
}

pub enum PrViewEvent {
    /// "← Pull requests": back to the home screen.
    Close,
}

/// What the main column is showing: a changed file's diff, or a plain file
/// (opened by go-to-definition) by absolute path.
#[derive(Clone, PartialEq)]
enum Shown {
    File(usize),
    Plain(String),
}

/// A place to come back to with Ctrl+-.
#[derive(Clone)]
struct NavEntry {
    shown: Shown,
    line: Option<(DiffSide, usize)>,
}

/// A language server in this window: its status, and the client once it runs.
struct ServerEntry {
    status: ServerStatus,
    client: Option<Arc<LspClient>>,
    /// What the language wants shown next to the status (see `ServerSpec::note`).
    note: Option<(String, String)>,
}

/// A list of locations to pick from (several definitions, or references),
/// like VS Code's peek.
struct Peek {
    point: Point<Pixels>,
    title: SharedString,
    items: Vec<PeekItem>,
    /// The item previewed on the left.
    selected: usize,
}

struct PeekItem {
    abs: String,
    /// Path shown (relative to the checkout when inside it).
    label: String,
    /// 1-based.
    line: usize,
    text: String,
}

struct Hover {
    point: Point<Pixels>,
    markdown: SharedString,
}

/// What the main column shows for the selected file.
enum FileView {
    Loading,
    Diff(Entity<DiffState>),
    /// No textual change to show (binary file, mode change, …), or an error.
    Message(String),
}

#[derive(Clone, Copy, PartialEq)]
enum MainView {
    Code,
    Conversation,
}

type Zones = Rc<RefCell<HashMap<String, Entity<ThreadZone>>>>;

fn zone_key(path: &str, side: &str, line: u64) -> String {
    format!("{side}:{line}:{path}")
}

fn side_name(side: DiffSide) -> &'static str {
    if side == DiffSide::Original { "LEFT" } else { "RIGHT" }
}

fn side_of(name: &str) -> DiffSide {
    if name == "LEFT" { DiffSide::Original } else { DiffSide::Modified }
}

enum AgentStatus {
    Idle,
    Running,
    Exited,
    Failed(String),
}

pub struct PrView {
    pub pr: PRDetails,
    pub checkout: Checkout,
    /// What the main column shows (a changed file's index, or a plain file).
    shown: Option<Shown>,
    file_view: FileView,
    /// Ignores results of loads that were superseded by a later selection.
    load_seq: usize,
    back_stack: Vec<NavEntry>,
    /// The line last scrolled to, recorded for the back stack.
    current_line: Option<(DiffSide, usize)>,
    /// Language servers by language id.
    lsp: HashMap<String, ServerEntry>,
    hover: Option<Hover>,
    peek: Option<Peek>,
    /// Identifies the latest hover request; older ones are dropped.
    hover_seq: usize,
    hover_over_popup: bool,
    /// The symbol last clicked, for F12.
    last_token: Option<TokenHit>,
    /// Source lines of files used for LSP column conversion, by absolute path.
    file_lines: HashMap<String, Arc<Vec<String>>>,
    focus: FocusHandle,
    _diff_subscription: Option<Subscription>,
    /// Debounces selection reports to the agent context.
    selection_seq: usize,
    review: Entity<Review>,
    /// Thread areas under diff lines, by zone_key (shared with the diff's annotation renderer).
    zones: Zones,
    view: MainView,
    conversation: Entity<Conversation>,
    review_panel: Option<Entity<ReviewPanel>>,
    /// Files whose thread list in the sidebar is collapsed.
    collapsed_files: HashSet<String>,
    show_outdated: bool,
    outdated_views: HashMap<String, Entity<ThreadView>>,
    _subs: Vec<Subscription>,
    agent_name: String,
    agent_open: bool,
    agent_height: f32,
    agent: Option<Entity<TerminalView>>,
    agent_status: AgentStatus,
    /// (window y, height) while the splitter is dragged.
    splitter_drag: Option<(Pixels, f32)>,
    sidebar_width: f32,
    /// (window x, width) while the sidebar's edge is dragged.
    sidebar_drag: Option<(Pixels, f32)>,
    split_ratio: f32,
    split_drag: bool,
    /// Where the diff is drawn (for the split divider).
    code_bounds: Rc<std::cell::Cell<Bounds<Pixels>>>,
    /// What the reviewer last selected or clicked (for ⌘L / ⌘⇧L).
    last_selection: Option<Value>,
    /// The symbol under the pointer (for the right-click menu).
    hovered_token: Option<TokenHit>,
    /// This window's MCP session: its agent sees only this PR and selection.
    mcp: Result<mcp::Session, String>,
    /// The agent's requests to act on GitHub, waiting for the user's confirmation (first is shown).
    agent_actions: std::collections::VecDeque<mcp::ActionRequest>,
    /// The confirmed action is running.
    agent_action_busy: bool,
    /// Dev driver only: confirmed actions don't reach GitHub.
    #[cfg(debug_assertions)]
    pub dry_run_actions: bool,
}

impl EventEmitter<PrViewEvent> for PrView {}

fn status_letter(status: &str) -> (&'static str, u32) {
    match status {
        "added" => ("A", theme::ADD),
        "removed" => ("D", theme::DEL),
        "renamed" => ("R", theme::LINK),
        "copied" => ("C", theme::LINK),
        "modified" | "changed" => ("M", theme::WARN),
        _ => ("?", theme::MUTED),
    }
}

impl PrView {
    pub fn new(pr: PRDetails, checkout: Checkout, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let first = pr.files.iter().position(|f| f.status != "removed").or(if pr.files.is_empty() { None } else { Some(0) });
        let review = cx.new(|cx| Review::new(&pr, cx));
        let review_sub = cx.observe(&review, |_, _, cx| cx.notify());
        let conversation = cx.new(|cx| Conversation::new(review.clone(), pr.clone(), cx));
        let mut view = Self {
            pr,
            checkout,
            shown: None,
            file_view: FileView::Message(String::new()),
            load_seq: 0,
            back_stack: Vec::new(),
            current_line: None,
            lsp: HashMap::new(),
            hover: None,
            peek: None,
            hover_seq: 0,
            hover_over_popup: false,
            last_token: None,
            file_lines: HashMap::new(),
            focus: cx.focus_handle(),
            _diff_subscription: None,
            selection_seq: 0,
            review: review.clone(),
            zones: Rc::new(RefCell::new(HashMap::new())),
            view: MainView::Code,
            conversation,
            review_panel: None,
            collapsed_files: HashSet::new(),
            show_outdated: false,
            outdated_views: HashMap::new(),
            _subs: vec![review_sub],
            agent_name: crate::backend::agent::info().name,
            agent_open: false,
            agent_height: crate::settings::get(cx).agent_panel_height,
            agent: None,
            agent_status: AgentStatus::Idle,
            splitter_drag: None,
            sidebar_width: crate::settings::get(cx).sidebar_width,
            sidebar_drag: None,
            split_ratio: crate::settings::get(cx).split_ratio,
            split_drag: false,
            code_bounds: Default::default(),
            last_selection: None,
            hovered_token: None,
            mcp: mcp::new_session(),
            agent_actions: Default::default(),
            agent_action_busy: false,
            #[cfg(debug_assertions)]
            dry_run_actions: false,
        };
        // The agent sees the PR and the selection through the app's MCP tools.
        let pr = &view.pr;
        let review = json!({
            "root": view.checkout.root,
            "mergeBase": view.checkout.merge_base,
            "pr": {
                "repo": format!("{}/{}", pr.owner, pr.repo),
                "number": pr.number,
                "title": pr.title,
                "url": pr.url,
                "author": pr.author,
                "baseBranch": pr.base_ref,
                "description": pr.body,
                "files": pr.files.iter().map(|f| json!({ "path": f.path, "status": f.status, "additions": f.additions, "deletions": f.deletions })).collect::<Vec<_>>(),
            }
        });
        if let Ok(session) = &view.mcp {
            session.set_review(review);
        }
        view.start_language_servers(cx);
        // The agent's review actions (comment, submit, …) arrive here.
        if let Ok(session) = &view.mcp {
            let mut rx = session.actions();
            cx.spawn_in(window, async move |this, cx| {
                use futures::StreamExt;
                while let Some(request) = rx.next().await {
                    if this.update_in(cx, |this, window, cx| this.on_agent_action(request, window, cx)).is_err() {
                        break;
                    }
                }
            })
            .detach();
        }
        // So the PR's shortcuts (⌘J, ⌘L, F12, …) work before anything is clicked.
        window.focus(&view.focus, cx);
        if let Some(ix) = first {
            view.show(Shown::File(ix), None, false, cx);
        }
        view
    }

    /// Index of the changed file shown, if a diff is shown (for the sidebar).
    fn selected(&self) -> Option<usize> {
        match &self.shown {
            Some(Shown::File(ix)) => Some(*ix),
            _ => None,
        }
    }

    pub fn focus_handle(&self) -> FocusHandle {
        self.focus.clone()
    }

    pub fn select_file(&mut self, ix: usize, cx: &mut Context<Self>) {
        self.view = MainView::Code;
        self.show(Shown::File(ix), None, true, cx);
    }

    fn open_conversation(&mut self, target: Option<String>, cx: &mut Context<Self>) {
        self.view = MainView::Conversation;
        self.conversation.update(cx, |c, cx| c.show(target, cx));
        cx.notify();
    }

    fn open_thread(&mut self, file_ix: usize, thread: &ReviewThread, cx: &mut Context<Self>) {
        self.view = MainView::Code;
        match thread.line {
            None => {
                self.show_outdated = true;
                self.show(Shown::File(file_ix), None, true, cx);
            }
            Some(line) => {
                // Put the thread a few lines below the top, like revealing it in Monaco.
                let top = (line as usize).saturating_sub(5).max(1);
                self.show(Shown::File(file_ix), Some((side_of(&thread.side), top)), true, cx);
            }
        }
    }

    /// Opens a comment box under `range`'s last line (GitHub only accepts lines in the patch).
    fn add_comment(&mut self, range: &DiffLineRange, window: &mut Window, cx: &mut Context<Self>) {
        let Some(Shown::File(ix)) = self.shown.clone() else { return };
        let Some(file) = self.pr.files.get(ix).cloned() else { return };
        let side = side_name(range.side());
        let (start, end) = (range.start().min(range.end()) as u64, range.start().max(range.end()) as u64);
        let ok = |a: u64, b: u64| match &file.commentable {
            None => true,
            Some(c) => (if side == "LEFT" { &c.left } else { &c.right }).iter().any(|&(x, y)| a >= x && b <= y),
        };
        let (start_line, line) = if end > start && ok(start, end) { (Some(start), end) } else { (None, end) };
        if !ok(line, line) {
            return;
        }
        let key = zone_key(&file.path, side, line);
        let zone = self.zone(&key, &file.path, side, line, cx);
        zone.update(cx, |z, cx| z.open_composer(start_line, window, cx));
        cx.notify();
    }

    /// "Add Review Comment" (⌘⌥C, right-click menu): on the selection, or the clicked line.
    fn add_comment_at_cursor(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(Shown::File(ix)) = self.shown.clone() else { return };
        let Some(path) = self.pr.files.get(ix).map(|f| f.path.clone()) else { return };
        let range = match &self.last_selection {
            Some(sel) => {
                let side = if sel["side"] == "old" { DiffSide::Original } else { DiffSide::Modified };
                DiffLineRange::new(path, side, sel["startLine"].as_u64().unwrap_or(1) as usize, sel["endLine"].as_u64().unwrap_or(1) as usize)
            }
            None => match &self.last_token {
                Some(t) => DiffLineRange::new(path, t.position.side(), t.position.line(), t.position.line()),
                None => return,
            },
        };
        self.add_comment(&range, window, cx);
    }

    fn zone(&mut self, key: &str, path: &str, side: &str, line: u64, cx: &mut Context<Self>) -> Entity<ThreadZone> {
        if let Some(z) = self.zones.borrow().get(key) {
            return z.clone();
        }
        let review = self.review.clone();
        let zone = cx.new(|cx| ThreadZone::new(review, path.to_string(), side.to_string(), line, cx));
        let sub = cx.subscribe(&zone, |_, _, _: &crate::review::ZoneEvent, cx| cx.notify());
        self._subs.push(sub);
        self.zones.borrow_mut().insert(key.to_string(), zone.clone());
        zone
    }

    /// The diff's annotations: one per line with threads or an open comment box.
    fn annotations(&mut self, cx: &mut Context<Self>) -> Vec<DiffAnnotation> {
        let Some(Shown::File(ix)) = self.shown.clone() else { return Vec::new() };
        let Some(path) = self.pr.files.get(ix).map(|f| f.path.clone()) else { return Vec::new() };
        let mut lines: Vec<(&'static str, u64)> = Vec::new();
        if let Some(state) = &self.review.read(cx).state {
            for t in state.threads.iter().filter(|t| t.path == path) {
                if let Some(line) = t.line {
                    lines.push((if t.side == "LEFT" { "LEFT" } else { "RIGHT" }, line));
                }
            }
        }
        for (key, zone) in self.zones.borrow().iter() {
            if zone.read(cx).has_composer() && key.ends_with(&format!(":{path}")) {
                let mut parts = key.splitn(3, ':');
                let side = if parts.next() == Some("LEFT") { "LEFT" } else { "RIGHT" };
                if let Some(line) = parts.next().and_then(|l| l.parse().ok()) {
                    lines.push((side, line));
                }
            }
        }
        lines.sort();
        lines.dedup();
        lines
            .into_iter()
            .map(|(side, line)| {
                let key = zone_key(&path, side, line);
                self.zone(&key, &path, side, line, cx);
                DiffAnnotation::line(SharedString::from(key), DiffLinePosition::new(path.clone(), side_of(side), line as usize))
            })
            .collect()
    }

    fn toggle_review_panel(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.review_panel.take().is_some() || self.review.read(cx).state.is_none() {
            cx.notify();
            return;
        }
        let panel = cx.new(|cx| ReviewPanel::new(self.review.clone(), self.pr.author.clone(), window, cx));
        let sub = cx.subscribe(&panel, |this, _, ev: &ReviewPanelEvent, cx| match ev {
            ReviewPanelEvent::Close => {
                this.review_panel = None;
                cx.notify();
            }
        });
        self._subs.push(sub);
        self.review_panel = Some(panel);
        cx.notify();
    }

    /// Shows a changed file (whole-file side-by-side diff) or a plain file, at
    /// `line` or, for diffs, at the first change. `remember` pushes the current
    /// place onto the back stack.
    fn show(&mut self, shown: Shown, line: Option<(DiffSide, usize)>, remember: bool, cx: &mut Context<Self>) {
        if remember && let Some(current) = self.shown.clone() {
            self.back_stack.push(NavEntry { shown: current, line: self.current_line });
            if self.back_stack.len() > 100 {
                self.back_stack.remove(0);
            }
        }
        self.hover = None;
        // Same file: just scroll.
        if self.shown.as_ref() == Some(&shown) && let (Some((side, n)), FileView::Diff(state)) = (line, &self.file_view) {
            let path = self.diff_path(&shown);
            state.update(cx, |s, cx| s.scroll_to_line(DiffLinePosition::new(path, side, n), cx));
            self.current_line = line;
            cx.notify();
            return;
        }
        self.shown = Some(shown.clone());
        self.current_line = line;
        self.file_view = FileView::Loading;
        self.load_seq += 1;
        let seq = self.load_seq;
        cx.notify();

        let (root, base, head) = (self.checkout.root.clone(), self.checkout.merge_base.clone(), self.checkout.head_sha.clone());
        let (patch_path, fut): (String, std::pin::Pin<Box<dyn std::future::Future<Output = Result<String, String>> + Send>>) = match &shown {
            Shown::File(ix) => {
                let Some(file) = self.pr.files.get(*ix).cloned() else { return };
                let (path, previous) = (file.path.clone(), file.previous_path.clone());
                (file.path.clone(), Box::pin(backend::task(async move { git::file_patch(&root, &base, &head, &path, previous.as_deref()).await })))
            }
            Shown::Plain(abs) => {
                let abs = abs.clone();
                (abs.clone(), Box::pin(backend::task(async move { plain_patch(&abs).await })))
            }
        };
        let is_plain = matches!(shown, Shown::Plain(_));
        cx.spawn(async move |this, cx| {
            let patch = fut.await;
            this.update(cx, |this, cx| {
                if this.load_seq != seq {
                    return;
                }
                // Like Monaco's revealLineInCenterIfOutsideViewport: stay at the top when the
                // first change is already on screen, otherwise put it about a third down.
                let first = if is_plain { None } else { patch.as_ref().ok().and_then(|p| first_change(p, &patch_path)) }
                    .map(|pos| {
                        let line = pos.line();
                        let top = if line <= 30 { 1 } else { line - 12 };
                        DiffLinePosition::new(patch_path.clone(), pos.side(), top)
                    });
                let target = line.map(|(side, n)| DiffLinePosition::new(patch_path.clone(), side, n)).or(first);
                this.file_view = match patch.and_then(|p| DiffFile::parse(&p).map_err(|e| e.message().to_string())) {
                    Ok(files) if !files.is_empty() => {
                        let language = language::registry().grammar_for(&patch_path);
                        let files: Vec<DiffFile> = files.into_iter().map(|f| f.with_language(language)).collect();
                        let state = cx.new(|cx| {
                            let mut state = DiffState::new(files, cx)
                                .with_mode(if is_plain { DiffMode::Unified } else { DiffMode::Split })
                                .with_context_lines(None)
                                .with_syntax_highlighter(highlight::highlight);
                            if let Some(pos) = target {
                                state.scroll_to_line(pos, cx);
                            }
                            state
                        });
                        this._diff_subscription = Some(cx.subscribe(&state, |this, state, event: &DiffEvent, cx| {
                            if matches!(event, DiffEvent::TextSelectionChanged | DiffEvent::SelectionChanged(_) | DiffEvent::SelectionEnded(_)) {
                                if let Some(selection) = state.read(cx).text_selection(cx) {
                                    this.report_selection(selection, false, cx);
                                }
                            }
                        }));
                        FileView::Diff(state)
                    }
                    Ok(_) => FileView::Message("No textual changes to show for this file.".into()),
                    Err(e) => FileView::Message(format!("Couldn't load the file: {e}")),
                };
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// The path the diff uses for positions: repo-relative for changed files, absolute for plain ones.
    fn diff_path(&self, shown: &Shown) -> String {
        match shown {
            Shown::File(ix) => self.pr.files.get(*ix).map(|f| f.path.clone()).unwrap_or_default(),
            Shown::Plain(abs) => abs.clone(),
        }
    }

    fn go_back(&mut self, _: &GoBack, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(entry) = self.back_stack.pop() {
            self.show(entry.shown, entry.line, false, cx);
        }
    }

    /// Path relative to the checkout, if inside it.
    fn relative(&self, abs: &str) -> Option<String> {
        abs.strip_prefix(&format!("{}/", self.checkout.root)).map(String::from)
    }

    // ---- Language servers ----

    /// Starts a server for each language among the PR's files (as the registry describes them).
    fn start_language_servers(&mut self, cx: &mut Context<Self>) {
        // Pick up languages.toml edits: they apply to PRs opened afterwards.
        language::reload();
        let registry = language::registry();
        let mut langs: Vec<Arc<dyn Language>> = Vec::new();
        for f in &self.pr.files {
            if let Some(l) = registry.for_path(&f.path)
                && !langs.iter().any(|x| x.id() == l.id())
            {
                langs.push(l);
            }
        }
        let root = self.checkout.root.clone();
        for lang in langs {
            let id = lang.id().to_string();
            let spec = match lang.server(std::path::Path::new(&root)) {
                None => continue,
                Some(Err(e)) => {
                    self.lsp.insert(id, ServerEntry { status: ServerStatus::Failed(e), client: None, note: None });
                    continue;
                }
                Some(Ok(spec)) => spec,
            };
            self.lsp.insert(id.clone(), ServerEntry { status: ServerStatus::Starting, client: None, note: spec.note.clone() });
            let fut = backend::task(LspClient::start(id.clone(), spec, lang.install_hint(), root.clone()));
            cx.spawn(async move |this, cx| {
                let result = fut.await;
                this.update(cx, |this, cx| {
                    if let Some(entry) = this.lsp.get_mut(&id) {
                        match result {
                            Ok(client) => {
                                entry.status = ServerStatus::Running;
                                entry.client = Some(client);
                            }
                            Err(e) => entry.status = ServerStatus::Failed(e),
                        }
                    }
                    cx.notify();
                })
                .ok();
            })
            .detach();
        }
    }

    /// Tells the agent what the reviewer is looking at (web: EditorHost.watchSelection).
    fn report_selection(&mut self, sel: CodeSelection, cursor_only: bool, cx: &mut Context<Self>) {
        let shown = self.shown.clone();
        let file = match &shown {
            Some(Shown::File(ix)) => self.pr.files.get(*ix).cloned(),
            _ => None,
        };
        let path = match (&shown, &file) {
            // The left side shows the file as it was; name it by its old path.
            (Some(Shown::File(_)), Some(f)) if sel.side == DiffSide::Original => f.previous_path.clone().unwrap_or(f.path.clone()),
            (Some(Shown::File(_)), Some(f)) => f.path.clone(),
            _ => self.relative(&sel.path).unwrap_or(sel.path.to_string()),
        };
        let changed = self.pr.files.iter().any(|f| f.path == path);
        let mut text = sel.text;
        if text.len() > 20000 {
            let mut cut = 20000;
            while !text.is_char_boundary(cut) {
                cut -= 1;
            }
            text.truncate(cut);
            text.push_str("\n… (truncated)");
        }
        let value = json!({
            "path": path,
            "side": if sel.side == DiffSide::Original { "old" } else { "new" },
            "changedInPR": changed,
            "startLine": sel.start_line,
            "endLine": sel.end_line,
            "cursorOnly": cursor_only,
            "text": text,
        });
        self.last_selection = Some(value.clone());
        self.selection_seq += 1;
        let seq = self.selection_seq;
        let Ok(session) = self.mcp.clone() else { return };
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(Duration::from_millis(120)).await;
            if this.read_with(cx, |this, _| this.selection_seq == seq).unwrap_or(false) {
                session.set_selection(value);
            }
        })
        .detach();
    }

    // ---- Agent panel ----

    fn toggle_agent(&mut self, _: &ToggleAgent, window: &mut Window, cx: &mut Context<Self>) {
        self.agent_open = !self.agent_open;
        if self.agent_open {
            self.ensure_agent(window, cx);
            if let Some(t) = &self.agent {
                t.update(cx, |t, cx| t.focus(window, cx));
            }
        } else {
            window.focus(&self.focus, cx);
        }
        cx.notify();
    }

    /// Starts the agent unless it's running.
    fn ensure_agent(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if matches!(self.agent_status, AgentStatus::Running) {
            return;
        }
        if let Some(old) = self.agent.take() {
            old.update(cx, |t, _| t.kill());
        }
        let pr = &self.pr;
        let label = format!("{}/{}#{} (\"{}\")", pr.owner, pr.repo, pr.number, pr.title);
        let launch = match self
            .mcp
            .as_ref()
            .map_err(|e| format!("Couldn't start the agent's context server: {e}"))
            .and_then(|session| crate::backend::agent::launch(&label, &self.checkout.merge_base, &self.checkout.root, session))
        {
            Ok(l) => l,
            Err(e) => {
                self.agent_status = AgentStatus::Failed(e);
                cx.notify();
                return;
            }
        };
        let started = match TerminalView::start(launch.program, launch.args, launch.env, self.checkout.root.clone()) {
            Ok(s) => s,
            Err(e) => {
                self.agent_status = AgentStatus::Failed(e);
                cx.notify();
                return;
            }
        };
        let view = cx.new(|cx| TerminalView::new(started, cx));
        let sub = cx.subscribe(&view, |this, _, ev: &TerminalEvent, cx| match ev {
            TerminalEvent::Exited => {
                this.agent_status = AgentStatus::Exited;
                cx.notify();
            }
        });
        self._subs.push(sub);
        view.update(cx, |t, cx| t.focus(window, cx));
        self.agent = Some(view);
        self.agent_status = AgentStatus::Running;
        cx.notify();
    }

    fn restart_agent(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.agent_status = AgentStatus::Idle;
        self.ensure_agent(window, cx);
    }

    /// ⌘L: type "About <file> lines A-B: " for the user to finish; ⌘⇧L: send "Explain …" right away.
    fn ask_agent(&mut self, explain: bool, window: &mut Window, cx: &mut Context<Self>) {
        let text = match &self.last_selection {
            None => String::new(),
            Some(sel) => {
                let (start, end) = (sel["startLine"].as_u64().unwrap_or(0), sel["endLine"].as_u64().unwrap_or(0));
                let lines = if start == end { format!("line {start}") } else { format!("lines {start}-{end}") };
                let side = if sel["side"] == "old" { " (old version, before the PR)" } else { "" };
                let what = format!("{} {lines}{side}", sel["path"].as_str().unwrap_or(""));
                if explain { format!("Explain {what}.") } else { format!("About {what}: ") }
            }
        };
        let submit = explain && self.last_selection.is_some();
        self.agent_open = true;
        let fresh = !matches!(self.agent_status, AgentStatus::Running);
        self.ensure_agent(window, cx);
        let Some(term) = self.agent.clone() else { return };
        term.update(cx, |t, cx| t.focus(window, cx));
        cx.notify();
        if text.is_empty() {
            return;
        }
        cx.spawn(async move |_, cx| {
            if fresh {
                // Wait for the agent to finish drawing its UI (output goes quiet), at most 10s,
                // and never type into a confirmation prompt (wait up to 5 minutes for the user).
                let begin = std::time::Instant::now();
                loop {
                    cx.background_executor().timer(Duration::from_millis(100)).await;
                    let (prompt, settled, exited) = term.read_with(cx, |t, _| (t.shows_confirmation_prompt(), terminal::settled(t, std::time::Instant::now()), t.exited));
                    if exited {
                        return;
                    }
                    let waited = begin.elapsed();
                    if prompt {
                        if waited > Duration::from_secs(300) {
                            return;
                        }
                        continue;
                    }
                    if settled || waited > Duration::from_secs(10) {
                        break;
                    }
                }
            }
            // Enter goes separately, a moment later: a fast burst reads as a paste, where Enter doesn't submit.
            term.update(cx, |t, _| t.write(text.into_bytes()));
            if submit {
                cx.background_executor().timer(Duration::from_millis(250)).await;
                term.update(cx, |t, _| t.write(&b"\r"[..]));
            }
        })
        .detach();
    }

    fn render_agent_dock(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        if !self.agent_open {
            return None;
        }
        let overlay = match &self.agent_status {
            AgentStatus::Exited => Some((
                "The agent exited.".to_string(),
                theme::FG,
                ui::primary_button("agent-restart", "Restart").on_click(cx.listener(|this, _, window, cx| this.restart_agent(window, cx))),
            )),
            AgentStatus::Failed(e) => Some((
                e.clone(),
                theme::DEL,
                ui::button("agent-retry", "Retry").on_click(cx.listener(|this, _, window, cx| this.restart_agent(window, cx))),
            )),
            _ => None,
        };
        Some(
            div()
                .flex_shrink_0()
                .flex()
                .flex_col()
                .child(
                    div()
                        .id("agent-splitter")
                        .h(px(5.))
                        .flex_shrink_0()
                        .cursor_row_resize()
                        .bg(hsla(theme::BORDER))
                        .border_t_2()
                        .border_color(hsla(theme::BG))
                        .hover(|el| el.bg(hsla(theme::ACCENT)))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, ev: &MouseDownEvent, _, cx| {
                                this.splitter_drag = Some((ev.position.y, this.agent_height));
                                cx.stop_propagation();
                            }),
                        ),
                )
                .child(
                    div()
                        .h(px(self.agent_height))
                        .flex()
                        .flex_col()
                        .bg(hsla(theme::PANEL))
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap(px(10.))
                                .px(px(12.))
                                .py(px(3.))
                                .text_size(theme::TEXT_SMALL)
                                .border_b_1()
                                .border_color(hsla(theme::BORDER))
                                .child(format!("✦ {}", self.agent_name))
                                .child(ui::muted(terminal::status_line_hint()))
                                .child(div().flex_1())
                                .child(ui::link("agent-hide", "✕").on_click(cx.listener(|this, _, window, cx| this.toggle_agent(&ToggleAgent, window, cx)))),
                        )
                        .child(
                            div()
                                .relative()
                                .flex_1()
                                .min_h_0()
                                .bg(hsla(terminal::BG))
                                .pt(px(4.))
                                .pl(px(10.))
                                .children(self.agent.clone())
                                .children(overlay.map(|(text, color, button)| {
                                    div().absolute().inset_0().flex().items_center().justify_center().child(
                                        div()
                                            .max_w(relative(0.8))
                                            .flex()
                                            .items_center()
                                            .gap(px(10.))
                                            .px(px(16.))
                                            .py(px(12.))
                                            .bg(hsla(theme::BG))
                                            .border_1()
                                            .border_color(hsla(theme::BORDER))
                                            .rounded(px(8.))
                                            .text_color(hsla(color))
                                            .child(text)
                                            .child(button),
                                    )
                                })),
                        ),
                )
                .into_any_element(),
        )
    }

    // ---- The agent acting on the review (always confirmed by the user) ----

    pub fn on_agent_action(&mut self, request: mcp::ActionRequest, window: &mut Window, cx: &mut Context<Self>) {
        if let mcp::ReviewAction::Threads = request.action {
            let _ = request.reply.send(Ok(self.threads_json(cx)));
            return;
        }
        // Check what we can before asking: unknown files/threads, lines outside the diff.
        if let Err(e) = self.check_action(&request.action, cx) {
            let _ = request.reply.send(Err(e));
            return;
        }
        self.agent_actions.push_back(request);
        // Bring this PR's window forward so the user sees the request.
        cx.activate(true);
        window.activate_window();
        cx.notify();
    }

    fn threads_json(&self, cx: &App) -> String {
        let Some(state) = &self.review.read(cx).state else { return "The review threads are still loading.".into() };
        let threads: Vec<Value> = state
            .threads
            .iter()
            .map(|t| {
                json!({
                    "id": t.id,
                    "path": t.path,
                    "side": if t.side == "LEFT" { "old" } else { "new" },
                    "line": t.line,
                    "start_line": t.start_line,
                    "resolved": t.is_resolved,
                    "outdated": t.is_outdated,
                    "comments": t.comments.iter().map(|c| json!({ "author": c.author, "body": c.body, "pending": c.pending })).collect::<Vec<_>>(),
                })
            })
            .collect();
        serde_json::to_string_pretty(&json!({ "viewer": state.viewer, "threads": threads })).unwrap_or_default()
    }

    fn check_action(&self, action: &mcp::ReviewAction, cx: &App) -> Result<(), String> {
        let state = self.review.read(cx).state.clone().ok_or("The review is still loading; try again in a moment.")?;
        match action {
            mcp::ReviewAction::Comment { path, side, line, start_line, .. } => {
                let file = self.pr.files.iter().find(|f| &f.path == path).ok_or_else(|| format!("{path} isn't changed in this PR."))?;
                if let Some(c) = &file.commentable {
                    let ranges = if side == "LEFT" { &c.left } else { &c.right };
                    let start = start_line.unwrap_or(*line);
                    if !ranges.iter().any(|&(a, b)| start >= a && *line <= b) {
                        return Err(format!(
                            "Line(s) {}{line} of {path} ({} side) aren't inside the diff, so GitHub won't take a comment there. Commentable ranges: {:?}",
                            start_line.map(|s| format!("{s}-")).unwrap_or_default(),
                            if side == "LEFT" { "old" } else { "new" },
                            ranges
                        ));
                    }
                }
                Ok(())
            }
            mcp::ReviewAction::Reply { thread_id, .. } | mcp::ReviewAction::Resolve { thread_id, .. } => {
                state.threads.iter().any(|t| &t.id == thread_id).then_some(()).ok_or_else(|| format!("No thread with id {thread_id}; see get_review_threads."))
            }
            mcp::ReviewAction::Submit { event, body } => {
                if *event != "COMMENT" && state.viewer == self.pr.author {
                    return Err("You can't approve or request changes on your own pull request.".into());
                }
                if *event == "REQUEST_CHANGES" && body.is_empty() && self.review.read(cx).pending_count() == 0 {
                    return Err("Requesting changes needs a summary (body) or pending comments.".into());
                }
                Ok(())
            }
            _ => Ok(()),
        }
    }

    /// The user answered the request on top of the queue.
    fn answer_agent_action(&mut self, confirmed: bool, cx: &mut Context<Self>) {
        if self.agent_action_busy {
            return;
        }
        let Some(request) = self.agent_actions.pop_front() else { return };
        if !confirmed {
            let _ = request.reply.send(Err("The user declined this in the app.".into()));
            cx.notify();
            return;
        }
        #[cfg(debug_assertions)]
        if self.dry_run_actions {
            let _ = request.reply.send(Ok("Dry run: confirmed, not sent to GitHub.".into()));
            cx.notify();
            return;
        }
        self.agent_action_busy = true;
        let action = request.action.clone();
        let task = self.review.update(cx, |r, cx| match action.clone() {
            mcp::ReviewAction::Comment { path, side, line, start_line, body } => r.add_thread(path, side, line, start_line, body, cx),
            mcp::ReviewAction::Reply { thread_id, body } => r.reply(thread_id, body, cx),
            mcp::ReviewAction::Resolve { thread_id, resolved } => r.set_resolved(thread_id, resolved, cx),
            mcp::ReviewAction::PrComment { body } => r.add_pr_comment(body, cx),
            mcp::ReviewAction::Submit { event, body } => r.submit(event, body, cx),
            mcp::ReviewAction::Threads => Task::ready(Ok(())),
        });
        cx.notify();
        cx.spawn(async move |this, cx| {
            let result = task.await;
            this.update(cx, |this, cx| {
                this.agent_action_busy = false;
                let outcome = result.map(|()| match &action {
                    mcp::ReviewAction::Comment { path, line, .. } => format!("Added to the pending review: comment on {path}:{line}. It's published when the review is submitted."),
                    mcp::ReviewAction::Reply { .. } => "Reply added to the pending review.".into(),
                    mcp::ReviewAction::Resolve { resolved, .. } => if *resolved { "Thread resolved." } else { "Thread unresolved." }.into(),
                    mcp::ReviewAction::PrComment { .. } => "Comment posted on the PR.".into(),
                    mcp::ReviewAction::Submit { event, .. } => format!("Review submitted ({}).", match *event {
                        "APPROVE" => "approved",
                        "REQUEST_CHANGES" => "changes requested",
                        _ => "comment",
                    }),
                    mcp::ReviewAction::Threads => String::new(),
                });
                // Show where the new comment went.
                if let (Ok(_), mcp::ReviewAction::Comment { path, side, line, .. }) = (&outcome, &action)
                    && let Some(ix) = this.pr.files.iter().position(|f| &f.path == path)
                {
                    this.view = MainView::Code;
                    let top = (*line as usize).saturating_sub(5).max(1);
                    this.show(Shown::File(ix), Some((side_of(side), top)), true, cx);
                }
                let _ = request.reply.send(outcome);
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// The card asking the user to confirm what the agent wants to do.
    fn render_agent_action(&self, cx: &mut Context<Self>) -> Option<impl IntoElement> {
        let request = self.agent_actions.front()?;
        let agent = self.agent_name.clone();
        let (title, confirm, primary, detail, body): (String, &str, bool, Option<String>, Option<String>) = match &request.action {
            mcp::ReviewAction::Comment { path, side, line, start_line, body } => (
                "add a review comment".into(),
                "Add comment",
                true,
                Some(format!(
                    "{path}, {}{}{}",
                    start_line.map(|s| format!("lines {s}–{line}")).unwrap_or_else(|| format!("line {line}")),
                    if side == "LEFT" { " (old version)" } else { "" },
                    " · goes into your pending review"
                )),
                Some(body.clone()),
            ),
            mcp::ReviewAction::Reply { thread_id, body } => {
                let thread = self.review.read(cx).state.as_ref().and_then(|s| s.threads.iter().find(|t| &t.id == thread_id).cloned());
                let detail = thread.map(|t| format!("On {}:{} · {}", t.path, t.line.map(|l| l.to_string()).unwrap_or("outdated".into()), t.comments.first().map(|c| format!("{}: {}", c.author, crate::markdown::snippet(&c.body))).unwrap_or_default()));
                ("reply to a thread".into(), "Reply", true, detail, Some(body.clone()))
            }
            mcp::ReviewAction::Resolve { thread_id, resolved } => {
                let thread = self.review.read(cx).state.as_ref().and_then(|s| s.threads.iter().find(|t| &t.id == thread_id).cloned());
                let detail = thread.map(|t| format!("{}:{} · {}", t.path, t.line.map(|l| l.to_string()).unwrap_or("outdated".into()), t.comments.first().map(|c| format!("{}: {}", c.author, crate::markdown::snippet(&c.body))).unwrap_or_default()));
                (if *resolved { "resolve a thread" } else { "unresolve a thread" }.into(), if *resolved { "Resolve" } else { "Unresolve" }, true, detail, None)
            }
            mcp::ReviewAction::PrComment { body } => ("post a comment on the PR".into(), "Post comment", true, Some("Posts right away, separate from your review".into()), Some(body.clone())),
            mcp::ReviewAction::Submit { event, body } => {
                let pending = self.review.read(cx).pending_count();
                let (title, confirm) = match *event {
                    "APPROVE" => ("approve this pull request", "Approve"),
                    "REQUEST_CHANGES" => ("request changes on this pull request", "Request changes"),
                    _ => ("submit your review as a comment", "Submit review"),
                };
                let detail = Some(if pending > 0 { format!("Also publishes {pending} pending comment{}", if pending == 1 { "" } else { "s" }) } else { "No pending comments".into() });
                (title.into(), confirm, true, detail, (!body.is_empty()).then(|| body.clone()))
            }
            mcp::ReviewAction::Threads => return None,
        };
        let queued = self.agent_actions.len();
        let busy = self.agent_action_busy;
        Some(deferred(
            div().absolute().top(px(46.)).left_0().right_0().flex().justify_center().child(
                div()
                    .id("agent-action")
                    .occlude()
                    .w(px(560.))
                    .p(px(14.))
                    .flex()
                    .flex_col()
                    .gap(px(8.))
                    .bg(hsla(theme::PANEL))
                    .border_1()
                    .border_color(hsla(theme::ACCENT))
                    .rounded(px(8.))
                    .shadow_lg()
                    .text_size(theme::TEXT)
                    .text_color(hsla(theme::FG))
                    .child(
                        div()
                            .flex()
                            .gap(px(6.))
                            .child(div().font_weight(FontWeight::SEMIBOLD).child(format!("✦ {agent} wants to {title}")))
                            .when(queued > 1, |el| el.child(ui::muted(format!("(1 of {queued})")))),
                    )
                    .children(detail.map(|d| ui::muted(d).text_size(theme::TEXT_SMALL)))
                    .children(body.map(|b| {
                        div()
                            .id("agent-action-body")
                            .max_h(px(260.))
                            .overflow_y_scroll()
                            .px(px(10.))
                            .py(px(8.))
                            .bg(hsla(theme::BG))
                            .border_1()
                            .border_color(hsla(theme::BORDER))
                            .rounded(px(4.))
                            .child(crate::review::markdown_body("agent-action-md", &b, cx))
                    }))
                    .child(
                        div()
                            .flex()
                            .justify_end()
                            .gap(px(8.))
                            .child(ui::button("agent-action-no", "Don't").when(!busy, |el| el.on_click(cx.listener(|this, _, _, cx| this.answer_agent_action(false, cx)))))
                            .child(
                                (if primary { ui::primary_button("agent-action-yes", if busy { "Working…" } else { confirm }) } else { ui::button("agent-action-yes", confirm) })
                                    .when(busy, |el| el.opacity(0.6))
                                    .when(!busy, |el| el.on_click(cx.listener(|this, _, _, cx| this.answer_agent_action(true, cx)))),
                            ),
                    ),
            ),
        )
        .with_priority(2))
    }

    /// Leaving the PR: stop its servers and clear the agent's context.
    pub fn close(&mut self, cx: &mut App) {
        for request in self.agent_actions.drain(..) {
            let _ = request.reply.send(Err("The review window was closed.".into()));
        }
        if let Some(agent) = self.agent.take() {
            agent.update(cx, |t, _| t.kill());
        }
        self.selection_seq += 1;
        if let Ok(session) = &self.mcp {
            session.end();
        }
        self.stop_language_servers();
    }

    pub fn stop_language_servers(&mut self) {
        for (_, entry) in self.lsp.drain() {
            if let Some(client) = entry.client {
                let _ = backend::task(async move { client.stop().await });
            }
        }
    }

    fn lines_of(&mut self, abs: &str) -> Option<Arc<Vec<String>>> {
        if let Some(lines) = self.file_lines.get(abs) {
            return Some(lines.clone());
        }
        let text = std::fs::read_to_string(abs).ok()?;
        let lines = Arc::new(text.lines().map(String::from).collect::<Vec<_>>());
        self.file_lines.insert(abs.to_string(), lines.clone());
        Some(lines)
    }

    /// Where a hit is for the language server: absolute path, language, 0-based
    /// line, UTF-16 column, and the client. The old side of a PR diff isn't on
    /// disk, so it has no language features (as in the web version).
    /// (absolute path, LSP languageId, 0-based line, UTF-16 column, client).
    fn resolve(&mut self, hit: &TokenHit) -> Option<(String, String, u32, u32, Arc<LspClient>)> {
        let shown = self.shown.clone()?;
        if matches!(shown, Shown::File(_)) && hit.position.side() == DiffSide::Original {
            return None;
        }
        let path = hit.position.path().to_string();
        let abs = if path.starts_with('/') { path.clone() } else { format!("{}/{}", self.checkout.root, path) };
        let lang = language::registry().for_path(&abs)?;
        let client = self.lsp.get(lang.id())?.client.clone()?;
        let language = lang.lsp_language_id(&abs);
        let lines = self.lines_of(&abs)?;
        let line_ix = hit.position.line().checked_sub(1)?;
        let text = lines.get(line_ix)?;
        Some((abs, language, line_ix as u32, lsp::utf16_column(text, hit.column), client))
    }

    fn on_token_hover(&mut self, hit: Option<TokenHit>, cx: &mut Context<Self>) {
        if hit.is_some() {
            self.hovered_token = hit.clone();
        }
        self.hover_seq += 1;
        let seq = self.hover_seq;
        let Some(hit) = hit else {
            // Leave the popup up briefly so the pointer can move into it.
            cx.spawn(async move |this, cx| {
                cx.background_executor().timer(Duration::from_millis(300)).await;
                this.update(cx, |this, cx| {
                    if this.hover_seq == seq && !this.hover_over_popup {
                        this.hover = None;
                        cx.notify();
                    }
                })
                .ok();
            })
            .detach();
            return;
        };
        let Some((abs, language, line, character, client)) = self.resolve(&hit) else { return };
        let point = hit.point;
        cx.spawn(async move |this, cx| {
            // VS Code's hover delay.
            cx.background_executor().timer(Duration::from_millis(250)).await;
            if this.read_with(cx, |this, _| this.hover_seq != seq).unwrap_or(true) {
                return;
            }
            let result = backend::task(async move { client.hover(&abs, &language, line, character).await }).await;
            this.update(cx, |this, cx| {
                if this.hover_seq != seq {
                    return;
                }
                this.hover = match result {
                    Ok(Some(md)) => Some(Hover { point, markdown: md.into() }),
                    _ => None,
                };
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn go_to_definition(&mut self, hit: TokenHit, cx: &mut Context<Self>) {
        self.definition(hit, false, cx);
    }

    /// Go to (or, with `peek`, list) the definition of the symbol at `hit`.
    fn definition(&mut self, hit: TokenHit, peek: bool, cx: &mut Context<Self>) {
        let Some((abs, language, line, character, client)) = self.resolve(&hit) else { return };
        self.hover = None;
        let fut = backend::task(async move { client.definition(&abs, &language, line, character).await });
        let point = hit.point;
        cx.spawn(async move |this, cx| {
            let Ok(locations) = fut.await else { return };
            this.update(cx, |this, cx| match locations.len() {
                0 => {}
                1 if !peek => this.open_location(&locations[0].path, locations[0].line as usize + 1, cx),
                1 => this.show_peek(point, "Definition".into(), locations, cx),
                n => this.show_peek(point, format!("{n} definitions"), locations, cx),
            })
            .ok();
        })
        .detach();
    }

    fn find_references(&mut self, hit: TokenHit, cx: &mut Context<Self>) {
        let Some((abs, language, line, character, client)) = self.resolve(&hit) else { return };
        self.hover = None;
        let fut = backend::task(async move { client.references(&abs, &language, line, character).await });
        let point = hit.point;
        cx.spawn(async move |this, cx| {
            let Ok(locations) = fut.await else { return };
            this.update(cx, |this, cx| {
                let n = locations.len();
                this.show_peek(point, format!("{n} reference{}", if n == 1 { "" } else { "s" }), locations, cx)
            })
            .ok();
        })
        .detach();
    }

    fn show_peek(&mut self, point: Point<Pixels>, title: String, locations: Vec<lsp::Location>, cx: &mut Context<Self>) {
        let items = locations
            .into_iter()
            .map(|loc| {
                let text = self.lines_of(&loc.path).and_then(|l| l.get(loc.line as usize).cloned()).unwrap_or_default();
                PeekItem { label: self.relative(&loc.path).unwrap_or(loc.path.clone()), abs: loc.path, line: loc.line as usize + 1, text: text.trim().to_string() }
            })
            .collect();
        let mut items: Vec<PeekItem> = items;
        // Grouped by file (stable order of first appearance), then by line, like VS Code.
        let mut order: Vec<String> = Vec::new();
        for item in &items {
            if !order.contains(&item.abs) {
                order.push(item.abs.clone());
            }
        }
        items.sort_by_key(|i| (order.iter().position(|a| a == &i.abs), i.line));
        self.peek = Some(Peek { point, title: title.into(), items, selected: 0 });
        cx.notify();
    }

    fn on_peek_definition(&mut self, _: &PeekDefinition, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(hit) = self.last_token.clone() {
            self.definition(hit, true, cx);
        }
    }

    fn on_find_references(&mut self, _: &FindReferences, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(hit) = self.last_token.clone() {
            self.find_references(hit, cx);
        }
    }

    fn close_popup(&mut self, _: &ClosePopup, _: &mut Window, cx: &mut Context<Self>) {
        self.peek = None;
        self.hover = None;
        cx.notify();
    }

    /// Opens a file position: a changed file opens as its diff, anything else as a plain file.
    fn open_location(&mut self, abs: &str, line: usize, cx: &mut Context<Self>) {
        let changed = self
            .relative(abs)
            .and_then(|rel| self.pr.files.iter().position(|f| f.path == rel && f.status != "removed"));
        match changed {
            Some(ix) => self.show(Shown::File(ix), Some((DiffSide::Modified, line)), true, cx),
            None => self.show(Shown::Plain(abs.to_string()), Some((DiffSide::Modified, line)), true, cx),
        }
    }

    fn on_go_to_definition(&mut self, _: &GoToDefinition, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(hit) = self.last_token.clone() {
            self.go_to_definition(hit, cx);
        }
    }

    fn render_header(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let pr = &self.pr;
        div()
            .h(theme::TITLEBAR)
            .flex_shrink_0()
            .flex()
            .items_center()
            .gap(px(12.))
            .pl(px(84.)) // traffic lights
            .pr(px(12.))
            .bg(hsla(theme::PANEL))
            .border_b_1()
            .border_color(hsla(theme::BORDER))
            .child(ui::link("back", "← Pull requests").on_click(cx.listener(|_, _, _, cx| cx.emit(PrViewEvent::Close))))
            .child(
                div()
                    .flex()
                    .gap(px(4.))
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .font_weight(FontWeight::MEDIUM)
                    .child(ui::muted(format!("{}/{}#{}", pr.owner, pr.repo, pr.number)))
                    .child(div().truncate().child(pr.title.clone())),
            )
            .child(ui::spacer())
            .when(self.checkout.local.is_some(), |el| el.child(ui::chip("your clone", theme::FG)))
            .when(self.checkout.worktree_of.is_some(), |el| el.child(ui::chip("copy of your clone", theme::FG)))
            // Each language draws its own server status (see Language::render_status).
            .children({
                let registry = language::registry();
                let mut chips = Vec::new();
                for lang in registry.all() {
                    if let Some(entry) = self.lsp.get(lang.id()) {
                        chips.push(lang.render_status(&entry.status, entry.note.as_ref(), cx));
                    }
                }
                chips
            })
            .child(
                ui::link("pr-settings", "⚙")
                    .on_click(|_, window, cx| window.dispatch_action(Box::new(crate::settings::OpenSettings), cx)),
            )
            .child({
                let url = pr.url.clone();
                ui::link("open-github", "Open on GitHub").on_click(move |_, _, cx| cx.open_url(&url))
            })
            .child(
                ui::button("agent", format!("✦ {}", self.agent_name))
                    .when(self.agent_open, |el| el.bg(hsla(theme::ACTIVE)).border_color(hsla(theme::ACCENT)))
                    .on_click(cx.listener(|this, _, window, cx| this.toggle_agent(&ToggleAgent, window, cx))),
            )
            .child({
                let pending = self.review.read(cx).pending_count();
                let ready = self.review.read(cx).state.is_some();
                let label = if pending > 0 { format!("Finish review ({pending})") } else { "Review".to_string() };
                div()
                    .relative()
                    .child(
                        ui::primary_button("review", label)
                            .when(!ready, |el| el.opacity(0.5))
                            .when(ready, |el| el.on_click(cx.listener(|this, _, window, cx| this.toggle_review_panel(window, cx)))),
                    )
                    .children(self.render_review_panel())
            })
    }

    fn render_sidebar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let pr = &self.pr;
        let snippet = crate::markdown::snippet(&pr.body);
        let review = self.review.read(cx);
        let state = review.state.clone();
        let error = review.error.clone();
        let conv_target = self.conversation.read(cx).target.clone();
        let in_conv = self.view == MainView::Conversation;
        let mut threads_by_path: HashMap<String, Vec<ReviewThread>> = HashMap::new();
        for t in state.iter().flat_map(|s| s.threads.iter()) {
            threads_by_path.entry(t.path.clone()).or_default().push(t.clone());
        }
        let changed: HashSet<&str> = pr.files.iter().map(|f| f.path.as_str()).collect();
        let orphans: Vec<ReviewThread> = state.iter().flat_map(|s| s.threads.iter()).filter(|t| !changed.contains(t.path.as_str())).cloned().collect();

        let row = |id: SharedString, active: bool| {
            div()
                .id(id)
                .flex()
                .items_center()
                .gap(px(6.))
                .px(px(12.))
                .h(px(24.))
                .whitespace_nowrap()
                .cursor_pointer()
                .when(active, |el| el.bg(hsla(theme::ACTIVE)))
                .when(!active, |el| el.hover(|s| s.bg(hsla(theme::HOVER))))
        };
        let title = |text: &str, count: String| {
            div()
                .px(px(12.))
                .pt(px(10.))
                .pb(px(6.))
                .flex()
                .gap(px(4.))
                .child(ui::section_title(text.to_string()))
                .child(div().text_size(theme::TEXT_TINY).text_color(hsla(theme::MUTED)).child(count))
        };

        let mut conv = div().flex().flex_col().pb(px(6.)).mb(px(4.)).border_b_1().border_color(hsla(theme::BORDER)).child(
            row("conv-description".into(), in_conv && conv_target.is_none())
                .on_click(cx.listener(|this, _, _, cx| this.open_conversation(None, cx)))
                .child(div().w(px(16.)).flex_shrink_0().text_center().text_color(hsla(theme::MUTED)).text_size(theme::TEXT_TINY).child("≡"))
                .child(div().flex_shrink_0().child("Description"))
                .child(div().flex_1().min_w_0().truncate().text_color(hsla(theme::MUTED)).child(if snippet.is_empty() { "No description".to_string() } else { snippet })),
        );
        for item in state.iter().flat_map(|s| s.conversation.iter()) {
            let (icon, color) = match item.review_state.as_deref() {
                Some("APPROVED") => ("✓", theme::ADD),
                Some("CHANGES_REQUESTED") => ("±", theme::DEL),
                _ => ("💬", theme::MUTED),
            };
            let text = crate::markdown::snippet(&item.body);
            let text = if !text.is_empty() {
                text
            } else {
                match item.review_state.as_deref() {
                    Some("APPROVED") => "approved".into(),
                    Some("CHANGES_REQUESTED") => "requested changes".into(),
                    _ => String::new(),
                }
            };
            let id = item.id.clone();
            conv = conv.child(
                row(SharedString::from(format!("conv-{}", item.id)), in_conv && conv_target.as_deref() == Some(item.id.as_str()))
                    .on_click(cx.listener(move |this, _, _, cx| this.open_conversation(Some(id.clone()), cx)))
                    .child(div().w(px(16.)).flex_shrink_0().text_center().text_color(hsla(color)).text_size(theme::TEXT_TINY).child(icon))
                    .child(div().flex_shrink_0().text_color(hsla(theme::MUTED)).child(item.author.clone()))
                    .child(div().flex_1().min_w_0().truncate().child(text)),
            );
        }

        let mut files = div().flex().flex_col().pb(px(12.));
        for (i, f) in pr.files.iter().enumerate() {
            let (letter, color) = status_letter(&f.status);
            let (dir, name) = match f.path.rfind('/') {
                Some(slash) => (f.path[..slash].to_string(), f.path[slash + 1..].to_string()),
                None => (String::new(), f.path.clone()),
            };
            let mut threads = threads_by_path.get(&f.path).cloned().unwrap_or_default();
            threads.sort_by_key(|t| t.line.unwrap_or(u64::MAX));
            let open = threads.iter().filter(|t| !t.is_resolved && !t.is_outdated).count();
            let expanded = !self.collapsed_files.contains(&f.path);
            let active = !in_conv && self.selected() == Some(i);
            let path = f.path.clone();
            // The directory, cut from the left so its end stays visible (like the web's rtl ellipsis).
            let stats = format!("+{} −{}", f.additions, f.deletions).chars().count() as f32 * 6.2;
            let count = if threads.is_empty() { 0. } else { 34. };
            let budget = self.sidebar_width - 24. - 10. - 12. - 30. - name.chars().count() as f32 * 6.6 - stats - count;
            let max_chars = (budget / 5.6).max(0.) as usize;
            let shown_dir = if dir.is_empty() {
                String::new()
            } else if dir.chars().count() + 1 <= max_chars {
                format!("/{dir}")
            } else {
                let tail: String = dir.chars().rev().take(max_chars.saturating_sub(3)).collect::<Vec<_>>().into_iter().rev().collect();
                format!("...{tail}")
            };
            files = files.child(
                row(SharedString::from(format!("file-{i}")), active)
                    .h(px(37.))
                    .on_click(cx.listener(move |this, _, _, cx| this.select_file(i, cx)))
                    .child(
                        div()
                            .id(SharedString::from(format!("twisty-{i}")))
                            .w(px(10.))
                            .flex_shrink_0()
                            .text_size(theme::TEXT_TINY)
                            .text_color(hsla(theme::MUTED))
                            .when(!threads.is_empty(), |el| {
                                el.child(if expanded { "▾" } else { "▸" }).on_click(cx.listener(move |this, _, _, cx| {
                                    cx.stop_propagation();
                                    if !this.collapsed_files.remove(&path) {
                                        this.collapsed_files.insert(path.clone());
                                    }
                                    cx.notify();
                                }))
                            }),
                    )
                    .child(div().w(px(12.)).flex_shrink_0().text_size(theme::TEXT_TINY).font_weight(FontWeight::SEMIBOLD).text_color(hsla(color)).child(letter))
                    .child(div().flex_shrink_0().child(name))
                    .child(div().flex_1().min_w_0().overflow_hidden().text_size(theme::TEXT_TINY).text_color(hsla(theme::MUTED)).child(shown_dir))
                    .when(!threads.is_empty(), |el| {
                        el.child(
                            div()
                                .flex_shrink_0()
                                .text_size(theme::TEXT_TINY)
                                .text_color(hsla(if open > 0 { theme::LINK } else { theme::MUTED }))
                                .child(format!("💬 {}", if open > 0 { open } else { threads.len() })),
                        )
                    })
                    .child(
                        div()
                            .flex_shrink_0()
                            .flex()
                            .gap(px(4.))
                            .text_size(theme::TEXT_TINY)
                            .child(div().text_color(hsla(theme::ADD)).child(format!("+{}", f.additions)))
                            .child(div().text_color(hsla(theme::DEL)).child(format!("−{}", f.deletions))),
                    ),
            );
            if expanded {
                for t in threads {
                    let thread = t.clone();
                    files = files.child(self.thread_row(&t, false).on_click(cx.listener(move |this, _, _, cx| this.open_thread(i, &thread, cx))));
                }
            }
        }

        let mut sidebar = div()
            .id("sidebar")
            .w(px(self.sidebar_width))
            .flex_shrink_0()
            .h_full()
            .overflow_y_scroll()
            .bg(hsla(theme::PANEL))
            .border_r_1()
            .border_color(hsla(theme::BORDER))
            .child(title("Conversation", format!("({})", state.as_ref().map(|s| s.conversation.len().to_string()).unwrap_or("…".into()))))
            .child(conv)
            .child(title("Changed files", format!("({})", pr.files.len())))
            .children(error.map(|e| div().px(px(12.)).child(ui::error_text(format!("Couldn't load comments: {e}")))))
            .child(files);
        if !orphans.is_empty() {
            sidebar = sidebar.child(title("Comments on files no longer in the diff", format!("({})", orphans.len())));
            for t in orphans {
                let url = t.comments.first().map(|c| c.url.clone()).unwrap_or(self.pr.url.clone());
                sidebar = sidebar.child(self.thread_row(&t, true).on_click(move |_, _, cx| cx.open_url(&url)));
            }
        }
        sidebar
    }

    /// A thread under its file in the sidebar: where, author, first comment.
    fn thread_row(&self, t: &ReviewThread, show_path: bool) -> Stateful<Div> {
        let first = t.comments.first();
        let where_ = if t.is_outdated || t.line.is_none() {
            "outdated".to_string()
        } else {
            format!("L{}{}", t.line.unwrap_or(0), if t.side == "LEFT" { " (old)" } else { "" })
        };
        let where_ = if show_path { format!("{} {where_}", t.path.rsplit('/').next().unwrap_or("")) } else { where_ };
        let pending = t.comments.iter().any(|c| c.pending);
        div()
            .id(SharedString::from(format!("thread-{}", t.id)))
            .flex()
            .items_center()
            .gap(px(6.))
            .pl(px(40.))
            .pr(px(12.))
            .h(px(20.))
            .whitespace_nowrap()
            .text_size(theme::TEXT_SMALL)
            .cursor_pointer()
            .hover(|s| s.bg(hsla(theme::HOVER)))
            .when(t.is_resolved || t.is_outdated, |el| el.opacity(0.55))
            .child(div().flex_shrink_0().font_family(theme::MONO_FONT).text_size(theme::TEXT_TINY).text_color(hsla(theme::LINK)).child(where_))
            .child(div().flex_shrink_0().text_color(hsla(theme::MUTED)).child(first.map(|c| c.author.clone()).unwrap_or_default()))
            .child(div().flex_1().min_w_0().truncate().child(first.map(|c| crate::markdown::snippet(&c.body)).unwrap_or_default()))
            .when(t.comments.len() > 1, |el| el.child(ui::muted(format!("+{}", t.comments.len() - 1))))
            .when(pending, |el| el.child(crate::review::badge("Pending", theme::WARN)))
            .when(t.is_resolved, |el| el.child(div().flex_shrink_0().text_color(hsla(theme::ADD)).child("✓")))
    }

    fn render_main(&mut self, cx: &mut Context<Self>) -> AnyElement {
        if self.view == MainView::Conversation {
            return div().flex_1().min_w_0().h_full().child(self.conversation.clone()).into_any_element();
        }
        let annotations = self.annotations(cx);
        let zones = self.zones.clone();
        let outdated = self.outdated_threads(cx);
        let show_outdated = self.show_outdated && !outdated.is_empty();
        let (path, plain) = match &self.shown {
            Some(Shown::File(ix)) => (self.pr.files.get(*ix).map(|f| f.path.clone()).unwrap_or_default(), false),
            Some(Shown::Plain(abs)) => (self.relative(abs).unwrap_or_else(|| abs.clone()), true),
            None => (String::new(), false),
        };
        let can_go_back = !self.back_stack.is_empty();
        let weak = cx.entity().downgrade();
        div()
            .flex_1()
            .min_w_0()
            .h_full()
            .flex()
            .flex_col()
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(10.))
                    .px(px(12.))
                    .py(px(4.))
                    .min_h(px(28.))
                    .border_b_1()
                    .border_color(hsla(theme::BORDER))
                    .child(
                        div()
                            .id("nav-back")
                            .text_color(hsla(if can_go_back { theme::LINK } else { theme::MUTED }))
                            .when(can_go_back, |el| {
                                el.cursor_pointer().on_click(cx.listener(|this, _, window, cx| this.go_back(&GoBack, window, cx)))
                            })
                            .tooltip(|window, cx| gpui_component::tooltip::Tooltip::new("Go back (Ctrl+-)").build(window, cx))
                            .child("←"),
                    )
                    .child(div().font_family(theme::MONO_FONT).text_size(theme::TEXT_SMALL).truncate().child(path))
                    .when(plain, |el| el.child(ui::muted("not changed in this PR")))
                    .child(div().flex_1())
                    .when(!outdated.is_empty(), |el| {
                        let n = outdated.len();
                        el.child(
                            ui::link("toggle-outdated", format!("{} {n} outdated thread{}", if self.show_outdated { "Hide" } else { "Show" }, if n == 1 { "" } else { "s" }))
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.show_outdated = !this.show_outdated;
                                    cx.notify();
                                })),
                        )
                    }),
            )
            .when(show_outdated, |el| {
                el.child(
                    div()
                        .id("outdated-panel")
                        .max_h(px(320.))
                        .overflow_y_scroll()
                        .p(px(8.))
                        .flex()
                        .flex_col()
                        .gap(px(6.))
                        .border_b_1()
                        .border_color(hsla(theme::BORDER))
                        .children(outdated.clone()),
                )
            })
            .child(match &self.file_view {
                FileView::Diff(state) => {
                    let (hover_view, click_view) = (weak.clone(), weak.clone());
                    let focus = self.focus.clone();
                    let bounds_cell = self.code_bounds.clone();
                    let ratio = self.split_ratio;
                    let show_divider = !matches!(self.shown, Some(Shown::Plain(_)));
                    let dragging_split = self.split_drag;
                    div()
                        .id("code")
                        .relative()
                        .flex_1()
                        .min_h_0()
                        .child(canvas(move |bounds, _, _| bounds_cell.set(bounds), |_, _, _, _| {}).absolute().size_full())
                        // The divider between the old and new sides: drag to resize.
                        .when(show_divider, |el| {
                            el.child(
                                div()
                                    .id("split-resize")
                                    .absolute()
                                    .top_0()
                                    .bottom_0()
                                    .left(relative(ratio))
                                    .ml(px(-3.))
                                    .w(px(6.))
                                    .cursor_col_resize()
                                    .hover(|el| el.bg(gpui::rgba(0x0078d466)))
                                    .when(dragging_split, |el| el.bg(gpui::rgba(0x0078d466)))
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(|this, _: &MouseDownEvent, _, cx| {
                                            this.split_drag = true;
                                            cx.stop_propagation();
                                        }),
                                    ),
                            )
                        })
                        .on_mouse_down(
                            MouseButton::Right,
                            cx.listener(|this, _: &MouseDownEvent, window, cx| {
                                // Right-click acts on the symbol under the pointer, like moving the cursor there.
                                if let Some(hit) = this.hovered_token.clone() {
                                    this.last_token = Some(hit.clone());
                                    let (side, line) = (hit.position.side(), hit.position.line());
                                    if let FileView::Diff(state) = &this.file_view
                                        && state.read(cx).text_selection(cx).is_none()
                                        && let Some(text) = state.read(cx).line_text(side, line)
                                    {
                                        let sel = CodeSelection { path: hit.position.path().clone(), side, start_line: line, end_line: line, text };
                                        this.report_selection(sel, true, cx);
                                    }
                                }
                                window.focus(&this.focus, cx);
                            }),
                        )
                        .context_menu(move |menu, _, _| {
                            menu.action_context(focus.clone())
                                .menu("Agent: Explain this code", Box::new(ExplainAgent))
                                .menu("Agent: About this code", Box::new(AskAgent))
                                .separator()
                                .menu("Go to Definition", Box::new(GoToDefinition))
                                .menu("Peek Definition", Box::new(PeekDefinition))
                                .menu("Find All References", Box::new(FindReferences))
                                .separator()
                                .menu("Add Review Comment", Box::new(AddComment))
                        })
                        .child(
                        Diff::new(state)
                            .size_full()
                            .line_number(true)
                            .syntax_highlight(true)
                            .header_visible(false)
                            .hunk_separator(DiffHunkSeparator::Simple)
                            .change_indicator(DiffChangeIndicator::Signs)
                            .split_ratio(ratio)
                            .annotations(annotations)
                            .render_annotation(move |annotation, _, _| {
                                let key = annotation.id().to_string();
                                zones.borrow().get(&key).map(|z| z.clone().into_any_element()).unwrap_or_else(|| div().into_any_element())
                            })
                            .can_add_annotation({
                                let commentable = match &self.shown {
                                    Some(Shown::File(ix)) => self.pr.files.get(*ix).and_then(|f| f.commentable.clone()),
                                    _ => None,
                                };
                                let plain = matches!(self.shown, Some(Shown::Plain(_)));
                                move |pos| {
                                    if plain {
                                        return false;
                                    }
                                    let Some(c) = &commentable else { return true };
                                    let line = pos.line() as u64;
                                    (if pos.side() == DiffSide::Original { &c.left } else { &c.right }).iter().any(|&(a, b)| line >= a && line <= b)
                                }
                            })
                            .on_add_annotation({
                                let weak = weak.clone();
                                move |range, window, cx| {
                                    weak.update(cx, |this, cx| this.add_comment(range, window, cx)).ok();
                                }
                            })
                            .on_token_hover(move |hit, _, cx| {
                                let hit = hit.cloned();
                                hover_view.update(cx, |this, cx| this.on_token_hover(hit, cx)).ok();
                            })
                            .on_token_click(move |hit, event, _, cx| {
                                let (hit, cmd) = (hit.clone(), event.modifiers().platform);
                                click_view
                                    .update(cx, |this, cx| {
                                        this.last_token = Some(hit.clone());
                                        // A click puts the cursor on a line: report it (cursor only).
                                        let (side, line) = (hit.position.side(), hit.position.line());
                                        if let FileView::Diff(state) = &this.file_view
                                            && let Some(text) = state.read(cx).line_text(side, line)
                                            && state.read(cx).text_selection(cx).is_none()
                                        {
                                            let sel = CodeSelection { path: hit.position.path().clone(), side, start_line: line, end_line: line, text };
                                            this.report_selection(sel, true, cx);
                                        }
                                        if cmd {
                                            this.go_to_definition(hit, cx);
                                        }
                                    })
                                    .ok();
                            }),
                    )
                }
                .into_any_element(),
                FileView::Loading => div().flex_1().into_any_element(),
                FileView::Message(m) => div().flex_1().flex().items_center().justify_center().child(ui::muted(m.clone())).into_any_element(),
            })
            .into_any_element()
    }

    /// Threads on the shown file with no current line (listed above the code, not inline).
    fn outdated_threads(&mut self, cx: &mut Context<Self>) -> Vec<Entity<ThreadView>> {
        let Some(Shown::File(ix)) = self.shown.clone() else { return Vec::new() };
        let Some(path) = self.pr.files.get(ix).map(|f| f.path.clone()) else { return Vec::new() };
        let threads: Vec<ReviewThread> =
            self.review.read(cx).state.iter().flat_map(|s| s.threads.iter()).filter(|t| t.path == path && t.line.is_none()).cloned().collect();
        let review = self.review.clone();
        threads
            .iter()
            .map(|t| self.outdated_views.entry(t.id.clone()).or_insert_with(|| cx.new(|cx| ThreadView::new(review.clone(), t, cx))).clone())
            .collect()
    }

    /// "Finish your review" popover, hanging under the Review button.
    fn render_review_panel(&self) -> Option<impl IntoElement> {
        let panel = self.review_panel.clone()?;
        Some(deferred(div().absolute().top_full().right_0().mt(px(4.)).child(panel)).with_priority(1))
    }

    /// VS Code-style peek list: file, line and the line's code; click to open.
    fn render_peek(&mut self, cx: &mut Context<Self>) -> Option<impl IntoElement> {
        let peek = self.peek.as_ref()?;
        let at = point(peek.point.x.min(px(600.)), peek.point.y + px(12.));
        let title = peek.title.clone();
        let selected = peek.selected.min(peek.items.len().saturating_sub(1));
        let target = peek.items.get(selected).map(|i| (i.abs.clone(), i.label.clone(), i.line));
        let items: Vec<(String, String, usize, String)> = peek.items.iter().map(|i| (i.abs.clone(), i.label.clone(), i.line, i.text.clone())).collect();

        // Code preview of the selected location (about a dozen lines around it).
        let font_size = px(crate::settings::get(cx).editor_font_size);
        let row_h = px(theme::code_row_height(f32::from(font_size)));
        let mut preview = div().flex().flex_col().font_family(theme::MONO_FONT).text_size(font_size).line_height(row_h).whitespace_nowrap();
        if let Some((abs, _, line)) = &target
            && let Some(lines) = self.lines_of(abs)
        {
            let language = language::registry().grammar_for(abs);
            let first = line.saturating_sub(4).max(1);
            let last = (first + 13).min(lines.len());
            for n in first..=last {
                let text = lines.get(n - 1).cloned().unwrap_or_default();
                let highlights = highlight::highlight(language, &text);
                preview = preview.child(
                    div()
                        .flex()
                        .h(row_h)
                        .when(n == *line, |el| el.bg(gpui::rgba(0xff8f0026)))
                        .child(div().w(px(48.)).flex_shrink_0().pr(px(12.)).text_right().text_color(hsla(if n == *line { 0xcccccc } else { 0x6e7681 })).child(n.to_string()))
                        .child(StyledText::new(text).with_highlights(highlights)),
                );
            }
        }

        // Results grouped by file.
        let mut list = div().flex().flex_col().py(px(2.));
        let mut last_file: Option<String> = None;
        for (i, (abs, label, line, text)) in items.iter().enumerate() {
            if last_file.as_deref() != Some(abs.as_str()) {
                let count = items.iter().filter(|x| &x.0 == abs).count();
                let (dir, name) = match label.rfind('/') {
                    Some(k) => (label[..k].to_string(), label[k + 1..].to_string()),
                    None => (String::new(), label.clone()),
                };
                list = list.child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(6.))
                        .px(px(8.))
                        .h(px(22.))
                        .whitespace_nowrap()
                        .child(div().flex_shrink_0().child(name))
                        .child(div().flex_1().min_w_0().truncate().text_size(theme::TEXT_TINY).text_color(hsla(theme::MUTED)).child(dir))
                        .child(div().flex_shrink_0().px(px(6.)).rounded(px(8.)).bg(hsla(0x4d4d4d)).text_size(theme::TEXT_TINY).child(count.to_string())),
                );
                last_file = Some(abs.clone());
            }
            let (abs, line) = (abs.clone(), *line);
            list = list.child(
                div()
                    .id(("peek-item", i))
                    .flex()
                    .gap(px(6.))
                    .pl(px(22.))
                    .pr(px(8.))
                    .h(px(22.))
                    .items_center()
                    .whitespace_nowrap()
                    .cursor_pointer()
                    .when(i == selected, |el| el.bg(hsla(theme::ACTIVE)))
                    .when(i != selected, |el| el.hover(|el| el.bg(hsla(theme::HOVER))))
                    .on_click(cx.listener(move |this, ev: &ClickEvent, _, cx| {
                        // Click previews; double-click opens (as in VS Code's peek).
                        if ev.click_count() >= 2 {
                            this.peek = None;
                            this.open_location(&abs, line, cx);
                        } else if let Some(peek) = this.peek.as_mut() {
                            peek.selected = i;
                            cx.notify();
                        }
                    }))
                    .child(div().flex_1().min_w_0().truncate().font_family(theme::MONO_FONT).text_size(theme::TEXT_SMALL).child(text.clone())),
            );
        }

        let header_title = match &target {
            Some((_, label, _)) => label.clone(),
            None => String::new(),
        };
        Some(deferred(
            anchored().position(at).snap_to_window_with_margin(px(8.)).child(
                div()
                    .id("peek")
                    .occlude()
                    .w(px(880.))
                    .flex()
                    .flex_col()
                    .bg(hsla(0x1f1f1f))
                    .border_t_2()
                    .border_b_2()
                    .border_color(hsla(0x3794ff))
                    .shadow_lg()
                    .text_size(theme::TEXT)
                    .text_color(hsla(theme::FG))
                    .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                        this.peek = None;
                        cx.notify();
                    }))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(8.))
                            .px(px(10.))
                            .h(px(26.))
                            .bg(hsla(0x252526))
                            .child(div().flex_shrink_0().child(header_title))
                            .child(div().flex_1().text_color(hsla(theme::MUTED)).text_size(theme::TEXT_SMALL).child(title))
                            .child(
                                ui::link("peek-close", "✕").on_click(cx.listener(|this, _, _, cx| {
                                    this.peek = None;
                                    cx.notify();
                                })),
                            ),
                    )
                    .child(
                        div()
                            .flex()
                            .h(px(280.))
                            .child(div().id("peek-preview").flex_1().min_w_0().overflow_hidden().pt(px(4.)).cursor_text().child(preview))
                            .child(
                                div()
                                    .id("peek-list")
                                    .w(px(300.))
                                    .flex_shrink_0()
                                    .overflow_y_scroll()
                                    .bg(hsla(0x1f1f1f))
                                    .border_l_1()
                                    .border_color(hsla(theme::BORDER))
                                    .child(list),
                            ),
                    ),
            ),
        ))
    }

    /// VS Code-style hover widget: code blocks as colored code (no box or
    /// language label, like Monaco), text as Markdown, separators between parts.
    fn render_hover(&self, cx: &mut Context<Self>) -> Option<impl IntoElement> {
        let hover = self.hover.as_ref()?;
        let at = point(hover.point.x, hover.point.y + px(10.));
        let mut parts = div().flex().flex_col();
        for (i, part) in hover_parts(&hover.markdown).into_iter().enumerate() {
            let section = div().px(px(8.)).py(px(4.)).cursor_text().when(i > 0, |el| el.border_t_1().border_color(hsla(0x454545)));
            parts = parts.child(match part {
                HoverPart::Code(language, code) => {
                    let language = match language.as_str() {
                        "go" => "go",
                        "c" => "c",
                        "cpp" | "c++" => "cpp",
                        "python" | "py" => "python",
                        "rust" | "rs" => "rust",
                        "bash" | "sh" | "shell" | "shellscript" => "bash",
                        _ => "",
                    };
                    let lines: Vec<AnyElement> = code
                        .lines()
                        .map(|line| {
                            let highlights = if language.is_empty() { Vec::new() } else { highlight::highlight(language, line) };
                            div().child(StyledText::new(line.to_string()).with_highlights(highlights)).into_any_element()
                        })
                        .collect();
                    section
                        .font_family(theme::MONO_FONT)
                        .text_size(px(crate::settings::get(cx).editor_font_size))
                        .line_height(px(19.5))
                        .whitespace_nowrap()
                        .children(lines)
                        .into_any_element()
                }
                HoverPart::Text(md) => section
                    .line_height(px(19.5))
                    .child(TextView::markdown(SharedString::from(format!("lsp-hover-{i}")), md).style(crate::review::markdown_style(cx)).selectable(true))
                    .into_any_element(),
            });
        }
        Some(deferred(
            anchored().position(at).snap_to_window_with_margin(px(8.)).child(
                div()
                    .id("lsp-hover")
                    .occlude()
                    .max_w(px(750.))
                    .max_h(px(360.))
                    .overflow_y_scroll()
                    .py(px(2.))
                    .bg(hsla(0x202020))
                    .border_1()
                    .border_color(hsla(0x454545))
                    .rounded(px(8.))
                    .shadow_md()
                    .text_size(theme::TEXT)
                    .text_color(hsla(theme::FG))
                    .on_hover(cx.listener(|this, hovered: &bool, _, cx| {
                        this.hover_over_popup = *hovered;
                        if !*hovered {
                            this.hover = None;
                            cx.notify();
                        }
                    }))
                    .child(parts),
            ),
        ))
    }
}

enum HoverPart {
    Code(String, String),
    Text(String),
}

/// Splits hover Markdown into fenced code blocks and text, dropping `---` rules
/// (each part gets a separator instead, as in VS Code).
fn hover_parts(markdown: &str) -> Vec<HoverPart> {
    let mut parts = Vec::new();
    let mut text = String::new();
    let mut code: Option<(String, String)> = None;
    let flush = |text: &mut String, parts: &mut Vec<HoverPart>| {
        let t = text.trim();
        if !t.is_empty() {
            parts.push(HoverPart::Text(t.to_string()));
        }
        text.clear();
    };
    for line in markdown.lines() {
        let trimmed = line.trim_start();
        if let Some((lang, body)) = code.as_mut() {
            if trimmed.starts_with("```") {
                parts.push(HoverPart::Code(std::mem::take(lang), body.trim_end().to_string()));
                code = None;
            } else {
                body.push_str(line);
                body.push('\n');
            }
        } else if let Some(lang) = trimmed.strip_prefix("```") {
            flush(&mut text, &mut parts);
            code = Some((lang.trim().to_lowercase(), String::new()));
        } else if matches!(trimmed.trim_end(), "---" | "***" | "___") {
            flush(&mut text, &mut parts);
        } else {
            text.push_str(line);
            text.push('\n');
        }
    }
    if let Some((lang, body)) = code {
        parts.push(HoverPart::Code(lang, body.trim_end().to_string()));
    }
    flush(&mut text, &mut parts);
    parts
}

/// Where the first change in a file's patch is, for scrolling there on open.
fn first_change(patch: &str, path: &str) -> Option<DiffLinePosition> {
    let (mut old_line, mut new_line, mut in_hunk) = (0usize, 0usize, false);
    for line in patch.lines() {
        if let Some(header) = line.strip_prefix("@@ -") {
            // @@ -a[,b] +c[,d] @@
            let mut parts = header.split(' ');
            old_line = parts.next()?.split(',').next()?.parse().ok()?;
            new_line = parts.next()?.trim_start_matches('+').split(',').next()?.parse().ok()?;
            in_hunk = true;
            continue;
        }
        if !in_hunk {
            continue;
        }
        match line.as_bytes().first() {
            Some(b'+') => return Some(DiffLinePosition::new(path.to_string(), DiffSide::Modified, new_line)),
            Some(b'-') => return Some(DiffLinePosition::new(path.to_string(), DiffSide::Original, old_line)),
            Some(b' ') => {
                old_line += 1;
                new_line += 1;
            }
            _ => {}
        }
    }
    None
}

impl Render for PrView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .relative()
            .flex()
            .flex_col()
            .bg(hsla(theme::BG))
            .text_color(hsla(theme::FG))
            .text_size(theme::TEXT)
            .key_context("PrView")
            .track_focus(&self.focus)
            .on_action(cx.listener(Self::go_back))
            .on_action(cx.listener(Self::on_go_to_definition))
            .on_action(cx.listener(Self::on_find_references))
            .on_action(cx.listener(Self::on_peek_definition))
            // While a peek is open: ↑/↓ choose a result, Enter opens it.
            .capture_key_down(cx.listener(|this, ev: &KeyDownEvent, _, cx| {
                let Some(peek) = this.peek.as_mut() else { return };
                let n = peek.items.len();
                match ev.keystroke.key.as_str() {
                    "up" if n > 0 => peek.selected = (peek.selected + n - 1) % n,
                    "down" if n > 0 => peek.selected = (peek.selected + 1) % n,
                    "enter" => {
                        if let Some(item) = peek.items.get(peek.selected) {
                            let (abs, line) = (item.abs.clone(), item.line);
                            this.peek = None;
                            this.open_location(&abs, line, cx);
                        }
                    }
                    _ => return,
                }
                cx.stop_propagation();
                cx.notify();
            }))
            // The mouse's back button goes back, like VS Code.
            .on_mouse_down(
                MouseButton::Navigate(NavigationDirection::Back),
                cx.listener(|this, _, window, cx| this.go_back(&GoBack, window, cx)),
            )
            .on_action(cx.listener(Self::close_popup))
            .on_action(cx.listener(Self::toggle_agent))
            .on_action(cx.listener(|this, _: &AskAgent, window, cx| this.ask_agent(false, window, cx)))
            .on_action(cx.listener(|this, _: &ExplainAgent, window, cx| this.ask_agent(true, window, cx)))
            .on_action(cx.listener(|this, _: &AddComment, window, cx| this.add_comment_at_cursor(window, cx)))
            .on_mouse_move(cx.listener(|this, ev: &MouseMoveEvent, window, cx| {
                if ev.pressed_button != Some(MouseButton::Left) {
                    this.sidebar_drag = None;
                    this.split_drag = false;
                }
                if let Some((x0, w0)) = this.sidebar_drag {
                    let max = (f32::from(window.viewport_size().width) - 400.).max(180.);
                    this.sidebar_width = (w0 + f32::from(ev.position.x - x0)).clamp(180., max);
                    cx.notify();
                }
                if this.split_drag {
                    let b = this.code_bounds.get();
                    if b.size.width > px(100.) {
                        this.split_ratio = (f32::from(ev.position.x - b.left()) / f32::from(b.size.width)).clamp(0.15, 0.85);
                        cx.notify();
                    }
                }
                if let Some((y0, h0)) = this.splitter_drag {
                    if ev.pressed_button != Some(MouseButton::Left) {
                        this.splitter_drag = None;
                        return;
                    }
                    let max = f32::from(window.viewport_size().height) - 200.;
                    this.agent_height = (h0 + f32::from(y0 - ev.position.y)).clamp(120., max.max(120.));
                    cx.notify();
                }
            }))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| {
                    if this.splitter_drag.take().is_some() {
                        let h = this.agent_height;
                        crate::settings::update(cx, |s| s.agent_panel_height = h);
                    }
                    if this.sidebar_drag.take().is_some() {
                        let w = this.sidebar_width;
                        crate::settings::update(cx, |s| s.sidebar_width = w);
                    }
                    if std::mem::take(&mut this.split_drag) {
                        let r = this.split_ratio;
                        crate::settings::update(cx, |s| s.split_ratio = r);
                    }
                }),
            )
            .child(self.render_header(cx))
            .child({
                let main = self.render_main(cx);
                let dock = self.render_agent_dock(cx);
                div()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .child(
                        div()
                            .relative()
                            .h_full()
                            .flex_shrink_0()
                            .child(self.render_sidebar(cx))
                            // Drag the sidebar's edge to resize it.
                            .child(
                                div()
                                    .id("sidebar-resize")
                                    .absolute()
                                    .top_0()
                                    .bottom_0()
                                    .right(px(-3.))
                                    .w(px(6.))
                                    .cursor_col_resize()
                                    .hover(|el| el.bg(hsla(theme::ACCENT)))
                                    .when(self.sidebar_drag.is_some(), |el| el.bg(hsla(theme::ACCENT)))
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(|this, ev: &MouseDownEvent, _, cx| {
                                            this.sidebar_drag = Some((ev.position.x, this.sidebar_width));
                                            cx.stop_propagation();
                                        }),
                                    ),
                            ),
                    )
                    .child(div().flex_1().min_w_0().h_full().flex().flex_col().child(div().flex_1().min_h_0().flex().child(main)).children(dock))
            })
            .children(self.render_hover(cx))
            .children({
                let peek = self.render_peek(cx);
                peek
            })
            .children(self.render_agent_action(cx))
    }
}

/// A plain file as a patch with no changes, so the diff view shows it as one
/// unchanged column (go-to-definition into files the PR didn't touch).
async fn plain_patch(abs: &str) -> Result<String, String> {
    let text = tokio::fs::read_to_string(abs).await.map_err(|e| format!("{abs}: {e}"))?;
    let lines: Vec<&str> = text.lines().collect();
    if lines.is_empty() {
        return Ok(String::new());
    }
    let n = lines.len();
    // No `diff --git` header and no a/ b/ prefixes: the parser keeps the absolute path as is.
    let mut out = format!("--- {abs}\n+++ {abs}\n@@ -1,{n} +1,{n} @@\n");
    for line in lines {
        out.push(' ');
        out.push_str(line);
        out.push('\n');
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    #[test]
    fn hover_parts_split_code_text_and_rules() {
        let md = "```go\nfunc os.Getenv(key string) string\n```\n\n---\n\nGetenv retrieves the value.\n\n[`os.Getenv` on pkg.go.dev](https://pkg.go.dev/os#Getenv)";
        let parts = super::hover_parts(md);
        assert_eq!(parts.len(), 2);
        assert!(matches!(&parts[0], super::HoverPart::Code(l, c) if l == "go" && c == "func os.Getenv(key string) string"));
        assert!(matches!(&parts[1], super::HoverPart::Text(t) if t.starts_with("Getenv retrieves") && t.contains("pkg.go.dev")));
    }

    #[tokio::test]
    async fn plain_patch_keeps_absolute_path() {
        let abs = concat!(env!("CARGO_MANIFEST_DIR"), "/Cargo.toml");
        let patch = super::plain_patch(abs).await.unwrap();
        let files = crate::diffview::DiffFile::parse(&patch).unwrap();
        assert_eq!(files[0].path().as_ref(), abs);
    }
}
