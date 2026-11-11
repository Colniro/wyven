//! Your own shortcuts: `%APPDATA%\Wyven\keymap.toml` holds only the differences from the defaults.
//!
//! ```toml
//! [keys]
//! "ctrl+shift+d" = "view.toggle_word_wrap"   # any command of the search field
//! "f12" = "none"                             # remove a shortcut
//!
//! [leader.g]                                 # Alt → g → s
//! "s" = "git.sync_pull_push"
//! ```
//!
//! Command names come from the titles in the search field: "View: Toggle Word Wrap" becomes
//! `view.toggle_word_wrap`. The file lists all of them as comments.

use std::path::PathBuf;

use winit::keyboard::{Key, ModifiersState, NamedKey};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Combo {
    pub ctrl: bool,
    pub shift: bool,
    pub alt: bool,
    /// Lowercase: a character ("d", ".", "#") or a key name ("f12", "enter", "pageup").
    pub key: String,
}

#[derive(Debug, Default)]
pub struct CustomKeys {
    /// Key → command name (`None` = the shortcut is removed).
    keys: Vec<(Combo, Option<String>)>,
    /// Leader group ("" = first level), letter → command name.
    leader: Vec<(String, char, String)>,
}

pub fn path() -> Option<PathBuf> {
    crate::config::config_dir().map(|d| d.join("keymap.toml"))
}

/// "View: Toggle Word Wrap" → "view.toggle_word_wrap".
pub fn command_id(title: &str) -> String {
    let title = title.split(" (").next().unwrap_or(title).replace('…', "");
    let (group, name) = title.split_once(':').unwrap_or(("", &title));
    let slug = |s: &str| {
        let mut out = String::new();
        for c in s.trim().to_lowercase().chars() {
            if c.is_alphanumeric() {
                out.push(c);
            } else if !out.ends_with('_') && !out.is_empty() {
                out.push('_');
            }
        }
        out.trim_end_matches('_').to_string()
    };
    if group.is_empty() { slug(name) } else { format!("{}.{}", slug(group), slug(name)) }
}

const NAMES: &[(&str, NamedKey)] = &[
    ("enter", NamedKey::Enter),
    ("escape", NamedKey::Escape),
    ("esc", NamedKey::Escape),
    ("tab", NamedKey::Tab),
    ("space", NamedKey::Space),
    ("backspace", NamedKey::Backspace),
    ("delete", NamedKey::Delete),
    ("del", NamedKey::Delete),
    ("insert", NamedKey::Insert),
    ("home", NamedKey::Home),
    ("end", NamedKey::End),
    ("pageup", NamedKey::PageUp),
    ("pagedown", NamedKey::PageDown),
    ("up", NamedKey::ArrowUp),
    ("down", NamedKey::ArrowDown),
    ("left", NamedKey::ArrowLeft),
    ("right", NamedKey::ArrowRight),
    ("f1", NamedKey::F1),
    ("f2", NamedKey::F2),
    ("f3", NamedKey::F3),
    ("f4", NamedKey::F4),
    ("f5", NamedKey::F5),
    ("f6", NamedKey::F6),
    ("f7", NamedKey::F7),
    ("f8", NamedKey::F8),
    ("f9", NamedKey::F9),
    ("f10", NamedKey::F10),
    ("f11", NamedKey::F11),
    ("f12", NamedKey::F12),
];

/// "ctrl+shift+d", "alt+f1", "f5", "ctrl+." …
pub fn parse_combo(s: &str) -> Result<Combo, String> {
    let lower = s.trim().to_lowercase();
    // "ctrl++" means the plus key.
    let (mods, key) = match lower.strip_suffix("++") {
        Some(rest) => (rest, "+".to_string()),
        None => match lower.rsplit_once('+') {
            Some((m, k)) => (m, k.to_string()),
            None => ("", lower.clone()),
        },
    };
    let mut c = Combo { ctrl: false, shift: false, alt: false, key: key.trim().to_string() };
    for m in mods.split('+').map(str::trim).filter(|m| !m.is_empty()) {
        match m {
            "ctrl" | "strg" | "control" => c.ctrl = true,
            "shift" => c.shift = true,
            "alt" => c.alt = true,
            other => return Err(format!("“{s}”: unknown modifier “{other}”")),
        }
    }
    let known = c.key.chars().count() == 1 || NAMES.iter().any(|(n, _)| *n == c.key);
    if !known {
        return Err(format!("“{s}”: unknown key “{}”", c.key));
    }
    // Same spelling for aliases.
    if let Some((_, named)) = NAMES.iter().find(|(n, _)| *n == c.key) {
        c.key = NAMES.iter().find(|(_, k)| k == named).map_or(c.key.clone(), |(n, _)| n.to_string());
    }
    Ok(c)
}

/// The combination of a key press (None for modifier keys alone).
pub fn combo_of(key: &Key, mods: ModifiersState) -> Option<Combo> {
    let name = match key {
        Key::Character(s) => s.to_lowercase(),
        Key::Named(n) => NAMES.iter().find(|(_, k)| k == n)?.0.to_string(),
        _ => return None,
    };
    Some(Combo { ctrl: mods.control_key(), shift: mods.shift_key(), alt: mods.alt_key(), key: name })
}

impl CustomKeys {
    /// Reads keymap.toml; `known` tells whether a command name exists. Problems come back as
    /// messages; the rest still works.
    pub fn load(known: impl Fn(&str) -> bool) -> (Self, Vec<String>) {
        let Some(text) = path().and_then(|p| std::fs::read_to_string(p).ok()) else { return (Self::default(), Vec::new()) };
        Self::parse(&text, known)
    }

    pub fn parse(text: &str, known: impl Fn(&str) -> bool) -> (Self, Vec<String>) {
        let mut me = Self::default();
        let mut errors = Vec::new();
        let table: toml::Table = match text.parse() {
            Ok(t) => t,
            Err(e) => return (me, vec![format!("keymap.toml: {e}")]),
        };
        let check = |id: &str, errors: &mut Vec<String>| {
            let ok = known(id);
            if !ok {
                errors.push(format!("keymap.toml: unknown command “{id}”"));
            }
            ok
        };
        if let Some(keys) = table.get("keys").and_then(|v| v.as_table()) {
            for (k, v) in keys {
                let Some(id) = v.as_str() else { continue };
                match parse_combo(k) {
                    Ok(combo) if id == "none" => me.keys.push((combo, None)),
                    Ok(combo) => {
                        if check(id, &mut errors) {
                            me.keys.push((combo, Some(id.to_string())));
                        }
                    }
                    Err(e) => errors.push(format!("keymap.toml: {e}")),
                }
            }
        }
        if let Some(leader) = table.get("leader").and_then(|v| v.as_table()) {
            for (k, v) in leader {
                match v {
                    toml::Value::String(id) => {
                        if let Some(c) = single_char(k)
                            && check(id, &mut errors)
                        {
                            me.leader.push((String::new(), c, id.clone()));
                        }
                    }
                    toml::Value::Table(group) => {
                        for (k2, v2) in group {
                            if let (Some(c), Some(id)) = (single_char(k2), v2.as_str())
                                && check(id, &mut errors)
                            {
                                me.leader.push((k.clone(), c, id.to_string()));
                            }
                        }
                    }
                    _ => {}
                }
            }
        }
        (me, errors)
    }

    /// The binding of a key press: `Some(None)` = removed, `Some(Some(id))` = a command.
    pub fn key(&self, combo: &Combo) -> Option<Option<&str>> {
        self.keys.iter().find(|(c, _)| c == combo).map(|(_, id)| id.as_deref())
    }

    pub fn leader(&self, group: &str, c: char) -> Option<&str> {
        self.leader.iter().find(|(g, k, _)| g == group && *k == c).map(|(_, _, id)| id.as_str())
    }

    pub fn is_empty(&self) -> bool {
        self.keys.is_empty() && self.leader.is_empty()
    }
}

fn single_char(s: &str) -> Option<char> {
    let mut chars = s.chars();
    let c = chars.next()?;
    chars.next().is_none().then(|| c.to_ascii_lowercase())
}

/// The file Wyven writes when you open it the first time: examples and every command name.
pub fn template(commands: &[(String, String, String)]) -> String {
    let mut out = String::from(
        "# Your own Wyven shortcuts: only what differs from the defaults.\n\
         # Saving this file in Wyven applies it.\n\
         #\n\
         # [keys]\n\
         # \"ctrl+shift+d\" = \"view.toggle_word_wrap\"\n\
         # \"f12\" = \"none\"                # removes a shortcut\n\
         #\n\
         # [leader.g]                       # Alt → g → x\n\
         # \"x\" = \"git.sync_pull_push\"\n\
         #\n\
         # Keys: ctrl, shift and alt with a letter, digit or symbol, or with f1…f12, enter, escape,\n\
         # tab, space, backspace, delete, insert, home, end, pageup, pagedown, up, down, left, right.\n\
         # The key is the character printed on it (\"ctrl+#\" works on a German keyboard).\n\
         #\n\
         # Commands (name, title, default keys):\n",
    );
    let width = commands.iter().map(|(id, _, _)| id.len()).max().unwrap_or(0);
    for (id, title, keys) in commands {
        let keys = if keys.is_empty() { String::new() } else { format!("  ({keys})") };
        out.push_str(&format!("#   {id:width$}  {title}{keys}\n"));
    }
    out.push_str("\n[keys]\n\n[leader]\n");
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn command_names_come_from_titles() {
        assert_eq!(command_id("View: Toggle Word Wrap"), "view.toggle_word_wrap");
        assert_eq!(command_id("Git: Sync (Pull + Push)"), "git.sync");
        assert_eq!(command_id("Workspace: Add Folder to Workspace …"), "workspace.add_folder_to_workspace");
        assert_eq!(command_id("Code: Organize Usings / Imports"), "code.organize_usings_imports");
    }

    #[test]
    fn combos_parse_like_they_are_written() {
        let c = parse_combo("Ctrl+Shift+D").unwrap();
        assert!(c.ctrl && c.shift && !c.alt && c.key == "d");
        assert_eq!(parse_combo("ctrl++").unwrap().key, "+");
        assert_eq!(parse_combo("del").unwrap(), parse_combo("delete").unwrap());
        assert!(parse_combo("hyper+x").is_err());
        assert!(parse_combo("ctrl+nosuchkey").is_err());
        let pressed = combo_of(&Key::Named(NamedKey::F12), ModifiersState::empty()).unwrap();
        assert_eq!(pressed, parse_combo("f12").unwrap());
    }

    #[test]
    fn the_file_binds_removes_and_extends_the_leader() {
        let text = r#"
            [keys]
            "ctrl+shift+d" = "view.toggle_word_wrap"
            "f12" = "none"
            "ctrl+j" = "no.such.command"
            [leader]
            "x" = "view.zen_mode"
            [leader.g]
            "x" = "git.sync"
        "#;
        let known = |id: &str| id != "no.such.command";
        let (k, errors) = CustomKeys::parse(text, known);
        assert_eq!(errors.len(), 1, "{errors:?}");
        assert_eq!(k.key(&parse_combo("ctrl+shift+d").unwrap()), Some(Some("view.toggle_word_wrap")));
        assert_eq!(k.key(&parse_combo("f12").unwrap()), Some(None));
        assert_eq!(k.key(&parse_combo("ctrl+j").unwrap()), None);
        assert_eq!(k.leader("", 'x'), Some("view.zen_mode"));
        assert_eq!(k.leader("g", 'x'), Some("git.sync"));
    }
}
