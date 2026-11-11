//! Docking like Visual Studio: the window is a tree. Rows and columns hold nodes side by side
//! or stacked (with relative sizes); a pane holds tool panels (Explorer, Search, Git, Debug,
//! Terminal) as tabs; the editor is one node of the tree. Panels dock next to a pane (which
//! splits it), into a pane (as a tab), next to the editor, or along a window edge; panels can
//! also float in their own window. Closing a panel remembers where it was, so it comes back
//! there. Everything is saved in `%APPDATA%\Wyven\layout.json`.
//!
//! The rules follow AvalonDock (an open-source reproduction of Visual Studio's docking): docking
//! on a pane's edge inside a row/column of the same direction inserts next to it, otherwise the
//! pane is replaced by a new row/column holding both; window edges wrap the whole tree.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use wyven_render::{Attrs, Buffer, Family, FontSystem, Metrics, Quad, Shaping, TextBlock, Wrap};

use crate::editor::{FontMetrics, Rect};
use crate::theme::Theme;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Panel {
    Explorer,
    Search,
    Git,
    Debug,
    Terminal,
    Problems,
}

impl Panel {
    pub const ALL: [Panel; 6] = [Panel::Explorer, Panel::Search, Panel::Git, Panel::Debug, Panel::Terminal, Panel::Problems];

    pub fn label(self) -> &'static str {
        match self {
            Panel::Explorer => "EXPLORER",
            Panel::Search => "SEARCH",
            Panel::Git => "GIT",
            Panel::Debug => "DEBUG",
            Panel::Terminal => "TERMINAL",
            Panel::Problems => "PROBLEMS",
        }
    }

    /// The name for window titles.
    pub fn name(self) -> &'static str {
        match self {
            Panel::Explorer => "Explorer",
            Panel::Search => "Search",
            Panel::Git => "Git",
            Panel::Debug => "Debug",
            Panel::Terminal => "Terminal",
            Panel::Problems => "Problems",
        }
    }

    fn index(self) -> usize {
        Self::ALL.iter().position(|&p| p == self).unwrap_or(0)
    }
}

/// Row: children side by side; Column: stacked.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Dir {
    Row,
    Column,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Side {
    Left,
    Right,
    Top,
    Bottom,
}

impl Side {
    fn dir(self) -> Dir {
        match self {
            Side::Left | Side::Right => Dir::Row,
            Side::Top | Side::Bottom => Dir::Column,
        }
    }

    /// The new node goes before the other one (left / top).
    fn first(self) -> bool {
        matches!(self, Side::Left | Side::Top)
    }
}

/// A node of the layout tree.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Node {
    /// The editor area (exactly once).
    Editor,
    /// Tool panels as tabs; `active` is shown.
    Pane { panels: Vec<Panel>, active: usize },
    /// Children in a row or column; `sizes` are fractions (summing to 1).
    Split { dir: Dir, children: Vec<Node>, sizes: Vec<f32> },
}

/// The way to a node: child indices from the root.
pub type NodePath = Vec<usize>;

/// A panel window floating on its own (screen position and size in physical pixels).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Floating {
    pub panels: Vec<Panel>,
    pub active: usize,
    pub rect: [i32; 4],
}

/// Where a closed panel comes back: as a tab next to `with`, else along `side` of the window.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Home {
    pub with: Option<Panel>,
    pub side: Side,
    /// It was right next to the editor (else along the window edge).
    #[serde(default)]
    pub editor: bool,
    /// Its share of the row/column it was in.
    #[serde(default)]
    pub size: f32,
}

impl Home {
    const LEFT: Home = Home { with: None, side: Side::Left, editor: true, size: 0.0 };
}

/// Where a dragged panel goes.
#[derive(Debug, Clone, PartialEq)]
pub enum DockTarget {
    /// Along a whole window edge.
    Edge(Side),
    /// Next to a node (pane or editor), splitting its place.
    Beside(NodePath, Side),
    /// As a tab of a pane.
    Into(NodePath),
}

/// Where a panel is now.
#[derive(Debug, Clone, PartialEq)]
pub enum Place {
    Docked(NodePath),
    Floating(usize),
    Closed,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Dock {
    pub root: Node,
    #[serde(default)]
    pub floating: Vec<Floating>,
    #[serde(default)]
    pub homes: Vec<(Panel, Home)>,
}

impl Default for Dock {
    fn default() -> Self {
        Self {
            root: Node::Split {
                dir: Dir::Row,
                children: vec![
                    Node::Pane { panels: vec![Panel::Explorer, Panel::Search, Panel::Git, Panel::Debug], active: 0 },
                    Node::Editor,
                ],
                sizes: vec![0.2, 0.8],
            },
            floating: Vec::new(),
            homes: vec![
                (Panel::Terminal, Home { with: None, side: Side::Bottom, editor: true, size: 0.3 }),
                (Panel::Problems, Home { with: Some(Panel::Terminal), side: Side::Bottom, editor: true, size: 0.3 }),
            ],
        }
    }
}

fn layout_path() -> Option<PathBuf> {
    crate::config::config_dir().map(|d| d.join("layout.json"))
}

/// One placed pane of a layout: its path, rectangle, panels and the shown one.
#[derive(Debug, Clone, PartialEq)]
pub struct PaneBox {
    pub path: NodePath,
    pub rect: Rect,
    pub panels: Vec<Panel>,
    pub active: Panel,
}

/// The gap between two children of a row/column (drag it to resize).
#[derive(Debug, Clone, PartialEq)]
pub struct Splitter {
    pub split: NodePath,
    /// Between child `index` and `index + 1`.
    pub index: usize,
    pub dir: Dir,
    pub rect: Rect,
    /// Length of the split along its direction (to turn pixels into fractions).
    pub total: f32,
}

/// The result of laying the tree out in a rectangle.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Arrangement {
    pub editor: Rect,
    pub editor_path: NodePath,
    pub panes: Vec<PaneBox>,
    pub splitters: Vec<Splitter>,
}

impl Node {
    fn at(&self, path: &[usize]) -> Option<&Node> {
        match path.split_first() {
            None => Some(self),
            Some((&i, rest)) => match self {
                Node::Split { children, .. } => children.get(i)?.at(rest),
                _ => None,
            },
        }
    }

    fn at_mut(&mut self, path: &[usize]) -> Option<&mut Node> {
        match path.split_first() {
            None => Some(self),
            Some((&i, rest)) => match self {
                Node::Split { children, .. } => children.get_mut(i)?.at_mut(rest),
                _ => None,
            },
        }
    }

    /// Paths of all nodes matching `f`.
    fn find(&self, f: &impl Fn(&Node) -> bool, here: &mut NodePath, out: &mut Vec<NodePath>) {
        if f(self) {
            out.push(here.clone());
        }
        if let Node::Split { children, .. } = self {
            for (i, c) in children.iter().enumerate() {
                here.push(i);
                c.find(f, here, out);
                here.pop();
            }
        }
    }

    fn path_of(&self, f: impl Fn(&Node) -> bool) -> Option<NodePath> {
        let mut out = Vec::new();
        self.find(&f, &mut Vec::new(), &mut out);
        out.into_iter().next()
    }

    /// Removes empty panes, dissolves rows/columns with one child, merges a row inside a row.
    fn tidy(self) -> Option<Node> {
        match self {
            Node::Pane { panels, .. } if panels.is_empty() => None,
            Node::Pane { panels, active } => {
                let active = active.min(panels.len() - 1);
                Some(Node::Pane { panels, active })
            }
            Node::Editor => Some(Node::Editor),
            Node::Split { dir, children, sizes } => {
                let mut kids: Vec<(Node, f32)> = Vec::new();
                for (c, s) in children.into_iter().zip(sizes.into_iter().chain(std::iter::repeat(0.0))) {
                    match c.tidy() {
                        Some(Node::Split { dir: d, children: cc, sizes: cs }) if d == dir => {
                            let sum: f32 = cs.iter().sum::<f32>().max(1e-6);
                            kids.extend(cc.into_iter().zip(cs.into_iter().map(|x| x / sum * s)));
                        }
                        Some(n) => kids.push((n, s)),
                        None => {}
                    }
                }
                match kids.len() {
                    0 => None,
                    1 => kids.pop().map(|(n, _)| n),
                    _ => {
                        let sum: f32 = kids.iter().map(|(_, s)| s.max(0.02)).sum();
                        let sizes = kids.iter().map(|(_, s)| s.max(0.02) / sum).collect();
                        Some(Node::Split { dir, children: kids.into_iter().map(|(n, _)| n).collect(), sizes })
                    }
                }
            }
        }
    }

    fn arrange(&self, r: Rect, gap: f32, path: &mut NodePath, out: &mut Arrangement) {
        match self {
            Node::Editor => {
                out.editor = r;
                out.editor_path = path.clone();
            }
            Node::Pane { panels, active } => out.panes.push(PaneBox {
                path: path.clone(),
                rect: r,
                panels: panels.clone(),
                active: panels[(*active).min(panels.len() - 1)],
            }),
            Node::Split { dir, children, sizes } => {
                let n = children.len();
                let total = if *dir == Dir::Row { r.w } else { r.h };
                let free = (total - gap * (n as f32 - 1.0)).max(0.0);
                let mut pos = if *dir == Dir::Row { r.x } else { r.y };
                for (i, c) in children.iter().enumerate() {
                    let len = if i + 1 == n {
                        (if *dir == Dir::Row { r.x + r.w } else { r.y + r.h }) - pos
                    } else {
                        (free * sizes.get(i).copied().unwrap_or(1.0 / n as f32)).round()
                    };
                    let cr = if *dir == Dir::Row { Rect { x: pos, w: len, ..r } } else { Rect { y: pos, h: len, ..r } };
                    path.push(i);
                    c.arrange(cr, gap, path, out);
                    path.pop();
                    pos += len;
                    if i + 1 < n {
                        let sr = if *dir == Dir::Row { Rect { x: pos, w: gap, ..r } } else { Rect { y: pos, h: gap, ..r } };
                        out.splitters.push(Splitter { split: path.clone(), index: i, dir: *dir, rect: sr, total: free });
                        pos += gap;
                    }
                }
            }
        }
    }
}

impl Dock {
    /// The saved layout, or the default one.
    pub fn load() -> Self {
        let mut dock = layout_path()
            .and_then(|p| std::fs::read_to_string(p).ok())
            .and_then(|s| serde_json::from_str::<Dock>(&s).ok())
            .unwrap_or_default();
        dock.repair();
        dock
    }

    pub fn save(&self) {
        let Some(path) = layout_path() else { return };
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        if let Ok(json) = serde_json::to_string_pretty(self)
            && let Err(e) = std::fs::write(&path, json)
        {
            log::warn!("layout.json: {e}");
        }
    }

    /// Exactly one editor, every panel at most once (after loading an old or edited file).
    fn repair(&mut self) {
        let mut editors = Vec::new();
        self.root.find(&|n| matches!(n, Node::Editor), &mut Vec::new(), &mut editors);
        if editors.len() != 1 {
            *self = Dock::default();
            return;
        }
        let mut seen: Vec<Panel> = Vec::new();
        let mut dedupe = |panels: &mut Vec<Panel>| {
            panels.retain(|p| {
                let new = !seen.contains(p);
                seen.push(*p);
                new
            })
        };
        fn walk(n: &mut Node, f: &mut impl FnMut(&mut Vec<Panel>)) {
            match n {
                Node::Pane { panels, .. } => f(panels),
                Node::Split { children, .. } => children.iter_mut().for_each(|c| walk(c, f)),
                Node::Editor => {}
            }
        }
        walk(&mut self.root, &mut dedupe);
        for fl in &mut self.floating {
            dedupe(&mut fl.panels);
        }
        self.floating.retain(|f| !f.panels.is_empty());
        self.tidy();
    }

    fn tidy(&mut self) {
        let root = std::mem::replace(&mut self.root, Node::Editor);
        self.root = root.tidy().unwrap_or(Node::Editor);
        self.floating.retain(|f| !f.panels.is_empty());
        for f in &mut self.floating {
            f.active = f.active.min(f.panels.len().saturating_sub(1));
        }
    }

    pub fn place(&self, p: Panel) -> Place {
        if let Some(path) = self.root.path_of(|n| matches!(n, Node::Pane { panels, .. } if panels.contains(&p))) {
            return Place::Docked(path);
        }
        match self.floating.iter().position(|f| f.panels.contains(&p)) {
            Some(i) => Place::Floating(i),
            None => Place::Closed,
        }
    }

    /// The panels sharing a pane (or floating window) with `p`, `p` included.
    pub fn neighbours(&self, p: Panel) -> Vec<Panel> {
        match self.place(p) {
            Place::Docked(path) => match self.root.at(&path) {
                Some(Node::Pane { panels, .. }) => panels.clone(),
                _ => vec![p],
            },
            Place::Floating(i) => self.floating[i].panels.clone(),
            Place::Closed => Vec::new(),
        }
    }

    /// The panel is open and the shown tab of its pane or floating window.
    pub fn shown(&self, p: Panel) -> bool {
        match self.place(p) {
            Place::Docked(path) => matches!(self.root.at(&path), Some(Node::Pane { panels, active }) if panels.get(*active) == Some(&p)),
            Place::Floating(i) => self.floating[i].panels.get(self.floating[i].active) == Some(&p),
            Place::Closed => false,
        }
    }

    /// Brings a panel to the front, opening it where it was before if it is closed.
    pub fn show(&mut self, p: Panel) {
        match self.place(p) {
            Place::Docked(path) => {
                if let Some(Node::Pane { panels, active }) = self.root.at_mut(&path) {
                    *active = panels.iter().position(|&q| q == p).unwrap_or(0);
                }
            }
            Place::Floating(i) => {
                let f = &mut self.floating[i];
                f.active = f.panels.iter().position(|&q| q == p).unwrap_or(0);
            }
            Place::Closed => {
                let home = self.homes.iter().find(|(q, _)| *q == p).map(|(_, h)| *h).unwrap_or(Home::LEFT);
                let into = home.with.and_then(|w| match self.place(w) {
                    Place::Docked(path) => Some(DockTarget::Into(path)),
                    _ => None,
                });
                // Like a side bar: a pane already on that side takes it as a tab (instead of a
                // new pane opening next to the others).
                let into = into.or_else(|| self.pane_beside_editor(home.side).map(DockTarget::Into));
                let target = match into {
                    Some(t) => t,
                    None if home.editor => match self.root.path_of(|n| matches!(n, Node::Editor)) {
                        Some(e) => DockTarget::Beside(e, home.side),
                        None => DockTarget::Edge(home.side),
                    },
                    None => DockTarget::Edge(home.side),
                };
                self.dock_sized(p, target, home.size);
            }
        }
    }

    /// The pane next to the editor on `side` (the nearest one, through rows and columns).
    fn pane_beside_editor(&self, side: Side) -> Option<NodePath> {
        let editor = self.root.path_of(|n| matches!(n, Node::Editor))?;
        // From the editor up: the first row/column of the side's direction with a neighbour there.
        for depth in (0..editor.len()).rev() {
            let parent = &editor[..depth];
            let i = editor[depth];
            let Some(Node::Split { dir, children, .. }) = self.root.at(parent) else { continue };
            if *dir != side.dir() {
                continue;
            }
            let n = if side.first() { i.checked_sub(1) } else { (i + 1 < children.len()).then_some(i + 1) };
            let Some(n) = n else { continue };
            let mut base = parent.to_vec();
            base.push(n);
            let sub = self.root.at(&base)?;
            let rel = sub.path_of(|x| matches!(x, Node::Pane { .. }))?;
            base.extend(rel);
            return Some(base);
        }
        None
    }

    /// Which side of the editor a docked node is on (the first difference of their paths).
    pub fn side_of_path(&self, path: &[usize]) -> Side {
        let Some(editor) = self.root.path_of(|n| matches!(n, Node::Editor)) else { return Side::Left };
        let mut node = &self.root;
        for (&a, &b) in path.iter().zip(editor.iter()) {
            if a != b {
                let dir = match node {
                    Node::Split { dir, .. } => *dir,
                    _ => Dir::Row,
                };
                return match (dir, a < b) {
                    (Dir::Row, true) => Side::Left,
                    (Dir::Row, false) => Side::Right,
                    (Dir::Column, true) => Side::Top,
                    (Dir::Column, false) => Side::Bottom,
                };
            }
            node = match node {
                Node::Split { children, .. } => &children[a],
                _ => break,
            };
        }
        Side::Left
    }

    /// Where a panel belongs in the activity bar: its side of the editor, `None` if floating.
    pub fn side_of(&self, p: Panel) -> Option<Side> {
        match self.place(p) {
            Place::Docked(path) => Some(self.side_of_path(&path)),
            Place::Floating(_) => None,
            Place::Closed => Some(self.homes.iter().find(|(q, _)| *q == p).map_or(Side::Left, |(_, h)| h.side)),
        }
    }

    /// Takes a panel out (remembering where it was) and tidies the tree.
    fn take(&mut self, p: Panel) {
        let home = match self.place(p) {
            Place::Docked(path) => {
                let side = self.side_of_path(&path);
                let with = match self.root.at(&path) {
                    Some(Node::Pane { panels, .. }) => panels.iter().copied().find(|&q| q != p),
                    _ => None,
                };
                // Right next to the editor, and its share of that row/column.
                let (editor, size) = match path.split_last().and_then(|(&i, parent)| Some((i, self.root.at(parent)?))) {
                    Some((i, Node::Split { children, sizes, .. })) => {
                        (children.iter().any(|c| matches!(c, Node::Editor)), sizes.get(i).copied().unwrap_or(0.0))
                    }
                    _ => (false, 0.0),
                };
                if let Some(Node::Pane { panels, active }) = self.root.at_mut(&path) {
                    let i = panels.iter().position(|&q| q == p).unwrap_or(0);
                    panels.remove(i);
                    if *active > i || *active >= panels.len() {
                        *active = active.saturating_sub(1);
                    }
                }
                Some(Home { with, side, editor, size })
            }
            Place::Floating(i) => {
                let f = &mut self.floating[i];
                f.panels.retain(|&q| q != p);
                f.active = f.active.min(f.panels.len().saturating_sub(1));
                None
            }
            Place::Closed => None,
        };
        if let Some(h) = home {
            self.homes.retain(|(q, _)| *q != p);
            self.homes.push((p, h));
        }
        self.tidy();
    }

    /// Closes a panel (it comes back at the same place with [`Dock::show`]).
    pub fn hide(&mut self, p: Panel) {
        self.take(p);
    }

    /// Closes the pane (or floating window) holding `p`, with all its panels.
    pub fn hide_pane(&mut self, p: Panel) {
        let mut all = self.neighbours(p);
        // `p` last: it remembers the editor (the others remember their neighbours).
        all.retain(|&q| q != p);
        all.push(p);
        for q in all {
            self.take(q);
        }
    }

    /// Docks a panel: next to a node, into a pane, or along a window edge.
    pub fn dock(&mut self, p: Panel, target: DockTarget) {
        self.dock_sized(p, target, 0.0);
    }

    /// Like [`Dock::dock`]; `size` (a fraction, 0 = default) is the new pane's share.
    fn dock_sized(&mut self, p: Panel, target: DockTarget, size: f32) {
        // Paths refer to the tree before taking the panel out: keep the target node findable.
        let target = match target {
            DockTarget::Edge(s) => Resolved::Edge(s),
            DockTarget::Beside(path, s) => Resolved::Beside(self.mark(&path, p), s),
            DockTarget::Into(path) => Resolved::Into(self.mark(&path, p)),
        };
        match &target {
            // Into its own pane: just show it.
            Resolved::Into(Mark::Panes(list)) if list.is_empty() => {
                self.show(p);
                return;
            }
            // Beside its own pane where it is alone: nothing changes.
            Resolved::Beside(Mark::Panes(list), _) if list.is_empty() => return,
            _ => {}
        }
        self.take(p);
        let pane = Node::Pane { panels: vec![p], active: 0 };
        let valid = |s: f32, default: f32| if s > 0.02 && s < 0.9 { s } else { default };
        match target {
            Resolved::Edge(side) => {
                let root = std::mem::replace(&mut self.root, Node::Editor);
                let share = valid(size, if side.dir() == Dir::Row { 0.2 } else { 0.3 });
                self.root = match root {
                    Node::Split { dir, mut children, mut sizes } if dir == side.dir() => {
                        for s in &mut sizes {
                            *s *= 1.0 - share;
                        }
                        if side.first() {
                            children.insert(0, pane);
                            sizes.insert(0, share);
                        } else {
                            children.push(pane);
                            sizes.push(share);
                        }
                        Node::Split { dir, children, sizes }
                    }
                    other => {
                        let (children, sizes) =
                            if side.first() { (vec![pane, other], vec![share, 1.0 - share]) } else { (vec![other, pane], vec![1.0 - share, share]) };
                        Node::Split { dir: side.dir(), children, sizes }
                    }
                };
            }
            Resolved::Into(mark) => {
                if let Some(Node::Pane { panels, active }) = self.find_mark(&mark).and_then(|path| self.root.at_mut(&path)) {
                    panels.push(p);
                    *active = panels.len() - 1;
                } else {
                    self.dock_sized(p, DockTarget::Edge(Side::Left), size);
                    return;
                }
            }
            Resolved::Beside(mark, side) => {
                let Some(path) = self.find_mark(&mark) else {
                    self.dock_sized(p, DockTarget::Edge(side), size);
                    return;
                };
                // Next to the editor a panel gets a smaller share than half.
                let editor = matches!(mark, Mark::Editor);
                let default = if !editor { 0.5 } else if side.dir() == Dir::Row { 0.22 } else { 0.3 };
                let share = valid(size, default);
                let parent_dir = path.split_last().and_then(|(_, parent)| match self.root.at(parent) {
                    Some(Node::Split { dir, .. }) => Some(*dir),
                    _ => None,
                });
                if let (Some((&i, parent)), Some(dir)) = (path.split_last(), parent_dir)
                    && dir == side.dir()
                    && let Some(Node::Split { children, sizes, .. }) = self.root.at_mut(parent)
                {
                    // Same direction: next to it, taking part of its space.
                    let own = sizes[i];
                    sizes[i] = own * (1.0 - share);
                    let at = if side.first() { i } else { i + 1 };
                    children.insert(at, pane);
                    sizes.insert(at, own * share);
                } else if let Some(node) = self.root.at_mut(&path) {
                    // Otherwise the node becomes a new row/column with the panel beside it.
                    let old = std::mem::replace(node, Node::Editor);
                    let (children, sizes) =
                        if side.first() { (vec![pane, old], vec![share, 1.0 - share]) } else { (vec![old, pane], vec![1.0 - share, share]) };
                    *node = Node::Split { dir: side.dir(), children, sizes };
                }
            }
        }
        self.tidy();
        self.homes.retain(|(q, _)| *q != p);
    }

    /// Lets a panel float in its own window at `rect` (screen pixels).
    pub fn float(&mut self, p: Panel, rect: [i32; 4]) {
        // `take` remembers where it was docked: closing the window brings it back there.
        self.take(p);
        self.floating.push(Floating { panels: vec![p], active: 0, rect });
    }

    /// Something that identifies a node through `take` (which can move it): the editor, or the
    /// other panels of the pane (without `moving`).
    fn mark(&self, path: &[usize], moving: Panel) -> Mark {
        match self.root.at(path) {
            Some(Node::Pane { panels, .. }) => Mark::Panes(panels.iter().copied().filter(|&q| q != moving).collect()),
            _ => Mark::Editor,
        }
    }

    fn find_mark(&self, m: &Mark) -> Option<NodePath> {
        match m {
            Mark::Editor => self.root.path_of(|n| matches!(n, Node::Editor)),
            Mark::Panes(list) => self.root.path_of(|n| matches!(n, Node::Pane { panels, .. } if panels.iter().any(|p| list.contains(p)))),
        }
    }

    /// Lays the tree out in `r` with `gap` pixels between nodes.
    pub fn arrange(&self, r: Rect, gap: f32) -> Arrangement {
        let mut out = Arrangement::default();
        self.root.arrange(r, gap, &mut Vec::new(), &mut out);
        out
    }

    /// Moves the splitter `index` of the row/column at `split` by `delta` pixels.
    pub fn resize(&mut self, split: &[usize], index: usize, delta: f32, total: f32, min: f32) {
        if let Some(Node::Split { sizes, .. }) = self.root.at_mut(split)
            && index + 1 < sizes.len()
            && total > 0.0
        {
            let d = delta / total;
            let pair = sizes[index] + sizes[index + 1];
            let min = (min / total).min(pair * 0.45);
            let a = (sizes[index] + d).clamp(min, pair - min);
            sizes[index] = a;
            sizes[index + 1] = pair - a;
        }
    }
}

enum Resolved {
    Edge(Side),
    Beside(Mark, Side),
    Into(Mark),
}

enum Mark {
    Editor,
    Panes(Vec<Panel>),
}

/// Height of the panel tab strip on top of a pane.
pub fn strip_h(ui: FontMetrics) -> f32 {
    (ui.line_h * 1.6).round()
}

/// The part of a pane below its tab strip, where the panel draws.
pub fn content(area: Rect, ui: FontMetrics) -> Rect {
    let h = strip_h(ui).min(area.h);
    Rect { x: area.x, y: area.y + h, w: area.w, h: area.h - h }
}

/// A panel tab on screen (in panes left of the editor only the shown panel's title: the
/// activity bar switches there). `pane` is the index in [`Arrangement::panes`].
#[derive(Debug, Clone, Copy)]
pub struct TabSpan {
    pub pane: usize,
    pub panel: Panel,
    pub x0: f32,
    pub x1: f32,
    pub y0: f32,
    pub y1: f32,
}

/// An icon button in a panel header.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HeaderButton {
    NewFile,
    NewFolder,
    Refresh,
    CollapseAll,
    /// Starts one more terminal.
    NewTerminal,
    /// A menu of shells for a new terminal.
    TerminalProfiles,
    /// Problems panel: list ↔ task output.
    ToggleOutput,
    /// Closes the pane.
    Close,
}

impl HeaderButton {
    fn icon(self) -> char {
        use crate::chrome::*;
        match self {
            HeaderButton::NewFile => ICON_NEW_FILE,
            HeaderButton::NewFolder => ICON_NEW_FOLDER,
            HeaderButton::Refresh => ICON_REFRESH,
            HeaderButton::CollapseAll => ICON_COLLAPSE,
            HeaderButton::NewTerminal => ICON_ADD,
            HeaderButton::TerminalProfiles => ICON_CHEVRON_DOWN,
            HeaderButton::ToggleOutput => ICON_OUTPUT,
            HeaderButton::Close => ICON_CLOSE,
        }
    }

    pub fn tooltip(self) -> &'static str {
        match self {
            HeaderButton::NewFile => "New File (Ctrl+N)",
            HeaderButton::NewFolder => "New Folder (Ctrl+Shift+N)",
            HeaderButton::Refresh => "Refresh",
            HeaderButton::CollapseAll => "Collapse Folders",
            HeaderButton::NewTerminal => "New Terminal",
            HeaderButton::TerminalProfiles => "New Terminal With …",
            HeaderButton::ToggleOutput => "Problems / Task Output (Tab)",
            HeaderButton::Close => "Close",
        }
    }
}

/// What is under the mouse in a header.
#[derive(Debug, Clone, Copy)]
pub enum HeaderHit {
    Tab(TabSpan),
    Button(usize, Panel, HeaderButton),
}

/// One pane's header to place: its rectangle, panels, the shown one, and whether only the
/// shown panel's title appears (instead of all tabs).
pub struct Strip {
    pub rect: Rect,
    pub panels: Vec<Panel>,
    pub active: Panel,
    pub title_only: bool,
}

/// Draws the headers of the panes and knows where tabs and buttons are.
pub struct DockBar {
    labels: Vec<Buffer>,
    widths: [f32; 6],
    icons: Vec<(HeaderButton, Buffer)>,
    pub spans: Vec<TabSpan>,
    pub buttons: Vec<(usize, Panel, HeaderButton, Rect)>,
    pub hover: Option<(usize, HeaderButton)>,
    ui: FontMetrics,
    family: String,
    icon_family: String,
}

impl DockBar {
    pub fn new(fs: &mut FontSystem, ui: FontMetrics, family: String) -> Self {
        let icon_family = crate::chrome::icon_family(fs);
        let mut bar = Self {
            labels: Vec::new(),
            widths: [0.0; 6],
            icons: Vec::new(),
            spans: Vec::new(),
            buttons: Vec::new(),
            hover: None,
            ui,
            family,
            icon_family,
        };
        bar.shape(fs);
        bar
    }

    pub fn set_font(&mut self, fs: &mut FontSystem, ui: FontMetrics) {
        self.ui = ui;
        self.shape(fs);
    }

    fn shape(&mut self, fs: &mut FontSystem) {
        let attrs = Attrs::new().family(Family::Name(&self.family)).weight(wyven_render::Weight::SEMIBOLD);
        self.labels = Panel::ALL
            .iter()
            .enumerate()
            .map(|(i, p)| {
                let mut b = Buffer::new(fs, Metrics::new(self.ui.size * 0.9, self.ui.line_h));
                b.set_wrap(Wrap::None);
                b.set_text(p.label(), &attrs, Shaping::Advanced, None);
                b.shape_until_scroll(fs, false);
                self.widths[i] = b.layout_runs().next().map_or(0.0, |r| r.line_w);
                b
            })
            .collect();
        let size = (self.ui.size * 1.05).round();
        self.icons = [
            HeaderButton::NewFile,
            HeaderButton::NewFolder,
            HeaderButton::Refresh,
            HeaderButton::CollapseAll,
            HeaderButton::NewTerminal,
            HeaderButton::TerminalProfiles,
            HeaderButton::ToggleOutput,
            HeaderButton::Close,
        ]
            .into_iter()
            .map(|b| (b, crate::chrome::label(fs, &b.icon().to_string(), &self.icon_family, size, size * 1.2)))
            .collect();
    }

    fn buttons_for(panel: Panel) -> Vec<HeaderButton> {
        let mut list = match panel {
            Panel::Explorer => vec![HeaderButton::NewFile, HeaderButton::NewFolder, HeaderButton::Refresh, HeaderButton::CollapseAll],
            Panel::Git => vec![HeaderButton::Refresh],
            Panel::Terminal => vec![HeaderButton::NewTerminal, HeaderButton::TerminalProfiles],
            Panel::Problems => vec![HeaderButton::ToggleOutput],
            _ => Vec::new(),
        };
        list.push(HeaderButton::Close);
        list
    }

    /// A panel's title, shaped.
    pub fn label(&self, p: Panel) -> &Buffer {
        &self.labels[p.index()]
    }

    /// Width of a panel's tab in the header.
    pub fn tab_w(&self, p: Panel) -> f32 {
        self.widths[p.index()] + 2.4 * self.ui.cell_w
    }

    /// Places tabs and buttons of every pane.
    pub fn place(&mut self, strips: &[Strip]) {
        self.spans.clear();
        self.buttons.clear();
        let pad = self.ui.cell_w * 1.2;
        let h = strip_h(self.ui);
        for (i, s) in strips.iter().enumerate() {
            let r = s.rect;
            let mut x = r.x + pad * 0.5;
            for &panel in &s.panels {
                if s.title_only && panel != s.active {
                    continue;
                }
                let w = self.tab_w(panel);
                self.spans.push(TabSpan { pane: i, panel, x0: x, x1: x + w, y0: r.y, y1: r.y + h });
                x += w;
            }
            let bw = (h * 0.95).round();
            let mut bx = r.x + r.w - pad * 0.5;
            for b in Self::buttons_for(s.active).into_iter().rev() {
                bx -= bw;
                if bx < x {
                    break;
                }
                self.buttons.push((i, s.active, b, Rect { x: bx, y: r.y + (h - bw) / 2.0, w: bw, h: bw }));
            }
        }
    }

    pub fn hit(&self, x: f32, y: f32) -> Option<HeaderHit> {
        let inside = |r: &Rect| x >= r.x && x < r.x + r.w && y >= r.y && y < r.y + r.h;
        if let Some((a, p, b, _)) = self.buttons.iter().find(|(_, _, _, r)| inside(r)) {
            return Some(HeaderHit::Button(*a, *p, *b));
        }
        self.spans.iter().copied().find(|s| x >= s.x0 && x < s.x1 && y >= s.y0 && y < s.y1).map(HeaderHit::Tab)
    }

    /// Header backgrounds, tabs (the shown one in its panel's color), buttons.
    #[allow(clippy::too_many_arguments)]
    pub fn draw<'a>(
        &'a self,
        theme: &Theme,
        scale: f32,
        strips: &[Strip],
        focused: Option<Panel>,
        under: &mut Vec<Quad>,
        cards: &mut Vec<wyven_render::RoundRect>,
        text: &mut Vec<TextBlock<'a>>,
    ) {
        let h = strip_h(self.ui);
        let pad = self.ui.cell_w * 1.2;
        for s in strips {
            let r = s.rect;
            under.push(Quad { x: r.x, y: r.y, w: r.w, h, color: theme.panel_bg });
            under.push(Quad { x: r.x + pad * 0.5, y: r.y + h - 1.0, w: r.w - pad, h: 1.0, color: theme.divider });
        }
        let accent_h = (2.0 * scale).round();
        for t in &self.spans {
            let Some(s) = strips.get(t.pane) else { continue };
            let r = s.rect;
            let is_active = s.active == t.panel;
            if t.x0 >= r.x + r.w {
                continue;
            }
            let color = crate::chrome::panel_color(t.panel, theme.dark);
            if is_active && !s.title_only {
                let w = (t.x1.min(r.x + r.w)) - t.x0 - pad;
                let c = if focused == Some(t.panel) { color } else { color.with_alpha(0x90) };
                cards.push(wyven_render::RoundRect::new(t.x0 + pad * 0.5, t.y1 - accent_h - 2.0, w, accent_h, c, accent_h / 2.0));
            }
            let text_color = if s.title_only {
                if focused == Some(t.panel) { color } else { theme.panel_text }
            } else if is_active {
                theme.panel_text
            } else {
                theme.panel_dim
            };
            text.push(TextBlock {
                buffer: &self.labels[t.panel.index()],
                x: t.x0 + pad,
                y: t.y0 + (h - self.ui.line_h) / 2.0,
                color: text_color,
                clip: Some([r.x as i32, r.y as i32, (r.x + r.w) as i32, (r.y + h) as i32]),
            });
        }
        let size = (self.ui.size * 1.05).round();
        for (pane, _, b, r) in &self.buttons {
            let Some((_, buf)) = self.icons.iter().find(|(k, _)| k == b) else { continue };
            let hover = self.hover == Some((*pane, *b));
            if hover {
                cards.push(wyven_render::RoundRect::new(r.x, r.y, r.w, r.h, theme.panel_text.with_alpha(0x22), (4.0 * scale).round()));
            }
            let w = buf.layout_runs().next().map_or(size, |run| run.line_w);
            text.push(TextBlock {
                buffer: buf,
                x: (r.x + (r.w - w) / 2.0).round(),
                y: (r.y + (r.h - size * 1.2) / 2.0).round(),
                color: if hover { theme.panel_text } else { theme.panel_dim },
                clip: None,
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pane_of(d: &Dock, p: Panel) -> Vec<Panel> {
        d.neighbours(p)
    }

    const R: Rect = Rect { x: 0.0, y: 0.0, w: 1000.0, h: 800.0 };

    #[test]
    fn the_default_has_the_side_bar_left_of_the_editor() {
        let d = Dock::default();
        assert!(d.shown(Panel::Explorer) && !d.shown(Panel::Search));
        assert_eq!(d.place(Panel::Terminal), Place::Closed);
        let a = d.arrange(R, 10.0);
        assert_eq!(a.panes.len(), 1);
        assert_eq!(a.panes[0].rect.w, 198.0);
        assert_eq!(a.editor.x, 208.0);
        assert_eq!(a.editor.x + a.editor.w, 1000.0);
        assert_eq!(a.splitters.len(), 1);
        assert_eq!(d.side_of(Panel::Git), Some(Side::Left));
        assert_eq!(d.side_of(Panel::Terminal), Some(Side::Bottom));
    }

    #[test]
    fn the_terminal_opens_below_the_editor_and_comes_back_there() {
        let mut d = Dock::default();
        d.show(Panel::Terminal);
        let a = d.arrange(R, 0.0);
        let t = a.panes.iter().find(|p| p.active == Panel::Terminal).unwrap();
        // Below the editor only, not below the side bar.
        assert_eq!(t.rect.x, a.editor.x);
        assert!(t.rect.y > a.editor.y);
        assert_eq!(d.side_of(Panel::Terminal), Some(Side::Bottom));
        d.hide(Panel::Terminal);
        assert_eq!(d.place(Panel::Terminal), Place::Closed);
        assert_eq!(d.root, Dock::default().root);
        d.show(Panel::Terminal);
        assert!(d.shown(Panel::Terminal));
        assert_eq!(d.side_of(Panel::Terminal), Some(Side::Bottom));
    }

    #[test]
    fn docking_beside_a_pane_splits_it_and_into_adds_a_tab() {
        let mut d = Dock::default();
        // Git below the Explorer pane: the left pane becomes a column.
        let explorer = match d.place(Panel::Explorer) {
            Place::Docked(p) => p,
            _ => unreachable!(),
        };
        d.dock(Panel::Git, DockTarget::Beside(explorer, Side::Bottom));
        assert_eq!(pane_of(&d, Panel::Git), vec![Panel::Git]);
        let a = d.arrange(R, 0.0);
        assert_eq!(a.panes.len(), 2);
        let (e, g) = (&a.panes[0], &a.panes[1]);
        assert_eq!(e.rect.x, g.rect.x);
        assert!(g.rect.y > e.rect.y);
        assert_eq!(d.side_of(Panel::Git), Some(Side::Left));
        // Search into the Git pane.
        let git = match d.place(Panel::Git) {
            Place::Docked(p) => p,
            _ => unreachable!(),
        };
        d.dock(Panel::Search, DockTarget::Into(git));
        assert_eq!(pane_of(&d, Panel::Git), vec![Panel::Git, Panel::Search]);
        assert!(d.shown(Panel::Search));
        // Every panel exactly once.
        let mut all = Vec::new();
        for p in Panel::ALL {
            if let Place::Docked(_) = d.place(p) {
                all.push(p);
            }
        }
        assert_eq!(all.len(), 4);
    }

    #[test]
    fn edges_wrap_the_whole_window() {
        let mut d = Dock::default();
        d.dock(Panel::Debug, DockTarget::Edge(Side::Right));
        d.dock(Panel::Search, DockTarget::Edge(Side::Bottom));
        let a = d.arrange(R, 0.0);
        let s = a.panes.iter().find(|p| p.active == Panel::Search).unwrap();
        assert_eq!((s.rect.x, s.rect.w), (0.0, 1000.0));
        let dbg = a.panes.iter().find(|p| p.active == Panel::Debug).unwrap();
        assert_eq!(dbg.rect.x + dbg.rect.w, 1000.0);
        assert_eq!(d.side_of(Panel::Debug), Some(Side::Right));
        assert_eq!(d.side_of(Panel::Search), Some(Side::Bottom));
    }

    #[test]
    fn dropping_a_panel_on_its_own_pane_changes_nothing() {
        let mut d = Dock::default();
        d.dock(Panel::Debug, DockTarget::Edge(Side::Right));
        let before = d.clone();
        let path = match d.place(Panel::Debug) {
            Place::Docked(p) => p,
            _ => unreachable!(),
        };
        d.dock(Panel::Debug, DockTarget::Beside(path.clone(), Side::Left));
        assert_eq!(d, before);
        d.dock(Panel::Debug, DockTarget::Into(path));
        assert_eq!(d, before);
    }

    #[test]
    fn closing_the_side_bar_and_opening_it_again() {
        let mut d = Dock::default();
        d.hide_pane(Panel::Explorer);
        assert_eq!(d.root, Node::Editor);
        d.show(Panel::Explorer);
        d.show(Panel::Search);
        assert_eq!(pane_of(&d, Panel::Explorer), vec![Panel::Explorer, Panel::Search]);
        assert_eq!(d.side_of(Panel::Explorer), Some(Side::Left));
        let a = d.arrange(R, 0.0);
        assert_eq!(a.panes[0].rect.x, 0.0);
        assert!((a.panes[0].rect.w - 200.0).abs() < 2.0);
    }

    #[test]
    fn splitters_resize_neighbours() {
        let mut d = Dock::default();
        let a = d.arrange(R, 10.0);
        let s = a.splitters[0].clone();
        d.resize(&s.split, s.index, 99.0, s.total, 50.0);
        let a = d.arrange(R, 10.0);
        assert_eq!(a.panes[0].rect.w, 297.0);
        // Not below the minimum.
        d.resize(&s.split, s.index, -5000.0, s.total, 50.0);
        assert_eq!(d.arrange(R, 10.0).panes[0].rect.w, 50.0);
    }

    #[test]
    fn floating_panels_leave_the_tree() {
        let mut d = Dock::default();
        d.float(Panel::Search, [10, 10, 300, 400]);
        assert_eq!(d.place(Panel::Search), Place::Floating(0));
        assert!(d.shown(Panel::Search));
        assert_eq!(d.side_of(Panel::Search), None);
        d.dock(Panel::Search, DockTarget::Edge(Side::Left));
        assert!(d.floating.is_empty());
    }

    #[test]
    fn a_broken_or_old_saved_layout_is_replaced() {
        // The old format (three areas) does not parse: the default is used.
        let old = r#"{"left":{"panels":["Git"],"active":"Git","visible":true}}"#;
        assert!(serde_json::from_str::<Dock>(old).is_err());
        // Two editors, a panel twice: repaired.
        let mut d = Dock {
            root: Node::Split {
                dir: Dir::Row,
                children: vec![
                    Node::Pane { panels: vec![Panel::Git, Panel::Git], active: 5 },
                    Node::Editor,
                    Node::Pane { panels: vec![Panel::Git], active: 0 },
                ],
                sizes: vec![0.2, 0.6, 0.2],
            },
            floating: vec![],
            homes: vec![],
        };
        d.repair();
        assert_eq!(pane_of(&d, Panel::Git), vec![Panel::Git]);
        assert_eq!(d.arrange(R, 0.0).panes.len(), 1);
        let mut e = d.clone();
        e.root = Node::Split { dir: Dir::Row, children: vec![Node::Editor, Node::Editor], sizes: vec![0.5, 0.5] };
        e.repair();
        assert_eq!(e, Dock::default());
        // Round trip.
        let json = serde_json::to_string(&d).unwrap();
        assert_eq!(serde_json::from_str::<Dock>(&json).unwrap(), d);
    }

    #[test]
    fn reopened_side_panels_share_one_pane() {
        let mut d = Dock::default();
        // Closing the side bar closes all its panels; opening them one by one (activity bar)
        // puts them back into one pane, not side by side.
        d.hide_pane(Panel::Explorer);
        d.show(Panel::Search);
        d.show(Panel::Git);
        d.show(Panel::Explorer);
        let a = d.arrange(R, 0.0);
        assert_eq!(a.panes.len(), 1);
        assert!(d.shown(Panel::Explorer) && !d.shown(Panel::Git));
        assert_eq!(pane_of(&d, Panel::Git).len(), 3);
        // The terminal at the bottom joins nothing on the left.
        d.show(Panel::Terminal);
        assert_eq!(pane_of(&d, Panel::Terminal), vec![Panel::Terminal]);
        assert_eq!(d.side_of(Panel::Terminal), Some(Side::Bottom));
    }
}
