//! The file explorer: a folder tree on the left. With several workspace folders each one is a
//! row of its own (with a × to remove it from the workspace).
//!
//! Only expanded folders are read from disk, so even huge projects open instantly. The tree is
//! re-read when the window gets focus again (files changed outside Wyven); a real file watcher
//! comes in Phase 2.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use wyven_render::{Attrs, Buffer, Family, FontSystem, Metrics, Quad, Rgba, Shaping, TextBlock, Wrap};

use crate::editor::{FontMetrics, Rect};
use crate::theme::Theme;

/// Folders that are hidden because they only contain generated files.
pub const IGNORED: &[&str] = &[
    ".git", ".vs", ".vscode", ".idea", ".godot", ".import", "bin", "obj", "target",
    "node_modules", "__pycache__",
];

#[derive(Debug, Clone)]
pub struct Row {
    pub path: PathBuf,
    pub name: String,
    pub depth: usize,
    pub is_dir: bool,
    pub expanded: bool,
    /// A workspace folder's own row (only with several folders).
    pub is_root: bool,
}

pub struct Explorer {
    /// The workspace folders; the first is the main folder.
    roots: Vec<PathBuf>,
    /// Header with several folders: the workspace file's name, else "WORKSPACE".
    title: Option<String>,
    expanded: HashSet<PathBuf>,
    rows: Vec<Row>,
    pub selected: usize,
    scroll: usize,
    area: Rect,
    font: FontMetrics,
    family: String,
    buffer: Buffer,
    header: Buffer,
    /// The × behind each folder row.
    close: Buffer,
    dirty: bool,
}

impl Explorer {
    pub fn new(root: PathBuf, fs: &mut FontSystem, font: FontMetrics, family: String) -> Self {
        let metrics = Metrics::new(font.size, font.line_h);
        let mut buffer = Buffer::new(fs, metrics);
        let mut header = Buffer::new(fs, metrics);
        let mut close = Buffer::new(fs, metrics);
        buffer.set_wrap(Wrap::None);
        header.set_wrap(Wrap::None);
        close.set_wrap(Wrap::None);
        let mut ex = Self {
            expanded: HashSet::from([root.clone()]),
            roots: vec![root],
            title: None,
            rows: Vec::new(),
            selected: 0,
            scroll: 0,
            area: Rect::default(),
            font,
            family,
            buffer,
            header,
            close,
            dirty: true,
        };
        ex.refresh();
        ex
    }

    /// The main folder (the first one).
    pub fn root(&self) -> &Path {
        &self.roots[0]
    }

    pub fn roots(&self) -> &[PathBuf] {
        &self.roots
    }

    fn multi(&self) -> bool {
        self.roots.len() > 1
    }

    /// Adds a workspace folder (expanded). Returns false if it is already there.
    pub fn add_root(&mut self, path: PathBuf) -> bool {
        if self.roots.iter().any(|r| same_path(r, &path)) {
            return false;
        }
        self.expanded.insert(path.clone());
        self.roots.push(path);
        self.refresh();
        true
    }

    /// Removes a workspace folder; the last one stays. Returns false if nothing changed.
    pub fn remove_root(&mut self, path: &Path) -> bool {
        if self.roots.len() < 2 {
            return false;
        }
        let before = self.roots.len();
        self.roots.retain(|r| !same_path(r, path));
        self.refresh();
        self.roots.len() != before
    }

    pub fn set_title(&mut self, title: Option<String>) {
        self.title = title;
        self.dirty = true;
    }

    /// The workspace folder a path is in.
    pub fn root_of(&self, path: &Path) -> Option<&Path> {
        self.roots.iter().map(PathBuf::as_path).filter(|r| starts_with(path, r)).max_by_key(|r| r.as_os_str().len())
    }

    pub fn set_area(&mut self, area: Rect) {
        if area != self.area {
            self.area = area;
            self.dirty = true;
        }
    }

    pub fn set_font(&mut self, font: FontMetrics) {
        self.font = font;
        let m = Metrics::new(font.size, font.line_h);
        self.buffer.set_metrics(m);
        self.header.set_metrics(m);
        self.dirty = true;
    }

    /// Re-reads all expanded folders from disk, keeping the selection on the same path.
    pub fn refresh(&mut self) {
        let selected = self.rows.get(self.selected).map(|r| r.path.clone());
        self.rows.clear();
        if self.multi() {
            for root in self.roots.clone() {
                let expanded = self.expanded.contains(&root);
                let name = root.file_name().map_or_else(|| root.display().to_string(), |n| n.to_string_lossy().into_owned());
                self.rows.push(Row { path: root.clone(), name, depth: 0, is_dir: true, expanded, is_root: true });
                if expanded {
                    self.walk(&root, 1);
                }
            }
        } else {
            let root = self.roots[0].clone();
            self.walk(&root, 0);
        }
        if let Some(p) = selected
            && let Some(i) = self.rows.iter().position(|r| r.path == p) {
                self.selected = i;
            }
        self.selected = self.selected.min(self.rows.len().saturating_sub(1));
        self.dirty = true;
    }

    fn walk(&mut self, dir: &Path, depth: usize) {
        let Ok(read) = std::fs::read_dir(dir) else { return };
        let mut entries: Vec<(String, PathBuf, bool)> = read
            .filter_map(Result::ok)
            .filter_map(|e| {
                let name = e.file_name().to_string_lossy().into_owned();
                let is_dir = e.file_type().ok()?.is_dir();
                if is_dir && IGNORED.iter().any(|i| i.eq_ignore_ascii_case(&name)) {
                    return None;
                }
                Some((name, e.path(), is_dir))
            })
            .collect();
        // Folders first, then case-insensitive by name.
        entries.sort_by(|a, b| b.2.cmp(&a.2).then_with(|| a.0.to_lowercase().cmp(&b.0.to_lowercase())));
        for (name, path, is_dir) in entries {
            let expanded = is_dir && self.expanded.contains(&path);
            self.rows.push(Row { path: path.clone(), name, depth, is_dir, expanded, is_root: false });
            if expanded {
                self.walk(&path, depth + 1);
            }
        }
    }

    fn rows_visible(&self) -> usize {
        (((self.area.h - self.header_h()) / self.font.line_h).floor() as usize).max(1)
    }

    fn header_h(&self) -> f32 {
        self.font.line_h * 1.4
    }

    pub fn contains(&self, x: f32, y: f32) -> bool {
        x >= self.area.x && x < self.area.x + self.area.w && y >= self.area.y
            && y < self.area.y + self.area.h
    }

    fn keep_selection_visible(&mut self) {
        let n = self.rows_visible();
        if self.selected < self.scroll {
            self.scroll = self.selected;
        } else if self.selected >= self.scroll + n {
            self.scroll = self.selected + 1 - n;
        }
        self.dirty = true;
    }

    pub fn scroll_by(&mut self, lines: isize) {
        let max = self.rows.len().saturating_sub(self.rows_visible()) as isize;
        self.scroll = (self.scroll as isize + lines).clamp(0, max.max(0)) as usize;
        self.dirty = true;
    }

    pub fn move_selection(&mut self, delta: isize) {
        if self.rows.is_empty() {
            return;
        }
        let max = self.rows.len() as isize - 1;
        self.selected = (self.selected as isize + delta).clamp(0, max) as usize;
        self.keep_selection_visible();
    }

    /// Enter / click on a row: folders toggle, files are returned to be opened.
    pub fn activate(&mut self, index: usize) -> Option<PathBuf> {
        let row = self.rows.get(index)?.clone();
        self.selected = index;
        if row.is_dir {
            self.set_expanded(&row.path, !row.expanded);
            None
        } else {
            self.dirty = true;
            Some(row.path)
        }
    }

    pub fn activate_selected(&mut self) -> Option<PathBuf> {
        self.activate(self.selected)
    }

    /// → opens a folder, ← closes it or jumps to the parent folder.
    pub fn expand_selected(&mut self, open: bool) {
        let Some(row) = self.rows.get(self.selected).cloned() else { return };
        if row.is_dir && row.expanded != open {
            self.set_expanded(&row.path, open);
        } else if !open && row.depth > 0 && !row.is_root
            && let Some(parent) = row.path.parent()
                && let Some(i) = self.rows.iter().position(|r| r.path == parent) {
                    self.selected = i;
                    self.keep_selection_visible();
                }
    }

    fn set_expanded(&mut self, path: &Path, open: bool) {
        if open {
            self.expanded.insert(path.to_path_buf());
        } else {
            self.expanded.remove(path);
        }
        self.refresh();
        self.keep_selection_visible();
    }

    /// Expands all parent folders of `path` and selects it.
    pub fn reveal(&mut self, path: &Path) {
        let Some(root) = self.root_of(path).map(Path::to_path_buf) else { return };
        let mut changed = self.multi() && self.expanded.insert(root.clone());
        for parent in path.ancestors().skip(1) {
            if parent == root {
                break;
            }
            changed |= self.expanded.insert(parent.to_path_buf());
        }
        if changed {
            self.refresh();
        }
        if let Some(i) = self.rows.iter().position(|r| r.path == path) {
            self.selected = i;
            self.keep_selection_visible();
        }
    }

    /// Row index under a window coordinate.
    pub fn row_at(&self, y: f32) -> Option<usize> {
        let top = self.area.y + self.header_h();
        if y < top {
            return None;
        }
        let i = self.scroll + ((y - top) / self.font.line_h) as usize;
        (i < self.rows.len()).then_some(i)
    }

    /// The workspace folder whose × is under the mouse (several folders only).
    pub fn close_hit(&self, x: f32, y: f32) -> Option<PathBuf> {
        if !self.multi() || x < self.area.x + self.area.w - self.font.cell_w * 2.5 {
            return None;
        }
        let row = self.rows.get(self.row_at(y)?)?;
        row.is_root.then(|| row.path.clone())
    }

    pub fn prepare(&mut self, fs: &mut FontSystem, theme: &Theme, active: Option<&Path>) {
        let name = if self.multi() {
            self.title.clone().unwrap_or_else(|| "Workspace".into())
        } else {
            let root = &self.roots[0];
            root.file_name().map_or_else(|| root.display().to_string(), |n| n.to_string_lossy().into_owned())
        };
        let attrs = Attrs::new().family(Family::Name(&self.family));
        self.header.set_text(&name.to_uppercase(), &attrs, Shaping::Advanced, None);
        self.header.shape_until_scroll(fs, false);
        if !self.dirty {
            return;
        }
        let visible = self.rows_visible();
        let mut spans: Vec<(String, Rgba)> = Vec::new();
        let mut closes = String::new();
        for row in self.rows.iter().skip(self.scroll).take(visible) {
            closes.push_str(if row.is_root { "×\n" } else { "\n" });
            if row.is_root {
                let marker = if row.expanded { "▾ " } else { "▸ " };
                spans.push((marker.to_string(), theme.panel_dim));
                spans.push((format!("{}\n", row.name.to_uppercase()), theme.panel_text));
                continue;
            }
            let indent = "  ".repeat(row.depth);
            let (marker, color) = if row.is_dir {
                (if row.expanded { "▾ " } else { "▸ " }, theme.panel_text)
            } else if Some(row.path.as_path()) == active {
                ("  ", theme.accent)
            } else {
                ("  ", theme.panel_text)
            };
            spans.push((format!("{indent}{marker}"), theme.panel_dim));
            spans.push((format!("{}\n", row.name), color));
        }
        self.buffer.set_rich_text(
            spans.iter().map(|(t, c)| (t.as_str(), attrs.clone().color(c.to_glyphon()))),
            &attrs,
            Shaping::Advanced,
            None,
        );
        self.buffer.shape_until_scroll(fs, false);
        self.close.set_text(&closes, &attrs, Shaping::Advanced, None);
        self.close.shape_until_scroll(fs, false);
        self.dirty = false;
    }

    /// Forces a re-render (e.g. when the active file changed).
    pub fn invalidate(&mut self) {
        self.dirty = true;
    }

    pub fn draw<'a>(
        &'a self,
        theme: &Theme,
        focused: bool,
        under: &mut Vec<Quad>,
        text: &mut Vec<TextBlock<'a>>,
    ) {
        let a = self.area;
        under.push(Quad { x: a.x, y: a.y, w: a.w, h: a.h, color: theme.panel_bg });
        let top = a.y + self.header_h();
        if self.selected >= self.scroll && self.selected < self.scroll + self.rows_visible() {
            let y = top + (self.selected - self.scroll) as f32 * self.font.line_h;
            let color = if focused { theme.row_selected } else { theme.row_selected_unfocused };
            under.push(Quad { x: a.x, y, w: a.w, h: self.font.line_h, color });
        }
        let clip = Some([a.x as i32, a.y as i32, (a.x + a.w) as i32, (a.y + a.h) as i32]);
        let pad = self.font.cell_w;
        text.push(TextBlock {
            buffer: &self.header,
            x: a.x + pad,
            y: a.y + (self.header_h() - self.font.line_h) / 2.0,
            color: theme.panel_dim,
            clip,
        });
        text.push(TextBlock { buffer: &self.buffer, x: a.x + pad, y: top, color: theme.panel_text, clip });
        if self.multi() {
            let x = a.x + a.w - pad * 2.0;
            text.push(TextBlock { buffer: &self.close, x, y: top, color: theme.panel_dim, clip });
        }
    }
}

/// Windows paths ignore case.
fn same_path(a: &Path, b: &Path) -> bool {
    a.to_string_lossy().to_lowercase() == b.to_string_lossy().to_lowercase()
}

fn starts_with(path: &Path, root: &Path) -> bool {
    path.starts_with(root) || {
        let (p, r) = (path.to_string_lossy().to_lowercase(), root.to_string_lossy().to_lowercase());
        p.starts_with(&r) && p[r.len()..].starts_with(['\\', '/'])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn several_folders_get_their_own_rows() {
        let dir = std::env::temp_dir().join(format!("wyven-explorer-test-{}", std::process::id()));
        let (a, b) = (dir.join("app"), dir.join("lib"));
        std::fs::create_dir_all(a.join("src")).unwrap();
        std::fs::create_dir_all(&b).unwrap();
        std::fs::write(a.join("src").join("main.rs"), "").unwrap();
        std::fs::write(b.join("lib.rs"), "").unwrap();
        let mut fs = FontSystem::new();
        let font = FontMetrics { size: 14.0, line_h: 20.0, cell_w: 8.0 };
        let mut ex = Explorer::new(a.clone(), &mut fs, font, String::new());
        assert_eq!(ex.rows.iter().map(|r| r.name.as_str()).collect::<Vec<_>>(), ["src"]);
        assert!(ex.add_root(b.clone()));
        assert!(!ex.add_root(b.clone()), "no folder twice");
        let names: Vec<(&str, usize, bool)> = ex.rows.iter().map(|r| (r.name.as_str(), r.depth, r.is_root)).collect();
        assert_eq!(names, [("app", 0, true), ("src", 1, false), ("lib", 0, true), ("lib.rs", 1, false)]);
        ex.reveal(&a.join("src").join("main.rs"));
        assert_eq!(ex.rows[ex.selected].name, "main.rs");
        assert_eq!(ex.root_of(&b.join("lib.rs")), Some(b.as_path()));
        assert!(ex.remove_root(&a));
        assert_eq!(ex.root(), b.as_path());
        assert!(!ex.remove_root(&b), "the last folder stays");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
