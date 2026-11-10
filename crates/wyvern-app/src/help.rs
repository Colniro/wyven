//! The shortcut cheat sheet (`Alt → h`), generated from the same tables the search field and
//! the leader menu use, so it never goes out of date.

use crate::leader::{self, Action};
use crate::palette;

/// Basics that are not commands of their own (typing, moving, mouse).
const BASICS: &[(&str, &str)] = &[
    ("Pfeiltasten", "Cursor bewegen (Strg = wortweise, Shift = auswählen)"),
    ("Pos1 / Ende", "Zeilenanfang (erst Text, dann Spalte 1) / Zeilenende"),
    ("Strg+Pos1 / Strg+Ende", "Dateianfang / Dateiende"),
    ("Bild↑ / Bild↓", "Seite hoch / runter"),
    ("Tab / Shift+Tab", "Einrücken / Ausrücken (mehrere Zeilen: alle)"),
    ("Enter", "Neue Zeile mit Einrückung; zwischen {} auf eigene Zeile"),
    ("Strg+Enter / Strg+Shift+Enter", "Neue Zeile darunter / darüber"),
    ("Strg+Backspace / Strg+Entf", "Wort löschen"),
    ("Strg+C / X / V", "Kopieren / Ausschneiden / Einfügen (ohne Auswahl: ganze Zeile)"),
    ("Esc", "Zurück zu einem Cursor; Popups schließen"),
    ("Alt+Klick", "Weiteren Cursor setzen / entfernen"),
    ("Doppelklick", "Wort auswählen"),
    ("Strg+Klick / F12", "Gehe zu Definition"),
    ("Strg+Leertaste", "Autovervollständigung"),
    ("Strg+K, Strg+I", "Info zum Symbol (oder Maus kurz stillhalten)"),
    ("F2", "Umbenennen"),
    ("Strg+.", "Quick Actions"),
    ("F8", "Nächster Fehler"),
    ("Strg+Enter im Git-Panel", "Alles committen (offene Dateien werden vorher gespeichert)"),
    ("F8 / Shift+F8 im Diff", "Nächste / vorige Änderung (Esc schließt den Diff)"),
    ("Strg+Tab", "Zuletzt benutzte Datei (Strg halten, Tab mehrmals)"),
    ("Mittlere Maustaste auf Tab", "Tab schließen"),
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
        "# Wyvern – Tastenkürzel\n\n\
         Alle Befehle findest du auch im Suchfeld: Strg+Q und dann > tippen.\n\n",
    );

    out.push_str("## Grundlagen\n\n");
    let basics: Vec<(String, String)> =
        BASICS.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
    table(&mut out, &basics);

    out.push_str("## Befehle\n\n");
    let mut cmds: Vec<(String, String)> = palette::commands()
        .into_iter()
        .map(|(label, keys, _)| (keys.to_string(), label.to_string()))
        .collect();
    cmds.sort_by(|a, b| a.1.cmp(&b.1));
    table(&mut out, &cmds);

    out.push_str("## Suchfeld (Strg+Q)\n\n");
    table(
        &mut out,
        &[
            ("text".into(), "Datei im Projekt (unscharf, z. B. prgcs → Program.cs)".into()),
            (">text".into(), "Befehl".into()),
            ("@text".into(), "Symbol in der aktuellen Datei".into()),
            (":42 / :42:7".into(), "Gehe zu Zeile (und Spalte)".into()),
        ],
    );

    out.push_str("## Leader-Menü (Alt kurz antippen)\n\n");
    let mut rows = Vec::new();
    for root in leader::items("") {
        match &root.action {
            Action::Group(g) => {
                for sub in leader::items(g) {
                    let later = matches!(sub.action, Action::Later(_));
                    let label = if later { format!("{} (kommt später)", sub.label) } else { sub.label.to_string() };
                    rows.push((format!("Alt → {} → {}", root.key, sub.key), label));
                }
            }
            action => {
                let later = matches!(action, Action::Later(_));
                let label = if later { format!("{} (kommt später)", root.label) } else { root.label.to_string() };
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
        assert!(text.contains("Strg+Shift+S"));
        assert!(text.contains("Alt → p → o"));
        assert!(text.contains("Alt → e"));
    }
}
