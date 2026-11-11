//! Workspaces with several folders.
//!
//! Two ways to keep them: automatically (extra folders are remembered for the main folder in
//! `%APPDATA%\Wyven\workspaces.json`, reopening the project brings them back) and as VS Code
//! `.code-workspace` files, which work in both editors.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde_json::{json, Value};

use crate::vscode_theme::strip_jsonc;

pub const EXTENSION: &str = "code-workspace";

pub fn is_workspace_file(path: &Path) -> bool {
    path.extension().is_some_and(|e| e.eq_ignore_ascii_case(EXTENSION))
}

/// The folders of a `.code-workspace` file (relative paths are relative to the file).
pub fn read_file(path: &Path) -> Result<Vec<PathBuf>, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let v: Value = serde_json::from_str(&strip_jsonc(&text)).map_err(|e| format!("{}: {e}", path.display()))?;
    let dir = path.parent().unwrap_or(Path::new("."));
    let folders: Vec<PathBuf> = v["folders"]
        .as_array()
        .map(|list| {
            list.iter()
                .filter_map(|f| f["path"].as_str())
                .map(|p| {
                    let p = PathBuf::from(p.replace('/', std::path::MAIN_SEPARATOR_STR));
                    let full = if p.is_absolute() { p } else { dir.join(p) };
                    clean(&full)
                })
                .collect()
        })
        .unwrap_or_default();
    if folders.is_empty() {
        return Err(format!("{}: no folders in the workspace", path.display()));
    }
    Ok(folders)
}

/// `a\b\..\c` → `a\c` (without touching the disk).
fn clean(p: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for c in p.components() {
        match c {
            std::path::Component::ParentDir => {
                out.pop();
            }
            std::path::Component::CurDir => {}
            other => out.push(other),
        }
    }
    out
}

/// A folder as VS Code writes it: relative to the workspace file when it is below it.
fn folder_entry(dir: &Path, folder: &Path) -> String {
    match folder.strip_prefix(dir) {
        Ok(rel) if rel.as_os_str().is_empty() => ".".into(),
        Ok(rel) => rel.to_string_lossy().replace('\\', "/"),
        Err(_) => folder.to_string_lossy().replace('\\', "/"),
    }
}

/// Writes the folders into a `.code-workspace` file, keeping its other content (settings …).
pub fn write_file(path: &Path, folders: &[PathBuf]) -> Result<(), String> {
    let mut v: Value = std::fs::read_to_string(path)
        .ok()
        .and_then(|t| serde_json::from_str(&strip_jsonc(&t)).ok())
        .filter(Value::is_object)
        .unwrap_or_else(|| json!({}));
    let dir = path.parent().unwrap_or(Path::new("."));
    v["folders"] = Value::Array(folders.iter().map(|f| json!({ "path": folder_entry(dir, f) })).collect());
    if v.get("settings").is_none() {
        v["settings"] = json!({});
    }
    let text = serde_json::to_string_pretty(&v).map_err(|e| e.to_string())?;
    std::fs::write(path, text + "\n").map_err(|e| format!("{}: {e}", path.display()))
}

fn store_path() -> Option<PathBuf> {
    crate::config::config_dir().map(|d| d.join("workspaces.json"))
}

fn key(main: &Path) -> String {
    main.to_string_lossy().to_lowercase()
}

fn read_store() -> BTreeMap<String, Vec<PathBuf>> {
    store_path()
        .and_then(|p| std::fs::read_to_string(p).ok())
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_default()
}

/// Folders added to the project `main` earlier (that still exist).
pub fn extra_folders(main: &Path) -> Vec<PathBuf> {
    read_store().remove(&key(main)).unwrap_or_default().into_iter().filter(|p| p.is_dir()).collect()
}

/// Remembers the added folders of `main` (none: forgets the entry).
pub fn remember_extra_folders(main: &Path, extras: &[PathBuf]) {
    let Some(path) = store_path() else { return };
    let mut store = read_store();
    if extras.is_empty() {
        store.remove(&key(main));
    } else {
        store.insert(key(main), extras.to_vec());
    }
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    if let Ok(text) = serde_json::to_string_pretty(&store)
        && let Err(e) = std::fs::write(&path, text)
    {
        log::warn!("workspaces.json: {e}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn code_workspace_files_round_trip_and_keep_settings() {
        let dir = std::env::temp_dir().join(format!("wyven-ws-test-{}", std::process::id()));
        let app = dir.join("app");
        let lib = dir.join("lib");
        std::fs::create_dir_all(&app).unwrap();
        std::fs::create_dir_all(&lib).unwrap();
        let file = app.join("both.code-workspace");
        std::fs::write(
            &file,
            "{\n // VS Code writes comments\n \"folders\": [ { \"path\": \".\" }, { \"path\": \"../lib\" }, ],\n \"settings\": { \"editor.tabSize\": 2 }\n}",
        )
        .unwrap();
        let folders = read_file(&file).unwrap();
        assert_eq!(folders, vec![app.clone(), lib.clone()]);

        let other = dir.join("other");
        write_file(&file, &[app.clone(), other.clone()]).unwrap();
        let v: Value = serde_json::from_str(&std::fs::read_to_string(&file).unwrap()).unwrap();
        assert_eq!(v["settings"]["editor.tabSize"], 2, "other content stays");
        assert_eq!(v["folders"][0]["path"], ".");
        assert_eq!(read_file(&file).unwrap(), vec![app, other]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn workspace_files_are_recognized() {
        assert!(is_workspace_file(Path::new("x/My.Code-Workspace")));
        assert!(!is_workspace_file(Path::new("x/settings.json")));
    }
}
