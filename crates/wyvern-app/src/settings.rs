//! User settings from `settings.toml` in Wyvern's config folder.
//!
//! The file is created with commented defaults on first start. Unknown keys are reported (they
//! are usually typos), and a broken file never stops Wyvern: the last good settings stay active.

use std::path::PathBuf;
use std::time::SystemTime;

use serde::Deserialize;

use crate::config::config_dir;

/// Written on first start. Every option is listed with its default value.
pub const TEMPLATE: &str = r#"# Wyvern settings
# Saving (Ctrl+S) in Wyvern applies them right away. Alt → , opens this file.

# Color theme: "dark" or "light" (also in the search field: Ctrl+Q, then >Theme)
theme = "dark"

[font]
# Font for code and interface. Empty = automatic
# (Cascadia Mono, Cascadia Code, JetBrains Mono, Consolas, …)
family = ""
# Font size in points (Ctrl++ / Ctrl+- only change it until restart)
size = 14
# Line height as a multiple of the font size
line_height = 1.5

[editor]
# Width of a tab in columns
tab_width = 4
# Indentation for files that have none yet: "spaces" or "tabs"
indent = "spaces"
# Show suggestions automatically while typing (Ctrl+Space always works)
auto_complete = true
# Show info when the mouse rests on a name for a moment
hover = true
# Minimap on the right: "blocks" (shape of the code), "bar" (narrow marker bar) or "off".
# Alt → v → m switches and saves it here.
minimap = "blocks"
# Keep the headers of the current namespace, class and method at the top while scrolling
# (Alt → v → s switches and saves it here)
sticky_scroll = true

[ui]
# Size of the interface font (explorer, tabs, status bar) relative to the code font
font_scale = 0.93
# Width of the explorer in characters
explorer_width = 30

[terminal]
# Shell for the terminal (Alt → t). Empty = automatic (PowerShell 7, else Windows PowerShell).
# Examples: "cmd.exe", "pwsh.exe", 'C:\Program Files\Git\bin\bash.exe'
# (write paths with \ in single quotes)
shell = ""
# Shell start arguments, e.g. ["-NoLogo"]
args = []
# Height of the terminal in percent of the window
height = 35
"#;

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Settings {
    pub theme: String,
    pub font: FontSettings,
    pub editor: EditorSettings,
    pub ui: UiSettings,
    pub terminal: TerminalSettings,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct TerminalSettings {
    pub shell: String,
    pub args: Vec<String>,
    pub height: f32,
}

impl Default for TerminalSettings {
    fn default() -> Self {
        Self { shell: String::new(), args: Vec::new(), height: 35.0 }
    }
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct FontSettings {
    pub family: String,
    pub size: f32,
    pub line_height: f32,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct EditorSettings {
    pub tab_width: usize,
    pub indent: String,
    pub auto_complete: bool,
    pub hover: bool,
    pub minimap: String,
    pub sticky_scroll: bool,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct UiSettings {
    pub font_scale: f32,
    pub explorer_width: f32,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            theme: "dark".into(),
            font: FontSettings::default(),
            editor: EditorSettings::default(),
            ui: UiSettings::default(),
            terminal: TerminalSettings::default(),
        }
    }
}

impl Default for FontSettings {
    fn default() -> Self {
        Self { family: String::new(), size: 14.0, line_height: 1.5 }
    }
}

impl Default for EditorSettings {
    fn default() -> Self {
        Self { tab_width: 4, indent: "spaces".into(), auto_complete: true, hover: true, minimap: "blocks".into(), sticky_scroll: true }
    }
}

impl Default for UiSettings {
    fn default() -> Self {
        Self { font_scale: 0.93, explorer_width: 30.0 }
    }
}

impl Settings {
    /// Parses settings text. Out-of-range numbers are clamped instead of rejected.
    pub fn parse(text: &str) -> Result<Self, String> {
        let mut s: Settings = toml::from_str(text).map_err(|e| e.to_string())?;
        s.font.size = s.font.size.clamp(6.0, 48.0);
        s.font.line_height = s.font.line_height.clamp(1.0, 3.0);
        s.editor.tab_width = s.editor.tab_width.clamp(1, 16);
        s.ui.font_scale = s.ui.font_scale.clamp(0.6, 1.5);
        s.ui.explorer_width = s.ui.explorer_width.clamp(12.0, 80.0);
        s.terminal.height = s.terminal.height.clamp(15.0, 85.0);
        Ok(s)
    }

    /// `"\t"` or the right number of spaces for files without own indentation.
    pub fn indent_unit(&self) -> String {
        match self.editor.indent.to_lowercase().as_str() {
            "tabs" | "tab" => "\t".into(),
            _ => " ".repeat(self.editor.tab_width),
        }
    }
}

pub fn path() -> Option<PathBuf> {
    config_dir().map(|d| d.join("settings.toml"))
}

/// Creates the settings file with the template if it does not exist yet.
pub fn ensure_file() -> Option<PathBuf> {
    let path = path()?;
    if !path.exists() {
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        if let Err(e) = std::fs::write(&path, TEMPLATE) {
            log::warn!("cannot create {}: {e}", path.display());
        }
    }
    Some(path)
}

pub fn modified() -> Option<SystemTime> {
    std::fs::metadata(path()?).and_then(|m| m.modified()).ok()
}

/// Reads the settings file. On error the message says what is wrong (with line number).
pub fn load() -> Result<Settings, String> {
    let Some(path) = ensure_file() else { return Ok(Settings::default()) };
    let text = std::fs::read_to_string(&path).map_err(|e| e.to_string())?;
    let text = text.strip_prefix('\u{feff}').unwrap_or(&text);
    Settings::parse(text)
}

/// Changes the `theme = "…"` line in the settings file, keeping everything else as it is.
pub fn store_theme(name: &str) -> Result<(), String> {
    store_value(None, "theme", &format!("\"{name}\""))
}

/// Sets `key = value` (value as TOML, e.g. `"\"bar\""`) in `[table]` or at the top level,
/// keeping comments and everything else as it is.
pub fn store_value(table: Option<&str>, key: &str, value: &str) -> Result<(), String> {
    let path = ensure_file().ok_or("no config folder")?;
    let text = std::fs::read_to_string(&path).map_err(|e| e.to_string())?;
    let new = replace_value(&text, table, key, value);
    std::fs::write(&path, new).map_err(|e| e.to_string())
}

fn replace_value(text: &str, table: Option<&str>, key: &str, value: &str) -> String {
    let line = format!("{key} = {value}");
    let mut found = false;
    // None = top level; Some(name) = inside [name].
    let mut current: Option<String> = None;
    let mut header_at: Option<usize> = None;
    let mut out: Vec<String> = Vec::new();
    for l in text.lines() {
        let t = l.trim_start();
        if let Some(rest) = t.strip_prefix('[') {
            current = Some(rest.split(']').next().unwrap_or("").trim().to_string());
            if current.as_deref() == table {
                header_at = Some(out.len());
            }
        }
        let is_key = t.strip_prefix(key).is_some_and(|r| r.trim_start().starts_with('='));
        if !found && is_key && current.as_deref() == table {
            out.push(line.clone());
            found = true;
        } else {
            out.push(l.to_string());
        }
    }
    if !found {
        match (table, header_at) {
            (None, _) => out.insert(0, line),
            (Some(_), Some(i)) => out.insert(i + 1, line),
            (Some(t), None) => {
                out.push(String::new());
                out.push(format!("[{t}]"));
                out.push(line);
            }
        }
    }
    let mut s = out.join("\n");
    s.push('\n');
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn template_parses_to_defaults() {
        assert_eq!(Settings::parse(TEMPLATE).unwrap(), Settings::default());
    }

    #[test]
    fn missing_keys_use_defaults_and_typos_are_reported() {
        let s = Settings::parse("theme = \"hell\"\n[font]\nsize = 16\n").unwrap();
        assert_eq!(s.theme, "hell");
        assert_eq!(s.font.size, 16.0);
        assert_eq!(s.editor.tab_width, 4);
        let err = Settings::parse("[font]\nsitze = 16\n").unwrap_err();
        assert!(err.contains("sitze"), "{err}");
    }

    #[test]
    fn windows_shell_path_example_is_valid_toml() {
        let s = Settings::parse("[terminal]\nshell = 'C:\\Program Files\\Git\\bin\\bash.exe'\n").unwrap();
        assert_eq!(s.terminal.shell, r"C:\Program Files\Git\bin\bash.exe");
    }

    #[test]
    fn numbers_are_clamped() {
        let s = Settings::parse("[font]\nsize = 200\n[editor]\ntab_width = 0\n").unwrap();
        assert_eq!((s.font.size, s.editor.tab_width), (48.0, 1));
    }

    #[test]
    fn theme_line_is_replaced_in_place() {
        let text = "# Kommentar\ntheme = \"dunkel\"\n\n[font]\nsize = 14\n";
        let theme = |t: &str, n: &str| replace_value(t, None, "theme", &format!("\"{n}\""));
        assert_eq!(theme(text, "hell"), "# Kommentar\ntheme = \"hell\"\n\n[font]\nsize = 14\n");
        assert_eq!(theme("[font]\nsize = 14\n", "hell"), "theme = \"hell\"\n[font]\nsize = 14\n");
    }

    #[test]
    fn table_values_are_replaced_or_added() {
        let text = "theme = \"dark\"\n[editor]\ntab_width = 4\nminimap = \"blocks\"\n";
        let set = |t: &str, v: &str| replace_value(t, Some("editor"), "minimap", v);
        assert_eq!(set(text, "\"bar\""), "theme = \"dark\"\n[editor]\ntab_width = 4\nminimap = \"bar\"\n");
        assert_eq!(
            set("[editor]\ntab_width = 4\n", "\"off\""),
            "[editor]\nminimap = \"off\"\ntab_width = 4\n"
        );
        assert_eq!(set("theme = \"dark\"\n", "\"off\""), "theme = \"dark\"\n\n[editor]\nminimap = \"off\"\n");
        // A key of the same name in another table stays untouched.
        let other = "[font]\nminimap = 1\n[editor]\n";
        assert_eq!(set(other, "\"bar\""), "[font]\nminimap = 1\n[editor]\nminimap = \"bar\"\n");
    }
}
