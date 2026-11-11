//! List of all files in the project, built on a background thread.
//!
//! Uses ripgrep's walker: respects `.gitignore`, skips hidden files and the generated folders
//! the explorer hides too. The UI never waits for it; the search field shows results as soon
//! as the list is ready.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use crate::explorer::IGNORED;

/// Stop after this many files so a mistakenly opened drive root stays usable.
const MAX_FILES: usize = 200_000;

#[derive(Clone, Default)]
pub struct FileIndex {
    files: Arc<Mutex<Option<Arc<Vec<String>>>>>,
}

impl FileIndex {
    /// Starts (re)building the index for the workspace folders in the background. Files of the
    /// first folder are relative to it; files of further folders start with that folder's name
    /// (`lib/src/x.rs`), see [`resolve`].
    pub fn rebuild(&self, roots: &[PathBuf]) {
        let roots = roots.to_vec();
        let slot = self.files.clone();
        std::thread::Builder::new()
            .name("file-index".into())
            .spawn(move || {
                let started = Instant::now();
                let mut files = Vec::new();
                for (i, root) in roots.iter().enumerate() {
                    let prefix = if i == 0 { String::new() } else { format!("{}/", folder_name(root)) };
                    files.extend(walk(root).into_iter().map(|f| format!("{prefix}{f}")));
                }
                log::info!("indexed {} files in {:?}", files.len(), started.elapsed());
                *slot.lock().unwrap() = Some(Arc::new(files));
            })
            .expect("cannot start index thread");
    }

    /// Relative paths with `/` separators, or `None` while the first build is still running.
    pub fn files(&self) -> Option<Arc<Vec<String>>> {
        self.files.lock().unwrap().clone()
    }
}

fn folder_name(root: &Path) -> String {
    root.file_name().map_or_else(|| root.display().to_string(), |n| n.to_string_lossy().into_owned())
}

/// The file behind an index entry (see [`FileIndex::rebuild`]).
pub fn resolve(roots: &[PathBuf], entry: &str) -> Option<PathBuf> {
    let native = |s: &str| s.replace('/', std::path::MAIN_SEPARATOR_STR);
    let first = roots.first()?.join(native(entry));
    if first.exists() {
        return Some(first);
    }
    roots.iter().skip(1).find_map(|r| {
        let rest = entry.strip_prefix(&format!("{}/", folder_name(r)))?;
        Some(r.join(native(rest)))
    })
}

fn walk(root: &PathBuf) -> Vec<String> {
    let mut files = Vec::new();
    let walker = ignore::WalkBuilder::new(root)
        .hidden(true)
        .git_ignore(true)
        .git_global(false)
        .require_git(false)
        .filter_entry(|e| {
            let name = e.file_name().to_string_lossy();
            !(e.file_type().is_some_and(|t| t.is_dir())
                && IGNORED.iter().any(|i| i.eq_ignore_ascii_case(&name)))
        })
        .build();
    for entry in walker.flatten() {
        if !entry.file_type().is_some_and(|t| t.is_file()) {
            continue;
        }
        if let Ok(rel) = entry.path().strip_prefix(root) {
            files.push(rel.to_string_lossy().replace('\\', "/"));
        }
        if files.len() >= MAX_FILES {
            log::warn!("index stopped at {MAX_FILES} files");
            break;
        }
    }
    files.sort_by_key(|f| f.to_lowercase());
    files
}
