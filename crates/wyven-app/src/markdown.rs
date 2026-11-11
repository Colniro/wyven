//! Live Markdown preview next to the editor (Alt → m).
//!
//! The Markdown is parsed with pulldown-cmark into styled text for one wrapping text buffer:
//! headings larger and bold, lists with bullets or numbers, code in the monospace font on a
//! background, quotes with a bar, tables aligned in the monospace font. Every preview line
//! remembers the source line it came from, so the preview can scroll along with the editor.

use pulldown_cmark::{Event, HeadingLevel, Options, Parser, Tag, TagEnd};
use wyven_render::{Attrs, Buffer, Family, FontSystem, Metrics, Quad, Shaping, TextBlock, Weight, Wrap};

use crate::editor::Rect;
use crate::theme::Theme;

#[derive(Debug, Clone, Copy, PartialEq)]
enum Color {
    Text,
    Dim,
    Accent,
    Code,
}

#[derive(Debug, Clone, PartialEq)]
struct Span {
    text: String,
    /// Font size relative to the base size.
    scale: f32,
    bold: bool,
    italic: bool,
    mono: bool,
    color: Color,
}

/// Kinds of preview lines that get a background or a mark.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LineKind {
    Normal,
    Code,
    Quote,
    Rule,
}

/// The parsed document: spans plus, for every line, its source line and kind.
#[derive(Debug, Default)]
struct Rendered {
    spans: Vec<Span>,
    src_lines: Vec<usize>,
    kinds: Vec<LineKind>,
}

/// Turns Markdown into styled spans. Separate from the drawing so it can be tested.
fn render(text: &str) -> Rendered {
    let line_starts: Vec<usize> = std::iter::once(0).chain(text.match_indices('\n').map(|(i, _)| i + 1)).collect();
    let src_line = |byte: usize| line_starts.partition_point(|&s| s <= byte).saturating_sub(1);

    let mut out = Rendered::default();
    let mut cur_src = 0;
    let mut line_kind = LineKind::Normal;
    // Style state.
    let mut scale = 1.0f32;
    let mut bold = 0;
    let mut italic = 0;
    let mut link = 0;
    let mut quote = 0;
    let mut in_code_block = false;
    let mut lists: Vec<Option<u64>> = Vec::new();
    // Tables are collected first and written aligned at the end.
    let mut table: Option<Vec<Vec<String>>> = None;
    let mut cell = String::new();

    // Starts the first line.
    out.src_lines.push(0);
    out.kinds.push(LineKind::Normal);

    let push = |out: &mut Rendered, s: &str, span: Span, cur_src: usize, kind: LineKind| {
        for (i, part) in s.split('\n').enumerate() {
            if i > 0 {
                out.spans.push(Span { text: "\n".into(), ..span.clone() });
                out.src_lines.push(cur_src);
                out.kinds.push(kind);
            }
            if !part.is_empty() {
                out.spans.push(Span { text: part.into(), ..span.clone() });
            }
        }
    };
    let ends_with_newline = |out: &Rendered| out.spans.last().is_none_or(|s| s.text.ends_with('\n'));
    let blank_line = |out: &mut Rendered, cur_src: usize| {
        // Ends the current line and leaves one empty line (no more than one in a row).
        let n = out.spans.len();
        let trailing = out.spans.iter().rev().take_while(|s| s.text == "\n").count();
        if n == 0 {
            return;
        }
        for _ in trailing..2 {
            out.spans.push(Span { text: "\n".into(), scale: 1.0, bold: false, italic: false, mono: false, color: Color::Text });
            out.src_lines.push(cur_src);
            out.kinds.push(LineKind::Normal);
        }
    };

    let parser = Parser::new_ext(text, Options::ENABLE_TABLES | Options::ENABLE_STRIKETHROUGH | Options::ENABLE_TASKLISTS);
    for (event, range) in parser.into_offset_iter() {
        // The style at the start of this event (a copy, so the state can change below).
        let (s_scale, s_bold, s_italic, s_link, s_quote) = (scale, bold > 0, italic > 0, link > 0, quote > 0);
        let style = move |color: Color| Span {
            text: String::new(),
            scale: s_scale,
            bold: s_bold,
            italic: s_italic,
            mono: false,
            color: if s_link { Color::Accent } else if s_quote && color == Color::Text { Color::Dim } else { color },
        };
        match event {
            Event::Start(tag) => {
                cur_src = src_line(range.start);
                if ends_with_newline(&out)
                    && let Some(k) = out.kinds.last_mut()
                {
                    *k = line_kind;
                    if let Some(s) = out.src_lines.last_mut() {
                        *s = cur_src;
                    }
                }
                match tag {
                    Tag::Heading { level, .. } => {
                        blank_line(&mut out, cur_src);
                        if let Some(s) = out.src_lines.last_mut() {
                            *s = cur_src;
                        }
                        scale = match level {
                            HeadingLevel::H1 => 1.8,
                            HeadingLevel::H2 => 1.45,
                            HeadingLevel::H3 => 1.25,
                            _ => 1.1,
                        };
                        bold += 1;
                    }
                    Tag::Paragraph => {
                        if lists.is_empty() {
                            blank_line(&mut out, cur_src);
                            if let (Some(s), Some(k)) = (out.src_lines.last_mut(), out.kinds.last_mut()) {
                                *s = cur_src;
                                *k = line_kind;
                            }
                        }
                    }
                    Tag::BlockQuote(_) => {
                        quote += 1;
                        line_kind = LineKind::Quote;
                        if let Some(k) = out.kinds.last_mut() {
                            *k = LineKind::Quote;
                        }
                    }
                    Tag::CodeBlock(_) => {
                        blank_line(&mut out, cur_src);
                        in_code_block = true;
                        line_kind = LineKind::Code;
                        cur_src += 1; // the fence line itself is not shown
                        if let (Some(k), Some(s)) = (out.kinds.last_mut(), out.src_lines.last_mut()) {
                            *k = LineKind::Code;
                            *s = cur_src;
                        }
                    }
                    Tag::List(start) => {
                        if lists.is_empty() {
                            blank_line(&mut out, cur_src);
                        } else if !ends_with_newline(&out) {
                            push(&mut out, "\n", style(Color::Text), cur_src, line_kind);
                        }
                        lists.push(start);
                    }
                    Tag::Item => {
                        if !ends_with_newline(&out) {
                            push(&mut out, "\n", style(Color::Text), cur_src, line_kind);
                        }
                        if let Some(s) = out.src_lines.last_mut() {
                            *s = cur_src;
                        }
                        let depth = lists.len().saturating_sub(1);
                        let marker = match lists.last_mut() {
                            Some(Some(n)) => {
                                let m = format!("{n}. ");
                                *n += 1;
                                m
                            }
                            _ => "• ".to_string(),
                        };
                        push(&mut out, &format!("{}{marker}", "    ".repeat(depth)), style(Color::Dim), cur_src, line_kind);
                    }
                    Tag::Emphasis => italic += 1,
                    Tag::Strong => bold += 1,
                    Tag::Link { .. } => link += 1,
                    Tag::Image { .. } => push(&mut out, "[image: ", style(Color::Dim), cur_src, line_kind),
                    Tag::Table(_) => {
                        blank_line(&mut out, cur_src);
                        table = Some(Vec::new());
                    }
                    Tag::TableHead | Tag::TableRow => {
                        if let Some(t) = &mut table {
                            t.push(Vec::new());
                        }
                    }
                    Tag::TableCell => cell.clear(),
                    _ => {}
                }
            }
            Event::End(tag) => match tag {
                TagEnd::Heading(_) => {
                    scale = 1.0;
                    bold -= 1;
                    push(&mut out, "\n", style(Color::Text), cur_src, line_kind);
                }
                TagEnd::Paragraph => {
                    if !ends_with_newline(&out) {
                        push(&mut out, "\n", style(Color::Text), cur_src, line_kind);
                    }
                }
                TagEnd::BlockQuote(_) => {
                    quote -= 1;
                    if quote == 0 {
                        line_kind = LineKind::Normal;
                    }
                }
                TagEnd::CodeBlock => {
                    in_code_block = false;
                    line_kind = if quote > 0 { LineKind::Quote } else { LineKind::Normal };
                    if let Some(k) = out.kinds.last_mut() {
                        *k = line_kind;
                    }
                }
                TagEnd::List(_) => {
                    lists.pop();
                    if lists.is_empty() && !ends_with_newline(&out) {
                        push(&mut out, "\n", style(Color::Text), cur_src, line_kind);
                    }
                }
                TagEnd::Emphasis => italic -= 1,
                TagEnd::Strong => bold -= 1,
                TagEnd::Link => link -= 1,
                TagEnd::Image => push(&mut out, "]", style(Color::Dim), cur_src, line_kind),
                TagEnd::TableCell => {
                    if let Some(row) = table.as_mut().and_then(|t| t.last_mut()) {
                        row.push(cell.trim().to_string());
                    }
                }
                TagEnd::Table => {
                    let rows = table.take().unwrap_or_default();
                    let cols = rows.iter().map(Vec::len).max().unwrap_or(0);
                    let widths: Vec<usize> = (0..cols)
                        .map(|c| rows.iter().filter_map(|r| r.get(c)).map(|s| s.chars().count()).max().unwrap_or(0))
                        .collect();
                    for (i, row) in rows.iter().enumerate() {
                        let mut line = String::new();
                        for (c, w) in widths.iter().enumerate() {
                            let v = row.get(c).map_or("", String::as_str);
                            // The last column is not padded: trailing spaces would wrap.
                            if c + 1 < cols {
                                line.push_str(&format!("{v:<w$} │ "));
                            } else {
                                line.push_str(v);
                            }
                        }
                        let mut span = style(Color::Text);
                        span.mono = true;
                        span.bold = i == 0;
                        push(&mut out, &format!("{line}\n"), span.clone(), cur_src + i, line_kind);
                        if i == 0 {
                            // Header separator: drawn as a line, so it never wraps.
                            if let Some(k) = out.kinds.last_mut() {
                                *k = LineKind::Rule;
                            }
                            let mut dim = span;
                            dim.bold = false;
                            push(&mut out, " \n", dim, cur_src + 1, line_kind);
                        }
                    }
                }
                _ => {}
            },
            Event::Text(t) => {
                if let Some(_t) = &table {
                    cell.push_str(&t);
                } else if in_code_block {
                    let mut span = style(Color::Code);
                    span.mono = true;
                    // Each source line of the block is its own preview line.
                    let body = t.strip_suffix('\n').unwrap_or(&t);
                    for (i, l) in body.split('\n').enumerate() {
                        if i > 0 {
                            cur_src += 1;
                        }
                        push(&mut out, &format!("  {l}\n"), span.clone(), cur_src + 1, LineKind::Code);
                    }
                    cur_src += 1;
                } else {
                    push(&mut out, &t, style(Color::Text), cur_src, line_kind);
                }
            }
            Event::Code(t) => {
                if let Some(_t) = &table {
                    cell.push_str(&t);
                } else {
                    let mut span = style(Color::Code);
                    span.mono = true;
                    push(&mut out, &t, span, cur_src, line_kind);
                }
            }
            Event::SoftBreak => {
                if table.is_some() {
                    cell.push(' ');
                } else {
                    push(&mut out, " ", style(Color::Text), cur_src, line_kind);
                }
            }
            Event::HardBreak => push(&mut out, "\n", style(Color::Text), src_line(range.end), line_kind),
            Event::Rule => {
                blank_line(&mut out, cur_src);
                if let (Some(k), Some(s)) = (out.kinds.last_mut(), out.src_lines.last_mut()) {
                    *k = LineKind::Rule;
                    *s = src_line(range.start);
                }
                push(&mut out, " \n", style(Color::Dim), cur_src, LineKind::Normal);
            }
            Event::TaskListMarker(done) => {
                // Replaces the bullet that the item already got.
                if let Some(last) = out.spans.last_mut()
                    && last.text.ends_with("• ")
                {
                    last.text.truncate(last.text.len() - "• ".len());
                }
                push(&mut out, if done { "☑ " } else { "☐ " }, style(Color::Dim), cur_src, line_kind);
            }
            Event::Html(h) | Event::InlineHtml(h) => push(&mut out, &h, style(Color::Dim), cur_src, line_kind),
            _ => {}
        }
    }
    // The kinds and source lines must cover every line of the text.
    let lines = out.spans.iter().map(|s| s.text.matches('\n').count()).sum::<usize>() + 1;
    out.src_lines.resize(lines, cur_src);
    out.kinds.resize(lines, LineKind::Normal);
    out
}

pub struct Preview {
    path: std::path::PathBuf,
    version: Option<u64>,
    rendered: Rendered,
    buffer: Buffer,
    area: Rect,
    size: f32,
    line_h: f32,
    prose: String,
    mono: String,
    width: f32,
    scroll_y: f32,
    shaped_for: Option<(u64, u32, &'static str)>,
}

impl Preview {
    /// `prose`: font for normal text (Segoe UI if available), `mono`: the code font.
    pub fn new(fs: &mut FontSystem, path: std::path::PathBuf, size: f32, line_h: f32, prose: String, mono: String) -> Self {
        let mut buffer = Buffer::new(fs, Metrics::new(size, line_h));
        buffer.set_wrap(Wrap::WordOrGlyph);
        Self {
            path,
            version: None,
            rendered: Rendered::default(),
            buffer,
            area: Rect::default(),
            size,
            line_h,
            prose,
            mono,
            width: 0.0,
            scroll_y: 0.0,
            shaped_for: None,
        }
    }

    pub fn path(&self) -> &std::path::Path {
        &self.path
    }

    pub fn set_area(&mut self, area: Rect) {
        self.area = area;
    }

    pub fn set_font(&mut self, size: f32, line_h: f32) {
        self.size = size;
        self.line_h = line_h;
        self.buffer.set_metrics(Metrics::new(size, line_h));
        self.shaped_for = None;
    }

    pub fn contains(&self, x: f32, y: f32) -> bool {
        let a = self.area;
        x >= a.x && x < a.x + a.w && y >= a.y && y < a.y + a.h
    }

    fn pad(&self) -> f32 {
        (self.size * 1.4).round()
    }

    /// Re-renders when the text changed and re-shapes when text, width or theme changed.
    pub fn prepare(&mut self, fs: &mut FontSystem, theme: &Theme, text: &str, version: u64) {
        if self.version != Some(version) {
            self.rendered = render(text);
            self.version = Some(version);
            self.shaped_for = None;
        }
        let width = (self.area.w - 2.0 * self.pad()).max(50.0);
        let key = (version, width.to_bits(), theme.name);
        if self.shaped_for == Some(key) {
            return;
        }
        self.width = width;
        let color = |c: Color| match c {
            Color::Text => theme.text,
            Color::Dim => theme.gutter_text,
            Color::Accent => theme.accent,
            Color::Code => theme.syntax.string,
        };
        let spans: Vec<(&str, Attrs)> = self
            .rendered
            .spans
            .iter()
            .map(|s| {
                let family = if s.mono { &self.mono } else { &self.prose };
                let mut a = Attrs::new()
                    .family(Family::Name(family))
                    .color(color(s.color).to_glyphon())
                    .metrics(Metrics::new(self.size * s.scale, self.line_h * s.scale));
                if s.bold {
                    a = a.weight(Weight::BOLD);
                }
                if s.italic {
                    a = a.style(wyven_render::Style::Italic);
                }
                (s.text.as_str(), a)
            })
            .collect();
        let base = Attrs::new().family(Family::Name(&self.prose));
        self.buffer.set_size(Some(width), None);
        self.buffer.set_rich_text(spans, &base, Shaping::Advanced, None);
        self.buffer.shape_until_scroll(fs, false);
        self.shaped_for = Some(key);
    }

    fn content_h(&self) -> f32 {
        self.buffer.layout_runs().last().map_or(0.0, |r| r.line_top + r.line_height)
    }

    /// Scrolls so the part made from source line `src` is at the top (follows the editor).
    pub fn follow(&mut self, src: usize) {
        let target = self.rendered.src_lines.iter().position(|&s| s >= src);
        let y = match target {
            Some(i) => self.buffer.layout_runs().find(|r| r.line_i >= i).map_or(0.0, |r| r.line_top),
            None => self.content_h(),
        };
        let max = (self.content_h() + 2.0 * self.pad() - self.area.h).max(0.0);
        self.scroll_y = (y - self.pad()).clamp(0.0, max);
    }

    pub fn draw<'a>(&'a self, theme: &Theme, scale: f32, under: &mut Vec<Quad>, text: &mut Vec<TextBlock<'a>>) {
        let a = self.area;
        let pad = self.pad();
        under.push(Quad { x: a.x, y: a.y, w: a.w, h: a.h, color: theme.panel_bg });
        under.push(Quad { x: a.x, y: a.y, w: scale.round().max(1.0), h: a.h, color: theme.divider });
        let top = a.y + pad - self.scroll_y;
        let left = a.x + pad;
        for run in self.buffer.layout_runs() {
            let y = top + run.line_top;
            if y + run.line_height < a.y || y > a.y + a.h {
                continue;
            }
            match self.rendered.kinds.get(run.line_i).copied().unwrap_or(LineKind::Normal) {
                LineKind::Code => under.push(Quad {
                    x: left - pad * 0.4,
                    y,
                    w: self.width + pad * 0.8,
                    h: run.line_height,
                    color: theme.current_line,
                }),
                LineKind::Quote => under.push(Quad {
                    x: left - pad * 0.5,
                    y,
                    w: (3.0 * scale).round(),
                    h: run.line_height,
                    color: theme.gutter_text,
                }),
                LineKind::Rule => under.push(Quad {
                    x: left,
                    y: y + run.line_height / 2.0,
                    w: self.width,
                    h: scale.round().max(1.0),
                    color: theme.divider,
                }),
                LineKind::Normal => {}
            }
        }
        text.push(TextBlock {
            buffer: &self.buffer,
            x: left,
            y: top,
            color: theme.text,
            clip: Some([a.x as i32, a.y as i32, (a.x + a.w) as i32, (a.y + a.h) as i32]),
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plain(r: &Rendered) -> String {
        r.spans.iter().map(|s| s.text.as_str()).collect()
    }

    #[test]
    fn headings_lists_and_code_are_rendered() {
        let md = "# Title\n\nSome **bold** text.\n\n- one\n- two\n\n1. first\n2. second\n\n```rust\nfn main() {}\nlet x = 1;\n```\n\n> quoted\n";
        let r = render(md);
        let text = plain(&r);
        assert!(text.starts_with("Title\n"), "{text:?}");
        assert!(text.contains("Some bold text."));
        assert!(text.contains("• one\n• two"), "{text:?}");
        assert!(text.contains("1. first\n2. second"), "{text:?}");
        assert!(text.contains("  fn main() {}\n  let x = 1;"), "{text:?}");
        let title = r.spans.iter().find(|s| s.text == "Title").unwrap();
        assert!(title.bold && title.scale > 1.5);
        assert!(r.spans.iter().any(|s| s.text == "bold" && s.bold));
        // Code lines are marked and come from source lines 11 and 12 (0-based).
        let lines: Vec<&str> = text.split('\n').collect();
        let code = lines.iter().position(|l| l.contains("fn main")).unwrap();
        assert_eq!(r.kinds[code], LineKind::Code);
        assert_eq!(r.src_lines[code], 11);
        assert_eq!(r.src_lines[code + 1], 12);
        let quote = lines.iter().position(|l| l.contains("quoted")).unwrap();
        assert_eq!(r.kinds[quote], LineKind::Quote);
        assert_eq!(r.src_lines.len(), lines.len());
    }

    #[test]
    fn tables_are_aligned() {
        let md = "| Key | Action |\n|---|---|\n| F5 | run |\n| Shift+F5 | stop |\n";
        let text = plain(&render(md));
        assert!(text.contains("Key      │ Action
"), "{text:?}");
        assert!(text.contains("Shift+F5 │ stop"), "{text:?}");
    }

    #[test]
    fn task_lists_get_boxes() {
        let text = plain(&render("- [x] done\n- [ ] open\n"));
        assert!(text.contains("☑ done") && text.contains("☐ open"), "{text:?}");
    }
}
