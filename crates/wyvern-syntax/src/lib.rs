//! Tree-sitter parsing and syntax highlighting.
//!
//! - The tree is updated **incrementally** from the edits the document reports, so a keystroke
//!   re-parses only the changed part.
//! - Highlighting runs the query **only on the visible byte range**, so cost does not grow
//!   with file size.

use std::path::Path;
use std::sync::OnceLock;

use streaming_iterator::StreamingIterator;
use tree_sitter::{InputEdit, Language, Node, Parser, Point, Query, QueryCursor, Tree};
use wyvern_buffer::{Rope, TextEdit};

/// Theme categories. The theme assigns a color to each.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Highlight {
    Keyword,
    Type,
    Function,
    String,
    Escape,
    Comment,
    Number,
    Constant,
    Property,
    Variable,
    Parameter,
    Operator,
    Punctuation,
    Attribute,
    Namespace,
    Tag,
    Label,
    Heading,
    Link,
}

impl Highlight {
    /// Maps a capture name like `keyword.control` or `string.escape` to a category.
    fn from_capture(name: &str) -> Option<Self> {
        use Highlight::*;
        let starts = |p: &str| name == p || name.starts_with(&format!("{p}."));
        Some(if starts("comment") {
            Comment
        } else if starts("string.escape") || starts("escape") {
            Escape
        } else if starts("string") || starts("character") || starts("markup.raw")
            || starts("text.literal")
        {
            String
        } else if starts("number") || starts("float") {
            Number
        } else if starts("boolean") || starts("constant") || starts("variable.builtin") {
            Constant
        } else if starts("keyword") || starts("conditional") || starts("repeat")
            || starts("include") || starts("exception") || starts("storageclass")
            || starts("type.qualifier")
        {
            Keyword
        } else if starts("type") || starts("constructor") {
            Type
        } else if starts("function") || starts("method") {
            Function
        } else if starts("property") || starts("field") {
            Property
        } else if starts("variable.parameter") || starts("parameter") {
            Parameter
        } else if starts("variable") {
            Variable
        } else if starts("operator") {
            Operator
        } else if starts("punctuation") {
            Punctuation
        } else if starts("attribute") {
            Attribute
        } else if starts("namespace") || starts("module") {
            Namespace
        } else if starts("tag") {
            Tag
        } else if starts("label") {
            Label
        } else if starts("markup.heading") || starts("text.title") {
            Heading
        } else if starts("markup.link") || starts("text.uri") || starts("text.reference") {
            Link
        } else {
            return None;
        })
    }
}

pub struct LanguageConfig {
    pub name: &'static str,
    extensions: &'static [&'static str],
    language: Language,
    query: Query,
    styles: Vec<Option<Highlight>>,
    /// Line comment token, e.g. `//`.
    pub line_comment: Option<&'static str>,
}

struct Def {
    name: &'static str,
    extensions: &'static [&'static str],
    language: fn() -> Language,
    query: fn() -> String,
    line_comment: Option<&'static str>,
}

fn definitions() -> Vec<Def> {
    vec![
        Def {
            name: "C#",
            extensions: &["cs", "csx"],
            language: || tree_sitter_c_sharp::LANGUAGE.into(),
            query: || tree_sitter_c_sharp::HIGHLIGHTS_QUERY.into(),
            line_comment: Some("//"),
        },
        Def {
            name: "JavaScript",
            extensions: &["js", "mjs", "cjs", "jsx"],
            language: || tree_sitter_javascript::LANGUAGE.into(),
            query: || {
                format!(
                    "{}\n{}",
                    tree_sitter_javascript::JSX_HIGHLIGHT_QUERY,
                    tree_sitter_javascript::HIGHLIGHT_QUERY
                )
            },
            line_comment: Some("//"),
        },
        Def {
            name: "TypeScript",
            extensions: &["ts", "mts", "cts"],
            language: || tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(),
            // The TypeScript query only adds to the JavaScript one; TS-specific rules first.
            query: || {
                format!(
                    "{}\n{}",
                    tree_sitter_typescript::HIGHLIGHTS_QUERY,
                    tree_sitter_javascript::HIGHLIGHT_QUERY
                )
            },
            line_comment: Some("//"),
        },
        Def {
            name: "TSX",
            extensions: &["tsx"],
            language: || tree_sitter_typescript::LANGUAGE_TSX.into(),
            query: || {
                format!(
                    "{}\n{}\n{}",
                    tree_sitter_typescript::HIGHLIGHTS_QUERY,
                    tree_sitter_javascript::JSX_HIGHLIGHT_QUERY,
                    tree_sitter_javascript::HIGHLIGHT_QUERY
                )
            },
            line_comment: Some("//"),
        },
        Def {
            name: "HTML",
            extensions: &["html", "htm", "xhtml"],
            language: || tree_sitter_html::LANGUAGE.into(),
            query: || tree_sitter_html::HIGHLIGHTS_QUERY.into(),
            line_comment: None,
        },
        Def {
            name: "CSS",
            extensions: &["css"],
            language: || tree_sitter_css::LANGUAGE.into(),
            query: || tree_sitter_css::HIGHLIGHTS_QUERY.into(),
            line_comment: None,
        },
        Def {
            name: "JSON",
            extensions: &["json", "jsonc", "csproj.user"],
            language: || tree_sitter_json::LANGUAGE.into(),
            query: || tree_sitter_json::HIGHLIGHTS_QUERY.into(),
            line_comment: Some("//"),
        },
        Def {
            name: "Markdown",
            extensions: &["md", "markdown"],
            language: || tree_sitter_md::LANGUAGE.into(),
            query: || tree_sitter_md::HIGHLIGHT_QUERY_BLOCK.into(),
            line_comment: None,
        },
        Def {
            name: "TOML",
            extensions: &["toml"],
            language: || tree_sitter_toml_ng::LANGUAGE.into(),
            query: || tree_sitter_toml_ng::HIGHLIGHTS_QUERY.into(),
            line_comment: Some("#"),
        },
        Def {
            name: "Rust",
            extensions: &["rs"],
            language: || tree_sitter_rust::LANGUAGE.into(),
            query: || tree_sitter_rust::HIGHLIGHTS_QUERY.into(),
            line_comment: Some("//"),
        },
    ]
}

fn registry() -> &'static [LanguageConfig] {
    static REGISTRY: OnceLock<Vec<LanguageConfig>> = OnceLock::new();
    REGISTRY.get_or_init(|| {
        definitions()
            .into_iter()
            .filter_map(|d| {
                let language = (d.language)();
                match Query::new(&language, &(d.query)()) {
                    Ok(query) => {
                        let styles = query
                            .capture_names()
                            .iter()
                            .map(|n| Highlight::from_capture(n))
                            .collect();
                        Some(LanguageConfig {
                            name: d.name,
                            extensions: d.extensions,
                            language,
                            query,
                            styles,
                            line_comment: d.line_comment,
                        })
                    }
                    Err(e) => {
                        log::error!("highlight query for {} failed: {e}", d.name);
                        None
                    }
                }
            })
            .collect()
    })
}

/// Finds the language for a file by its extension.
pub fn language_for_path(path: &Path) -> Option<&'static LanguageConfig> {
    let name = path.file_name()?.to_str()?.to_ascii_lowercase();
    registry().iter().find(|c| {
        c.extensions
            .iter()
            .any(|ext| name.ends_with(&format!(".{ext}")))
    })
}

/// A highlighted byte range.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Span {
    pub start: usize,
    pub end: usize,
    pub highlight: Highlight,
}

pub struct Syntax {
    config: &'static LanguageConfig,
    parser: Parser,
    tree: Option<Tree>,
}

impl Syntax {
    pub fn new(config: &'static LanguageConfig, rope: &Rope) -> Self {
        let mut parser = Parser::new();
        if let Err(e) = parser.set_language(&config.language) {
            log::error!("tree-sitter language {} incompatible: {e}", config.name);
        }
        let mut syntax = Self { config, parser, tree: None };
        syntax.reparse(rope);
        syntax
    }

    pub fn config(&self) -> &'static LanguageConfig {
        self.config
    }

    fn reparse(&mut self, rope: &Rope) {
        let mut read = |byte: usize, _: Point| -> &[u8] {
            if byte >= rope.len_bytes() {
                return &[];
            }
            let (chunk, chunk_start, _, _) = rope.chunk_at_byte(byte);
            &chunk.as_bytes()[byte - chunk_start..]
        };
        self.tree = self
            .parser
            .parse_with_options(&mut read, self.tree.as_ref(), None);
    }

    /// Applies the document's edits to the old tree and re-parses incrementally.
    pub fn update(&mut self, rope: &Rope, edits: &[TextEdit]) {
        if edits.is_empty() {
            return;
        }
        if let Some(tree) = &mut self.tree {
            let p = |(row, column): (usize, usize)| Point { row, column };
            for e in edits {
                tree.edit(&InputEdit {
                    start_byte: e.start_byte,
                    old_end_byte: e.old_end_byte,
                    new_end_byte: e.new_end_byte,
                    start_position: p(e.start_point),
                    old_end_position: p(e.old_end_point),
                    new_end_position: p(e.new_end_point),
                });
            }
        }
        self.reparse(rope);
    }

    /// Highlight spans in `bytes` (usually the visible lines). Inner (more specific) nodes win
    /// over outer ones; for the same node the first matching pattern wins.
    pub fn highlights(&self, rope: &Rope, bytes: std::ops::Range<usize>) -> Vec<Span> {
        let Some(tree) = &self.tree else { return Vec::new() };
        let mut cursor = QueryCursor::new();
        cursor.set_byte_range(bytes.clone());
        let text = |node: Node| {
            rope.byte_slice(node.start_byte()..node.end_byte())
                .chunks()
                .map(str::as_bytes)
        };
        let mut captures = cursor.captures(&self.config.query, tree.root_node(), text);
        let mut spans: Vec<Span> = Vec::new();
        let mut last: Option<(usize, usize)> = None;
        while let Some((m, idx)) = captures.next() {
            let cap = m.captures()[*idx];
            let Some(highlight) = self.config.styles[cap.index as usize] else { continue };
            let (s, e) = (cap.node.start_byte(), cap.node.end_byte());
            if last == Some((s, e)) {
                continue;
            }
            last = Some((s, e));
            spans.push(Span { start: s.max(bytes.start), end: e.min(bytes.end), highlight });
        }
        spans
    }
}

/// Flattens nested spans into non-overlapping runs over `bytes`, later spans painting over
/// earlier ones. Returns `(start, end, highlight)` covering only highlighted parts.
pub fn flatten(spans: &[Span], bytes: std::ops::Range<usize>) -> Vec<Span> {
    let len = bytes.end.saturating_sub(bytes.start);
    let mut paint: Vec<Option<Highlight>> = vec![None; len];
    for s in spans {
        let a = s.start.saturating_sub(bytes.start).min(len);
        let b = s.end.saturating_sub(bytes.start).min(len);
        paint[a..b].fill(Some(s.highlight));
    }
    let mut out = Vec::new();
    let mut i = 0;
    while i < len {
        let h = paint[i];
        let start = i;
        while i < len && paint[i] == h {
            i += 1;
        }
        if let Some(highlight) = h {
            out.push(Span { start: bytes.start + start, end: bytes.start + i, highlight });
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use wyvern_buffer::{Document, Movement};

    #[test]
    fn all_queries_compile() {
        let names: Vec<_> = registry().iter().map(|c| c.name).collect();
        assert_eq!(names.len(), definitions().len(), "loaded: {names:?}");
    }

    #[test]
    fn csharp_keywords_and_strings_are_highlighted() {
        let config = language_for_path(Path::new("Program.cs")).unwrap();
        let rope = Rope::from_str("class A { string s = \"hi\"; }");
        let syntax = Syntax::new(config, &rope);
        let spans = flatten(&syntax.highlights(&rope, 0..rope.len_bytes()), 0..rope.len_bytes());
        let at = |b: usize| spans.iter().find(|s| s.start <= b && b < s.end).map(|s| s.highlight);
        assert_eq!(at(0), Some(Highlight::Keyword)); // class
        assert_eq!(at(22), Some(Highlight::String)); // "hi"
    }

    #[test]
    fn incremental_update_matches_full_parse() {
        let config = language_for_path(Path::new("a.rs")).unwrap();
        let mut doc = Document::from_str("fn main() {}\n");
        let mut syntax = Syntax::new(config, doc.text());
        doc.move_cursors(Movement::DocEnd, false);
        doc.insert("// note");
        let edits = doc.take_edits();
        syntax.update(doc.text(), &edits);
        let len = doc.text().len_bytes();
        let incremental = flatten(&syntax.highlights(doc.text(), 0..len), 0..len);
        let fresh = Syntax::new(config, doc.text());
        assert_eq!(incremental, flatten(&fresh.highlights(doc.text(), 0..len), 0..len));
        assert!(incremental.iter().any(|s| s.highlight == Highlight::Comment));
    }
}
