//! The git panel in the left sidebar, and the background worker for all git commands.
//!
//! Every git call runs on its own thread; results come back as [`GitResult`] through a channel
//! and a wake-up of the event loop, so pushing over a slow network never blocks typing.

use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::Arc;
use std::time::{Duration, Instant};

use winit::event::KeyEvent;
use winit::keyboard::{Key, ModifiersState, NamedKey};
use winit::platform::modifier_supplement::KeyEventExtModifierSupplement;
use wyven_git::{BlameLine, ChangeKind, Repo, Status};
use wyven_render::{Attrs, Buffer, Family, FontSystem, Metrics, Quad, Rgba, Shaping, TextBlock, Wrap};

use crate::editor::{FontMetrics, Rect};
use crate::keymap::Keymap;
use crate::theme::Theme;

/// How often the status is refreshed while the panel is visible, and how often we fetch.
pub const STATUS_INTERVAL: Duration = Duration::from_secs(5);
pub const FETCH_INTERVAL: Duration = Duration::from_secs(180);

pub enum GitResult {
    Status(Result<Status, String>),
    Committed(Result<String, String>),
    Synced(Result<String, String>),
    Branches(Result<Vec<String>, String>),
    Switched(Result<String, String>),
    /// Committed content of a file (`None`: new file or not in the repository).
    Head(PathBuf, Option<String>),
    Blame(Result<BlameLine, String>, f32, f32),
    /// Changes of a file thrown away (its relative path).
    Discarded(Result<String, String>),
}

/// What the app should do after input in the panel.
pub enum GitAction {
    None,
    Commit,
    OpenDiff(String),
    OpenFile(String),
    /// Ask before throwing away the changes of a file.
    Discard(String, ChangeKind),
    Sync,
    Branches,
    Close,
    /// A right-click on a changed file (its path) at the mouse.
    Menu(String, ChangeKind),
}

/// A clickable part of the panel.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GitHover {
    Branch,
    Sync,
    Message,
    Commit,
    /// A changed file (index), and one of its buttons: 0 none, 1 open file, 2 discard.
    Row(usize, u8),
}

pub struct GitPanel {
    pub repo: Option<Arc<Repo>>,
    pub status: Option<Status>,
    /// A long-running command (commit, sync, switch) is in progress.
    pub busy: Option<String>,
    pub message: String,
    selected: usize,
    scroll: usize,
    tx: Sender<GitResult>,
    rx: Receiver<GitResult>,
    wake: Option<Arc<dyn Fn() + Send + Sync>>,
    pub last_status: Option<Instant>,
    pub last_fetch: Option<Instant>,
    area: Rect,
    font: FontMetrics,
    family: String,
    top: Buffer,
    list: Buffer,
    list_y: f32,
    /// Shaped labels: branch, sync, commit button, changes heading, icons (open, discard).
    branch_buf: Buffer,
    sync_buf: Buffer,
    commit_buf: Buffer,
    heading_buf: Buffer,
    icon_open: Buffer,
    icon_discard: Buffer,
    pub hover: Option<GitHover>,
    /// Where the parts were drawn.
    branch_rect: Rect,
    sync_rect: Rect,
    field_rect: Rect,
    commit_rect: Rect,
}

impl GitPanel {
    pub fn new(fs: &mut FontSystem, font: FontMetrics, family: String) -> Self {
        let m = Metrics::new(font.size, font.line_h);
        let mut top = Buffer::new(fs, m);
        let mut list = Buffer::new(fs, m);
        top.set_wrap(Wrap::None);
        list.set_wrap(Wrap::None);
        let (tx, rx) = mpsc::channel();
        let mk = |fs: &mut FontSystem| {
            let mut b = Buffer::new(fs, m);
            b.set_wrap(Wrap::None);
            b
        };
        let icons = crate::chrome::icon_family(fs);
        let isz = (font.size * 0.95).round();
        let icon_open = crate::chrome::label(fs, &crate::chrome::ICON_OUTPUT.to_string(), &icons, isz, isz * 1.2);
        let icon_discard = crate::chrome::label(fs, &crate::chrome::ICON_UNDO.to_string(), &icons, isz, isz * 1.2);
        let (branch_buf, sync_buf, commit_buf, heading_buf) = (mk(fs), mk(fs), mk(fs), mk(fs));
        Self {
            branch_buf,
            sync_buf,
            commit_buf,
            heading_buf,
            icon_open,
            icon_discard,
            hover: None,
            branch_rect: Rect::default(),
            sync_rect: Rect::default(),
            field_rect: Rect::default(),
            commit_rect: Rect::default(),
            repo: None,
            status: None,
            busy: None,
            message: String::new(),
            selected: 0,
            scroll: 0,
            tx,
            rx,
            wake: None,
            last_status: None,
            last_fetch: None,
            area: Rect::default(),
            font,
            family,
            top,
            list,
            list_y: 0.0,
        }
    }

    pub fn set_font(&mut self, font: FontMetrics) {
        self.font = font;
        let m = Metrics::new(font.size, font.line_h);
        self.top.set_metrics(m);
        self.list.set_metrics(m);
        for b in [&mut self.branch_buf, &mut self.sync_buf, &mut self.commit_buf, &mut self.heading_buf] {
            b.set_metrics(m);
        }
    }

    pub fn set_area(&mut self, area: Rect) {
        self.area = area;
    }

    pub fn contains(&self, x: f32, y: f32) -> bool {
        let a = self.area;
        x >= a.x && x < a.x + a.w && y >= a.y && y < a.y + a.h
    }

    pub fn set_wake(&mut self, wake: impl Fn() + Send + Sync + 'static) {
        self.wake = Some(Arc::new(wake));
    }

    /// Looks for a repository containing the project folder.
    pub fn open_repo(&mut self, root: &Path) {
        self.repo = Repo::open(root).map(Arc::new);
        self.status = None;
        self.last_status = None;
        self.last_fetch = None;
        if self.repo.is_some() {
            self.refresh_status();
        }
    }

    fn spawn(&self, job: impl FnOnce(&Repo) -> GitResult + Send + 'static) {
        let (Some(repo), Some(wake)) = (self.repo.clone(), self.wake.clone()) else { return };
        let tx = self.tx.clone();
        std::thread::Builder::new()
            .name("git".into())
            .spawn(move || {
                if tx.send(job(&repo)).is_ok() {
                    wake();
                }
            })
            .ok();
    }

    pub fn refresh_status(&mut self) {
        self.last_status = Some(Instant::now());
        self.spawn(|r| GitResult::Status(r.status()));
    }

    pub fn fetch(&mut self) {
        self.last_fetch = Some(Instant::now());
        // Fetch quietly, then report the new ahead/behind numbers.
        self.spawn(|r| {
            let _ = r.fetch();
            GitResult::Status(r.status())
        });
    }

    pub fn load_head(&self, file: &Path) {
        let file = file.to_path_buf();
        self.spawn(move |r| {
            let content = r.relative(&file).and_then(|rel| r.head_content(&rel));
            GitResult::Head(file, content)
        });
    }

    pub fn commit(&mut self) {
        let msg = self.message.trim().to_string();
        self.busy = Some("Committing …".into());
        self.spawn(move |r| GitResult::Committed(r.commit_all(&msg)));
    }

    pub fn sync(&mut self) {
        self.busy = Some("Syncing …".into());
        self.spawn(|r| GitResult::Synced(r.sync()));
    }

    pub fn branches(&self) {
        self.spawn(|r| GitResult::Branches(r.branches()));
    }

    pub fn switch(&mut self, branch: String, create: bool) {
        self.busy = Some(format!("Switching to “{branch}” …"));
        self.spawn(move |r| {
            let res = if create { r.create_branch(&branch) } else { r.switch(&branch) };
            GitResult::Switched(res.map(|_| branch))
        });
    }

    pub fn discard(&mut self, rel: String, kind: ChangeKind) {
        self.busy = Some(format!("Discarding {rel} …"));
        self.spawn(move |r| {
            let mut res = r.discard(&rel, kind);
            // New files go to the recycle bin.
            if res.is_ok() && matches!(kind, ChangeKind::Untracked | ChangeKind::Added) {
                res = trash::delete(r.root().join(&rel)).map_err(|e| e.to_string());
            }
            GitResult::Discarded(res.map(|_| rel))
        });
    }

    fn change(&self, i: usize) -> Option<(String, ChangeKind)> {
        self.status.as_ref()?.changes.get(i).map(|c| (c.path.clone(), c.kind))
    }

    /// What is under the mouse.
    pub fn hover_at(&self, x: f32, y: f32) -> Option<GitHover> {
        let inside = |r: &Rect| x >= r.x && x < r.x + r.w && y >= r.y && y < r.y + r.h;
        if !self.contains(x, y) || self.repo.is_none() {
            return None;
        }
        if inside(&self.sync_rect) {
            return Some(GitHover::Sync);
        }
        if inside(&self.branch_rect) {
            return Some(GitHover::Branch);
        }
        if inside(&self.field_rect) {
            return Some(GitHover::Message);
        }
        if inside(&self.commit_rect) {
            return Some(GitHover::Commit);
        }
        if y >= self.list_y {
            let i = self.scroll + ((y - self.list_y) / self.font.line_h) as usize;
            if i < self.change_count() {
                let right = self.area.x + self.area.w;
                let b = self.font.line_h;
                let button = if x >= right - b - self.font.cell_w * 0.5 {
                    2
                } else if x >= right - 2.0 * b - self.font.cell_w * 0.5 {
                    1
                } else {
                    0
                };
                return Some(GitHover::Row(i, button));
            }
        }
        None
    }

    /// A click (`right`: right mouse button).
    pub fn click_at(&mut self, x: f32, y: f32, right: bool) -> GitAction {
        match self.hover_at(x, y) {
            Some(GitHover::Sync) if !right => GitAction::Sync,
            Some(GitHover::Branch) if !right => GitAction::Branches,
            Some(GitHover::Commit) if !right => GitAction::Commit,
            Some(GitHover::Row(i, b)) => {
                self.selected = i;
                let Some((path, kind)) = self.change(i) else { return GitAction::None };
                match (right, b) {
                    (true, _) => GitAction::Menu(path, kind),
                    (false, 1) => GitAction::OpenFile(path),
                    (false, 2) => GitAction::Discard(path, kind),
                    _ => GitAction::OpenDiff(path),
                }
            }
            _ => GitAction::None,
        }
    }

    pub fn blame(&self, file: &Path, line: usize, x: f32, y: f32) {
        let file = file.to_path_buf();
        self.spawn(move |r| {
            let res = match r.relative(&file) {
                Some(rel) => r.blame_line(&rel, line),
                None => Err("File is not in the git repository".into()),
            };
            GitResult::Blame(res, x, y)
        });
    }

    pub fn poll(&mut self) -> Vec<GitResult> {
        let mut out = Vec::new();
        while let Ok(r) = self.rx.try_recv() {
            match &r {
                GitResult::Status(Ok(s)) => {
                    self.status = Some(s.clone());
                    let n = s.changes.len();
                    if n > 0 {
                        self.selected = self.selected.min(n - 1);
                    }
                }
                GitResult::Committed(_) | GitResult::Synced(_) | GitResult::Switched(_) | GitResult::Discarded(_) => self.busy = None,
                _ => {}
            }
            out.push(r);
        }
        out
    }

    fn change_count(&self) -> usize {
        self.status.as_ref().map_or(0, |s| s.changes.len())
    }

    fn visible_rows(&self) -> usize {
        (((self.area.y + self.area.h - self.list_y) / self.font.line_h).floor() as usize).max(1)
    }

    fn move_selection(&mut self, delta: isize) {
        let n = self.change_count();
        if n == 0 {
            return;
        }
        self.selected = (self.selected as isize + delta).clamp(0, n as isize - 1) as usize;
        let vis = self.visible_rows();
        if self.selected < self.scroll {
            self.scroll = self.selected;
        } else if self.selected >= self.scroll + vis {
            self.scroll = self.selected + 1 - vis;
        }
    }

    pub fn scroll_by(&mut self, lines: isize) {
        let max = self.change_count().saturating_sub(self.visible_rows()) as isize;
        self.scroll = (self.scroll as isize + lines).clamp(0, max.max(0)) as usize;
    }

    fn selected_path(&self) -> Option<String> {
        self.status.as_ref()?.changes.get(self.selected).map(|c| c.path.clone())
    }

    pub fn paste(&mut self, text: &str) {
        self.message.push_str(text.lines().next().unwrap_or(""));
    }

    pub fn key(&mut self, event: &KeyEvent, mods: ModifiersState) -> GitAction {
        let ctrl = mods.control_key() && !mods.alt_key();
        match event.key_without_modifiers() {
            Key::Named(NamedKey::Escape) => GitAction::Close,
            Key::Named(NamedKey::Enter) if ctrl => GitAction::Commit,
            Key::Named(NamedKey::Enter) => self.selected_path().map_or(GitAction::None, GitAction::OpenDiff),
            Key::Named(NamedKey::ArrowDown) => {
                self.move_selection(1);
                GitAction::None
            }
            Key::Named(NamedKey::ArrowUp) => {
                self.move_selection(-1);
                GitAction::None
            }
            Key::Named(NamedKey::Backspace) => {
                if ctrl {
                    self.message.clear();
                } else {
                    self.message.pop();
                }
                GitAction::None
            }
            _ => {
                if let Some(t) = Keymap::text(event, mods) {
                    self.message.push_str(&t);
                }
                GitAction::None
            }
        }
    }

    pub fn prepare(&mut self, fs: &mut FontSystem, theme: &Theme, focused: bool) {
        let attrs = Attrs::new().family(Family::Name(&self.family));
        let set = |b: &mut Buffer, fs: &mut FontSystem, spans: &[(String, Rgba)]| {
            b.set_rich_text(spans.iter().map(|(t, c)| (t.as_str(), attrs.clone().color(c.to_glyphon()))), &attrs, Shaping::Advanced, None);
            b.shape_until_scroll(fs, false);
        };
        match (&self.repo, &self.status) {
            (None, _) => {
                let top = [
                    ("No git repository in this folder.\n".to_string(), theme.panel_text),
                    ("In the terminal: git init\n".to_string(), theme.panel_dim),
                ];
                set(&mut self.top, fs, &top);
            }
            (Some(_), status) => {
                let (branch, sync) = match status {
                    Some(s) if s.has_upstream => (s.branch.clone(), format!("↑{} ↓{}", s.ahead, s.behind)),
                    Some(s) => (s.branch.clone(), "Publish".into()),
                    None => ("…".into(), String::new()),
                };
                set(&mut self.branch_buf, fs, &[(format!("⎇ {branch}  ˅"), theme.accent)]);
                set(&mut self.sync_buf, fs, &[(format!("⟳ {}", if sync.is_empty() { "Sync".into() } else { sync }), theme.panel_text)]);
                let (msg, color) = if self.message.is_empty() {
                    ("Message (Ctrl+Enter commits)".to_string(), theme.panel_dim)
                } else {
                    (self.message.clone(), if focused { theme.tab_text_active } else { theme.panel_text })
                };
                set(&mut self.top, fs, &[(msg, color)]);
                let n = self.change_count();
                let label = match &self.busy {
                    Some(b) => b.clone(),
                    None if n == 0 => "Nothing to commit".into(),
                    None => format!("✓ Commit {n} change{}", if n == 1 { "" } else { "s" }),
                };
                set(&mut self.commit_buf, fs, &[(label, theme.panel_text)]);
                set(&mut self.heading_buf, fs, &[(format!("CHANGES  {n}"), theme.panel_dim)]);
            }
        }

        let mut spans: Vec<(String, Rgba)> = Vec::new();
        if let Some(s) = &self.status {
            let vis = self.visible_rows();
            for c in s.changes.iter().skip(self.scroll).take(vis) {
                let color = match c.kind {
                    ChangeKind::Added | ChangeKind::Untracked => theme.git_added,
                    ChangeKind::Deleted | ChangeKind::Conflict => theme.git_deleted,
                    _ => theme.git_modified,
                };
                let (dir, name) = match c.path.rsplit_once('/') {
                    Some((d, n)) => (d.to_string(), n.to_string()),
                    None => (String::new(), c.path.clone()),
                };
                spans.push((format!("{} ", c.kind.letter()), color));
                spans.push((name, theme.panel_text));
                spans.push((if dir.is_empty() { "\n".into() } else { format!("  {dir}\n") }, theme.panel_dim));
            }
        }
        set(&mut self.list, fs, &spans);
    }

    pub fn draw<'a>(&'a mut self, theme: &Theme, focused: bool, under: &mut Vec<Quad>, text: &mut Vec<TextBlock<'a>>) {
        let a = self.area;
        let lh = self.font.line_h;
        let pad = self.font.cell_w;
        under.push(Quad { x: a.x, y: a.y, w: a.w, h: a.h, color: theme.panel_bg });
        let clip = Some([a.x as i32, a.y as i32, (a.x + a.w) as i32, (a.y + a.h) as i32]);
        let top_y = a.y + lh * 0.4;
        let width = |b: &Buffer| b.layout_runs().next().map_or(0.0, |r| r.line_w);
        let hover = self.hover;
        if self.repo.is_none() {
            self.list_y = a.y + a.h;
            text.push(TextBlock { buffer: &self.top, x: a.x + pad, y: top_y, color: theme.panel_text, clip });
            return;
        }
        let (x0, w) = (a.x + pad * 0.6, a.w - pad * 1.2);
        let btn = |r: Rect, on: bool, base: Rgba, under: &mut Vec<Quad>| {
            under.push(Quad { x: r.x, y: r.y, w: r.w, h: r.h, color: if on { base.with_alpha(0xFF) } else { base } });
        };
        // Branch (opens the branch list) and sync, side by side.
        let row_h = (lh * 1.3).round();
        let sync_w = (width(&self.sync_buf) + pad * 1.6).min(w * 0.45);
        self.sync_rect = Rect { x: x0 + w - sync_w, y: top_y, w: sync_w, h: row_h };
        self.branch_rect = Rect { x: x0, y: top_y, w: (w - sync_w - pad * 0.5).max(0.0), h: row_h };
        let soft = theme.panel_text.with_alpha(0x14);
        btn(self.branch_rect, hover == Some(GitHover::Branch), soft, under);
        btn(self.sync_rect, hover == Some(GitHover::Sync), soft, under);
        if hover == Some(GitHover::Branch) {
            under.push(Quad { x: self.branch_rect.x, y: self.branch_rect.y, w: self.branch_rect.w, h: self.branch_rect.h, color: theme.panel_text.with_alpha(0x22) });
        }
        if hover == Some(GitHover::Sync) {
            under.push(Quad { x: self.sync_rect.x, y: self.sync_rect.y, w: self.sync_rect.w, h: self.sync_rect.h, color: theme.panel_text.with_alpha(0x22) });
        }
        let ty = |r: Rect| r.y + (r.h - lh) / 2.0;
        let r = self.branch_rect;
        text.push(TextBlock { buffer: &self.branch_buf, x: r.x + pad * 0.6, y: ty(r), color: theme.accent, clip: Some([r.x as i32, r.y as i32, (r.x + r.w) as i32, (r.y + r.h) as i32]) });
        let r = self.sync_rect;
        text.push(TextBlock { buffer: &self.sync_buf, x: r.x + pad * 0.8, y: ty(r), color: theme.panel_text, clip: Some([r.x as i32, r.y as i32, (r.x + r.w) as i32, (r.y + r.h) as i32]) });
        // The message field (a box; with the focus a colored edge and the cursor).
        let fy = top_y + row_h + lh * 0.5;
        self.field_rect = Rect { x: x0, y: fy, w, h: row_h };
        let edge = if focused { theme.accent } else { theme.divider };
        under.push(Quad { x: x0, y: fy, w, h: row_h, color: edge });
        under.push(Quad { x: x0 + 1.0, y: fy + 1.0, w: w - 2.0, h: row_h - 2.0, color: theme.background });
        let r = self.field_rect;
        text.push(TextBlock { buffer: &self.top, x: r.x + pad * 0.6, y: ty(r), color: theme.panel_text, clip: Some([r.x as i32, r.y as i32, (r.x + r.w - pad * 0.3) as i32, (r.y + r.h) as i32]) });
        if focused {
            let cx = (r.x + pad * 0.6 + if self.message.is_empty() { 0.0 } else { width(&self.top) }).min(r.x + r.w - pad);
            under.push(Quad { x: cx, y: ty(r), w: 2.0, h: lh, color: theme.cursor });
        }
        // The commit button.
        let cy = fy + row_h + lh * 0.4;
        self.commit_rect = Rect { x: x0, y: cy, w, h: row_h };
        let can = self.change_count() > 0 && self.busy.is_none();
        let base = if can { theme.accent.with_alpha(if hover == Some(GitHover::Commit) { 0xFF } else { 0xD0 }) } else { theme.panel_text.with_alpha(0x18) };
        under.push(Quad { x: x0, y: cy, w, h: row_h, color: base });
        let cw = width(&self.commit_buf);
        let r = self.commit_rect;
        text.push(TextBlock {
            buffer: &self.commit_buf,
            x: (r.x + (r.w - cw) / 2.0).max(r.x + pad * 0.5).round(),
            y: ty(r),
            color: if can { theme.background } else { theme.panel_dim },
            clip: Some([r.x as i32, r.y as i32, (r.x + r.w) as i32, (r.y + r.h) as i32]),
        });
        // The changes.
        let hy = cy + row_h + lh * 0.6;
        text.push(TextBlock { buffer: &self.heading_buf, x: x0, y: hy, color: theme.panel_dim, clip });
        self.list_y = hy + lh * 1.2;
        let vis = self.visible_rows();
        if self.change_count() > 0 && self.selected >= self.scroll && self.selected < self.scroll + vis {
            let y = self.list_y + (self.selected - self.scroll) as f32 * lh;
            let color = if focused { theme.row_selected_unfocused } else { theme.panel_bg };
            under.push(Quad { x: a.x, y, w: a.w, h: lh, color });
        }
        let mut list_clip = [a.x as i32, a.y as i32, (a.x + a.w) as i32, (a.y + a.h) as i32];
        if let Some(GitHover::Row(i, b)) = hover
            && i >= self.scroll
            && i < self.scroll + vis
        {
            let y = self.list_y + (i - self.scroll) as f32 * lh;
            under.push(Quad { x: a.x, y, w: a.w, h: lh, color: theme.panel_text.with_alpha(0x12) });
            // Open file and discard, at the right end of the row.
            let size = (self.font.size * 0.95).round();
            for (k, icon) in [(1u8, &self.icon_open), (2u8, &self.icon_discard)] {
                let bx = a.x + a.w - (3 - k) as f32 * lh - pad * 0.5;
                if b == k {
                    under.push(Quad { x: bx, y, w: lh, h: lh, color: theme.panel_text.with_alpha(0x28) });
                }
                let iw = width(icon).max(size * 0.8);
                text.push(TextBlock { buffer: icon, x: (bx + (lh - iw) / 2.0).round(), y: (y + (lh - size * 1.2) / 2.0).round(), color: if b == k { theme.panel_text } else { theme.panel_dim }, clip: None });
            }
            list_clip[2] = (a.x + a.w - 2.0 * lh - pad) as i32;
        }
        text.push(TextBlock { buffer: &self.list, x: a.x + pad, y: self.list_y, color: theme.panel_text, clip: Some(list_clip) });
    }
}
