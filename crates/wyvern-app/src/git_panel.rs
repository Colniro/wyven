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
use wyvern_git::{BlameLine, ChangeKind, Repo, Status};
use wyvern_render::{Attrs, Buffer, Family, FontSystem, Metrics, Quad, Rgba, Shaping, TextBlock, Wrap};

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
}

/// What the app should do after input in the panel.
pub enum GitAction {
    None,
    Commit,
    OpenDiff(String),
    Close,
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
}

impl GitPanel {
    pub fn new(fs: &mut FontSystem, font: FontMetrics, family: String) -> Self {
        let m = Metrics::new(font.size, font.line_h);
        let mut top = Buffer::new(fs, m);
        let mut list = Buffer::new(fs, m);
        top.set_wrap(Wrap::None);
        list.set_wrap(Wrap::None);
        let (tx, rx) = mpsc::channel();
        Self {
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
        self.busy = Some("Commit …".into());
        self.spawn(move |r| GitResult::Committed(r.commit_all(&msg)));
    }

    pub fn sync(&mut self) {
        self.busy = Some("Synchronisiere …".into());
        self.spawn(|r| GitResult::Synced(r.sync()));
    }

    pub fn branches(&self) {
        self.spawn(|r| GitResult::Branches(r.branches()));
    }

    pub fn switch(&mut self, branch: String, create: bool) {
        self.busy = Some(format!("Wechsle zu „{branch}“ …"));
        self.spawn(move |r| {
            let res = if create { r.create_branch(&branch) } else { r.switch(&branch) };
            GitResult::Switched(res.map(|_| branch))
        });
    }

    pub fn blame(&self, file: &Path, line: usize, x: f32, y: f32) {
        let file = file.to_path_buf();
        self.spawn(move |r| {
            let res = match r.relative(&file) {
                Some(rel) => r.blame_line(&rel, line),
                None => Err("Datei liegt nicht im Git-Repository".into()),
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
                GitResult::Committed(_) | GitResult::Synced(_) | GitResult::Switched(_) => self.busy = None,
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

    pub fn click(&mut self, y: f32) -> GitAction {
        if y < self.list_y {
            return GitAction::None;
        }
        let i = self.scroll + ((y - self.list_y) / self.font.line_h) as usize;
        if i < self.change_count() {
            self.selected = i;
            return self.selected_path().map_or(GitAction::None, GitAction::OpenDiff);
        }
        GitAction::None
    }

    pub fn prepare(&mut self, fs: &mut FontSystem, theme: &Theme, focused: bool) {
        let attrs = Attrs::new().family(Family::Name(&self.family));
        let mut top: Vec<(String, Rgba)> = Vec::new();
        match (&self.repo, &self.status) {
            (None, _) => {
                top.push(("GIT\n".into(), theme.panel_dim));
                top.push(("Kein Git-Repository in diesem Ordner.\n".into(), theme.panel_text));
                top.push(("Im Terminal: git init\n".into(), theme.panel_dim));
            }
            (Some(_), status) => {
                let (branch, sync) = match status {
                    Some(s) if s.has_upstream => (s.branch.clone(), format!("   ↑{} ↓{}", s.ahead, s.behind)),
                    Some(s) => (s.branch.clone(), "   (nicht veröffentlicht)".into()),
                    None => ("…".into(), String::new()),
                };
                top.push(("GIT  ·  ".into(), theme.panel_dim));
                top.push((format!("⎇ {branch}"), theme.accent));
                top.push((format!("{sync}\n"), theme.panel_dim));
                let (msg, color) = if self.message.is_empty() {
                    ("Commit-Nachricht …".to_string(), theme.panel_dim)
                } else {
                    (self.message.clone(), if focused { theme.tab_text_active } else { theme.panel_text })
                };
                top.push((format!("{msg}\n"), color));
                let hint = match &self.busy {
                    Some(b) => b.clone(),
                    None => "Strg+Enter: alles committen  ·  Alt → g → s: Sync".into(),
                };
                top.push((format!("{hint}\n"), if self.busy.is_some() { theme.accent } else { theme.panel_dim }));
                let n = self.change_count();
                top.push((format!("Änderungen ({n})\n"), theme.panel_dim));
            }
        }
        self.top.set_rich_text(
            top.iter().map(|(t, c)| (t.as_str(), attrs.clone().color(c.to_glyphon()))),
            &attrs,
            Shaping::Advanced,
            None,
        );
        self.top.shape_until_scroll(fs, false);

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
        self.list.set_rich_text(
            spans.iter().map(|(t, c)| (t.as_str(), attrs.clone().color(c.to_glyphon()))),
            &attrs,
            Shaping::Advanced,
            None,
        );
        self.list.shape_until_scroll(fs, false);
    }

    pub fn draw<'a>(&'a mut self, theme: &Theme, focused: bool, under: &mut Vec<Quad>, text: &mut Vec<TextBlock<'a>>) {
        let a = self.area;
        let lh = self.font.line_h;
        let pad = self.font.cell_w;
        under.push(Quad { x: a.x, y: a.y, w: a.w, h: a.h, color: theme.panel_bg });
        let top_y = a.y + lh * 0.2;
        if self.repo.is_some() {
            // Message input background and cursor.
            let y = top_y + lh;
            let color = if focused { theme.row_selected } else { theme.popup_bg };
            under.push(Quad { x: a.x + pad * 0.5, y, w: a.w - pad, h: lh, color });
            if focused {
                let x = a.x + pad + self.message.chars().count() as f32 * pad;
                under.push(Quad { x, y, w: 2.0, h: lh, color: theme.cursor });
            }
        }
        let header_rows = if self.repo.is_some() { 4.0 } else { 3.0 };
        self.list_y = top_y + header_rows * lh + lh * 0.3;
        if self.change_count() > 0 && self.selected >= self.scroll && self.selected < self.scroll + self.visible_rows() {
            let y = self.list_y + (self.selected - self.scroll) as f32 * lh;
            let color = if focused { theme.row_selected_unfocused } else { theme.panel_bg };
            under.push(Quad { x: a.x, y, w: a.w, h: lh, color });
        }
        let clip = Some([a.x as i32, a.y as i32, (a.x + a.w) as i32, (a.y + a.h) as i32]);
        text.push(TextBlock { buffer: &self.top, x: a.x + pad, y: top_y, color: theme.panel_text, clip });
        text.push(TextBlock { buffer: &self.list, x: a.x + pad, y: self.list_y, color: theme.panel_text, clip });
    }
}
