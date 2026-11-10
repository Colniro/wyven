//! The leader menu: tap `Alt`, then type letters (SHORTCUTS.md, "Leader-Menü").

/// What a leader key does.
#[derive(Debug, Clone, PartialEq)]
pub enum Action {
    /// Opens a sub-menu.
    Group(&'static str),
    ToggleExplorer,
    FocusExplorer,
    OpenFolder,
    RecentProjects,
    OpenFile,
    NewFile,
    SaveAs,
    CloseAll,
    /// Opens the search field with a mode prefix.
    Palette(&'static str),
    /// Runs an editor command (rename, format, …).
    Run(crate::keymap::Command),
    /// Known, but built in a later phase.
    Later(&'static str),
    OpenSettings,
    CheatSheet,
    /// Switches to a theme by its settings name ("dunkel", "hell") and stores it.
    SetTheme(&'static str),
    ToggleTheme,
    /// Opens/focuses/closes the terminal panel.
    ToggleTerminal,
    Git(GitCmd),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GitCmd {
    Panel,
    Commit,
    Sync,
    Branch,
    NewBranch,
    Diff,
    Blame,
}

pub struct Item {
    pub key: char,
    pub label: &'static str,
    pub action: Action,
}

const fn item(key: char, label: &'static str, action: Action) -> Item {
    Item { key, label, action }
}

use crate::keymap::Command;
use Action::*;

const ROOT: &[Item] = &[
    item('e', "Explorer ein/aus", ToggleExplorer),
    item('t', "Terminal", ToggleTerminal),
    item('s', "Suche im Projekt", Run(Command::ProjectSearch { replace: false })),
    item('g', "git …", Group("g")),
    item('d', "debug …", Group("d")),
    item('j', "jump …", Group("j")),
    item('c', "code …", Group("c")),
    item('f', "file …", Group("f")),
    item('p', "project …", Group("p")),
    item('w', "window …", Group("w")),
    item('v', "view …", Group("v")),
    item('a', "ai …", Group("a")),
    item('m', "Markdown-Vorschau", Later("Die Markdown-Vorschau kommt in Phase 3")),
    item(',', "Einstellungen", OpenSettings),
    item('h', "Hilfe: alle Kürzel", CheatSheet),
];

const FILE: &[Item] = &[
    item('n', "neue Datei", NewFile),
    item('o', "Datei öffnen", OpenFile),
    item('s', "speichern unter", SaveAs),
    item('w', "alle schließen", CloseAll),
    item('r', "umbenennen", Later("Umbenennen im Explorer kommt in Phase 2")),
];

const PROJECT: &[Item] = &[
    item('o', "Ordner öffnen", OpenFolder),
    item('r', "zuletzt geöffnet", RecentProjects),
    item('e', "Explorer fokussieren", FocusExplorer),
    item('a', "Ordner hinzufügen", Later("Mehrere Ordner kommen in Phase 4")),
];

const GIT: &[Item] = &[
    item('g', "Git-Panel", Git(GitCmd::Panel)),
    item('c', "alles committen", Git(GitCmd::Commit)),
    item('s', "Sync (pull + push)", Git(GitCmd::Sync)),
    item('b', "Branch wechseln", Git(GitCmd::Branch)),
    item('n', "neuer Branch", Git(GitCmd::NewBranch)),
    item('d', "Diff der Datei", Git(GitCmd::Diff)),
    item('l', "Blame der Zeile", Git(GitCmd::Blame)),
];

const DEBUG: &[Item] = &[
    item('d', "Debug-Panel", Later("Debuggen kommt in Phase 3")),
    item('b', "alle Breakpoints", Later("Debuggen kommt in Phase 3")),
    item('w', "Watch hinzufügen", Later("Debuggen kommt in Phase 3")),
    item('a', "an Prozess anhängen", Later("Debuggen kommt in Phase 3")),
];

const JUMP: &[Item] = &[
    item('d', "Definition", Run(Command::GotoDefinition)),
    item('i', "Implementierung", Later("Kommt mit dem C#-Language-Server")),
    item('r', "Referenzen", Later("Kommt mit dem C#-Language-Server")),
    item('s', "Symbol in Datei", Palette("@")),
    item('l', "Zeile", Palette(":")),
    item('f', "Datei", Palette("")),
];

const CODE: &[Item] = &[
    item('r', "umbenennen", Run(Command::Rename)),
    item('f', "formatieren", Run(Command::Format)),
    item('a', "Quick Actions", Run(Command::QuickActions)),
    item('o', "Usings ordnen", Later("Usings ordnen kommt in Phase 3")),
    item('e', "nächster Fehler", Run(Command::NextProblem)),
];

const WINDOW: &[Item] = &[
    item('v', "rechts teilen", Later("Geteilte Ansichten kommen in Phase 2")),
    item('h', "unten teilen", Later("Geteilte Ansichten kommen in Phase 2")),
    item('c', "Teilung schließen", Later("Geteilte Ansichten kommen in Phase 2")),
];

const VIEW: &[Item] = &[
    item('m', "Minimap", Later("Die Minimap kommt in Phase 2")),
    item('s', "Sticky Scroll", Later("Sticky Scroll kommt in Phase 2")),
    item('i', "Inlay Hints", Later("Inlay Hints kommen in Phase 3")),
    item('w', "Zeilenumbruch", Later("Zeilenumbruch kommt in Phase 2")),
    item('z', "Zen-Modus", Later("Der Zen-Modus kommt in Phase 4")),
    item('t', "Theme hell/dunkel", ToggleTheme),
];

const AI: &[Item] = &[
    item('t', "Vorschläge an/aus", Later("KI-Vorschläge kommen in Phase 3")),
    item('m', "Modell wechseln", Later("KI-Vorschläge kommen in Phase 3")),
    item('k', "API-Key setzen", Later("KI-Vorschläge kommen in Phase 3")),
    item('c', "Claude Code", Later("Claude Code im Terminal kommt in Phase 2/3")),
];

pub fn items(group: &str) -> &'static [Item] {
    match group {
        "f" => FILE,
        "p" => PROJECT,
        "g" => GIT,
        "d" => DEBUG,
        "j" => JUMP,
        "c" => CODE,
        "w" => WINDOW,
        "v" => VIEW,
        "a" => AI,
        _ => ROOT,
    }
}

pub fn title(group: &str) -> String {
    let name = match group {
        "f" => "file",
        "p" => "project",
        "g" => "git",
        "d" => "debug",
        "j" => "jump",
        "c" => "code",
        "w" => "window",
        "v" => "view",
        "a" => "ai",
        _ => return "Alt →".into(),
    };
    format!("Alt → {group}  ({name})")
}

/// Looks up a key in a group.
pub fn find(group: &str, key: char) -> Option<Action> {
    items(group)
        .iter()
        .find(|i| i.key == key.to_ascii_lowercase())
        .map(|i| i.action.clone())
}
