//! The leader menu: tap `Alt`, then type letters (SHORTCUTS.md, "Leader-Menü").

/// What a leader key does.
#[derive(Debug, Clone, PartialEq)]
pub enum Action {
    /// Opens a sub-menu.
    Group(&'static str),
    ToggleExplorer,
    FocusExplorer,
    OpenFolder,
    /// Rename / delete / create in the explorer (or the active file).
    ExplorerOp(crate::ExplorerOp),
    /// Another folder in the same window.
    AddFolder,
    /// A VS Code `.code-workspace` file.
    OpenWorkspace,
    SaveWorkspace,
    RecentProjects,
    OpenFile,
    NewFile,
    /// An unnamed tab (saved somewhere later).
    NewUntitled,
    SaveAs,
    CloseAll,
    /// Opens the search field with a mode prefix.
    Palette(&'static str),
    /// Runs an editor command (rename, format, …).
    Run(crate::keymap::Command),
    /// The settings page (a tab).
    OpenSettings,
    /// Loads the plugins folder again.
    ReloadPlugins,
    /// Looks for a new Wyven version now.
    CheckUpdates,
    /// Opens a new GitHub issue with version and system filled in (and the last crash).
    ReportBug,
    ShowPlugins,
    OpenPluginsFolder,
    /// keymap.toml: your own shortcuts.
    OpenKeymap,
    /// Own snippets for the current language (VS Code format).
    OpenSnippets,
    /// settings.toml itself.
    OpenSettingsFile,
    CheatSheet,
    /// Switches to a theme by its settings name ("dark", "light") and stores it.
    SetTheme(&'static str),
    ToggleTheme,
    /// Picker with all themes (Wyven's and VS Code's), previewing while moving.
    ChooseTheme,
    /// Opens/focuses/closes the terminal panel.
    ToggleTerminal,
    NewTerminal,
    CloseTerminal,
    NextTerminal,
    PreviousTerminal,
    /// Minimap: blocks → bar → off.
    CycleMinimap,
    ToggleStickyScroll,
    /// Only the text, fullscreen.
    ToggleZen,
    ToggleWordWrap,
    ToggleInlayHints,
    ToggleCodeLens,
    /// Panels back to their first places and sizes.
    ResetLayout,
    /// The color picker for the color at the cursor (CSS, HTML, XAML).
    PickColor,
    /// Saved panel layouts: switch, save the current one, delete.
    Layouts,
    /// Two editors side by side (`vertical`) or stacked.
    Split { vertical: bool },
    FocusOtherHalf,
    CloseSplit,
    /// Alt → m: live preview of a Markdown file next to it.
    MarkdownPreview,
    AiToggle,
    AiModel,
    AiKey,
    /// Starts Claude Code in Wyven's terminal (it connects to the bridge there).
    ClaudeCode,
    Git(GitCmd),
    /// Build / test / run / clean the project.
    Task(crate::tasks::TaskKind),
    StopTask,
    ShowProblems,
    ShowOutput,
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
    item('r', "run & build …", Group("r")),
    item('m', "markdown preview", MarkdownPreview),
    item(',', "settings", OpenSettings),
    item('h', "help: all shortcuts", CheatSheet),
];

const FILE: &[Item] = &[
    item('n', "new file", NewFile),
    item('o', "open file", OpenFile),
    item('s', "save as", SaveAs),
    item('c', "close all", CloseAll),
    item('r', "rename file …", ExplorerOp(crate::ExplorerOp::Rename)),
    item('d', "delete file", ExplorerOp(crate::ExplorerOp::Delete)),
    item('f', "new file in explorer folder …", ExplorerOp(crate::ExplorerOp::NewFile)),
    item('k', "new folder …", ExplorerOp(crate::ExplorerOp::NewFolder)),
];

const PROJECT: &[Item] = &[
    item('o', "open folder", OpenFolder),
    item('r', "recent projects", RecentProjects),
    item('e', "focus explorer", FocusExplorer),
    item('a', "add folder to workspace", AddFolder),
    item('f', "open workspace file …", OpenWorkspace),
    item('w', "save workspace as …", SaveWorkspace),
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
    item('i', "implementation", Run(Command::GotoImplementation)),
    item('r', "references", Run(Command::FindReferences)),
    item('s', "symbol in file", Palette("@")),
    item('l', "line", Palette(":")),
    item('f', "file", Palette("")),
];

const CODE: &[Item] = &[
    item('r', "rename", Run(Command::Rename)),
    item('f', "format", Run(Command::Format)),
    item('a', "quick actions", Run(Command::QuickActions)),
    item('o', "organize usings / imports", Run(Command::OrganizeUsings)),
    item('e', "next error", Run(Command::NextProblem)),
    item('k', "pick color at cursor", PickColor),
];

const WINDOW: &[Item] = &[
    item('v', "split right", Split { vertical: true }),
    item('h', "split down", Split { vertical: false }),
    item('o', "other half (or ←↑→↓)", FocusOtherHalf),
    item('c', "close split", CloseSplit),
    item('r', "reset panel layout", ResetLayout),
    item('l', "layouts: switch / save …", Layouts),
];

const VIEW: &[Item] = &[
    item('m', "minimap: blocks / bar / off", CycleMinimap),
    item('s', "sticky scroll on/off", ToggleStickyScroll),
    item('i', "inlay hints on/off", ToggleInlayHints),
    item('l', "code lens on/off", ToggleCodeLens),
    item('w', "word wrap on/off", ToggleWordWrap),
    item('z', "zen mode", ToggleZen),
    item('t', "toggle light/dark theme", ToggleTheme),
    item('c', "color theme …", ChooseTheme),
    item('f', "fold / unfold at cursor", Run(Command::Fold(FoldCmd::Toggle))),
    item('a', "fold all / unfold all", Run(Command::Fold(FoldCmd::ToggleAll))),
    item('d', "fold to definitions", Run(Command::Fold(FoldCmd::Definitions))),
    item('e', "expand all folds", Run(Command::Fold(FoldCmd::ExpandAll))),
];

const RUN: &[Item] = &[
    item('b', "build (Ctrl+Shift+B)", Task(crate::tasks::TaskKind::Build)),
    item('t', "test", Task(crate::tasks::TaskKind::Test)),
    item('r', "run in a terminal", Task(crate::tasks::TaskKind::Run)),
    item('c', "clean", Task(crate::tasks::TaskKind::Clean)),
    item('x', "stop the running task", StopTask),
    item('p', "problems", ShowProblems),
    item('o', "task output", ShowOutput),
];

const AI: &[Item] = &[
    item('t', "suggestions on/off", AiToggle),
    item('m', "switch model (Haiku / Sonnet)", AiModel),
    item('k', "set API key", AiKey),
    item('c', "claude code in the terminal", ClaudeCode),
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
        "r" => RUN,
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
        "r" => "run & build",
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
