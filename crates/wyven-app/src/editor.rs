//! The code editor view: draws a [`Document`] with line numbers, highlighting, selections and
//! cursors, and maps mouse positions back to text positions.
//!
//! Only the visible lines are shaped. The shaped text is cached and rebuilt only when the
//! text, the scroll position, the size or the current line changes; cursors and selections
//! are cheap rectangles drawn every frame.

use std::path::Path;

use wyven_buffer::{char_to_display_col, char_width, display_col_to_char, line_len_chars, Document, Rope, TextEdit};
use wyven_render::{Attrs, Buffer, Family, FontSystem, Metrics, Quad, Rgba, Shaping, TextBlock, Wrap};
use wyven_syntax::{flatten, language_for_path, Syntax};

use crate::theme::Theme;

#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FontMetrics {
    pub size: f32,
    pub line_h: f32,
    pub cell_w: f32,
}

#[derive(PartialEq)]
struct CacheKey {
    version: u64,
    scroll_line: usize,
    rows: usize,
    current_line: usize,
    font: (u32, u32),
    gutter_cells: usize,
    fold_gen: u64,
    gutter_hover: bool,
    marks_gen: u64,
    /// Wrap width in cells (0 = no word wrap).
    wrap: usize,
}

/// How the minimap on the right is shown (`[editor] minimap` in settings.toml, Alt → v → m).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MinimapMode {
    /// The shape of the code as colored blocks, plus a marker strip.
    Blocks,
    /// Only a narrow bar with markers and the visible area.
    Bar,
    Off,
}

impl MinimapMode {
    pub fn parse(s: &str) -> Self {
        match s.trim().to_lowercase().as_str() {
            "bar" | "leiste" => Self::Bar,
            "off" | "none" | "aus" | "false" => Self::Off,
            _ => Self::Blocks,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Blocks => "blocks",
            Self::Bar => "bar",
            Self::Off => "off",
        }
    }

    /// Alt → v → m: blocks → bar → off → blocks.
    pub fn next(self) -> Self {
        match self {
            Self::Blocks => Self::Bar,
            Self::Bar => Self::Off,
            Self::Off => Self::Blocks,
        }
    }
}

/// Minimap sizes in logical pixels.
const MINI_BLOCKS_W: f32 = 80.0;
const MINI_BAR_W: f32 = 14.0;
const MINI_STRIP_W: f32 = 4.0;
const MINI_LINE_H: f32 = 2.0;
const MINI_CHAR_W: f32 = 1.0;

/// Where the minimap is and where its box for the visible area sits.
struct MiniGeom {
    rect: Rect,
    /// First document line shown (blocks mode).
    first: usize,
    line_h: f32,
    box_y: f32,
    box_h: f32,
    /// Lines scrolled per pixel when dragging the box.
    lines_per_px: f32,
}

#[derive(PartialEq)]
struct MiniKey {
    version: u64,
    first: usize,
    rows: usize,
    cols: usize,
    theme: &'static str,
}

/// One block: row below `first`, start column, length in columns, color.
type MiniRun = (usize, usize, usize, Rgba);

/// At most this many scope headers stick at the top.
const STICKY_MAX: usize = 5;

/// Kinds of definitions that stick at the top (types and members with a body).
const STICKY_KINDS: &[&str] =
    &["namespace", "class", "struct", "record", "interface", "enum", "impl", "method", "constructor", "property", "function"];

/// A definition spanning several lines: first line (the one with the name) and last line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Scope {
    pub line: usize,
    pub end: usize,
}

/// The scope headers to show over `lines` (the document lines on screen, top first; folded
/// lines are skipped): outer first, at most `max`. A header sticks once its own line has
/// scrolled under the sticky area, and only while the line right below the sticky area is
/// still inside the scope.
pub fn sticky_for(scopes: &[Scope], lines: &[usize], max: usize) -> Vec<Scope> {
    let chain = |probe: usize| -> Vec<Scope> {
        let mut c: Vec<Scope> = scopes.iter().copied().filter(|s| s.line < probe && probe <= s.end).collect();
        c.sort_by_key(|s| (s.line, std::cmp::Reverse(s.end)));
        c
    };
    // The largest row count n whose first code line (top + n) still lies in n scopes, where
    // header row i only covers a line after its own (so a header never hides itself).
    for n in (1..=max).rev() {
        let Some(&probe) = lines.get(n) else { continue };
        let c = chain(probe);
        if c.len() < n {
            continue;
        }
        // Deeper than allowed: the innermost headers win; otherwise the outer ones (an inner
        // scope that ends right below the headers is left out).
        let skip = c.len().saturating_sub(max);
        let pick: Vec<Scope> = c.into_iter().skip(skip).take(n).collect();
        if pick.iter().enumerate().all(|(i, s)| s.line < lines[i]) {
            return pick;
        }
    }
    Vec::new()
}

/// Folds found in the text itself: `#region` … `#endregion` (also as `// #region`) and runs
/// of using/import/use lines. Same format as syntax folds: (line that stays, last hidden line).
fn text_folds(rope: &Rope) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    let mut regions: Vec<usize> = Vec::new();
    let mut imports: Option<(usize, usize)> = None;
    for (i, line) in rope.lines().enumerate() {
        let line = line.to_string();
        let t = line.trim();
        let marker = t.trim_start_matches('/').trim_start();
        if marker.starts_with("#region") {
            regions.push(i);
        } else if marker.starts_with("#endregion")
            && let Some(start) = regions.pop()
        {
            out.push((start, i));
        }
        let is_import = (t.starts_with("using ") && t.ends_with(';') && !t.starts_with("using var ") && !t.starts_with("using ("))
            || t.starts_with("import ")
            || t.starts_with("use ")
            || t.starts_with("pub use ");
        match (&mut imports, is_import) {
            (Some(r), true) => r.1 = i,
            (None, true) => imports = Some((i, i)),
            (Some(r), false) => {
                if r.1 > r.0 {
                    out.push(*r);
                }
                imports = None;
            }
            (None, false) => {}
        }
    }
    if let Some(r) = imports
        && r.1 > r.0
    {
        out.push(r);
    }
    out
}

/// Ctrl+M chords (like Visual Studio).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FoldCmd {
    /// Ctrl+M Ctrl+M: fold or unfold the block at the cursor.
    Toggle,
    /// Ctrl+M Ctrl+L: fold everything, or unfold everything if something is folded.
    ToggleAll,
    /// Ctrl+M Ctrl+O: fold the bodies of methods, constructors, properties and functions.
    Definitions,
    /// Ctrl+M Ctrl+P: unfold everything.
    ExpandAll,
}

/// Member kinds whose bodies Ctrl+M Ctrl+O folds.
const MEMBER_KINDS: &[&str] = &["method", "constructor", "property", "function"];

/// A gray hint inside the code from the language server: a parameter name or an inferred type.
#[derive(Debug, Clone, PartialEq)]
pub struct InlayHint {
    pub line: usize,
    /// Char index within the line; the hint is shown before that char.
    pub col: usize,
    pub label: String,
    pub pad_left: bool,
    pub pad_right: bool,
}

impl InlayHint {
    fn text(&self) -> String {
        format!("{}{}{}", if self.pad_left { " " } else { "" }, self.label, if self.pad_right { " " } else { "" })
    }

    fn width(&self) -> usize {
        self.text().chars().count()
    }
}

/// A CodeLens line above a declaration ("3 references").
#[derive(Debug, Clone, PartialEq)]
pub struct CodeLens {
    pub line: usize,
    /// Char index within the line where the declared name starts (for "find references").
    pub col: usize,
    pub title: String,
}

/// What a screen row shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RowKind {
    /// The CodeLens row above a line.
    Lens,
    Text,
    /// Continuation row `n` (1, 2 …) of a wrapped line.
    Wrap(usize),
    /// A further line of a multi-line AI suggestion below a line.
    Ghost,
}

/// Where a line wraps: the char index and screen column each continuation row starts at, and
/// the indentation of continuation rows (in cells).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct LineWrap {
    pub breaks: Vec<(usize, usize)>,
    pub indent: usize,
}

impl LineWrap {
    /// Screen column where row `sub` (0 = the first) starts, and its indentation.
    fn row_start(&self, sub: usize) -> (usize, usize) {
        if sub == 0 { (0, 0) } else { (self.breaks[sub - 1].1, self.indent) }
    }
}

/// Which document lines are on screen and in which rows (CodeLens rows above a line and the
/// rest of an AI suggestion below it take rows of their own).
pub struct RowMap {
    pub lines: Vec<usize>,
    /// Screen row of each line's text, parallel to `lines`.
    pub rows: Vec<usize>,
    above: Vec<usize>,
    below: Vec<usize>,
    /// Word wrap of each line (empty when it fits or wrapping is off).
    wraps: Vec<LineWrap>,
}

impl RowMap {
    pub fn row(&self, line: usize) -> Option<usize> {
        self.lines.binary_search(&line).ok().map(|i| self.rows[i])
    }

    /// The line a screen row belongs to, and what the row shows.
    pub fn at(&self, row: usize) -> Option<(usize, RowKind)> {
        for (i, &line) in self.lines.iter().enumerate() {
            let text = self.rows[i];
            if row < text && row + self.above[i] >= text {
                return Some((line, RowKind::Lens));
            }
            if row == text {
                return Some((line, RowKind::Text));
            }
            let wraps = self.wraps[i].breaks.len();
            if row > text && row <= text + wraps {
                return Some((line, RowKind::Wrap(row - text)));
            }
            if row > text && row <= text + self.below[i] {
                return Some((line, RowKind::Ghost));
            }
        }
        None
    }

    fn wrap_of(&self, line: usize) -> Option<&LineWrap> {
        self.lines.binary_search(&line).ok().map(|i| &self.wraps[i])
    }

    /// Continuation rows of a shown line.
    pub fn wrap_rows(&self, line: usize) -> usize {
        self.wrap_of(line).map_or(0, |w| w.breaks.len())
    }

    /// Screen row and column of screen column `vcol` of `line` (wrapping included).
    pub fn place(&self, line: usize, vcol: usize) -> Option<(usize, usize)> {
        let i = self.lines.binary_search(&line).ok()?;
        let w = &self.wraps[i];
        let sub = w.breaks.iter().take_while(|b| b.1 <= vcol).count();
        let (start, indent) = w.row_start(sub);
        Some((self.rows[i] + sub, vcol - start + indent))
    }

    /// The pieces of the column span `start..end` of `line`, one per screen row:
    /// (row, first column, end column) in screen columns.
    pub fn segments(&self, line: usize, start: usize, end: usize) -> Vec<(usize, usize, usize)> {
        let Some(i) = self.lines.binary_search(&line).ok() else { return Vec::new() };
        let w = &self.wraps[i];
        let mut out = Vec::new();
        for sub in 0..=w.breaks.len() {
            let (rs, indent) = w.row_start(sub);
            let re = w.breaks.get(sub).map_or(usize::MAX, |b| b.1);
            let (lo, hi) = (start.max(rs), end.min(re));
            if hi > lo {
                out.push((self.rows[i] + sub, lo - rs + indent, hi - rs + indent));
            }
        }
        out
    }
}

/// A problem reported by the language server, as a char range in the current text.
#[derive(Debug, Clone)]
pub struct DiagSpan {
    pub start: usize,
    pub end: usize,
    /// The same range in bytes: moved along with every edit until the server sends new ones,
    /// so the squiggles stay under their words while typing.
    pub start_byte: usize,
    pub end_byte: usize,
    /// 1 error, 2 warning, 3 info.
    pub severity: u8,
    pub message: String,
    /// The server's own diagnostic object (sent back when asking for quick actions).
    pub raw: wyven_lsp::Value,
}

pub struct EditorView {
    pub doc: Document,
    pub diagnostics: Vec<DiagSpan>,
    /// Matches of the find box (char ranges, sorted) and which one is current.
    pub find_matches: Vec<(usize, usize)>,
    pub find_current: Option<usize>,
    /// The file's content in the last commit; enables the git bars in the gutter.
    pub git_base: Option<String>,
    git_marks: Vec<(usize, wyven_git::LineMark)>,
    git_marks_version: Option<u64>,
    /// Background color per line (diff view).
    pub line_bg: Vec<Option<Rgba>>,
    /// Replaces the line numbers (diff view: original numbers, blank for filler lines).
    pub gutter_labels: Option<Vec<String>>,
    /// Modification time of the file when it was loaded or saved.
    disk_mtime: Option<std::time::SystemTime>,
    /// Draw cursor and current-line highlight (off in the diff view).
    pub show_cursor: bool,
    syntax: Option<Syntax>,
    pub scroll_line: usize,
    pub scroll_x: f32,
    area: Rect,
    font: FontMetrics,
    family: String,
    gutter_cells: usize,
    text_buf: Buffer,
    gutter_buf: Buffer,
    cache: Option<CacheKey>,
    /// Tab title for documents without a file (e.g. the shortcut cheat sheet).
    title: Option<String>,
    /// This tab shows the settings page instead of text.
    pub settings_page: bool,
    pub minimap: MinimapMode,
    /// The mouse is over the minimap (the visible-area box is drawn brighter).
    pub minimap_hover: bool,
    /// Display scale, for the minimap's pixel sizes.
    pub scale: f32,
    mini_cache: Option<(MiniKey, Vec<MiniRun>)>,
    /// Sticky scroll on/off (`[editor] sticky_scroll`, Alt → v → s).
    pub sticky_scroll: bool,
    /// Long lines continue in the next row (Alt → v → w).
    pub wrap: bool,
    scopes: Vec<Scope>,
    scopes_version: Option<u64>,
    /// Scope headers currently stuck at the top.
    sticky: Vec<Scope>,
    sticky_key: Option<(u64, Vec<Scope>, &'static str, usize)>,
    sticky_text: Buffer,
    sticky_gutter: Buffer,
    /// Foldable blocks (line that stays, last hidden line), sorted by start; computed lazily.
    fold_ranges: Vec<(usize, usize)>,
    fold_ranges_version: Option<u64>,
    /// Folded blocks, sorted by start.
    folded: Vec<(usize, usize)>,
    /// Hidden lines as merged inclusive ranges, sorted.
    hidden: Vec<(usize, usize)>,
    /// Bumped whenever folds change (part of the render cache key).
    fold_gen: u64,
    /// The mouse is over the gutter: fold arrows are shown.
    pub gutter_hover: bool,
    fold_buf: Buffer,
    /// Breakpoint lines (0-based, sorted).
    breakpoints: Vec<usize>,
    /// Line where the debugger is paused.
    debug_line: Option<usize>,
    /// Bumped when breakpoints, the debug line or the AI suggestion change.
    marks_gen: u64,
    mark_buf: Buffer,
    /// AI suggestion shown as ghost text at a char position (only while the cursor is there).
    ghost: Option<(usize, String)>,
    /// Inlay hints, sorted by (line, col); shown when `show_inlay_hints`.
    inlay_hints: Vec<InlayHint>,
    pub show_inlay_hints: bool,
    /// CodeLens rows, sorted by line; shown when `show_code_lens`.
    lenses: Vec<CodeLens>,
    pub show_code_lens: bool,
    /// Brackets colored by nesting depth.
    pub bracket_colors: bool,
    /// Thin lines at each indentation level.
    pub indent_guides: bool,
    /// Small squares showing colors written in CSS, HTML and XAML.
    pub color_preview: bool,
    /// Bracket depth at the start of each line (for the document version), up to some line.
    bracket_depths: Option<(u64, Vec<i32>)>,
}

/// An empty error range in front of a line's code (servers put "; expected" there, at the start
/// of the next line) moves to the end of the code on the line above, where the `;` is missing.
pub fn place_empty_diagnostic(rope: &Rope, start: usize, end: usize) -> (usize, usize) {
    let start = start.min(rope.len_chars());
    if end > start {
        return (start, end);
    }
    let line = rope.char_to_line(start);
    let line_start = rope.line_to_char(line);
    let in_front = (line_start..start).all(|i| rope.char(i).is_whitespace());
    if !in_front || line == 0 {
        return (start, start);
    }
    // The nearest line above with code; after its last non-blank character.
    for l in (line.saturating_sub(5)..line).rev() {
        let ls = rope.line_to_char(l);
        let text = rope.line(l);
        let len = text.chars().count();
        if let Some(last) = (0..len).rev().find(|&i| !text.char(i).is_whitespace()) {
            let pos = ls + last + 1;
            return (pos, pos);
        }
    }
    (start, start)
}

/// Where a byte position (start or end of a range) is after an edit.
fn shift_byte(p: usize, e: &TextEdit, is_end: bool) -> usize {
    if p < e.start_byte || (is_end && p == e.start_byte) {
        p
    } else if p >= e.old_end_byte {
        p + e.new_end_byte - e.old_end_byte
    } else if is_end {
        e.new_end_byte
    } else {
        e.start_byte
    }
}

/// Bracket colors by depth (gold, orchid, blue – like VS Code), dark and light.
fn bracket_color(depth: i32, dark: bool) -> Rgba {
    const DARK: [Rgba; 3] = [Rgba(0xFF, 0xD7, 0x00, 0xFF), Rgba(0xDA, 0x70, 0xD6, 0xFF), Rgba(0x17, 0x9F, 0xFF, 0xFF)];
    const LIGHT: [Rgba; 3] = [Rgba(0x9A, 0x7B, 0x00, 0xFF), Rgba(0xA0, 0x30, 0x9C, 0xFF), Rgba(0x0B, 0x6B, 0xB8, 0xFF)];
    let i = depth.rem_euclid(3) as usize;
    if dark { DARK[i] } else { LIGHT[i] }
}

/// Bracket depth at the start of each line `0..=upto`: strings ("…", '…', `…`), line comments
/// (// and #) and block comments are skipped, so brackets in them do not count.
fn bracket_depths(rope: &Rope, upto: usize) -> Vec<i32> {
    let upto = upto.min(rope.len_lines().saturating_sub(1));
    let mut out = Vec::with_capacity(upto + 1);
    let mut depth = 0i32;
    let mut block = false;
    for line in 0..=upto {
        out.push(depth);
        if line == upto {
            break;
        }
        let mut quote: Option<char> = None;
        let mut prev = '\0';
        let mut chars = rope.line(line).chars().peekable();
        while let Some(c) = chars.next() {
            if block {
                if prev == '*' && c == '/' {
                    block = false;
                }
            } else if let Some(q) = quote {
                if c == q && prev != '\\' {
                    quote = None;
                }
            } else {
                match c {
                    '/' if chars.peek() == Some(&'/') => break,
                    '/' if chars.peek() == Some(&'*') => {
                        block = true;
                        chars.next();
                        prev = '\0';
                        continue;
                    }
                    '"' | '`' => quote = Some(c),
                    // A quote after a letter is an apostrophe (or a Rust lifetime).
                    '\'' if !prev.is_alphanumeric() && prev != '<' && prev != '&' => quote = Some(c),
                    '(' | '[' | '{' => depth += 1,
                    ')' | ']' | '}' => depth -= 1,
                    _ => {}
                }
            }
            prev = if prev == '\\' && c == '\\' { '\0' } else { c };
        }
    }
    out
}

impl EditorView {
    pub fn new(doc: Document, fs: &mut FontSystem, font: FontMetrics, family: String) -> Self {
        #[allow(clippy::let_and_return)]
        let view = Self::build(doc, fs, font, family);
        view
    }

    fn build(doc: Document, fs: &mut FontSystem, font: FontMetrics, family: String) -> Self {
        let syntax = doc
            .path()
            .and_then(language_for_path)
            .map(|cfg| Syntax::new(cfg, doc.text()));
        let metrics = Metrics::new(font.size, font.line_h);
        let mut text_buf = Buffer::new(fs, metrics);
        let mut gutter_buf = Buffer::new(fs, metrics);
        let mut sticky_text = Buffer::new(fs, metrics);
        let mut sticky_gutter = Buffer::new(fs, metrics);
        let mut fold_buf = Buffer::new(fs, metrics);
        let mut mark_buf = Buffer::new(fs, metrics);
        for b in [&mut text_buf, &mut gutter_buf, &mut sticky_text, &mut sticky_gutter, &mut fold_buf, &mut mark_buf] {
            b.set_wrap(Wrap::None);
            b.set_monospace_width(Some(font.cell_w));
        }
        let mut view = Self {
            doc,
            diagnostics: Vec::new(),
            find_matches: Vec::new(),
            find_current: None,
            git_base: None,
            git_marks: Vec::new(),
            git_marks_version: None,
            line_bg: Vec::new(),
            gutter_labels: None,
            disk_mtime: None,
            show_cursor: true,
            syntax,
            scroll_line: 0,
            scroll_x: 0.0,
            area: Rect::default(),
            font,
            family,
            gutter_cells: 4,
            text_buf,
            gutter_buf,
            cache: None,
            title: None,
            settings_page: false,
            minimap: MinimapMode::Off,
            minimap_hover: false,
            scale: 1.0,
            mini_cache: None,
            sticky_scroll: false,
            wrap: false,
            scopes: Vec::new(),
            scopes_version: None,
            sticky: Vec::new(),
            sticky_key: None,
            sticky_text,
            sticky_gutter,
            fold_ranges: Vec::new(),
            fold_ranges_version: None,
            folded: Vec::new(),
            hidden: Vec::new(),
            fold_gen: 0,
            gutter_hover: false,
            fold_buf,
            breakpoints: Vec::new(),
            debug_line: None,
            marks_gen: 0,
            mark_buf,
            ghost: None,
            inlay_hints: Vec::new(),
            show_inlay_hints: false,
            lenses: Vec::new(),
            show_code_lens: false,
            bracket_colors: true,
            indent_guides: true,
            color_preview: true,
            bracket_depths: None,
        };
        view.refresh_disk_mtime();
        view
    }

    fn file_mtime(&self) -> Option<std::time::SystemTime> {
        self.doc.path().and_then(|p| std::fs::metadata(p).and_then(|m| m.modified()).ok())
    }

    /// Remembers the file's current modification time (after loading or saving).
    pub fn refresh_disk_mtime(&mut self) {
        self.disk_mtime = self.file_mtime();
    }

    /// Another view of the same document for the other half of a split, at the same place.
    pub fn twin(&self, fs: &mut FontSystem) -> EditorView {
        let mut v = EditorView::new(Document::default(), fs, self.font, self.family.clone());
        v.doc.tab_width = self.doc.tab_width;
        v.doc.indent_unit = self.doc.indent_unit.clone();
        v.title = self.title.clone();
        v.settings_page = self.settings_page;
        v.minimap = self.minimap;
        v.sticky_scroll = self.sticky_scroll;
        v.wrap = self.wrap;
        v.show_inlay_hints = self.show_inlay_hints;
        v.show_code_lens = self.show_code_lens;
        v.bracket_colors = self.bracket_colors;
        v.indent_guides = self.indent_guides;
        v.color_preview = self.color_preview;
        v.scale = self.scale;
        v.git_base = self.git_base.clone();
        v.mirror_from(self);
        // After the text: on the empty document the cursor would be clamped to 0.
        v.doc.set_selection(self.doc.selection().clone());
        v.scroll_line = self.scroll_line;
        v
    }

    /// Takes over the text of the other half's view of the same file (after an edit there).
    pub fn mirror_from(&mut self, other: &EditorView) {
        self.doc.mirror_from(&other.doc);
        self.syntax = match (&other.syntax, self.doc.path()) {
            (Some(_), Some(p)) => language_for_path(p).map(|cfg| Syntax::new(cfg, self.doc.text())),
            (Some(s), None) => Some(Syntax::new(s.config(), self.doc.text())),
            (None, _) => None,
        };
        self.diagnostics = other.diagnostics.clone();
        self.inlay_hints = other.inlay_hints.clone();
        self.lenses = other.lenses.clone();
        self.git_marks_version = None;
        self.folded.clear();
        self.rebuild_hidden();
        self.scroll_line = self.scroll_line.min(self.doc.text().len_lines().saturating_sub(1));
        self.marks_gen += 1;
        self.invalidate();
    }

    /// If the file changed on disk (another program, git, Claude Code) and has no unsaved
    /// changes here, loads it again, keeping the cursor and scroll position. Returns true if
    /// the text was reloaded.
    pub fn reload_if_changed_on_disk(&mut self) -> bool {
        let Some(path) = self.doc.path().map(Path::to_path_buf) else { return false };
        let now = self.file_mtime();
        if now.is_none() || now == self.disk_mtime || self.doc.is_dirty() {
            return false;
        }
        self.disk_mtime = now;
        let Ok(mut doc) = Document::open(&path) else { return false };
        if doc.text() == self.doc.text() {
            return false;
        }
        let head = self.doc.selection().primary().head.min(doc.text().len_chars());
        doc.tab_width = self.doc.tab_width;
        if !doc.indent_detected() {
            doc.indent_unit = self.doc.indent_unit.clone();
        }
        doc.set_cursor(head);
        self.syntax = doc.path().and_then(language_for_path).map(|cfg| Syntax::new(cfg, doc.text()));
        self.doc = doc;
        self.doc.take_edits();
        self.diagnostics.clear();
        self.git_marks_version = None;
        self.folded.clear();
        self.rebuild_hidden();
        self.invalidate();
        true
    }

    /// A document without a file, with its own tab title and highlighting chosen by
    /// `language_hint` (a file name such as "x.md").
    pub fn virtual_doc(
        title: &str,
        text: &str,
        language_hint: &str,
        fs: &mut FontSystem,
        font: FontMetrics,
        family: String,
    ) -> Self {
        let doc = Document::from_str(text);
        let mut view = Self::new(doc, fs, font, family);
        view.syntax = language_for_path(Path::new(language_hint)).map(|cfg| Syntax::new(cfg, view.doc.text()));
        view.title = Some(title.to_string());
        view
    }

    /// Forces a re-render (theme or settings changed).
    pub fn invalidate(&mut self) {
        self.cache = None;
        self.mini_cache = None;
        self.sticky_key = None;
        self.scopes_version = None;
        self.fold_ranges_version = None;
    }

    pub fn language_name(&self) -> &'static str {
        self.syntax.as_ref().map_or("Text", |s| s.config().name)
    }

    pub fn line_comment(&self) -> &'static str {
        self.syntax
            .as_ref()
            .and_then(|s| s.config().line_comment)
            .unwrap_or("//")
    }

    pub fn file_name(&self) -> String {
        if let Some(t) = &self.title {
            return t.clone();
        }
        self.doc
            .path()
            .and_then(Path::file_name)
            .map_or("Untitled".into(), |n| n.to_string_lossy().into_owned())
    }

    /// An unnamed, never edited, empty document (can be replaced when opening a file).
    pub fn is_untouched(&self) -> bool {
        self.doc.path().is_none()
            && self.title.is_none()
            && !self.settings_page
            && self.doc.version() == 0
            && self.doc.text().len_chars() == 0
    }

    /// Highlights the text as another language (`None`: plain text).
    pub fn set_language(&mut self, name: Option<&str>) {
        self.syntax = name.and_then(wyven_syntax::language_by_name).map(|cfg| Syntax::new(cfg, self.doc.text()));
        self.doc.take_edits();
        self.cache = None;
    }

    /// Saves under a new path and re-detects the language.
    pub fn save_as(&mut self, path: std::path::PathBuf) -> std::io::Result<()> {
        self.doc.save_as(path)?;
        self.syntax = self
            .doc
            .path()
            .and_then(language_for_path)
            .map(|cfg| Syntax::new(cfg, self.doc.text()));
        self.doc.take_edits();
        self.cache = None;
        self.refresh_disk_mtime();
        Ok(())
    }

    /// The language server this file needs, if its language has one.
    pub fn lsp_kind(&self) -> Option<crate::lsp::ServerKind> {
        crate::lsp::ServerKind::for_language(self.language_name())
    }

    /// Window position (left, top) of a char position, and the line height.
    pub fn screen_pos(&self, pos: usize) -> (f32, f32) {
        let rope = self.doc.text();
        let pos = pos.min(rope.len_chars());
        let line = rope.char_to_line(pos);
        let col = self.visual_col(line, pos - rope.line_to_char(line));
        let map = self.row_map();
        let (row, col) = map.place(line, col).map_or((None, col), |(r, c)| (Some(r), c));
        let x = self.text_x() - self.scroll_x + col as f32 * self.font.cell_w;
        let rows = match row {
            Some(r) => r as f32,
            None if line >= self.scroll_line => self.rows_between(self.scroll_line, line) as f32,
            None => -(self.rows_between(line, self.scroll_line) as f32),
        };
        let y = self.area.y + rows * self.font.line_h;
        (x, y)
    }

    /// Screen column (in cells) of char `ch` of `line`: tabs expanded, inlay hints before it
    /// included. A hint at the same position counts after the cursor, so typing goes before it.
    fn visual_col(&self, line: usize, ch: usize) -> usize {
        let rope = self.doc.text();
        let display = char_to_display_col(rope.line(line), ch, self.doc.tab_width);
        display + self.hints_on(line).iter().filter(|h| h.col < ch).map(InlayHint::width).sum::<usize>()
    }

    /// Screen column of every char of `line` and of its end (like `visual_col`, all at once).
    fn vcols(&self, line: usize) -> Vec<usize> {
        let rope = self.doc.text();
        let len = line_len_chars(rope, line);
        let hints = self.hints_on(line);
        let (mut disp, mut hint_w, mut hi) = (0, 0, 0);
        let mut out = Vec::with_capacity(len + 1);
        for (i, c) in rope.line(line).chars().take(len).enumerate() {
            while hi < hints.len() && hints[hi].col < i {
                hint_w += hints[hi].width();
                hi += 1;
            }
            out.push(disp + hint_w);
            disp += char_width(c, disp, self.doc.tab_width);
        }
        while hi < hints.len() && hints[hi].col < len {
            hint_w += hints[hi].width();
            hi += 1;
        }
        out.push(disp + hint_w);
        out
    }

    /// Cells per row when wrapping.
    fn wrap_cols(&self) -> usize {
        (((self.text_right() - self.text_x()) / self.font.cell_w).floor() as usize).saturating_sub(1).max(20)
    }

    /// Where `line` wraps (nothing when wrapping is off or the line fits). Breaks go before a
    /// word; a word longer than the row is cut. Continuation rows keep the line's indentation.
    pub fn line_wrap(&self, line: usize) -> LineWrap {
        if !self.wrap {
            return LineWrap::default();
        }
        let width = self.wrap_cols();
        let v = self.vcols(line);
        let len = v.len() - 1;
        if v[len] <= width {
            return LineWrap::default();
        }
        let chars: Vec<char> = self.doc.text().line(line).chars().take(len).collect();
        let lead = chars.iter().take_while(|c| **c == ' ' || **c == '\t').count();
        let indent = v[lead].min(width / 2);
        let mut breaks = Vec::new();
        let (mut start, mut start_v, mut avail) = (0, 0, width);
        let mut i = 0;
        while i < len {
            if v[i + 1] - start_v > avail && i > start {
                let k = (start + 1..=i)
                    .rev()
                    .find(|&k| k > lead && chars[k - 1] == ' ' && chars[k] != ' ')
                    .unwrap_or(i);
                breaks.push((k, v[k]));
                start = k;
                start_v = v[k];
                avail = width - indent;
                if breaks.len() >= 5000 {
                    break;
                }
                continue;
            }
            i += 1;
        }
        LineWrap { breaks, indent }
    }

    /// The char of `line` closest to screen column `vcol`.
    fn char_at_visual_col(&self, line: usize, vcol: usize) -> usize {
        let rope = self.doc.text();
        let len = line_len_chars(rope, line);
        let mut best = (usize::MAX, 0);
        for ch in 0..=len {
            let c = self.visual_col(line, ch);
            let d = c.abs_diff(vcol);
            if d < best.0 {
                best = (d, ch);
            }
            if c > vcol {
                break;
            }
        }
        best.1
    }

    pub fn line_height(&self) -> f32 {
        self.font.line_h
    }

    /// The most severe problem touching the primary cursor.
    /// The errors and warnings at a char position, worst first, as tooltip text.
    pub fn diagnostics_at(&self, pos: usize) -> String {
        let mut here: Vec<&DiagSpan> = self.diagnostics.iter().filter(|d| d.start <= pos && pos < d.end.max(d.start + 1)).collect();
        here.sort_by_key(|d| d.severity);
        here.iter()
            .map(|d| {
                let kind = match d.severity {
                    1 => "Error",
                    2 => "Warning",
                    _ => "Info",
                };
                format!("{kind}: {}", d.message)
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    pub fn diagnostic_at_cursor(&self) -> Option<&DiagSpan> {
        let head = self.doc.selection().primary().head;
        self.diagnostics
            .iter()
            .filter(|d| d.start <= head && head <= d.end.max(d.start + 1))
            .min_by_key(|d| d.severity)
    }

    /// Start of the identifier the cursor is in or right after (for completion).
    pub fn word_start(&self) -> usize {
        let rope = self.doc.text();
        let mut i = self.doc.selection().primary().head;
        while i > 0 {
            let c = rope.char(i - 1);
            if c.is_alphanumeric() || c == '_' {
                i -= 1;
            } else {
                break;
            }
        }
        i
    }

    pub fn symbols(&self) -> Vec<wyven_syntax::Symbol> {
        self.syntax
            .as_ref()
            .map_or_else(Vec::new, |s| s.symbols(self.doc.text()))
    }

    /// Puts the cursor at a char position and scrolls it to about a third of the view.
    pub fn jump_to(&mut self, pos: usize) {
        self.doc.set_cursor(pos.min(self.doc.text().len_chars()));
        let line = self.doc.text().char_to_line(self.doc.selection().primary().head);
        self.reveal_line(line);
        self.scroll_line = self.back_visible(line, self.full_rows() / 3);
        self.ensure_cursor_visible();
    }

    /// 1-based line and optional 1-based column, clamped to the document.
    pub fn jump_to_line(&mut self, line: usize, col: Option<usize>) {
        let rope = self.doc.text();
        let line = line.saturating_sub(1).min(rope.len_lines().saturating_sub(1));
        let len = line_len_chars(rope, line);
        let col = col.map_or(0, |c| c.saturating_sub(1)).min(len);
        let pos = rope.line_to_char(line) + col;
        self.jump_to(pos);
    }

    pub fn jump_to_byte(&mut self, byte: usize) {
        let rope = self.doc.text();
        let pos = rope.byte_to_char(byte.min(rope.len_bytes()));
        self.jump_to(pos);
    }

    pub fn set_area(&mut self, area: Rect) {
        self.area = area;
    }

    pub fn set_font(&mut self, fs: &mut FontSystem, font: FontMetrics) {
        self.font = font;
        let metrics = Metrics::new(font.size, font.line_h);
        for b in [&mut self.text_buf, &mut self.gutter_buf, &mut self.sticky_text, &mut self.sticky_gutter, &mut self.fold_buf, &mut self.mark_buf] {
            b.set_metrics(metrics);
            b.set_monospace_width(Some(font.cell_w));
        }
        let _ = fs;
        self.cache = None;
        self.sticky_key = None;
    }

    /// Visible line count (the last one may be cut off).
    pub fn rows(&self) -> usize {
        ((self.area.h / self.font.line_h).ceil() as usize).max(1)
    }

    fn full_rows(&self) -> usize {
        ((self.area.h / self.font.line_h).floor() as usize).max(1)
    }

    /// Line numbers, a column for fold arrows, and room for the git bars.
    fn gutter_w(&self) -> f32 {
        (self.gutter_cells as f32 + 3.0) * self.font.cell_w
    }

    fn text_x(&self) -> f32 {
        self.area.x + self.gutter_w() + self.font.cell_w * 0.5
    }

    /// Width of the minimap in pixels (0 when off or the editor is too narrow).
    fn minimap_w(&self) -> f32 {
        let w = match self.minimap {
            MinimapMode::Blocks => MINI_BLOCKS_W,
            MinimapMode::Bar => MINI_BAR_W,
            MinimapMode::Off => return 0.0,
        } * self.scale;
        if self.area.w < w * 5.0 { 0.0 } else { w.round() }
    }

    /// Right edge of the text (left edge of the minimap).
    fn text_right(&self) -> f32 {
        self.area.x + self.area.w - self.minimap_w()
    }

    fn mini_geom(&self) -> Option<MiniGeom> {
        let w = self.minimap_w();
        if w <= 0.0 {
            return None;
        }
        let a = self.area;
        let rect = Rect { x: a.x + a.w - w, y: a.y, w, h: a.h };
        let total = self.doc.text().len_lines().max(1);
        let max_scroll = total.saturating_sub(1).max(1) as f32;
        let rows = self.full_rows() as f32;
        let scroll = self.scroll_line as f32;
        Some(match self.minimap {
            MinimapMode::Blocks => {
                let line_h = (MINI_LINE_H * self.scale).max(1.0);
                let mm_rows = ((a.h / line_h).floor() as usize).max(1);
                // Long files: the minimap scrolls along so the whole file can be reached.
                let (first, k) = if total <= mm_rows {
                    (0, 0.0)
                } else {
                    let spare = (total - mm_rows) as f32;
                    (((scroll / max_scroll) * spare).round().min(spare) as usize, spare / max_scroll)
                };
                let box_y = a.y + (self.scroll_line as f32 - first as f32) * line_h;
                let lines_per_px = 1.0 / (line_h * (1.0 - k).max(0.02));
                MiniGeom { rect, first, line_h, box_y, box_h: rows * line_h, lines_per_px }
            }
            _ => {
                let box_h = (rows / total as f32 * a.h).max(12.0 * self.scale).min(a.h);
                let box_y = a.y + (scroll / max_scroll).min(1.0) * (a.h - box_h);
                let lines_per_px = max_scroll / (a.h - box_h).max(1.0);
                MiniGeom { rect, first: 0, line_h: a.h / total as f32, box_y, box_h, lines_per_px }
            }
        })
    }

    /// Height of the sticky header area in pixels.
    fn sticky_h(&self) -> f32 {
        self.sticky.len() as f32 * self.font.line_h
    }

    /// Click on a sticky header: scrolls that declaration to the top and puts the cursor on it.
    /// Returns false if (x, y) is not on a sticky header.
    pub fn sticky_click(&mut self, x: f32, y: f32) -> bool {
        let a = self.area;
        if self.sticky.is_empty() || x < a.x || x >= self.text_right() || y < a.y || y >= a.y + self.sticky_h() {
            return false;
        }
        let row = ((y - a.y) / self.font.line_h) as usize;
        let Some(scope) = self.sticky.get(row).copied() else { return false };
        let rope = self.doc.text();
        let start = rope.line_to_char(scope.line);
        let indent = rope.line(scope.line).chars().take_while(|c| *c == ' ' || *c == '\t').count();
        self.doc.set_cursor(start + indent);
        // Parents of the scope stay sticky above it.
        let parents = self.sticky[..row].len();
        self.reveal_line(scope.line);
        self.scroll_line = self.back_visible(scope.line, parents);
        true
    }

    pub fn minimap_contains(&self, x: f32, y: f32) -> bool {
        self.mini_geom().is_some_and(|g| {
            let r = g.rect;
            x >= r.x && x < r.x + r.w && y >= r.y && y < r.y + r.h
        })
    }

    /// Mouse press in the minimap: outside the visible-area box it jumps there (centered).
    /// Returns the scroll line to start dragging from, or None if (x, y) is not in the minimap.
    pub fn minimap_press(&mut self, x: f32, y: f32) -> Option<f32> {
        if !self.minimap_contains(x, y) {
            return None;
        }
        let g = self.mini_geom()?;
        if y < g.box_y || y >= g.box_y + g.box_h {
            let total = self.doc.text().len_lines().max(1);
            let line = match self.minimap {
                MinimapMode::Blocks => g.first + ((y - g.rect.y) / g.line_h) as usize,
                _ => ((y - g.rect.y) / g.rect.h * total as f32) as usize,
            }
            .min(total - 1);
            self.scroll_line = line.saturating_sub(self.full_rows() / 2);
            self.scroll_by(0);
        }
        Some(self.scroll_line as f32)
    }

    /// Dragging the box: `dy` pixels since the press that started at `start_scroll`.
    pub fn minimap_drag(&mut self, start_scroll: f32, dy: f32) {
        let Some(g) = self.mini_geom() else { return };
        let target = (start_scroll + dy * g.lines_per_px).max(0.0).round() as usize;
        self.scroll_line = target;
        self.scroll_by(0);
    }

    /// Call after every command that may have changed the text.
    pub fn after_edit(&mut self) {
        let edits = self.doc.take_edits();
        self.shift_folds(&edits);
        self.shift_breakpoints(&edits);
        self.shift_line_items(&edits);
        self.shift_diagnostics(&edits);
        if let Some(syntax) = &mut self.syntax {
            syntax.update(self.doc.text(), &edits);
        }
        let max = self.doc.text().len_lines().saturating_sub(1);
        self.scroll_line = self.scroll_line.min(max);
    }

    pub fn page_lines(&self) -> usize {
        self.full_rows().saturating_sub(1).max(1)
    }

    pub fn scroll_by(&mut self, lines: isize) {
        let max = self.doc.text().len_lines().saturating_sub(1);
        if self.hidden.is_empty() {
            self.scroll_line = (self.scroll_line as isize + lines).clamp(0, max as isize) as usize;
            return;
        }
        self.scroll_line = self.scroll_line.min(max);
        if let Some((s, _)) = self.hidden_range(self.scroll_line) {
            self.scroll_line = s - 1;
        }
        let n = lines.unsigned_abs().min(max + 1);
        self.scroll_line = if lines > 0 {
            self.forward_visible(self.scroll_line, n)
        } else {
            self.back_visible(self.scroll_line, n)
        };
    }

    pub fn scroll_x_by(&mut self, dx: f32) {
        if self.wrap {
            self.scroll_x = 0.0;
            return;
        }
        self.scroll_x = (self.scroll_x + dx).max(0.0);
    }

    /// Scrolls so the primary cursor is visible, with a little margin.
    pub fn ensure_cursor_visible(&mut self) {
        let rope = self.doc.text();
        let head = self.doc.selection().primary().head;
        let line = rope.char_to_line(head);
        self.reveal_line(line);
        let rows = self.full_rows();
        let margin = 2.min(rows / 3);
        // Lines under the sticky headers are hidden, so keep the cursor below them.
        let top_margin = (margin + self.sticky.len()).min(rows / 2);
        if line < self.scroll_line || self.rows_between(self.scroll_line, line) < top_margin {
            self.scroll_line = self.back_visible(line, top_margin);
        } else if self.rows_between(self.scroll_line, line) + margin >= rows {
            self.scroll_line = self.back_visible(line, rows - margin - 1);
        }
        if self.wrap {
            // Wrapped lines take several rows: scroll on until the cursor's row fits.
            self.scroll_x = 0.0;
            let col = self.visual_col(line, head - self.doc.text().line_to_char(line));
            for _ in 0..rows.max(1) {
                let map = self.row_map();
                match map.place(line, col) {
                    Some((r, _)) if r + margin < rows || self.scroll_line >= line => break,
                    None if line < self.scroll_line => break,
                    _ => self.scroll_line = self.forward_visible(self.scroll_line, 1),
                }
            }
            return;
        }
        let rope = self.doc.text();

        let col = self.visual_col(line, head - rope.line_to_char(line)) as f32;
        let x = col * self.font.cell_w;
        let visible = self.text_right() - self.area.x - self.gutter_w() - self.font.cell_w;
        if x < self.scroll_x {
            self.scroll_x = (x - 4.0 * self.font.cell_w).max(0.0);
        } else if x > self.scroll_x + visible - 2.0 * self.font.cell_w {
            self.scroll_x = x - visible + 6.0 * self.font.cell_w;
        }
    }

    /// Text position under a window coordinate.
    pub fn hit_test(&self, x: f32, y: f32) -> usize {
        let rope = self.doc.text();
        let row = ((y - self.area.y) / self.font.line_h).floor().max(0.0) as usize;
        let map = self.row_map();
        let (line, kind) = map
            .at(row)
            .or_else(|| map.lines.last().map(|&l| (l, RowKind::Text)))
            .unwrap_or((0, RowKind::Text));
        let mut col = ((x - self.text_x() + self.scroll_x) / self.font.cell_w)
            .round()
            .max(0.0) as usize;
        if let Some(w) = map.wrap_of(line).filter(|w| !w.breaks.is_empty()) {
            let sub = if let RowKind::Wrap(n) = kind { n } else { 0 };
            let (start, indent) = w.row_start(sub);
            col = start + col.saturating_sub(indent);
            // Past the end of a row: stay on it instead of jumping to the next one.
            if let Some(next) = w.breaks.get(sub) {
                col = col.min(next.1.saturating_sub(1));
            }
            return rope.line_to_char(line) + self.char_at_visual_col(line, col);
        }
        if self.hints_on(line).is_empty() {
            return rope.line_to_char(line) + display_col_to_char(rope.line(line), col, self.doc.tab_width);
        }
        rope.line_to_char(line) + self.char_at_visual_col(line, col)
    }

    pub fn contains(&self, x: f32, y: f32) -> bool {
        x >= self.area.x && x < self.area.x + self.area.w && y >= self.area.y
            && y < self.area.y + self.area.h
    }

    // ----- AI suggestion (ghost text) -----

    pub fn set_ghost(&mut self, ghost: Option<(usize, String)>) {
        let ghost = ghost.filter(|(_, t)| !t.is_empty());
        if ghost != self.ghost {
            self.ghost = ghost;
            self.marks_gen += 1;
        }
    }

    /// The suggestion if it is shown: the cursor is a single caret at its position.
    pub fn active_ghost(&self) -> Option<(usize, &str)> {
        let (pos, text) = self.ghost.as_ref()?;
        let sel = self.doc.selection();
        let r = sel.primary();
        (sel.len() == 1 && r.is_empty() && r.head == *pos && *pos <= self.doc.text().len_chars())
            .then_some((*pos, text.as_str()))
    }

    /// Line of the suggestion and how many extra rows its further lines take.
    fn ghost_rows(&self) -> Option<(usize, usize)> {
        let (pos, text) = self.active_ghost()?;
        Some((self.doc.text().char_to_line(pos), text.matches('\n').count()))
    }

    /// Rows above a line: its CodeLens.
    fn rows_above(&self, line: usize) -> usize {
        usize::from(self.show_code_lens && self.lenses.binary_search_by_key(&line, |l| l.line).is_ok())
    }

    /// Rows below a line: the further lines of an AI suggestion.
    fn rows_below(&self, line: usize, ghost: Option<(usize, usize)>) -> usize {
        ghost.filter(|&(g, _)| g == line).map_or(0, |(_, k)| k)
    }

    /// The lines on screen and their rows.
    pub fn row_map(&self) -> RowMap {
        let total_rows = self.rows();
        let ghost = self.ghost_rows();
        let doc_lines = self.doc.text().len_lines();
        let mut map = RowMap { lines: Vec::new(), rows: Vec::new(), above: Vec::new(), below: Vec::new(), wraps: Vec::new() };
        let mut used = 0;
        let mut l = self.scroll_line;
        while l < doc_lines && used < total_rows {
            if let Some((_, e)) = self.hidden_range(l) {
                l = e + 1;
                continue;
            }
            let above = self.rows_above(l);
            let wrap = self.line_wrap(l);
            let below = wrap.breaks.len() + self.rows_below(l, ghost);
            map.lines.push(l);
            map.rows.push(used + above);
            map.above.push(above);
            map.below.push(below);
            map.wraps.push(wrap);
            used += above + 1 + below;
            l += 1;
        }
        map
    }

    /// Screen row of a document line on screen; rows below lenses and suggestions move down.
    #[cfg(test)]
    fn row_of(&self, lines: &[usize], line: usize) -> Option<usize> {
        let _ = lines;
        self.row_map().row(line)
    }

    /// The document line shown in screen row `row` (lens and suggestion rows belong to their
    /// line).
    #[cfg(test)]
    fn line_at_row(&self, row: usize) -> Option<usize> {
        self.row_map().at(row).map(|(line, _)| line)
    }

    /// The CodeLens under a window position: its line and the char position of the declared
    /// name (where to look for references).
    pub fn lens_at(&self, x: f32, y: f32) -> Option<usize> {
        let a = self.area;
        if x < a.x + self.gutter_w() || x >= self.text_right() || y < a.y + self.sticky_h() || y >= a.y + a.h {
            return None;
        }
        let row = ((y - a.y) / self.font.line_h) as usize;
        let (line, kind) = self.row_map().at(row)?;
        if kind != RowKind::Lens {
            return None;
        }
        let lens = self.lenses.iter().find(|l| l.line == line)?;
        let rope = self.doc.text();
        Some(rope.line_to_char(line) + lens.col.min(line_len_chars(rope, line)))
    }

    // ----- inlay hints and CodeLens -----

    fn hints_on(&self, line: usize) -> &[InlayHint] {
        if !self.show_inlay_hints {
            return &[];
        }
        let start = self.inlay_hints.partition_point(|h| h.line < line);
        let end = self.inlay_hints.partition_point(|h| h.line <= line);
        &self.inlay_hints[start..end]
    }

    pub fn set_inlay_hints(&mut self, mut hints: Vec<InlayHint>) {
        hints.sort_by_key(|h| (h.line, h.col));
        if hints != self.inlay_hints {
            self.inlay_hints = hints;
            self.marks_gen += 1;
        }
    }

    pub fn set_lenses(&mut self, mut lenses: Vec<CodeLens>) {
        lenses.sort_by_key(|l| l.line);
        lenses.dedup_by(|b, a| {
            if a.line == b.line {
                a.title = format!("{}  |  {}", a.title, b.title);
                true
            } else {
                false
            }
        });
        if lenses != self.lenses {
            self.lenses = lenses;
            self.marks_gen += 1;
        }
    }

    /// Squiggles move with the text: before an edit they stay, after it they shift by its
    /// length, inside it they shrink to it (the server sends the real ones shortly after).
    fn shift_diagnostics(&mut self, edits: &[TextEdit]) {
        if self.diagnostics.is_empty() || edits.is_empty() {
            return;
        }
        let map = shift_byte;
        for e in edits {
            for d in &mut self.diagnostics {
                d.start_byte = map(d.start_byte, e, false);
                d.end_byte = map(d.end_byte, e, true).max(d.start_byte);
            }
        }
        let rope = self.doc.text();
        let len = rope.len_bytes();
        for d in &mut self.diagnostics {
            d.start = rope.byte_to_char(d.start_byte.min(len));
            d.end = rope.byte_to_char(d.end_byte.min(len));
        }
    }

    /// Inlay hints on edited lines go away until the server sends new ones; code lenses stay
    /// (their numbers update when the server answers); both move with the lines below an edit.
    fn shift_line_items(&mut self, edits: &[TextEdit]) {
        if (self.inlay_hints.is_empty() && self.lenses.is_empty()) || edits.is_empty() {
            return;
        }
        for e in edits {
            let (sr, or, nr) = (e.start_point.0, e.old_end_point.0, e.new_end_point.0);
            let delta = nr as isize - or as isize;
            let moved = |l: usize| (l as isize + delta).max(0) as usize;
            self.inlay_hints.retain_mut(|h| {
                if h.line > or {
                    h.line = moved(h.line);
                    true
                } else {
                    h.line < sr
                }
            });
            for lens in &mut self.lenses {
                if lens.line > or {
                    lens.line = moved(lens.line);
                } else if lens.line > sr {
                    // Its line was joined into the edited one.
                    lens.line = sr;
                }
            }
            self.lenses.dedup_by_key(|l| l.line);
        }
        self.marks_gen += 1;
    }

    // ----- breakpoints and the debugger's line -----

    pub fn breakpoints(&self) -> &[usize] {
        &self.breakpoints
    }

    pub fn set_breakpoints(&mut self, mut lines: Vec<usize>) {
        lines.sort_unstable();
        lines.dedup();
        if lines != self.breakpoints {
            self.breakpoints = lines;
            self.marks_gen += 1;
        }
    }

    /// F9 / click: adds or removes a breakpoint on `line`.
    pub fn toggle_breakpoint(&mut self, line: usize) {
        let mut lines = self.breakpoints.clone();
        match lines.binary_search(&line) {
            Ok(i) => {
                lines.remove(i);
            }
            Err(i) => lines.insert(i, line),
        }
        self.set_breakpoints(lines);
    }

    pub fn set_debug_line(&mut self, line: Option<usize>) {
        if line != self.debug_line {
            self.debug_line = line;
            self.marks_gen += 1;
        }
    }

    /// Lines below an edit move along; a breakpoint on a deleted line stays on the line
    /// where the deletion started.
    fn shift_breakpoints(&mut self, edits: &[TextEdit]) {
        if self.breakpoints.is_empty() {
            return;
        }
        let mut lines = self.breakpoints.clone();
        for e in edits {
            let (sr, or, nr) = (e.start_point.0, e.old_end_point.0, e.new_end_point.0);
            let delta = nr as isize - or as isize;
            // Text inserted at the very start of a line pushes that line down.
            let pushed = |l: usize| l == sr && sr == or && e.start_point.1 == 0 && nr > or;
            for l in &mut lines {
                if *l > or || pushed(*l) {
                    *l = (*l as isize + delta) as usize;
                } else if *l > sr {
                    // The line was (partly) deleted: the breakpoint stays where the deletion began.
                    *l = sr;
                }
            }
        }
        self.set_breakpoints(lines);
    }

    /// The line whose number was clicked (for toggling a breakpoint), if (x, y) is on a
    /// line number.
    /// Files where colors in the code get a preview square.
    fn shows_colors(&self) -> bool {
        matches!(self.language_name(), "CSS" | "HTML")
            || self.doc.path().and_then(|p| p.extension()).is_some_and(|e| {
                let e = e.to_string_lossy().to_ascii_lowercase();
                matches!(e.as_str(), "css" | "scss" | "less" | "xaml" | "axaml" | "svg" | "html" | "htm" | "vue")
            })
    }

    /// The color preview square under the mouse: the color's char range and value.
    pub fn color_at(&self, x: f32, y: f32) -> Option<(usize, usize, Rgba)> {
        if !self.color_preview || !self.shows_colors() || !self.text_contains(x, y) {
            return None;
        }
        let row = ((y - self.area.y) / self.font.line_h) as usize;
        let (line, kind) = self.row_map().at(row)?;
        if kind != RowKind::Text {
            return None;
        }
        let rope = self.doc.text();
        let chars: Vec<char> = rope.line(line).chars().collect();
        let text: String = chars.iter().collect();
        let col = ((x - (self.text_x() - self.scroll_x)) / self.font.cell_w).floor() as isize;
        let start = rope.line_to_char(line);
        crate::colors::find(&text).into_iter().find_map(|(s, e, c)| {
            let square = self.visual_col(line, s) as isize - 1;
            (s > 0 && chars[s - 1].is_whitespace() && col == square).then_some((start + s, start + e, c))
        })
    }

    /// The color under (or right after) the cursor, for "pick color".
    pub fn color_at_cursor(&self) -> Option<(usize, usize, Rgba)> {
        let rope = self.doc.text();
        let head = self.doc.selection().primary().head;
        let line = rope.char_to_line(head);
        let start = rope.line_to_char(line);
        let text: String = rope.line(line).chars().collect();
        let col = head - start;
        crate::colors::find(&text).into_iter().find(|&(s, e, _)| col >= s && col <= e).map(|(s, e, c)| (start + s, start + e, c))
    }

    /// A git change bar at the left edge of the text under the mouse: the line it belongs to
    /// (`usize::MAX`: lines deleted at the very top).
    pub fn git_mark_at(&self, x: f32, y: f32) -> Option<usize> {
        let a = self.area;
        let edge = a.x + self.gutter_w();
        let cw = self.font.cell_w;
        if self.git_marks.is_empty() || x < edge - cw * 1.3 || x > edge + cw * 0.2 || y < a.y + self.sticky_h() || y >= a.y + a.h {
            return None;
        }
        let lh = self.font.line_h;
        let row = ((y - a.y) / lh) as usize;
        let (line, kind) = self.row_map().at(row)?;
        if kind != RowKind::Text {
            return None;
        }
        let frac = (y - a.y) / lh - row as f32;
        let has = |l: usize, m: wyven_git::LineMark| self.git_marks.iter().any(|&(ml, mm)| ml == l && mm == m);
        use wyven_git::LineMark::*;
        if has(line, Added) || has(line, Modified) {
            return Some(line);
        }
        // Deletion marks sit on the border between two lines.
        if frac > 0.6 && has(line, DeletedAfter) {
            return Some(line);
        }
        if frac < 0.4 {
            if line == 0 && has(usize::MAX, DeletedAfter) {
                return Some(usize::MAX);
            }
            if line > 0 && has(line - 1, DeletedAfter) {
                return Some(line - 1);
            }
        }
        None
    }

    /// The changed block at a marked line, compared with the committed version.
    pub fn git_hunk(&self, line: usize) -> Option<wyven_git::Hunk> {
        let base = self.git_base.as_ref()?;
        wyven_git::hunk_at(base, &self.doc.text().to_string(), line)
    }

    /// Screen y below a line (with its wrapped rows), if it is visible.
    pub fn line_bottom(&self, line: usize) -> Option<f32> {
        let map = self.row_map();
        if line == usize::MAX {
            return (self.scroll_line == 0).then_some(self.area.y);
        }
        let row = map.row(line)?;
        Some(self.area.y + (row + 1 + map.wrap_rows(line)) as f32 * self.font.line_h)
    }

    pub fn line_number_at(&self, x: f32, y: f32) -> Option<usize> {
        let a = self.area;
        let number_right = a.x + (self.gutter_cells as f32 + 1.0) * self.font.cell_w;
        if x < a.x || x >= number_right || y < a.y + self.sticky_h() || y >= a.y + a.h {
            return None;
        }
        let row = ((y - a.y) / self.font.line_h) as usize;
        match self.row_map().at(row)? {
            (line, RowKind::Text) => Some(line),
            _ => None,
        }
    }

    // ----- folding -----

    /// The hidden range containing `line`, if it is folded away.
    fn hidden_range(&self, line: usize) -> Option<(usize, usize)> {
        let i = self.hidden.partition_point(|&(s, _)| s <= line);
        let (s, e) = *self.hidden.get(i.checked_sub(1)?)?;
        (line <= e).then_some((s, e))
    }

    /// Up to `count` shown lines from `top` on, skipping folded lines.
    fn visible_from(&self, top: usize, count: usize) -> Vec<usize> {
        let total = self.doc.text().len_lines();
        let mut out = Vec::with_capacity(count);
        let mut l = top;
        while out.len() < count && l < total {
            if let Some((_, e)) = self.hidden_range(l) {
                l = e + 1;
                continue;
            }
            out.push(l);
            l += 1;
        }
        out
    }

    /// The document lines on screen, top first.
    #[cfg(test)]
    fn screen_lines(&self) -> Vec<usize> {
        self.row_map().lines
    }

    /// Number of shown lines in `a..b`.
    fn rows_between(&self, a: usize, b: usize) -> usize {
        if b <= a {
            return 0;
        }
        let mut n = b - a;
        for &(s, e) in &self.hidden {
            if e < a {
                continue;
            }
            if s >= b {
                break;
            }
            n -= e.min(b - 1) - s.max(a) + 1;
        }
        n
    }

    /// The shown line `n` shown lines above `line`.
    fn back_visible(&self, line: usize, n: usize) -> usize {
        let mut l = line;
        for _ in 0..n {
            if l == 0 {
                break;
            }
            l -= 1;
            if let Some((s, _)) = self.hidden_range(l) {
                l = s - 1;
            }
        }
        l
    }

    /// The shown line `n` shown lines below `line` (stops at the last line).
    fn forward_visible(&self, line: usize, n: usize) -> usize {
        let max = self.doc.text().len_lines().saturating_sub(1);
        let mut l = line;
        for _ in 0..n {
            let mut next = l + 1;
            if let Some((_, e)) = self.hidden_range(next) {
                next = e + 1;
            }
            if next > max {
                break;
            }
            l = next;
        }
        l
    }

    fn is_folded_start(&self, line: usize) -> bool {
        self.folded.binary_search_by_key(&line, |f| f.0).is_ok()
    }

    /// Finds the foldable blocks again after the text changed. Folded blocks follow their
    /// first line; a block that no longer exists opens.
    fn ensure_fold_ranges(&mut self) {
        if self.fold_ranges_version == Some(self.doc.version()) {
            return;
        }
        let rope = self.doc.text();
        let mut r = self.syntax.as_ref().map_or_else(Vec::new, |syn| syn.fold_ranges(rope));
        r.extend(text_folds(rope));
        r.sort_unstable();
        r.dedup_by(|b, a| {
            if a.0 == b.0 {
                a.1 = a.1.max(b.1);
                true
            } else {
                false
            }
        });
        self.fold_ranges = r;
        self.fold_ranges_version = Some(self.doc.version());
        let ranges = &self.fold_ranges;
        let folded: Vec<(usize, usize)> = self
            .folded
            .iter()
            .filter_map(|&(start, _)| ranges.binary_search_by_key(&start, |r| r.0).ok().map(|i| ranges[i]))
            .collect();
        if folded != self.folded {
            self.folded = folded;
            self.rebuild_hidden();
        }
    }

    /// Recomputes the hidden lines; keeps scroll position and cursor on shown lines.
    fn rebuild_hidden(&mut self) {
        self.folded.sort_unstable();
        self.folded.dedup_by_key(|f| f.0);
        let mut h: Vec<(usize, usize)> = Vec::new();
        for &(s, e) in &self.folded {
            let (a, b) = (s + 1, e);
            if b < a {
                continue;
            }
            match h.last_mut() {
                Some(last) if a <= last.1 + 1 => last.1 = last.1.max(b),
                _ => h.push((a, b)),
            }
        }
        self.hidden = h;
        self.fold_gen += 1;
        if let Some((s, _)) = self.hidden_range(self.scroll_line) {
            self.scroll_line = s - 1;
        }
        let rope = self.doc.text();
        let head = self.doc.selection().primary().head;
        let line = rope.char_to_line(head);
        if let Some((s, _)) = self.hidden_range(line) {
            let target = s - 1;
            let col = (head - rope.line_to_char(line)).min(line_len_chars(rope, target));
            let pos = rope.line_to_char(target) + col;
            self.doc.set_cursor(pos);
        }
    }

    /// Edits move the folds below them; an edit inside a folded block (or one that adds or
    /// removes lines on its first line) opens it.
    fn shift_folds(&mut self, edits: &[TextEdit]) {
        if self.folded.is_empty() || edits.is_empty() {
            return;
        }
        let before = self.folded.clone();
        for e in edits {
            let (sr, or, nr) = (e.start_point.0, e.old_end_point.0, e.new_end_point.0);
            let delta = nr as isize - or as isize;
            let shift = |l: usize| (l as isize + delta).max(0) as usize;
            self.folded.retain_mut(|f| {
                if sr > f.1 {
                    true
                } else if or < f.0 {
                    *f = (shift(f.0), shift(f.1));
                    true
                } else {
                    sr == f.0 && or == f.0 && delta == 0
                }
            });
        }
        if self.folded != before {
            self.rebuild_hidden();
        }
    }

    /// Opens the folds that hide `line`.
    fn reveal_line(&mut self, line: usize) {
        if self.hidden_range(line).is_some() {
            self.folded.retain(|f| !(f.0 < line && line <= f.1));
            self.rebuild_hidden();
        }
    }

    pub fn fold(&mut self, cmd: FoldCmd) -> bool {
        self.ensure_fold_ranges();
        match cmd {
            FoldCmd::Toggle => {
                let line = self.doc.text().char_to_line(self.doc.selection().primary().head);
                if let Ok(i) = self.folded.binary_search_by_key(&line, |f| f.0) {
                    self.folded.remove(i);
                } else {
                    // The innermost block around the line that is still open.
                    let Some(r) = self
                        .fold_ranges
                        .iter()
                        .filter(|r| r.0 <= line && line <= r.1 && !self.folded.contains(r))
                        .max_by_key(|r| r.0)
                        .copied()
                    else {
                        return false;
                    };
                    self.folded.push(r);
                }
            }
            FoldCmd::ToggleAll => {
                if self.folded.is_empty() {
                    self.folded = self.fold_ranges.clone();
                } else {
                    self.folded.clear();
                }
            }
            FoldCmd::Definitions => {
                let members: Vec<(usize, usize)> = self
                    .symbols()
                    .into_iter()
                    .filter(|s| MEMBER_KINDS.contains(&s.kind) && s.end_line > s.line)
                    .map(|s| (s.line, s.end_line))
                    .collect();
                self.folded = members
                    .iter()
                    .filter_map(|&(l, end)| self.fold_ranges.iter().find(|r| r.0 >= l && r.0 <= end).copied())
                    .collect();
            }
            FoldCmd::ExpandAll => self.folded.clear(),
        }
        self.rebuild_hidden();
        true
    }

    /// After a cursor move: a cursor that landed on a folded line jumps over the fold.
    pub fn skip_folds(&mut self, old_head: usize) {
        if self.hidden.is_empty() {
            return;
        }
        let rope = self.doc.text();
        let head = self.doc.selection().primary().head;
        let line = rope.char_to_line(head);
        let Some((s, e)) = self.hidden_range(line) else { return };
        let col = head - rope.line_to_char(line);
        let at_end = col >= line_len_chars(rope, line);
        let forward = head > old_head;
        let target = if forward { e + 1 } else { s - 1 };
        if target >= rope.len_lines() {
            self.reveal_line(line);
            return;
        }
        let len = line_len_chars(rope, target);
        let col = if !forward && at_end { len } else { col.min(len) };
        let pos = rope.line_to_char(target) + col;
        self.doc.set_cursor(pos);
    }

    /// The text itself (not the gutter or minimap): the mouse shows an I-beam there.
    pub fn text_contains(&self, x: f32, y: f32) -> bool {
        let a = self.area;
        x >= a.x + self.gutter_w() && x < self.text_right() && y >= a.y && y < a.y + a.h
    }

    /// The gutter (line numbers and fold arrows).
    pub fn gutter_contains(&self, x: f32, y: f32) -> bool {
        let a = self.area;
        x >= a.x && x < a.x + self.gutter_w() && y >= a.y + self.sticky_h() && y < a.y + a.h
    }

    /// Click on a fold arrow in the gutter or on the `…` marker after a folded line.
    pub fn fold_click(&mut self, x: f32, y: f32) -> bool {
        let a = self.area;
        if y < a.y + self.sticky_h() || y >= a.y + a.h || x < a.x || x >= self.text_right() {
            return false;
        }
        let row = ((y - a.y) / self.font.line_h) as usize;
        let Some((line, RowKind::Text)) = self.row_map().at(row) else { return false };
        let arrow_left = a.x + (self.gutter_cells as f32 + 1.0) * self.font.cell_w;
        let in_gutter = x >= arrow_left && x < a.x + self.gutter_w();
        if x < arrow_left {
            return false;
        }
        if self.is_folded_start(line) {
            if !in_gutter {
                // Only the marker itself opens the fold.
                let rope = self.doc.text();
                let end_col = char_to_display_col(rope.line(line), line_len_chars(rope, line), self.doc.tab_width);
                let marker_x = self.text_x() - self.scroll_x + end_col as f32 * self.font.cell_w;
                if x < marker_x || x > marker_x + 4.0 * self.font.cell_w {
                    return false;
                }
            }
            self.folded.retain(|f| f.0 != line);
            self.rebuild_hidden();
            return true;
        }
        if !in_gutter {
            return false;
        }
        self.ensure_fold_ranges();
        let Ok(i) = self.fold_ranges.binary_search_by_key(&line, |r| r.0) else { return false };
        self.folded.push(self.fold_ranges[i]);
        self.rebuild_hidden();
        true
    }

    /// Re-shapes the visible text if anything relevant changed.
    pub fn prepare(&mut self, fs: &mut FontSystem, theme: &Theme) {
        if let Some(base) = &self.git_base {
            if self.git_marks_version != Some(self.doc.version()) {
                self.git_marks = wyven_git::line_marks(base, &self.doc.text().to_string());
                self.git_marks_version = Some(self.doc.version());
            }
        } else if !self.git_marks.is_empty() {
            self.git_marks.clear();
            self.git_marks_version = None;
        }
        if self.gutter_hover || !self.folded.is_empty() {
            self.ensure_fold_ranges();
        }
        self.prepare_minimap(theme);
        self.prepare_sticky(fs, theme);
        let rope = self.doc.text();
        let digits = rope.len_lines().max(1).to_string().len().max(3);
        self.gutter_cells = digits;
        let current_line = rope.char_to_line(self.doc.selection().primary().head);
        let key = CacheKey {
            version: self.doc.version(),
            scroll_line: self.scroll_line,
            rows: self.rows(),
            current_line,
            font: (self.font.size.to_bits(), self.font.cell_w.to_bits()),
            gutter_cells: digits,
            fold_gen: self.fold_gen,
            gutter_hover: self.gutter_hover,
            marks_gen: self.marks_gen,
            wrap: if self.wrap { self.wrap_cols() } else { 0 },
        };
        if self.cache.as_ref() == Some(&key) {
            return;
        }

        let row_map = self.row_map();
        let lines = row_map.lines.clone();
        let lens_title = |line: usize| -> Option<&str> {
            if !self.show_code_lens {
                return None;
            }
            let i = self.lenses.binary_search_by_key(&line, |l| l.line).ok()?;
            Some(self.lenses[i].title.as_str())
        };
        // The suggestion: its line, its char column there, and its text.
        let ghost = self.active_ghost().map(|(pos, text)| {
            let line = rope.char_to_line(pos);
            (line, pos - rope.line_to_char(line), text.to_string())
        });
        let ghost_extra = ghost.as_ref().map_or(0, |(_, _, t)| t.matches('\n').count());

        // Bracket depth at the start of each visible line (large files: no bracket colors).
        let pair_colors = self.bracket_colors && rope.len_chars() < 3_000_000;
        let line_depth: Vec<i32> = if pair_colors {
            let last = lines.last().copied().unwrap_or(0);
            let version = self.doc.version();
            let fresh = self.bracket_depths.as_ref().is_some_and(|(v, d)| *v == version && d.len() > last);
            if !fresh {
                self.bracket_depths = Some((version, bracket_depths(rope, last + 200)));
            }
            let depths = &self.bracket_depths.as_ref().expect("just computed").1;
            lines.iter().map(|&l| depths.get(l).copied().unwrap_or(0)).collect()
        } else {
            Vec::new()
        };
        let dark = theme.dark;
        // Build colored runs for the visible lines; tabs become spaces so the grid holds.
        let tab_width = self.doc.tab_width;
        let mut runs: Vec<(String, Rgba)> = Vec::new();
        let mut push = |text: &str, color: Rgba| match runs.last_mut() {
            Some((t, c)) if *c == color => t.push_str(text),
            _ => runs.push((text.to_string(), color)),
        };
        // Highlight each run of consecutive lines separately: a fold may hide thousands.
        let mut seg = 0;
        while seg < lines.len() {
            let mut seg_end = seg + 1;
            while seg_end < lines.len() && lines[seg_end] == lines[seg_end - 1] + 1 {
                seg_end += 1;
            }
            let start_byte = rope.line_to_byte(lines[seg]);
            let end_byte = rope.line_to_byte(lines[seg_end - 1] + 1);
            let spans = match &self.syntax {
                Some(s) => flatten(&s.highlights(rope, start_byte..end_byte), start_byte..end_byte),
                None => Vec::new(),
            };
            let mut span_i = 0;
            for (li, &line) in lines[seg..seg_end].iter().enumerate() {
            let mut depth = line_depth.get(seg + li).copied().unwrap_or(0);
            // CodeLens row above the line, indented like it.
            if let Some(title) = lens_title(line) {
                let indent: String = rope
                    .line(line)
                    .chars()
                    .take_while(|c| *c == ' ' || *c == '\t')
                    .map(|c| if c == '\t' { " ".repeat(tab_width) } else { " ".into() })
                    .collect();
                push(&indent, theme.text);
                push(title, theme.gutter_text);
                push("\n", theme.text);
            }
            let hints = self.hints_on(line);
            let mut byte = rope.line_to_byte(line);
            let mut col = 0;
            let ghost_here = ghost.as_ref().filter(|(g, _, _)| *g == line).map(|(_, c, t)| (*c, t.as_str()));
            // Writes the suggestion (dimmed) at the cursor; the rest of the line follows its
            // last line.
            let put_ghost = |push: &mut dyn FnMut(&str, Rgba), col: &mut usize, text: &str| {
                for (i, part) in text.split('\n').enumerate() {
                    if i > 0 {
                        push("\n", theme.text);
                        *col = 0;
                    }
                    let mut expanded = String::new();
                    for ch in part.chars() {
                        if ch == '\t' {
                            let n = tab_width - *col % tab_width;
                            expanded.push_str(&" ".repeat(n));
                            *col += n;
                        } else if ch != '\r' {
                            expanded.push(ch);
                            *col += 1;
                        }
                    }
                    push(&expanded, theme.gutter_text);
                }
            };
            let mut char_i = 0;
            let wrap = row_map.wrap_of(line).cloned().unwrap_or_default();
            let mut next_break = 0;
            for ch in rope.line(line).chars() {
                if ch == '\n' || ch == '\r' {
                    break;
                }
                // Word wrap: the rest continues in the next row, indented.
                if wrap.breaks.get(next_break).is_some_and(|b| b.0 == char_i) {
                    push("\n", theme.text);
                    push(&" ".repeat(wrap.indent), theme.text);
                    next_break += 1;
                }
                if let Some((gc, gt)) = ghost_here
                    && gc == char_i
                {
                    put_ghost(&mut push, &mut col, gt);
                }
                for h in hints.iter().filter(|h| h.col == char_i) {
                    push(&h.text(), theme.gutter_text);
                }
                char_i += 1;
                while span_i < spans.len() && spans[span_i].end <= byte {
                    span_i += 1;
                }
                let hl = spans.get(span_i).filter(|s| s.start <= byte).map(|s| s.highlight);
                let mut color = hl.map_or(theme.text, |h| theme.highlight(h));
                // Bracket pairs in colors by depth (not inside strings and comments).
                if pair_colors && !matches!(hl, Some(wyven_syntax::Highlight::String | wyven_syntax::Highlight::Comment)) {
                    match ch {
                        '(' | '[' | '{' => {
                            color = bracket_color(depth, dark);
                            depth += 1;
                        }
                        ')' | ']' | '}' => {
                            depth -= 1;
                            color = if depth < 0 { theme.error } else { bracket_color(depth, dark) };
                        }
                        _ => {}
                    }
                }
                if ch == '\t' {
                    let n = tab_width - col % tab_width;
                    push(&" ".repeat(n), color);
                    col += n;
                } else {
                    let mut tmp = [0u8; 4];
                    push(ch.encode_utf8(&mut tmp), color);
                    col += 1;
                }
                byte += ch.len_utf8();
            }
            if let Some((gc, gt)) = ghost_here
                && gc >= char_i
            {
                put_ghost(&mut push, &mut col, gt);
            }
            for h in hints.iter().filter(|h| h.col >= char_i) {
                push(&h.text(), theme.gutter_text);
            }
            if self.folded.binary_search_by_key(&line, |f| f.0).is_ok() {
                push(" … ", theme.gutter_current);
            }
            push("\n", theme.text);
            }
            seg = seg_end;
        }

        let family = Family::Name(&self.family);
        let base = Attrs::new().family(family);
        self.text_buf.set_rich_text(
            runs.iter()
                .map(|(t, c)| (t.as_str(), base.clone().color(c.to_glyphon()))),
            &base,
            Shaping::Advanced,
            None,
        );
        self.text_buf.shape_until_scroll(fs, false);

        let mut gutter: Vec<(String, Rgba)> = Vec::new();
        let mut arrows = String::new();
        let mut marks: Vec<(String, Rgba)> = Vec::new();
        for &line in &lines {
            if lens_title(line).is_some() {
                gutter.push(("\n".into(), theme.gutter_text));
                marks.push(("\n".into(), theme.text));
                arrows.push('\n');
            }
            let mark = if self.debug_line == Some(line) {
                ("\u{27a4}\n", theme.accent)
            } else if self.breakpoints.binary_search(&line).is_ok() {
                ("\u{25cf}\n", theme.error)
            } else {
                ("\n", theme.text)
            };
            marks.push((mark.0.to_string(), mark.1));
            if self.is_folded_start(line) {
                arrows.push('▸');
            } else if self.gutter_hover && self.fold_ranges.binary_search_by_key(&line, |r| r.0).is_ok() {
                arrows.push('▾');
            }
            arrows.push('\n');
            let color = if line == current_line { theme.gutter_current } else { theme.gutter_text };
            let label = match &self.gutter_labels {
                Some(labels) => labels.get(line).cloned().unwrap_or_default(),
                None => (line + 1).to_string(),
            };
            gutter.push((format!("{label:>digits$}\n"), color));
            for _ in 0..row_map.wrap_rows(line) {
                gutter.push(("\n".into(), color));
                marks.push(("\n".into(), theme.text));
                arrows.push('\n');
            }
            if ghost.as_ref().is_some_and(|(g, _, _)| *g == line) {
                for _ in 0..ghost_extra {
                    gutter.push(("\n".into(), color));
                    marks.push(("\n".into(), theme.text));
                    arrows.push('\n');
                }
            }
        }
        self.gutter_buf.set_rich_text(
            gutter
                .iter()
                .map(|(t, c)| (t.as_str(), base.clone().color(c.to_glyphon()))),
            &base,
            Shaping::Basic,
            None,
        );
        self.gutter_buf.shape_until_scroll(fs, false);
        self.fold_buf.set_text(&arrows, &base.clone().color(theme.gutter_text.to_glyphon()), Shaping::Advanced, None);
        self.fold_buf.shape_until_scroll(fs, false);
        self.mark_buf.set_rich_text(
            marks.iter().map(|(t, c)| (t.as_str(), base.clone().color(c.to_glyphon()))),
            &base,
            Shaping::Advanced,
            None,
        );
        self.mark_buf.shape_until_scroll(fs, false);
        self.cache = Some(key);
    }

    /// Adds the editor's rectangles and text to the frame.
    pub fn draw<'a>(
        &'a self,
        theme: &Theme,
        scale: f32,
        under: &mut Vec<Quad>,
        over: &mut Vec<Quad>,
        text: &mut Vec<TextBlock<'a>>,
    ) {
        let a = self.area;
        let rope = self.doc.text();
        let lh = self.font.line_h;
        let cw = self.font.cell_w;
        let text_left = self.area.x + self.gutter_w();
        let text_right = self.text_right();
        let text_x = self.text_x() - self.scroll_x;
        let map = self.row_map();
        let lines = map.lines.clone();
        let first = lines.first().copied().unwrap_or(0);
        let last = lines.last().map_or(first, |l| l + 1);
        let row_of = |line: usize| map.row(line);
        let row_y = |row: usize| a.y + row as f32 * lh;
        // Line and screen column (inlay hints included) of a char position.
        // Positions from servers (errors, hints) can be stale and past the end: clamp them.
        let col_of = |pos: usize| {
            let pos = pos.min(rope.len_chars());
            let line = rope.char_to_line(pos);
            (line, self.visual_col(line, pos - rope.line_to_char(line)))
        };
        // Horizontal clip of a rectangle to the text area.
        let clip = |x: f32, w: f32| -> Option<(f32, f32)> {
            let x0 = x.max(text_left);
            let x1 = (x + w).min(text_right);
            (x1 > x0).then_some((x0, x1 - x0))
        };

        under.push(Quad { x: a.x, y: a.y, w: a.w, h: a.h, color: theme.background });

        let primary = self.doc.selection().primary();
        let (cur_line, _) = col_of(primary.head);
        if self.show_cursor
            && primary.is_empty()
            && let Some(row) = row_of(cur_line)
        {
            under.push(Quad {
                x: text_left,
                y: row_y(row),
                w: text_right - text_left,
                h: lh * (1 + map.wrap_rows(cur_line)) as f32,
                color: theme.current_line,
            });
        }

        // The debugger's line, and a faint tint on breakpoint lines.
        for &line in &lines {
            let Some(row) = row_of(line) else { continue };
            let color = if self.debug_line == Some(line) {
                Some(theme.debug_line)
            } else if self.breakpoints.binary_search(&line).is_ok() {
                Some(theme.error.with_alpha(0x22))
            } else {
                None
            };
            if let Some(color) = color {
                let h = lh * (1 + map.wrap_rows(line)) as f32;
                under.push(Quad { x: text_left, y: row_y(row), w: text_right - text_left, h, color });
            }
        }

        // Find matches: only those on visible lines.
        let visible_start = rope.line_to_char(first);
        let visible_end = rope.line_to_char(last);
        let begin = self.find_matches.partition_point(|&(_, e)| e < visible_start);
        for (i, &(s, e)) in self.find_matches.iter().enumerate().skip(begin) {
            if s > visible_end {
                break;
            }
            let color = if self.find_current == Some(i) {
                theme.accent.with_alpha(0x90)
            } else {
                theme.accent.with_alpha(0x38)
            };
            let (l0, c0) = col_of(s);
            let (l1, c1) = col_of(e);
            for line in l0.max(first)..=l1.min(last.saturating_sub(1)) {
                let start = if line == l0 { c0 } else { 0 };
                let end = if line == l1 {
                    c1
                } else {
                    self.visual_col(line, line_len_chars(rope, line))
                };
                for (row, s, e) in map.segments(line, start, end.max(start + 1)) {
                    if let Some((x, w)) = clip(text_x + s as f32 * cw, (e - s) as f32 * cw) {
                        under.push(Quad { x, y: row_y(row), w, h: lh, color });
                    }
                }
            }
        }

        for r in self.doc.selection().ranges() {
            if r.is_empty() {
                continue;
            }
            let (l0, c0) = col_of(r.from());
            let (l1, c1) = col_of(r.to());
            for line in l0.max(first)..=l1.min(last.saturating_sub(1)) {
                let start = if line == l0 { c0 } else { 0 };
                let end = if line == l1 {
                    c1
                } else {
                    let len = line_len_chars(rope, line);
                    self.visual_col(line, len) + 1
                };
                for (row, s, e) in map.segments(line, start, end) {
                    if let Some((x, w)) = clip(text_x + s as f32 * cw, (e - s) as f32 * cw) {
                        under.push(Quad { x, y: row_y(row), w, h: lh, color: theme.selection });
                    }
                }
            }
        }

        let sticky_bottom = a.y + self.sticky_h();
        let cursor_w = (2.0 * scale).round().max(1.0);
        for r in self.doc.selection().ranges() {
            let (line, col) = col_of(r.head);
            let Some((row, col)) = map.place(line, col) else { continue };
            if row_y(row) < sticky_bottom {
                continue;
            }
            if let Some((x, w)) = clip(text_x + col as f32 * cw - cursor_w / 2.0, cursor_w) {
                over.push(Quad { x, y: row_y(row), w, h: lh, color: theme.cursor });
            }
        }

        // Indent guides: a thin line at each indentation level (blank lines continue them).
        if self.indent_guides {
            let unit = if self.doc.indent_unit == "\t" { self.doc.tab_width } else { self.doc.indent_unit.len().max(1) };
            let indent_of = |line: usize| -> Option<usize> {
                let mut cols = 0;
                for c in rope.line(line).chars() {
                    match c {
                        ' ' => cols += 1,
                        '\t' => cols += self.doc.tab_width - cols % self.doc.tab_width,
                        '\r' | '\n' => return None,
                        _ => return Some(cols),
                    }
                }
                None
            };
            let guide = theme.gutter_text.with_alpha(0x38);
            let w = scale.round().max(1.0);
            for &line in &lines {
                let Some(row) = row_of(line) else { continue };
                let cols = indent_of(line).unwrap_or_else(|| {
                    // A blank line: as deep as the shallower of its neighbours.
                    let up = (line.saturating_sub(100)..line).rev().find_map(indent_of).unwrap_or(0);
                    let down = (line + 1..(line + 100).min(rope.len_lines())).find_map(indent_of).unwrap_or(0);
                    up.min(down)
                });
                let h = lh * (1 + map.wrap_rows(line)) as f32;
                for k in 0..cols / unit {
                    let x = text_x + (k * unit) as f32 * cw;
                    if x >= text_left && x < text_right {
                        under.push(Quad { x, y: row_y(row), w, h, color: guide });
                    }
                }
            }
        }

        // Colors written in the code: a small square in front (or a line under them).
        if self.color_preview && self.shows_colors() {
            for &line in &lines {
                let chars: Vec<char> = rope.line(line).chars().collect();
                let text: String = chars.iter().collect();
                for (s, e, c) in crate::colors::find(&text) {
                    let Some((row, col)) = map.place(line, self.visual_col(line, s)) else { continue };
                    let y = row_y(row);
                    if y < sticky_bottom {
                        continue;
                    }
                    let x = text_x + col as f32 * cw;
                    if s > 0 && chars[s - 1].is_whitespace() {
                        let size = (lh * 0.6).min(cw * 0.95).round();
                        let sx = (x - cw + (cw - size) / 2.0).round();
                        let sy = (y + (lh - size) / 2.0).round();
                        if sx >= text_left && sx + size <= text_right {
                            over.push(Quad { x: sx - 1.0, y: sy - 1.0, w: size + 2.0, h: size + 2.0, color: theme.text.with_alpha(0x90) });
                            over.push(Quad { x: sx, y: sy, w: size, h: size, color: c });
                        }
                    } else if let Some((ux, uw)) = clip(x, (e - s) as f32 * cw) {
                        over.push(Quad { x: ux, y: y + lh - (2.0 * scale).round(), w: uw, h: (2.0 * scale).round(), color: c });
                    }
                }
            }
        }

        // Squiggles under problems: a thin zigzag made of short steps.
        let thick = scale.round().max(1.0);
        let step = (2.0 * scale).round().max(2.0);
        for d in &self.diagnostics {
            let color = match d.severity {
                1 => theme.error,
                2 => theme.warning,
                _ => theme.info,
            };
            let (l0, c0) = col_of(d.start);
            let (l1, c1) = col_of(d.end.max(d.start + 1).min(rope.len_chars()));
            for line in l0.max(first)..=l1.min(last.saturating_sub(1)) {
                let s0 = if line == l0 { c0 } else { 0 };
                let e0 = if line == l1 {
                    c1.max(s0 + 1)
                } else {
                    self.visual_col(line, line_len_chars(rope, line))
                };
                for (row, s, e) in map.segments(line, s0, e0) {
                    if row_y(row) < sticky_bottom {
                        continue;
                    }
                    let base = row_y(row) + lh - 2.0 * step;
                    let x0 = text_x + s as f32 * cw;
                    let x1 = text_x + e as f32 * cw;
                    let mut x = x0;
                    let mut up = false;
                    while x < x1 {
                        let w = step.min(x1 - x);
                        if let Some((cx, cw2)) = clip(x, w) {
                            let y = if up { base } else { base + step - thick };
                            over.push(Quad { x: cx, y, w: cw2, h: thick, color });
                            // Vertical connector so it reads as a wave.
                            over.push(Quad { x: cx, y: base, w: thick, h: step, color });
                        }
                        x += step;
                        up = !up;
                    }
                }
            }
        }

        // A soft box behind each inlay hint.
        for &line in &lines {
            for h in self.hints_on(line) {
                let start = self.visual_col(line, h.col) + usize::from(h.pad_left);
                let Some((row, start)) = map.place(line, start) else { continue };
                let w = h.label.chars().count() as f32 * cw;
                if let Some((x, w)) = clip(text_x + start as f32 * cw, w) {
                    let pad = (lh * 0.12).round();
                    under.push(Quad { x, y: row_y(row) + pad, w, h: lh - 2.0 * pad, color: theme.selection.with_alpha(0x38) });
                }
            }
        }

        // Line backgrounds (diff view).
        for &line in &lines {
            let Some(row) = row_of(line) else { continue };
            if let Some(Some(color)) = self.line_bg.get(line) {
                under.push(Quad { x: text_left, y: row_y(row), w: text_right - text_left, h: lh, color: *color });
            }
            // Box behind the `…` of a folded line.
            if self.is_folded_start(line) {
                let end_col = self.visual_col(line, line_len_chars(rope, line));
                let (row, end_col) = map.place(line, end_col).unwrap_or((row, end_col));
                let x = text_x + (end_col as f32 + 0.5) * cw;
                if let Some((x, w)) = clip(x, 2.0 * cw) {
                    let pad = (lh * 0.15).round();
                    under.push(Quad { x, y: row_y(row) + pad, w, h: lh - 2.0 * pad, color: theme.selection.with_alpha(0x70) });
                }
            }
        }

        // Git bars in the gutter: green new, blue changed, red triangle-ish mark for deletions.
        let bar_w = (3.0 * scale).round().max(2.0);
        let bar_x = text_left - cw * 0.6;
        for &(line, mark) in &self.git_marks {
            match mark {
                wyven_git::LineMark::Added | wyven_git::LineMark::Modified => {
                    if let Some(row) = row_of(line) {
                        let color = if mark == wyven_git::LineMark::Added { theme.git_added } else { theme.git_modified };
                        let h = lh * (1 + map.wrap_rows(line)) as f32;
                        under.push(Quad { x: bar_x, y: row_y(row), w: bar_w, h, color });
                    }
                }
                wyven_git::LineMark::DeletedAfter => {
                    // usize::MAX = deleted at the very top: mark above the first line.
                    let y = if line == usize::MAX {
                        (self.scroll_line == 0).then_some(a.y)
                    } else {
                        row_of(line).map(|row| row_y(row) + lh)
                    };
                    if let Some(y) = y {
                        let h = (lh * 0.3).round();
                        under.push(Quad { x: bar_x - bar_w, y: y - h / 2.0, w: bar_w * 3.0, h, color: theme.git_deleted });
                    }
                }
            }
        }

        let bottom = (a.y + a.h) as i32;
        let code_top = sticky_bottom as i32;
        text.push(TextBlock {
            buffer: &self.gutter_buf,
            x: a.x + cw,
            y: a.y,
            color: theme.gutter_text,
            clip: Some([a.x as i32, code_top, text_left as i32, bottom]),
        });
        text.push(TextBlock {
            buffer: &self.mark_buf,
            x: a.x + 0.1 * cw,
            y: a.y,
            color: theme.error,
            clip: Some([a.x as i32, code_top, text_left as i32, bottom]),
        });
        text.push(TextBlock {
            buffer: &self.fold_buf,
            x: a.x + (self.gutter_cells as f32 + 1.2) * cw,
            y: a.y,
            color: theme.gutter_text,
            clip: Some([a.x as i32, code_top, text_left as i32, bottom]),
        });
        text.push(TextBlock {
            buffer: &self.text_buf,
            x: text_x,
            y: a.y,
            color: theme.text,
            clip: Some([text_left as i32, code_top, text_right as i32, bottom]),
        });
        if !self.sticky.is_empty() {
            // Covers selections and highlights of the lines underneath.
            let h = self.sticky_h();
            under.push(Quad { x: a.x, y: a.y, w: text_right - a.x, h, color: theme.background });
            let shadow = (2.0 * scale).round().max(1.0);
            under.push(Quad { x: a.x, y: a.y + h, w: text_right - a.x, h: shadow, color: theme.divider });
            text.push(TextBlock {
                buffer: &self.sticky_gutter,
                x: a.x + cw,
                y: a.y,
                color: theme.gutter_text,
                clip: Some([a.x as i32, a.y as i32, text_left as i32, code_top]),
            });
            text.push(TextBlock {
                buffer: &self.sticky_text,
                x: text_x,
                y: a.y,
                color: theme.text,
                clip: Some([text_left as i32, a.y as i32, text_right as i32, code_top]),
            });
        }
        self.draw_minimap(theme, under, over);
    }

    fn prepare_sticky(&mut self, fs: &mut FontSystem, theme: &Theme) {
        if !self.sticky_scroll || self.syntax.is_none() {
            self.sticky.clear();
            return;
        }
        if self.scopes_version != Some(self.doc.version()) {
            self.scopes = self
                .symbols()
                .into_iter()
                .filter(|s| s.end_line > s.line && STICKY_KINDS.contains(&s.kind))
                .map(|s| Scope { line: s.line, end: s.end_line })
                .collect();
            self.scopes_version = Some(self.doc.version());
        }
        // Never cover more than a third of the editor.
        let max = STICKY_MAX.min(self.full_rows() / 3);
        let shown = self.visible_from(self.scroll_line, max + 1);
        self.sticky = sticky_for(&self.scopes, &shown, max);
        if self.sticky.is_empty() {
            return;
        }
        let digits = self.doc.text().len_lines().max(1).to_string().len().max(3);
        let key = (self.doc.version(), self.sticky.clone(), theme.name, digits);
        if self.sticky_key.as_ref() == Some(&key) {
            return;
        }
        let rope = self.doc.text();
        let tab = self.doc.tab_width;
        let mut runs: Vec<(String, Rgba)> = Vec::new();
        let mut gutter: Vec<(String, Rgba)> = Vec::new();
        for scope in &self.sticky {
            let line = scope.line;
            let start_byte = rope.line_to_byte(line);
            let end_byte = rope.line_to_byte((line + 1).min(rope.len_lines()));
            let spans = match &self.syntax {
                Some(syn) => flatten(&syn.highlights(rope, start_byte..end_byte), start_byte..end_byte),
                None => Vec::new(),
            };
            let mut byte = start_byte;
            let mut col = 0;
            let mut span_i = 0;
            for ch in rope.line(line).chars() {
                if ch == '\n' || ch == '\r' {
                    break;
                }
                while span_i < spans.len() && spans[span_i].end <= byte {
                    span_i += 1;
                }
                let color = spans
                    .get(span_i)
                    .filter(|sp| sp.start <= byte)
                    .map_or(theme.text, |sp| theme.highlight(sp.highlight));
                let piece = if ch == '\t' {
                    let n = tab - col % tab;
                    col += n;
                    " ".repeat(n)
                } else {
                    col += 1;
                    ch.to_string()
                };
                match runs.last_mut() {
                    Some((t, c)) if *c == color => t.push_str(&piece),
                    _ => runs.push((piece, color)),
                }
                byte += ch.len_utf8();
            }
            runs.push(("\n".into(), theme.text));
            gutter.push((format!("{:>digits$}\n", line + 1), theme.gutter_text));
        }
        let base = Attrs::new().family(Family::Name(&self.family));
        self.sticky_text.set_rich_text(
            runs.iter().map(|(t, c)| (t.as_str(), base.clone().color(c.to_glyphon()))),
            &base,
            Shaping::Advanced,
            None,
        );
        self.sticky_text.shape_until_scroll(fs, false);
        self.sticky_gutter.set_rich_text(
            gutter.iter().map(|(t, c)| (t.as_str(), base.clone().color(c.to_glyphon()))),
            &base,
            Shaping::Basic,
            None,
        );
        self.sticky_gutter.shape_until_scroll(fs, false);
        self.sticky_key = Some(key);
    }

    /// Recomputes the colored blocks of the minimap if text, scroll position or theme changed.
    fn prepare_minimap(&mut self, theme: &Theme) {
        let Some(g) = self.mini_geom() else {
            self.mini_cache = None;
            return;
        };
        if self.minimap != MinimapMode::Blocks {
            return;
        }
        let rope = self.doc.text();
        let rows = ((g.rect.h / g.line_h).ceil() as usize).max(1);
        let cols = ((g.rect.w - (MINI_STRIP_W + 2.0) * self.scale) / (MINI_CHAR_W * self.scale)).max(1.0) as usize;
        let key = MiniKey { version: self.doc.version(), first: g.first, rows, cols, theme: theme.name };
        if self.mini_cache.as_ref().is_some_and(|(k, _)| *k == key) {
            return;
        }
        let first = g.first.min(rope.len_lines().saturating_sub(1));
        let last = (first + rows).min(rope.len_lines());
        let start_byte = rope.line_to_byte(first);
        let end_byte = rope.line_to_byte(last);
        let spans = match &self.syntax {
            Some(syn) => flatten(&syn.highlights(rope, start_byte..end_byte), start_byte..end_byte),
            None => Vec::new(),
        };
        let tab = self.doc.tab_width;
        let mut runs: Vec<MiniRun> = Vec::new();
        let mut span_i = 0;
        for line in first..last {
            let row = line - first;
            let mut byte = rope.line_to_byte(line);
            let mut col = 0;
            // Current run: start column, length, color.
            let mut run: Option<(usize, usize, Rgba)> = None;
            for ch in rope.line(line).chars() {
                if ch == '\n' || ch == '\r' || col >= cols {
                    break;
                }
                while span_i < spans.len() && spans[span_i].end <= byte {
                    span_i += 1;
                }
                let width = if ch == '\t' { tab - col % tab } else { 1 };
                if ch.is_whitespace() {
                    if let Some(r) = run.take() {
                        runs.push((row, r.0, r.1, r.2));
                    }
                } else {
                    let color = spans
                        .get(span_i)
                        .filter(|sp| sp.start <= byte)
                        .map_or(theme.text, |sp| theme.highlight(sp.highlight));
                    match &mut run {
                        Some((_, len, c)) if *c == color => *len += width,
                        _ => {
                            if let Some(r) = run.take() {
                                runs.push((row, r.0, r.1, r.2));
                            }
                            run = Some((col, width, color));
                        }
                    }
                }
                col += width;
                byte += ch.len_utf8();
            }
            if let Some(r) = run {
                runs.push((row, r.0, r.1.min(cols.saturating_sub(r.0)), r.2));
            }
        }
        self.mini_cache = Some((key, runs));
    }

    fn draw_minimap(&self, theme: &Theme, under: &mut Vec<Quad>, over: &mut Vec<Quad>) {
        let Some(g) = self.mini_geom() else { return };
        let r = g.rect;
        let sc = self.scale;
        let rope = self.doc.text();
        let total = rope.len_lines().max(1) as f32;

        // Code shape.
        if self.minimap == MinimapMode::Blocks
            && let Some((key, runs)) = &self.mini_cache
            && key.first == g.first
        {
            let cw = MINI_CHAR_W * sc;
            let h = (g.line_h * 0.75).max(1.0);
            let x0 = r.x + 2.0 * sc;
            for &(row, col, len, color) in runs {
                let y = r.y + row as f32 * g.line_h;
                if y >= r.y + r.h {
                    break;
                }
                under.push(Quad { x: x0 + col as f32 * cw, y, w: len as f32 * cw, h, color: color.with_alpha(0xA0) });
            }
        }

        // Visible area.
        let alpha = if self.minimap_hover { 0x30 } else { 0x16 };
        let box_y = g.box_y.max(r.y);
        let box_h = (g.box_y + g.box_h).min(r.y + r.h) - box_y;
        if box_h > 0.0 {
            over.push(Quad { x: r.x, y: box_y, w: r.w, h: box_h, color: theme.text.with_alpha(alpha) });
        }

        // Marker strip over the whole file: git left, find matches middle, problems right.
        let (sx, sw) = match self.minimap {
            MinimapMode::Blocks => (r.x + r.w - MINI_STRIP_W * sc, MINI_STRIP_W * sc),
            _ => (r.x, r.w),
        };
        let third = (sw / 3.0).max(1.0);
        let mark_h = (2.0 * sc).round().max(1.0);
        let y_of = |line: usize| r.y + (line as f32 / total) * r.h;
        let mut last_y = f32::MIN;
        for &(line, mark) in &self.git_marks {
            let color = match mark {
                wyven_git::LineMark::Added => theme.git_added,
                wyven_git::LineMark::Modified => theme.git_modified,
                wyven_git::LineMark::DeletedAfter => theme.git_deleted,
            };
            let line = if line == usize::MAX { 0 } else { line };
            let y = y_of(line);
            if (y - last_y).abs() < 0.5 {
                continue;
            }
            last_y = y;
            over.push(Quad { x: sx, y, w: third, h: mark_h.max(r.h / total), color });
        }
        last_y = f32::MIN;
        for &(start, _) in self.find_matches.iter().take(20_000) {
            let y = y_of(rope.char_to_line(start.min(rope.len_chars())));
            if (y - last_y).abs() < 1.0 {
                continue;
            }
            last_y = y;
            over.push(Quad { x: sx + third, y, w: third, h: mark_h, color: theme.accent });
        }
        for d in &self.diagnostics {
            let color = match d.severity {
                1 => theme.error,
                2 => theme.warning,
                _ => continue,
            };
            let y = y_of(rope.char_to_line(d.start.min(rope.len_chars())));
            over.push(Quad { x: sx + 2.0 * third, y, w: sw - 2.0 * third, h: mark_h, color });
        }
        // Cursor line.
        let cur = rope.char_to_line(self.doc.selection().primary().head);
        over.push(Quad { x: sx, y: y_of(cur), w: sw, h: sc.round().max(1.0), color: theme.cursor.with_alpha(0xC0) });
    }
}

#[cfg(test)]
mod tests {
    use super::{sticky_for, Scope};

    fn sc(line: usize, end: usize) -> Scope {
        Scope { line, end }
    }

    fn from(top: usize) -> Vec<usize> {
        (top..top + 10).collect()
    }

    #[test]
    fn a_missing_semicolon_is_marked_at_the_end_of_its_line() {
        let rope = wyven_buffer::Rope::from_str("    string s = \"x\"\r\n    using (a)\n");
        let next = rope.line_to_char(1);
        // Reported at the start of the next line: moves behind "x".
        let end_of_code = "    string s = \"x\"".chars().count();
        assert_eq!(super::place_empty_diagnostic(&rope, next, next), (end_of_code, end_of_code));
        // In front of the next line's code too.
        assert_eq!(super::place_empty_diagnostic(&rope, next + 2, next + 2), (end_of_code, end_of_code));
        // A real range or one inside code stays.
        assert_eq!(super::place_empty_diagnostic(&rope, next + 4, next + 9), (next + 4, next + 9));
        assert_eq!(super::place_empty_diagnostic(&rope, next + 6, next + 6), (next + 6, next + 6));
    }

    #[test]
    fn squiggles_move_with_typing() {
        let typed = |at: usize, n: usize| wyven_buffer::TextEdit {
            start_byte: at,
            old_end_byte: at,
            new_end_byte: at + n,
            start_point: (0, at),
            old_end_point: (0, at),
            new_end_point: (0, at + n),
        };
        // An error at bytes 20..25; typing 3 bytes before it moves it.
        let e = typed(10, 3);
        assert_eq!((super::shift_byte(20, &e, false), super::shift_byte(25, &e, true)), (23, 28));
        // Typing after it leaves it; typing right at its end does not stretch it.
        let e = typed(25, 2);
        assert_eq!((super::shift_byte(20, &e, false), super::shift_byte(25, &e, true)), (20, 25));
        // Deleting bytes 5..8 before it pulls it back.
        let del = wyven_buffer::TextEdit { start_byte: 5, old_end_byte: 8, new_end_byte: 5, start_point: (0, 5), old_end_point: (0, 8), new_end_point: (0, 5) };
        assert_eq!(super::shift_byte(20, &del, false), 17);
    }

    #[test]
    fn bracket_depth_skips_strings_and_comments() {
        let rope = wyven_buffer::Rope::from_str("fn a() {\n  let s = \"(\"; // (\n  /* { */ if x { y(); }\n}\n");
        assert_eq!(super::bracket_depths(&rope, 4), vec![0, 1, 1, 1, 0]);
    }

    #[test]
    fn sticky_headers_skip_folded_lines() {
        // The method 5..20 is folded except its first line: below line 5 comes line 21.
        let scopes = [sc(0, 100), sc(2, 90), sc(5, 20)];
        let shown = [4, 5, 21, 22, 23, 24];
        assert_eq!(sticky_for(&scopes, &shown, 5), vec![sc(0, 100), sc(2, 90)]);
    }

    fn view(text: &str) -> super::EditorView {
        let mut fs = wyven_render::FontSystem::new();
        let font = super::FontMetrics { size: 14.0, line_h: 20.0, cell_w: 8.0 };
        let mut v = super::EditorView::virtual_doc("t", text, "a.cs", &mut fs, font, "monospace".into());
        v.set_area(super::Rect { x: 0.0, y: 0.0, w: 800.0, h: 400.0 });
        v
    }

    #[test]
    fn long_lines_wrap_at_words_and_positions_follow() {
        // 800 px wide, 8 px cells: about 90 cells of text per row.
        let long = format!("    {}", "word ".repeat(40));
        let mut v = view(&format!("{long}
short
"));
        v.wrap = true;
        let w = v.line_wrap(0);
        assert!(!w.breaks.is_empty(), "the long line wraps");
        assert_eq!(w.indent, 4, "continuation rows keep the indentation");
        let rope = v.doc.text();
        for &(k, _) in &w.breaks {
            assert_eq!(rope.char(k - 1), ' ', "breaks go before a word");
        }
        let map = v.row_map();
        assert_eq!(map.row(1), Some(1 + w.breaks.len()), "the next line moves down");
        // A position after the first break lands in the second row, indented.
        let (k, vk) = w.breaks[0];
        assert_eq!(map.place(0, vk + 2), Some((1, 4 + 2)));
        // Clicking there finds the same char again.
        let (x, y) = v.screen_pos(k + 2);
        assert_eq!(v.hit_test(x + 1.0, y + 1.0), k + 2);
        // A selection over the break is split into one piece per row.
        assert_eq!(map.segments(0, vk - 3, vk + 3).len(), 2);
        v.wrap = false;
        assert!(v.line_wrap(0).breaks.is_empty());
    }

    const CS: &str = "class A
{
    void M()
    {
        x();
        y();
    }
    void N()
    {
        z();
    }
}
";

    #[test]
    fn folding_hides_lines_and_the_cursor_jumps_over_them() {
        use super::FoldCmd;
        use wyven_buffer::Movement;
        let mut v = view(CS);
        assert!(v.fold(FoldCmd::Definitions));
        // M (lines 3..6) and N (8..10) are folded; their signature lines stay.
        assert_eq!(v.screen_lines(), vec![0, 1, 2, 7, 11, 12]);
        // Row 3 on screen is line 7.
        assert_eq!(v.doc.text().char_to_line(v.hit_test(100.0, 3.5 * 20.0)), 7);
        // Arrow down from line 2 lands on line 7, not inside M.
        v.doc.set_cursor(v.doc.text().line_to_char(2) + 4);
        let old = v.doc.selection().primary().head;
        v.doc.move_cursors(Movement::Down, false);
        v.skip_folds(old);
        assert_eq!(v.doc.text().char_to_line(v.doc.selection().primary().head), 7);
        // Typing above the folds moves them along.
        v.doc.set_cursor(0);
        v.doc.insert("// c
");
        v.after_edit();
        assert_eq!(v.screen_lines(), vec![0, 1, 2, 3, 8, 12, 13]);
        // Jumping into a folded body opens it.
        let inside = v.doc.text().line_to_char(5);
        v.jump_to(inside);
        assert!(v.screen_lines().contains(&5));
        assert!(!v.screen_lines().contains(&10), "N stays folded");
        // Toggle all: everything open again.
        assert!(v.fold(FoldCmd::ExpandAll));
        assert_eq!(v.screen_lines().len(), 14);
    }

    #[test]
    fn editing_inside_a_fold_opens_it() {
        use super::FoldCmd;
        let mut v = view(CS);
        v.doc.set_cursor(v.doc.text().line_to_char(2));
        assert!(v.fold(FoldCmd::Toggle));
        assert_eq!(v.screen_lines(), vec![0, 1, 2, 7, 8, 9, 10, 11, 12]);
        // Replace-all style edit in line 4 (hidden).
        let at = v.doc.text().line_to_char(4) + 8;
        v.doc.apply_changes(vec![wyven_buffer::Change { start: at, end: at + 1, text: "w".into() }]);
        v.after_edit();
        assert_eq!(v.screen_lines().len(), 13);
    }

    #[test]
    fn a_multi_line_suggestion_pushes_the_lines_below_down() {
        let mut v = view(CS);
        let at = v.doc.text().line_to_char(4) + 8; // line 5: "        x();" after the indent
        v.doc.set_cursor(at);
        v.set_ghost(Some((at, "a();\n        b();".into())));
        assert_eq!(v.ghost_rows(), Some((4, 1)));
        let lines = v.screen_lines();
        assert_eq!(v.row_of(&lines, 4), Some(4));
        assert_eq!(v.row_of(&lines, 5), Some(6), "line 6 moves one row down");
        assert_eq!(v.line_at_row(5), Some(4), "the suggestion row belongs to its line");
        assert_eq!(v.line_at_row(6), Some(5));
        // Moving the cursor hides it.
        v.doc.set_cursor(0);
        assert_eq!(v.ghost_rows(), None);
        assert_eq!(v.row_of(&v.screen_lines(), 5), Some(5));
    }

    /// The crash report: Tab on a long chat-style suggestion after `for(`.
    #[test]
    fn accepting_a_long_suggestion_and_redrawing_works() {
        let mut fs = wyven_render::FontSystem::new();
        let font = super::FontMetrics { size: 14.0, line_h: 20.0, cell_w: 8.0 };
        let theme = crate::theme::Theme::dark();
        let text = "int count = 0;
for()
";
        let mut v = super::EditorView::virtual_doc("t", text, "a.cs", &mut fs, font, "monospace".into());
        v.set_area(super::Rect { x: 0.0, y: 0.0, w: 800.0, h: 400.0 });
        // As in the app: minimap and sticky scroll on.
        v.minimap = super::MinimapMode::Blocks;
        v.sticky_scroll = true;
        v.scale = 1.5;
        let at = v.doc.text().line_to_char(1) + 4; // after "for("
        v.doc.set_cursor(at);
        let ghost = "It looks like you're trying to write a program that reads a list of numbers from the user, removes

```csharp
string? input;
List<int> numbers = new List<int>();
while (true)
{
    System.Console.Write(\"Gib eine Zahl ein: \");
    input = Console.ReadLine();";
        v.set_ghost(Some((at, ghost.into())));
        v.prepare(&mut fs, &theme);
        let (mut under, mut over, mut texts) = (Vec::new(), Vec::new(), Vec::new());
        v.draw(&theme, 1.0, &mut under, &mut over, &mut texts);
        drop(texts);
        // Tab: accept all of it.
        v.doc.insert(ghost);
        v.after_edit();
        v.ensure_cursor_visible();
        v.set_ghost(None);
        v.prepare(&mut fs, &theme);
        let (mut under, mut over, mut texts) = (Vec::new(), Vec::new(), Vec::new());
        v.draw(&theme, 1.0, &mut under, &mut over, &mut texts);
    }

    #[test]
    fn inlay_hints_shift_columns_and_lenses_take_a_row() {
        use super::{CodeLens, InlayHint, RowKind};
        let mut v = view(CS);
        v.show_inlay_hints = true;
        v.show_code_lens = true;
        // Line 5 "        x();": a hint "n: " before char 10 (inside the parentheses).
        v.set_inlay_hints(vec![InlayHint { line: 4, col: 10, label: "n:".into(), pad_left: false, pad_right: true }]);
        assert_eq!(v.visual_col(4, 10), 10, "the cursor stays before the hint");
        assert_eq!(v.visual_col(4, 11), 11 + 3, "text after the hint moves right");
        // A click right of the hint lands after it.
        assert_eq!(v.char_at_visual_col(4, 14), 11);
        // A lens above line 3 ("    void M()").
        v.set_lenses(vec![CodeLens { line: 2, col: 9, title: "2 references".into() }]);
        let map = v.row_map();
        assert_eq!(map.row(1), Some(1));
        assert_eq!(map.row(2), Some(3), "the lens row comes first");
        assert_eq!(map.at(2), Some((2, RowKind::Lens)));
        assert_eq!(map.row(3), Some(4));
        // Typing on line 5 drops its hint; a new line above moves the lens down.
        v.doc.set_cursor(v.doc.text().line_to_char(4) + 8);
        v.doc.insert("y");
        v.after_edit();
        assert!(v.hints_on(4).is_empty());
        v.doc.set_cursor(0);
        v.doc.insert("// top
");
        v.after_edit();
        assert_eq!(v.row_map().at(3), Some((3, RowKind::Lens)));
        assert_eq!(v.row_map().at(4), Some((3, RowKind::Text)));
    }

    #[test]
    fn breakpoints_follow_edits() {
        let mut v = view(CS);
        v.toggle_breakpoint(4);
        v.toggle_breakpoint(9);
        // A new line at the top moves both down.
        v.doc.set_cursor(0);
        v.doc.insert("// c
");
        v.after_edit();
        assert_eq!(v.breakpoints(), &[5, 10]);
        // Enter at the start of the breakpoint line pushes it down too.
        v.doc.set_cursor(v.doc.text().line_to_char(5));
        v.doc.insert("
");
        v.after_edit();
        assert_eq!(v.breakpoints(), &[6, 11]);
        // Deleting the line above it moves it up.
        let (s, e) = (v.doc.text().line_to_char(5), v.doc.text().line_to_char(6));
        v.doc.apply_changes(vec![wyven_buffer::Change { start: s, end: e, text: String::new() }]);
        v.after_edit();
        assert_eq!(v.breakpoints(), &[5, 10]);
        // Toggle removes.
        v.toggle_breakpoint(5);
        assert_eq!(v.breakpoints(), &[10]);
    }

    #[test]
    fn regions_and_import_runs_fold() {
        let rope = wyven_buffer::Rope::from_str(
            "using A;\nusing B;\nusing C;\n\n#region Helpers\nint x;\n#endregion\n// #region js\nfoo();\n// #endregion\n",
        );
        assert_eq!(super::text_folds(&rope), vec![(0, 2), (4, 6), (7, 9)]);
    }

    #[test]
    fn sticky_headers_follow_the_scroll_position() {
        // namespace 0..100, class 2..90, method 5..20, method 30..40
        let scopes = [sc(0, 100), sc(2, 90), sc(5, 20), sc(30, 40)];
        assert_eq!(sticky_for(&scopes, &from(0), 5), vec![]);
        assert_eq!(sticky_for(&scopes, &from(1), 5), vec![sc(0, 100)]);
        assert_eq!(sticky_for(&scopes, &from(3), 5), vec![sc(0, 100), sc(2, 90)]);
        assert_eq!(sticky_for(&scopes, &from(10), 5), vec![sc(0, 100), sc(2, 90), sc(5, 20)]);
        // Between the methods only namespace and class stick.
        assert_eq!(sticky_for(&scopes, &from(22), 5), vec![sc(0, 100), sc(2, 90)]);
        // Limit: the innermost headers win.
        assert_eq!(sticky_for(&scopes, &from(10), 2), vec![sc(2, 90), sc(5, 20)]);
        // The method's last line comes up under the headers: it is no longer sticky.
        assert_eq!(sticky_for(&scopes, &from(18), 5), vec![sc(0, 100), sc(2, 90)]);
    }
}
