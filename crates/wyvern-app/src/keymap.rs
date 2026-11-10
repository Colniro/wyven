//! Turns key presses into commands, following SHORTCUTS.md.
//!
//! Shortcuts match the key *without* modifiers applied (`Strg+Shift+L` is "l", not "L"), so
//! they work the same on every keyboard layout. Typed text comes from the OS separately,
//! which keeps AltGr characters like `{` `@` `\` on a German keyboard working.

use winit::event::KeyEvent;
use winit::keyboard::{Key, ModifiersState, NamedKey};
use winit::platform::modifier_supplement::KeyEventExtModifierSupplement;
use wyvern_buffer::{CommentMode, Direction, Movement};

use crate::debug::DebugCmd;
use crate::editor::FoldCmd;

#[derive(Debug, Clone, PartialEq)]
pub enum Command {
    Move(Movement, bool),
    Insert(String),
    Newline,
    NewlineBelow,
    NewlineAbove,
    Indent,
    Outdent,
    Backspace,
    Delete,
    DeleteWord { forward: bool },
    Undo,
    Redo,
    Copy,
    Cut,
    Paste,
    SelectAll,
    AddNextOccurrence,
    Collapse,
    MoveLines(Direction),
    DuplicateLines(Direction),
    DeleteLines,
    Comment(CommentMode),
    Save,
    SaveAs,
    OpenFile,
    NewFile,
    CloseTab,
    /// Strg+Tab (forward) / Strg+Shift+Tab (backward) through recently used tabs.
    CycleTabs { back: bool },
    ReopenClosed,
    /// Opens the search field with a mode prefix ("" files, ">" commands, "@" symbols, ":" line).
    OpenPalette(&'static str),
    TriggerCompletion,
    GotoDefinition,
    Format,
    /// Strg+K Strg+I: information about the symbol under the cursor.
    QuickInfo,
    Rename,
    QuickActions,
    /// Strg+F / Strg+H: find (and replace) in the file.
    Find { replace: bool },
    /// F3 / Shift+F3.
    FindNext { back: bool },
    /// Strg+Shift+F / Strg+Shift+H: search (and replace) in the project.
    ProjectSearch { replace: bool },
    NextProblem,
    Scroll(isize),
    FontSize(i32),
    FontSizeReset,
    /// Ctrl+M chords: code folding.
    Fold(FoldCmd),
    Debug(DebugCmd),
}

/// Keeps state between key presses: `Ctrl+K` and `Ctrl+M` chords.
#[derive(Default)]
pub struct Keymap {
    /// First key of a pending chord ('k' or 'm').
    chord: Option<char>,
}

impl Keymap {
    pub fn chord_pending(&self) -> Option<char> {
        self.chord
    }

    pub fn handle(&mut self, event: &KeyEvent, mods: ModifiersState) -> Option<Command> {
        let ctrl = mods.control_key();
        let shift = mods.shift_key();
        let alt = mods.alt_key();
        let key = event.key_without_modifiers();
        log::debug!("key {key:?} text {:?} mods {mods:?}", event.text);

        // Pressing a modifier alone (e.g. Strg for the second half of Strg+K, Strg+C) is not
        // a command and must not cancel a pending chord.
        if matches!(
            key,
            Key::Named(
                NamedKey::Control
                    | NamedKey::Shift
                    | NamedKey::Alt
                    | NamedKey::AltGraph
                    | NamedKey::Super
                    | NamedKey::Meta
            )
        ) {
            return None;
        }

        if let Some(first) = self.chord.take() {
            if let Key::Character(c) = &key {
                return match (first, c.as_str()) {
                    ('k', "c") => Some(Command::Comment(CommentMode::Add)),
                    ('k', "u") => Some(Command::Comment(CommentMode::Remove)),
                    ('k', "d") => Some(Command::Format),
                    ('k', "i") => Some(Command::QuickInfo),
                    ('m', "m") => Some(Command::Fold(FoldCmd::Toggle)),
                    ('m', "l") => Some(Command::Fold(FoldCmd::ToggleAll)),
                    ('m', "o") => Some(Command::Fold(FoldCmd::Definitions)),
                    ('m', "p") => Some(Command::Fold(FoldCmd::ExpandAll)),
                    _ => None,
                };
            }
            return None;
        }

        use Command::*;
        use Movement as M;
        let cmd = match &key {
            Key::Named(named) => match named {
                NamedKey::ArrowLeft if ctrl => Move(M::WordLeft, shift),
                NamedKey::ArrowRight if ctrl => Move(M::WordRight, shift),
                NamedKey::ArrowLeft => Move(M::Left, shift),
                NamedKey::ArrowRight => Move(M::Right, shift),
                NamedKey::ArrowUp if alt && shift => DuplicateLines(Direction::Up),
                NamedKey::ArrowDown if alt && shift => DuplicateLines(Direction::Down),
                NamedKey::ArrowUp if alt => MoveLines(Direction::Up),
                NamedKey::ArrowDown if alt => MoveLines(Direction::Down),
                NamedKey::ArrowUp if ctrl => Scroll(-1),
                NamedKey::ArrowDown if ctrl => Scroll(1),
                NamedKey::ArrowUp => Move(M::Up, shift),
                NamedKey::ArrowDown => Move(M::Down, shift),
                NamedKey::Home if ctrl => Move(M::DocStart, shift),
                NamedKey::End if ctrl => Move(M::DocEnd, shift),
                NamedKey::Home => Move(M::LineStart, shift),
                NamedKey::End => Move(M::LineEnd, shift),
                // Page size is filled in by the editor, which knows how many lines are visible.
                NamedKey::PageUp => Move(M::PageUp(0), shift),
                NamedKey::PageDown => Move(M::PageDown(0), shift),
                NamedKey::Backspace if ctrl => DeleteWord { forward: false },
                NamedKey::Delete if ctrl => DeleteWord { forward: true },
                NamedKey::Backspace => Backspace,
                NamedKey::Delete => Delete,
                NamedKey::Enter if ctrl && shift => NewlineAbove,
                NamedKey::Enter if ctrl => NewlineBelow,
                NamedKey::Enter => Newline,
                NamedKey::Tab if ctrl => CycleTabs { back: shift },
                NamedKey::F4 if ctrl => CloseTab,
                NamedKey::Tab if shift => Outdent,
                NamedKey::Tab => Indent,
                NamedKey::Escape => Collapse,
                NamedKey::F5 if shift => Debug(DebugCmd::Stop),
                NamedKey::F5 if ctrl => Debug(DebugCmd::RunWithoutDebugging),
                NamedKey::F5 => Debug(DebugCmd::StartOrContinue),
                NamedKey::F9 => Debug(DebugCmd::ToggleBreakpoint),
                NamedKey::F10 => Debug(DebugCmd::StepOver),
                NamedKey::F11 if shift => Debug(DebugCmd::StepOut),
                NamedKey::F11 => Debug(DebugCmd::StepInto),
                NamedKey::F12 => GotoDefinition,
                NamedKey::F2 => Rename,
                NamedKey::F8 => NextProblem,
                NamedKey::F3 => FindNext { back: shift },
                NamedKey::Space if ctrl => TriggerCompletion,
                _ => return None,
            },
            Key::Character(c) if ctrl && !alt => match (c.as_str(), shift) {
                ("c", false) => Copy,
                ("x", false) => Cut,
                ("v", false) => Paste,
                ("z", false) => Undo,
                ("z", true) | ("y", false) => Redo,
                ("s", false) => Save,
                ("s", true) => SaveAs,
                ("o", false) => OpenFile,
                ("n", false) => NewFile,
                ("w", false) => CloseTab,
                ("t", true) => ReopenClosed,
                ("a", false) => SelectAll,
                ("d", false) => AddNextOccurrence,
                ("l", true) => DeleteLines,
                ("#", false) => Comment(CommentMode::Toggle),
                ("+", _) => FontSize(1),
                ("-", false) => FontSize(-1),
                ("0", false) => FontSizeReset,
                ("k", false) => {
                    self.chord = Some('k');
                    return None;
                }
                ("m", false) => {
                    self.chord = Some('m');
                    return None;
                }
                ("q", false) => OpenPalette(""),
                ("p", true) => OpenPalette(">"),
                ("g", false) => OpenPalette(":"),
                ("f", false) => Find { replace: false },
                ("h", false) => Find { replace: true },
                ("f", true) => ProjectSearch { replace: false },
                ("h", true) => ProjectSearch { replace: true },
                (" ", _) => TriggerCompletion,
                (".", false) => QuickActions,
                _ => return None,
            },
            _ => return None,
        };
        Some(cmd)
    }

    /// Printable text of a key press, or `None` for shortcuts. AltGr (Strg+Alt on Windows)
    /// still produces text.
    pub fn text(event: &KeyEvent, mods: ModifiersState) -> Option<String> {
        let ctrl_only = mods.control_key() && !mods.alt_key();
        if ctrl_only || (mods.alt_key() && !mods.control_key()) {
            return None;
        }
        let text = event.text.as_ref()?;
        if text.chars().any(char::is_control) {
            return None;
        }
        Some(text.to_string())
    }
}
