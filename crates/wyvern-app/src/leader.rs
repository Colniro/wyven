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
    /// Switches to a theme by its settings name ("dark", "light") and stores it.
    SetTheme(&'static str),
    ToggleTheme,
    /// Opens/focuses/closes the terminal panel.
    ToggleTerminal,
    /// Minimap: blocks → bar → off.
    CycleMinimap,
    ToggleStickyScroll,
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

use crate::debug::DebugCmd;
use crate::editor::FoldCmd;
use crate::keymap::Command;
use Action::*;

const ROOT: &[Item] = &[
    item('e', "toggle explorer", ToggleExplorer),
    item('t', "terminal", ToggleTerminal),
    item('s', "search in project", Run(Command::ProjectSearch { replace: false })),
    item('g', "git …", Group("g")),
    item('d', "debug …", Group("d")),
    item('j', "jump …", Group("j")),
    item('c', "code …", Group("c")),
    item('f', "file …", Group("f")),
    item('p', "project …", Group("p")),
    item('w', "window …", Group("w")),
    item('v', "view …", Group("v")),
    item('a', "ai …", Group("a")),
    item('m', "markdown preview", Later("Markdown preview comes in phase 3")),
    item(',', "settings", OpenSettings),
    item('h', "help: all shortcuts", CheatSheet),
];

const FILE: &[Item] = &[
    item('n', "new file", NewFile),
    item('o', "open file", OpenFile),
    item('s', "save as", SaveAs),
    item('c', "close all", CloseAll),
    item('r', "rename", Later("Renaming in the explorer comes in phase 2")),
];

const PROJECT: &[Item] = &[
    item('o', "open folder", OpenFolder),
    item('r', "recent projects", RecentProjects),
    item('e', "focus explorer", FocusExplorer),
    item('a', "add folder", Later("Multiple folders come in phase 4")),
];

const GIT: &[Item] = &[
    item('g', "git panel", Git(GitCmd::Panel)),
    item('c', "commit all", Git(GitCmd::Commit)),
    item('s', "sync (pull + push)", Git(GitCmd::Sync)),
    item('b', "switch branch", Git(GitCmd::Branch)),
    item('n', "new branch", Git(GitCmd::NewBranch)),
    item('d', "diff current file", Git(GitCmd::Diff)),
    item('l', "line blame", Git(GitCmd::Blame)),
];

const DEBUG: &[Item] = &[
    item('d', "debug panel", Run(Command::Debug(DebugCmd::ShowPanel))),
    item('r', "run / continue (F5)", Run(Command::Debug(DebugCmd::StartOrContinue))),
    item('s', "stop (Shift+F5)", Run(Command::Debug(DebugCmd::Stop))),
    item('b', "toggle breakpoint (F9)", Run(Command::Debug(DebugCmd::ToggleBreakpoint))),
    item('w', "add watch", Run(Command::Debug(DebugCmd::AddWatch))),
    item('a', "attach to process", Run(Command::Debug(DebugCmd::Attach))),
];

const JUMP: &[Item] = &[
    item('d', "definition", Run(Command::GotoDefinition)),
    item('i', "implementation", Later("Comes with the C# language server")),
    item('r', "references", Later("Comes with the C# language server")),
    item('s', "symbol in file", Palette("@")),
    item('l', "line", Palette(":")),
    item('f', "file", Palette("")),
];

const CODE: &[Item] = &[
    item('r', "rename", Run(Command::Rename)),
    item('f', "format", Run(Command::Format)),
    item('a', "quick actions", Run(Command::QuickActions)),
    item('o', "organize usings", Later("Organize usings comes in phase 3")),
    item('e', "next error", Run(Command::NextProblem)),
];

const WINDOW: &[Item] = &[
    item('v', "split vertically", Later("Split views come in phase 2")),
    item('h', "split horizontally", Later("Split views come in phase 2")),
    item('c', "close split", Later("Split views come in phase 2")),
];

const VIEW: &[Item] = &[
    item('m', "minimap: blocks / bar / off", CycleMinimap),
    item('s', "sticky scroll on/off", ToggleStickyScroll),
    item('i', "inlay hints", Later("Inlay hints come in phase 3")),
    item('w', "word wrap", Later("Word wrap comes in phase 2")),
    item('z', "zen mode", Later("Zen mode comes in phase 4")),
    item('t', "toggle light/dark theme", ToggleTheme),
    item('f', "fold / unfold at cursor", Run(Command::Fold(FoldCmd::Toggle))),
    item('a', "fold all / unfold all", Run(Command::Fold(FoldCmd::ToggleAll))),
    item('d', "fold to definitions", Run(Command::Fold(FoldCmd::Definitions))),
    item('e', "expand all folds", Run(Command::Fold(FoldCmd::ExpandAll))),
];

const AI: &[Item] = &[
    item('t', "toggle suggestions", Later("AI suggestions come in phase 3")),
    item('m', "switch model", Later("AI suggestions come in phase 3")),
    item('k', "set API key", Later("AI suggestions come in phase 3")),
    item('c', "claude code", Later("Claude Code in the terminal comes in phase 2/3")),
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
