//! Git for Wyvern, through the installed `git` command line (like VS Code does).
//!
//! All functions block; the app calls them on a background thread. Errors come back as German
//! messages that can be shown in the status bar.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use similar::{capture_diff_slices, Algorithm, DiffOp};

/// One changed file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Change {
    /// Path relative to the repository root, with `/`.
    pub path: String,
    pub kind: ChangeKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChangeKind {
    Modified,
    Added,
    Deleted,
    Renamed,
    Untracked,
    Conflict,
}

impl ChangeKind {
    /// Letter shown in the git panel (like VS Code).
    pub fn letter(self) -> char {
        match self {
            ChangeKind::Modified => 'M',
            ChangeKind::Added => 'A',
            ChangeKind::Deleted => 'D',
            ChangeKind::Renamed => 'R',
            ChangeKind::Untracked => 'U',
            ChangeKind::Conflict => '!',
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Status {
    pub branch: String,
    /// Commits not yet pushed / not yet pulled (only with an upstream branch).
    pub ahead: usize,
    pub behind: usize,
    pub has_upstream: bool,
    pub changes: Vec<Change>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlameLine {
    pub hash: String,
    pub author: String,
    /// Unix time of the change.
    pub time: i64,
    pub summary: String,
    /// The line is changed locally and not committed yet.
    pub uncommitted: bool,
}

pub struct Repo {
    root: PathBuf,
}

fn git(dir: &Path, args: &[&str]) -> std::io::Result<Output> {
    let mut cmd = Command::new("git");
    cmd.args(args)
        .current_dir(dir)
        // Never wait for a password on a terminal we do not have; the credential manager
        // (a normal Windows dialog) still works.
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("LC_ALL", "C");
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    cmd.output()
}

fn run(dir: &Path, args: &[&str]) -> Result<String, String> {
    let out = git(dir, args).map_err(|e| format!("git kann nicht gestartet werden: {e}"))?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    } else {
        let err = String::from_utf8_lossy(&out.stderr);
        let msg = err.lines().rfind(|l| !l.trim().is_empty()).unwrap_or("unbekannter Fehler");
        Err(msg.trim().to_string())
    }
}

impl Repo {
    /// The repository containing `dir`, if any.
    pub fn open(dir: &Path) -> Option<Self> {
        let out = run(dir, &["rev-parse", "--show-toplevel"]).ok()?;
        let root = PathBuf::from(out.trim());
        // git prints forward slashes; normalize for comparisons on Windows.
        let root = std::fs::canonicalize(&root).map(|p| strip_unc(&p)).unwrap_or(root);
        Some(Self { root })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Path of `file` relative to the repository root, with `/`.
    pub fn relative(&self, file: &Path) -> Option<String> {
        let file = std::fs::canonicalize(file).map(|p| strip_unc(&p)).unwrap_or_else(|_| file.to_path_buf());
        let lower_root = self.root.to_string_lossy().to_lowercase();
        let lower_file = file.to_string_lossy().to_lowercase();
        if !lower_file.starts_with(&lower_root) {
            return None;
        }
        let rel = &file.to_string_lossy()[self.root.to_string_lossy().len()..];
        Some(rel.trim_start_matches(['\\', '/']).replace('\\', "/"))
    }

    pub fn status(&self) -> Result<Status, String> {
        let out = run(&self.root, &["status", "--porcelain=v2", "--branch", "-z", "--untracked-files=all"])?;
        Ok(parse_status(&out))
    }

    /// The file's content in the last commit (`None` for new files).
    pub fn head_content(&self, rel: &str) -> Option<String> {
        let out = git(&self.root, &["show", &format!("HEAD:{rel}")]).ok()?;
        out.status
            .success()
            .then(|| String::from_utf8_lossy(&out.stdout).trim_start_matches('\u{feff}').replace("\r\n", "\n"))
    }

    /// Commits all changes (including new files) with `message`.
    pub fn commit_all(&self, message: &str) -> Result<String, String> {
        if message.trim().is_empty() {
            return Err("Commit-Nachricht fehlt".into());
        }
        run(&self.root, &["add", "-A"])?;
        let out = run(&self.root, &["commit", "-q", "-m", message])?;
        let hash = run(&self.root, &["rev-parse", "--short", "HEAD"]).unwrap_or_default();
        let _ = out;
        Ok(hash.trim().to_string())
    }

    /// Pull (merge, no editor), then push. Without an upstream branch the branch is published.
    pub fn sync(&self) -> Result<String, String> {
        let status = self.status()?;
        let remotes = run(&self.root, &["remote"])?;
        let Some(remote) = remotes.lines().next().map(str::to_string) else {
            return Err("Kein Remote eingerichtet (z. B. GitHub) – nichts zu synchronisieren".into());
        };
        if !status.has_upstream {
            run(&self.root, &["push", "-u", &remote, &status.branch])?;
            return Ok(format!("Branch „{}“ veröffentlicht", status.branch));
        }
        if let Err(e) = run(&self.root, &["pull", "--no-edit", "--no-rebase"]) {
            if e.contains("CONFLICT") || e.contains("conflict") {
                return Err("Pull hat Konflikte: bitte im Git-Panel auflösen".into());
            }
            return Err(format!("Pull fehlgeschlagen: {e}"));
        }
        run(&self.root, &["push"]).map_err(|e| format!("Push fehlgeschlagen: {e}"))?;
        Ok("Synchronisiert".into())
    }

    /// `git fetch` (quietly; errors ignored by the caller if offline).
    pub fn fetch(&self) -> Result<(), String> {
        run(&self.root, &["fetch", "--quiet"]).map(|_| ())
    }

    /// Local branches, current one first.
    pub fn branches(&self) -> Result<Vec<String>, String> {
        let out = run(&self.root, &["branch", "--format=%(HEAD)%(refname:short)"])?;
        let mut list: Vec<(bool, String)> = out
            .lines()
            .filter(|l| !l.trim().is_empty())
            .map(|l| (l.starts_with('*'), l.trim_start_matches(['*', ' ']).to_string()))
            .collect();
        list.sort_by_key(|(current, name)| (!current, name.to_lowercase()));
        Ok(list.into_iter().map(|(_, n)| n).collect())
    }

    pub fn switch(&self, branch: &str) -> Result<(), String> {
        run(&self.root, &["switch", branch]).map(|_| ())
    }

    pub fn create_branch(&self, branch: &str) -> Result<(), String> {
        run(&self.root, &["switch", "-c", branch]).map(|_| ())
    }

    /// Who last changed a line (1-based).
    pub fn blame_line(&self, rel: &str, line: usize) -> Result<BlameLine, String> {
        let range = format!("{line},{line}");
        let out = run(&self.root, &["blame", "--porcelain", "-L", &range, "--", rel])?;
        parse_blame(&out).ok_or_else(|| "Keine Blame-Information".into())
    }
}

/// `\\?\C:\...` (from canonicalize on Windows) → `C:\...`.
fn strip_unc(p: &Path) -> PathBuf {
    let s = p.to_string_lossy();
    PathBuf::from(s.strip_prefix(r"\\?\").unwrap_or(&s).to_string())
}

pub fn parse_status(out: &str) -> Status {
    let mut st = Status::default();
    let mut parts = out.split('\0').peekable();
    while let Some(entry) = parts.next() {
        if entry.is_empty() {
            continue;
        }
        if let Some(head) = entry.strip_prefix("# branch.head ") {
            st.branch = head.to_string();
        } else if let Some(ab) = entry.strip_prefix("# branch.ab ") {
            st.has_upstream = true;
            for part in ab.split_whitespace() {
                if let Some(n) = part.strip_prefix('+') {
                    st.ahead = n.parse().unwrap_or(0);
                } else if let Some(n) = part.strip_prefix('-') {
                    st.behind = n.parse().unwrap_or(0);
                }
            }
        } else if let Some(rest) = entry.strip_prefix("1 ") {
            // 1 XY sub mH mI mW hH hI path
            let fields: Vec<&str> = rest.splitn(8, ' ').collect();
            if fields.len() == 8 {
                let kind = kind_from_xy(fields[0]);
                st.changes.push(Change { path: fields[7].to_string(), kind });
            }
        } else if let Some(rest) = entry.strip_prefix("2 ") {
            // 2 XY sub mH mI mW hH hI Xscore path  (then the original path as next entry)
            let fields: Vec<&str> = rest.splitn(9, ' ').collect();
            if fields.len() == 9 {
                st.changes.push(Change { path: fields[8].to_string(), kind: ChangeKind::Renamed });
            }
            parts.next();
        } else if let Some(rest) = entry.strip_prefix("u ") {
            if let Some(path) = rest.splitn(10, ' ').nth(9) {
                st.changes.push(Change { path: path.to_string(), kind: ChangeKind::Conflict });
            }
        } else if let Some(path) = entry.strip_prefix("? ") {
            st.changes.push(Change { path: path.to_string(), kind: ChangeKind::Untracked });
        }
    }
    st.changes.sort_by_key(|c| c.path.to_lowercase());
    st
}

fn kind_from_xy(xy: &str) -> ChangeKind {
    if xy.contains('D') {
        ChangeKind::Deleted
    } else if xy.starts_with('A') {
        ChangeKind::Added
    } else {
        ChangeKind::Modified
    }
}

fn parse_blame(out: &str) -> Option<BlameLine> {
    let mut lines = out.lines();
    let hash = lines.next()?.split_whitespace().next()?.to_string();
    let mut b = BlameLine {
        uncommitted: hash.chars().all(|c| c == '0'),
        hash: hash.chars().take(7).collect(),
        author: String::new(),
        time: 0,
        summary: String::new(),
    };
    for l in lines {
        if let Some(a) = l.strip_prefix("author ") {
            b.author = a.to_string();
        } else if let Some(t) = l.strip_prefix("author-time ") {
            b.time = t.parse().unwrap_or(0);
        } else if let Some(s) = l.strip_prefix("summary ") {
            b.summary = s.to_string();
        }
    }
    Some(b)
}

/// "vor 3 Tagen" for a unix time.
pub fn relative_time(time: i64, now: i64) -> String {
    let d = (now - time).max(0);
    let (n, unit_one, unit_many) = match d {
        0..60 => return "gerade eben".into(),
        60..3600 => (d / 60, "Minute", "Minuten"),
        3600..86_400 => (d / 3600, "Stunde", "Stunden"),
        86_400..2_592_000 => (d / 86_400, "Tag", "Tagen"),
        2_592_000..31_536_000 => (d / 2_592_000, "Monat", "Monaten"),
        _ => (d / 31_536_000, "Jahr", "Jahren"),
    };
    format!("vor {n} {}", if n == 1 { unit_one } else { unit_many })
}

// ----- line diff -----

/// How a line of the current text differs from the committed version (for the gutter bars).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LineMark {
    Added,
    Modified,
    /// Lines were deleted right after this line (`usize::MAX` line = at the very top).
    DeletedAfter,
}

fn lines(text: &str) -> Vec<&str> {
    text.split('\n').collect()
}

/// Gutter marks for `new` compared to `old`: (0-based line in `new`, mark).
pub fn line_marks(old: &str, new: &str) -> Vec<(usize, LineMark)> {
    let (o, n) = (lines(old), lines(new));
    let mut marks = Vec::new();
    for op in capture_diff_slices(Algorithm::Myers, &o, &n) {
        match op {
            DiffOp::Equal { .. } => {}
            DiffOp::Insert { new_index, new_len, .. } => {
                marks.extend((new_index..new_index + new_len).map(|l| (l, LineMark::Added)));
            }
            DiffOp::Delete { new_index, .. } => {
                marks.push((new_index.wrapping_sub(1), LineMark::DeletedAfter));
            }
            DiffOp::Replace { new_index, new_len, .. } => {
                marks.extend((new_index..new_index + new_len).map(|l| (l, LineMark::Modified)));
            }
        }
    }
    marks
}

/// One row of a side-by-side diff.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiffRow {
    /// Line in the old text (0-based) or `None` for a filler.
    pub old: Option<usize>,
    pub new: Option<usize>,
    pub kind: RowKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RowKind {
    Same,
    Removed,
    Added,
    Changed,
}

/// Aligns old and new lines for a side-by-side view; changed blocks are paired line by line.
pub fn side_by_side(old: &str, new: &str) -> Vec<DiffRow> {
    let (o, n) = (lines(old), lines(new));
    let mut rows = Vec::new();
    for op in capture_diff_slices(Algorithm::Myers, &o, &n) {
        match op {
            DiffOp::Equal { old_index, new_index, len } => {
                rows.extend((0..len).map(|i| DiffRow {
                    old: Some(old_index + i),
                    new: Some(new_index + i),
                    kind: RowKind::Same,
                }));
            }
            DiffOp::Delete { old_index, old_len, .. } => {
                rows.extend((0..old_len).map(|i| DiffRow { old: Some(old_index + i), new: None, kind: RowKind::Removed }));
            }
            DiffOp::Insert { new_index, new_len, .. } => {
                rows.extend((0..new_len).map(|i| DiffRow { old: None, new: Some(new_index + i), kind: RowKind::Added }));
            }
            DiffOp::Replace { old_index, old_len, new_index, new_len } => {
                for i in 0..old_len.max(new_len) {
                    rows.push(DiffRow {
                        old: (i < old_len).then_some(old_index + i),
                        new: (i < new_len).then_some(new_index + i),
                        kind: RowKind::Changed,
                    });
                }
            }
        }
    }
    rows
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_porcelain_v2_status() {
        let out = "# branch.oid abc\0# branch.head main\0# branch.upstream origin/main\0# branch.ab +2 -1\0\
                   1 .M N... 100644 100644 100644 aaa bbb src/Program.cs\0\
                   1 A. N... 000000 100644 100644 000 ccc neu.cs\0\
                   2 R. N... 100644 100644 100644 ddd eee R100 neuer Name.cs\0alter Name.cs\0\
                   ? notizen.txt\0";
        let st = parse_status(out);
        assert_eq!((st.branch.as_str(), st.ahead, st.behind, st.has_upstream), ("main", 2, 1, true));
        let kinds: Vec<(&str, char)> = st.changes.iter().map(|c| (c.path.as_str(), c.kind.letter())).collect();
        assert_eq!(kinds, vec![("neu.cs", 'A'), ("neuer Name.cs", 'R'), ("notizen.txt", 'U'), ("src/Program.cs", 'M')]);
    }

    #[test]
    fn gutter_marks() {
        let old = "a\nb\nc\nd\n";
        let new = "a\nB\nc\nneu\nd\n";
        assert_eq!(line_marks(old, new), vec![(1, LineMark::Modified), (3, LineMark::Added)]);
        assert_eq!(line_marks("a\nb\nc", "a\nc"), vec![(0, LineMark::DeletedAfter)]);
    }

    #[test]
    fn side_by_side_rows_align() {
        let rows = side_by_side("a\nb\nc", "a\nx\ny\nc");
        let kinds: Vec<RowKind> = rows.iter().map(|r| r.kind).collect();
        assert_eq!(kinds, vec![RowKind::Same, RowKind::Changed, RowKind::Changed, RowKind::Same]);
        assert_eq!(rows[2], DiffRow { old: None, new: Some(2), kind: RowKind::Changed });
    }

    #[test]
    fn relative_times() {
        assert_eq!(relative_time(0, 30), "gerade eben");
        assert_eq!(relative_time(0, 3 * 86_400), "vor 3 Tagen");
        assert_eq!(relative_time(0, 3600), "vor 1 Stunde");
    }

    /// Full round trip with a real temporary repository.
    #[test]
    fn real_repository_commit_and_blame() {
        let dir = std::env::temp_dir().join(format!("wyvern-git-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        run(&dir, &["init", "-q", "-b", "main"]).unwrap();
        run(&dir, &["config", "user.name", "Test"]).unwrap();
        run(&dir, &["config", "user.email", "test@example.invalid"]).unwrap();
        run(&dir, &["config", "commit.gpgsign", "false"]).unwrap();
        std::fs::write(dir.join("a.txt"), "eins\nzwei\n").unwrap();
        let repo = Repo::open(&dir).expect("repo");
        assert_eq!(repo.status().unwrap().changes[0].kind, ChangeKind::Untracked);
        assert!(repo.commit_all("").is_err());
        repo.commit_all("Erster Commit").unwrap();
        assert!(repo.status().unwrap().changes.is_empty());
        assert_eq!(repo.head_content("a.txt").unwrap(), "eins\nzwei\n");
        std::fs::write(dir.join("a.txt"), "eins\nZWEI\n").unwrap();
        let st = repo.status().unwrap();
        assert_eq!((st.branch.as_str(), st.changes[0].kind), ("main", ChangeKind::Modified));
        let blame = repo.blame_line("a.txt", 1).unwrap();
        assert_eq!((blame.author.as_str(), blame.summary.as_str()), ("Test", "Erster Commit"));
        assert!(repo.blame_line("a.txt", 2).unwrap().uncommitted);
        repo.create_branch("feature").unwrap();
        assert_eq!(repo.branches().unwrap(), vec!["feature", "main"]);
        assert_eq!(repo.relative(&dir.join("a.txt")).unwrap(), "a.txt");
        assert!(repo.sync().unwrap_err().contains("Kein Remote"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
