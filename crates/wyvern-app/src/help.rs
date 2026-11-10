//! The shortcut cheat sheet (`Alt → h`), generated from the same tables the search field and
//! the leader menu use, so it never goes out of date.

use crate::leader::{self, Action};
use crate::palette;

/// Basics that are not commands of their own (typing, moving, mouse).
const BASICS: &[(&str, &str)] = &[
    ("Arrow keys", "Move cursor (Ctrl = by word, Shift = select)"),
    ("Home / End", "Line start (first text, then column 1) / line end"),
    ("Ctrl+Home / Ctrl+End", "Start / end of file"),
    ("PgUp / PgDn", "Page up / down"),
    ("Tab / Shift+Tab", "Indent / outdent (several lines: all of them)"),
    ("Enter", "New line with indentation; between {} on its own line"),
    ("Ctrl+Enter / Ctrl+Shift+Enter", "New line below / above"),
    ("Ctrl+Backspace / Ctrl+Del", "Delete word"),
    ("Ctrl+C / X / V", "Copy / cut / paste (no selection: whole line)"),
    ("Esc", "Back to one cursor; close popups"),
    ("Alt+Click", "Add / remove a cursor"),
    ("Double-click", "Select word"),
    ("Ctrl+Click / F12", "Go to definition"),
    ("Ctrl+Space", "Autocomplete"),
    ("Ctrl+K, Ctrl+I", "Symbol info (or rest the mouse briefly)"),
    ("F2", "Rename"),
    ("Ctrl+.", "Quick actions"),
    ("F8", "Next error"),
    ("Click on a line number", "Toggle a breakpoint (like F9)"),
    ("Mouse on a name while paused", "Shows its value"),
    ("Ctrl+Enter in the git panel", "Commit all (open files are saved first)"),
    ("F8 / Shift+F8 in a diff", "Next / previous change (Esc closes the diff)"),
    ("Ctrl+Tab", "Recently used file (hold Ctrl, press Tab repeatedly)"),
    ("Middle click on a tab", "Close tab"),
];

fn table(out: &mut String, rows: &[(String, String)]) {
    let w = rows.iter().map(|(k, _)| k.chars().count()).max().unwrap_or(0);
    for (k, v) in rows {
        out.push_str(&format!("  {k:<w$}   {v}\n"));
    }
    out.push('\n');
}

pub fn cheat_sheet() -> String {
    let mut out = String::from(
        "# Wyvern – Keyboard Shortcuts\n\n\
         Every command is also in the search field: Ctrl+Q, then type >.\n\n",
    );

    out.push_str("## Basics\n\n");
    let basics: Vec<(String, String)> =
        BASICS.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
    table(&mut out, &basics);

    out.push_str("## Commands\n\n");
    let mut cmds: Vec<(String, String)> = palette::commands()
        .into_iter()
        .map(|(label, keys, _)| (keys.to_string(), label.to_string()))
        .collect();
    cmds.sort_by(|a, b| a.1.cmp(&b.1));
    table(&mut out, &cmds);

    out.push_str("## Search field (Ctrl+Q)\n\n");
    table(
        &mut out,
        &[
            ("text".into(), "File in the project (fuzzy, e.g. prgcs → Program.cs)".into()),
            (">text".into(), "Command".into()),
            ("@text".into(), "Symbol in the current file".into()),
            (":42 / :42:7".into(), "Go to line (and column)".into()),
        ],
    );

    out.push_str("## Leader menu (tap Alt)\n\n");
    let mut rows = Vec::new();
    for root in leader::items("") {
        match &root.action {
            Action::Group(g) => {
                for sub in leader::items(g) {
                    let later = matches!(sub.action, Action::Later(_));
                    let label = if later { format!("{} (coming later)", sub.label) } else { sub.label.to_string() };
                    rows.push((format!("Alt → {} → {}", root.key, sub.key), label));
                }
            }
            action => {
                let later = matches!(action, Action::Later(_));
                let label = if later { format!("{} (coming later)", root.label) } else { root.label.to_string() };
                rows.push((format!("Alt → {}", root.key), label));
            }
        }
    }
    table(&mut out, &rows);
    out
}

#[cfg(test)]
mod tests {
    #[test]
    fn cheat_sheet_lists_commands_and_leader_keys() {
        let text = super::cheat_sheet();
        assert!(text.contains("Ctrl+Shift+S"));
        assert!(text.contains("Alt → p → o"));
        assert!(text.contains("Alt → e"));
    }
}
