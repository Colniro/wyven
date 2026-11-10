//! User settings from `settings.toml` in Wyvern's config folder.
//!
//! The file is created with commented defaults on first start. Unknown keys are reported (they
//! are usually typos), and a broken file never stops Wyvern: the last good settings stay active.

use std::path::PathBuf;
use std::time::SystemTime;

use serde::Deserialize;

use crate::config::config_dir;

/// Written on first start. Every option is listed with its default value.
pub const TEMPLATE: &str = r#"# Wyvern-Einstellungen
# Speichern (Strg+S) in Wyvern wendet sie sofort an. Alt → , öffnet diese Datei.

# Farbschema: "dunkel" oder "hell" (auch im Suchfeld: Strg+Q, dann >Theme)
theme = "dunkel"

[font]
# Schriftart für Code und Oberfläche. Leer = automatisch
# (Cascadia Mono, Cascadia Code, JetBrains Mono, Consolas, …)
family = ""
# Schriftgröße in Punkt (Strg++ / Strg+- ändert sie nur bis zum Neustart)
size = 14
# Zeilenhöhe als Vielfaches der Schriftgröße
line_height = 1.5

[editor]
# Breite eines Tabs in Spalten
tab_width = 4
# Einrückung, wenn eine Datei noch keine hat: "leerzeichen" oder "tabs"
indent = "leerzeichen"
# Vorschläge beim Tippen automatisch zeigen (Strg+Leertaste geht immer)
auto_complete = true
# Info anzeigen, wenn die Maus kurz über einem Namen stillsteht
hover = true

[ui]
# Größe der Oberflächenschrift (Explorer, Tabs, Statusleiste) relativ zur Code-Schrift
font_scale = 0.93
# Breite des Explorers in Zeichen
explorer_width = 30

[terminal]
# Shell für das Terminal (Alt → t). Leer = automatisch (PowerShell 7, sonst Windows PowerShell).
# Beispiele: "cmd.exe", "pwsh.exe", 'C:\Program Files\Git\bin\bash.exe'
# (Pfade mit \ in einfachen Anführungszeichen schreiben)
shell = ""
# Startargumente der Shell, z. B. ["-NoLogo"]
args = []
# Höhe des Terminals in Prozent des Fensters
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
            theme: "dunkel".into(),
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
        Self { tab_width: 4, indent: "leerzeichen".into(), auto_complete: true, hover: true }
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
    let path = ensure_file().ok_or("kein Konfigurationsordner")?;
    let text = std::fs::read_to_string(&path).map_err(|e| e.to_string())?;
    let new = replace_theme_line(&text, name);
    std::fs::write(&path, new).map_err(|e| e.to_string())
}

fn replace_theme_line(text: &str, name: &str) -> String {
    let line = format!("theme = \"{name}\"");
    let mut found = false;
    let mut in_table = false;
    let mut out: Vec<String> = Vec::new();
    for l in text.lines() {
        let t = l.trim_start();
        if t.starts_with('[') {
            in_table = true;
        }
        if !in_table && !found && t.starts_with("theme") && t[5..].trim_start().starts_with('=') {
            out.push(line.clone());
            found = true;
        } else {
            out.push(l.to_string());
        }
    }
    if !found {
        out.insert(0, line);
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
        assert_eq!(replace_theme_line(text, "hell"), "# Kommentar\ntheme = \"hell\"\n\n[font]\nsize = 14\n");
        assert_eq!(replace_theme_line("[font]\nsize = 14\n", "hell"), "theme = \"hell\"\n[font]\nsize = 14\n");
    }
}
