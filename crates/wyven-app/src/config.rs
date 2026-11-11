//! Wyven's own files: config folder and the list of recently opened projects.

use std::path::{Path, PathBuf};

const MAX_RECENT: usize = 9;

/// `%APPDATA%\Wyven` on Windows, `$XDG_CONFIG_HOME/wyven` or `~/.config/wyven` elsewhere.
pub fn config_dir() -> Option<PathBuf> {
    let base = if cfg!(windows) {
        std::env::var_os("APPDATA").map(PathBuf::from)
    } else {
        std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
    }?;
    Some(base.join(if cfg!(windows) { "Wyven" } else { "wyven" }))
}

/// The project was called "Wyvern" until November 2026. Moves the old config folder (settings,
/// recent projects, netcoredbg, debug hook) to the new name once, if there is no new one yet.
pub fn migrate_old_config_dir() {
    let Some(new) = config_dir() else { return };
    let old = new.with_file_name(if cfg!(windows) { "Wyvern" } else { "wyvern" });
    if new.exists() || !old.is_dir() {
        return;
    }
    match std::fs::rename(&old, &new) {
        Ok(()) => log::info!("moved config folder {} -> {}", old.display(), new.display()),
        Err(e) => log::warn!("cannot move config folder {}: {e}", old.display()),
    }
}

fn recent_file() -> Option<PathBuf> {
    config_dir().map(|d| d.join("recent-projects.txt"))
}

/// Recently opened project folders, newest first; folders that no longer exist are skipped.
pub fn recent_projects() -> Vec<PathBuf> {
    let Some(file) = recent_file() else { return Vec::new() };
    std::fs::read_to_string(file)
        .unwrap_or_default()
        .lines()
        .map(PathBuf::from)
        .filter(|p| p.is_dir())
        .take(MAX_RECENT)
        .collect()
}

/// Moves `project` to the top of the recent list.
pub fn remember_project(project: &Path) {
    let Some(file) = recent_file() else { return };
    let mut list = recent_projects();
    list.retain(|p| p != project);
    list.insert(0, project.to_path_buf());
    list.truncate(MAX_RECENT);
    let text: String = list.iter().map(|p| format!("{}\n", p.display())).collect();
    if let Some(dir) = file.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    if let Err(e) = std::fs::write(&file, text) {
        log::warn!("cannot save recent projects: {e}");
    }
}
