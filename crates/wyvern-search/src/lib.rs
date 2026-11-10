//! Find and replace: in one text (the open file) and across all files of a project.
//!
//! Every search goes through a [`Query`]: plain text, whole word, case and regex are all turned
//! into one compiled regular expression, so both kinds of search behave the same.

use std::ops::Range;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use regex::{Regex, RegexBuilder};

/// Files larger than this are skipped in project search.
const MAX_FILE_SIZE: u64 = 4 * 1024 * 1024;
/// Stop collecting after this many matches (a typo like "e" would otherwise flood the list).
pub const MAX_MATCHES: usize = 20_000;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Query {
    pub text: String,
    pub case_sensitive: bool,
    pub whole_word: bool,
    pub regex: bool,
}

impl Query {
    /// The query as a regular expression; `Err` explains a broken regex in German.
    pub fn compile(&self) -> Result<Option<Regex>, String> {
        if self.text.is_empty() {
            return Ok(None);
        }
        let mut pattern = if self.regex { self.text.clone() } else { regex::escape(&self.text) };
        if self.whole_word {
            pattern = format!(r"\b(?:{pattern})\b");
        }
        RegexBuilder::new(&pattern)
            .case_insensitive(!self.case_sensitive)
            .multi_line(true)
            .size_limit(10 * (1 << 20))
            .build()
            .map(Some)
            .map_err(|e| match e {
                regex::Error::Syntax(s) => {
                    format!("Invalid regular expression: {}", s.lines().last().unwrap_or(&s))
                }
                other => format!("Invalid regular expression: {other}"),
            })
    }
}

/// All matches in `text` as byte ranges. Empty matches (e.g. `^`) are skipped.
pub fn find_all(re: &Regex, text: &str) -> Vec<Range<usize>> {
    re.find_iter(text)
        .filter(|m| !m.is_empty())
        .take(MAX_MATCHES)
        .map(|m| m.range())
        .collect()
}

/// The text that replaces `matched`. With regex on, `$1`, `${name}` and `$0` refer to groups.
pub fn replacement_for(re: &Regex, query: &Query, matched: &str, replacement: &str) -> String {
    if query.regex {
        re.replace(matched, replacement).into_owned()
    } else {
        replacement.to_string()
    }
}

/// One line with matches.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LineMatch {
    /// 0-based line number.
    pub line: usize,
    /// The line without its line break, shortened around the first match if very long.
    pub text: String,
    /// Match positions as byte ranges in `text`.
    pub ranges: Vec<Range<usize>>,
    /// Match positions as byte ranges in the whole file.
    pub file_ranges: Vec<Range<usize>>,
    /// Byte column of the first match in the full (not shortened) line, for jumping there.
    pub column: usize,
    /// Length in chars of the first match.
    pub match_chars: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileMatches {
    pub path: PathBuf,
    pub lines: Vec<LineMatch>,
}

impl FileMatches {
    pub fn count(&self) -> usize {
        self.lines.iter().map(|l| l.ranges.len()).sum()
    }
}

/// Groups byte ranges of matches in `text` into lines.
pub fn group_by_line(text: &str, ranges: &[Range<usize>]) -> Vec<LineMatch> {
    const MAX_LINE: usize = 300;
    let mut out: Vec<LineMatch> = Vec::new();
    let mut line_no = 0;
    let mut line_start = 0;
    let mut scanned = 0;
    for r in ranges {
        // Advance line counters up to the match start.
        for (i, b) in text.as_bytes()[scanned..r.start].iter().enumerate() {
            if *b == b'\n' {
                line_no += 1;
                line_start = scanned + i + 1;
            }
        }
        scanned = r.start;
        let line_end = text[line_start..].find('\n').map_or(text.len(), |e| line_start + e);
        let full = text[line_start..line_end].trim_end_matches('\r');
        let in_line = (r.start - line_start)..(r.end.min(line_start + full.len()) - line_start);
        match out.last_mut() {
            Some(m) if m.line == line_no => {
                m.ranges.push(in_line);
                m.file_ranges.push(r.clone());
            }
            _ => out.push(LineMatch {
                line: line_no,
                text: full.to_string(),
                column: in_line.start,
                match_chars: text[r.clone()].chars().count(),
                ranges: vec![in_line],
                file_ranges: vec![r.clone()],
            }),
        }
    }
    // Shorten very long lines (minified files) around the first match.
    for m in &mut out {
        if m.text.len() > MAX_LINE {
            let first = m.ranges[0].start;
            let mut start = first.saturating_sub(60);
            while !m.text.is_char_boundary(start) {
                start -= 1;
            }
            let mut end = (start + MAX_LINE).min(m.text.len());
            while !m.text.is_char_boundary(end) {
                end -= 1;
            }
            let prefix = if start > 0 { "…" } else { "" };
            let shift = prefix.len() as isize - start as isize;
            m.text = format!("{prefix}{}", &m.text[start..end]);
            let len = m.text.len();
            m.ranges = m
                .ranges
                .iter()
                .filter(|r| r.start >= start && r.start < end)
                .map(|r| {
                    let s = (r.start as isize + shift) as usize;
                    let e = ((r.end as isize + shift) as usize).min(len);
                    s..e
                })
                .collect();
        }
    }
    out
}

/// Folders never searched (generated files), in addition to `.gitignore`.
pub const IGNORED_DIRS: &[&str] = &[
    ".git", ".vs", ".vscode", ".idea", ".godot", ".import", "bin", "obj", "target",
    "node_modules", "__pycache__",
];

/// Searches all text files under `root`, calling `on_file` for each file with matches.
/// Stops early when `cancel` is set. Returns the number of files searched.
pub fn search_project(
    root: &Path,
    re: &Regex,
    cancel: &Arc<AtomicBool>,
    mut on_file: impl FnMut(FileMatches),
) -> usize {
    let mut searched = 0;
    let mut total = 0;
    let walker = ignore::WalkBuilder::new(root)
        .hidden(true)
        .git_ignore(true)
        .git_global(false)
        .require_git(false)
        .filter_entry(|e| {
            let name = e.file_name().to_string_lossy();
            !(e.file_type().is_some_and(|t| t.is_dir())
                && IGNORED_DIRS.iter().any(|i| i.eq_ignore_ascii_case(&name)))
        })
        .sort_by_file_path(|a, b| a.cmp(b))
        .build();
    for entry in walker.flatten() {
        if cancel.load(Ordering::Relaxed) || total >= MAX_MATCHES {
            break;
        }
        if !entry.file_type().is_some_and(|t| t.is_file()) {
            continue;
        }
        if entry.metadata().map_or(true, |m| m.len() > MAX_FILE_SIZE) {
            continue;
        }
        let Ok(bytes) = std::fs::read(entry.path()) else { continue };
        // Binary files contain NUL bytes near the start.
        if bytes.iter().take(8192).any(|&b| b == 0) {
            continue;
        }
        let Ok(text) = String::from_utf8(bytes) else { continue };
        searched += 1;
        let ranges = find_all(re, &text);
        if ranges.is_empty() {
            continue;
        }
        total += ranges.len();
        on_file(FileMatches { path: entry.path().to_path_buf(), lines: group_by_line(&text, &ranges) });
    }
    searched
}

#[cfg(test)]
mod tests {
    use super::*;

    fn q(text: &str) -> Query {
        Query { text: text.into(), ..Query::default() }
    }

    #[test]
    fn plain_text_is_escaped_and_case_insensitive_by_default() {
        let re = q("a.b").compile().unwrap().unwrap();
        assert_eq!(find_all(&re, "a.b axb A.B"), vec![0..3, 8..11]);
    }

    #[test]
    fn whole_word_and_case() {
        let mut query = q("Wert");
        query.whole_word = true;
        query.case_sensitive = true;
        let re = query.compile().unwrap().unwrap();
        assert_eq!(find_all(&re, "Wert Werte wert Wert."), vec![0..4, 16..20]);
    }

    #[test]
    fn regex_errors_are_reported() {
        let mut query = q("(abc");
        query.regex = true;
        assert!(query.compile().unwrap_err().starts_with("Invalid regular expression"));
        assert!(q("").compile().unwrap().is_none());
    }

    #[test]
    fn regex_replacement_uses_groups() {
        let mut query = q(r"(\w+)=(\d+)");
        query.regex = true;
        let re = query.compile().unwrap().unwrap();
        assert_eq!(replacement_for(&re, &query, "x=42", "$2=$1"), "42=x");
        let plain = q("$1");
        let re2 = plain.compile().unwrap().unwrap();
        assert_eq!(replacement_for(&re2, &plain, "$1", "$2"), "$2");
    }

    #[test]
    fn matches_are_grouped_by_line() {
        let text = "eins zwei\r\nzwei zwei\ndrei";
        let re = q("zwei").compile().unwrap().unwrap();
        let lines = group_by_line(text, &find_all(&re, text));
        assert_eq!(lines.len(), 2);
        assert_eq!((lines[0].line, lines[0].text.as_str()), (0, "eins zwei"));
        assert_eq!((lines[0].ranges.len(), lines[0].ranges[0].clone()), (1, 5..9));
        assert_eq!((lines[1].line, lines[1].ranges.clone()), (1, vec![0..4, 5..9]));
        assert_eq!(lines[1].file_ranges, vec![11..15, 16..20]);
        assert_eq!((lines[0].column, lines[0].match_chars), (5, 4));
    }

    #[test]
    fn project_search_skips_ignored_and_binary_files() {
        let root = std::env::temp_dir().join(format!("wyvern-search-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::create_dir_all(root.join("bin")).unwrap();
        std::fs::write(root.join("src/a.cs"), "var ziel = 1;\nziel++;\n").unwrap();
        std::fs::write(root.join("bin/b.cs"), "ziel").unwrap();
        std::fs::write(root.join("c.dat"), b"ziel\0\0binary").unwrap();
        let re = q("ziel").compile().unwrap().unwrap();
        let mut found = Vec::new();
        search_project(&root, &re, &Arc::new(AtomicBool::new(false)), |f| found.push(f));
        assert_eq!(found.len(), 1);
        assert!(found[0].path.ends_with("a.cs"));
        assert_eq!(found[0].count(), 2);
        let _ = std::fs::remove_dir_all(&root);
    }
}
