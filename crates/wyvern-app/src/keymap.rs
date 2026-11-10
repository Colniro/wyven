//! Turns key presses into commands, following SHORTCUTS.md.
//!
//! Shortcuts match the key *without* modifiers applied (`Strg+Shift+L` is "l", not "L"), so
//! they work the same on every keyboard layout. Typed text comes from the OS separately,
//! which keeps AltGr characters like `{` `@` `\` on a German keyboard working.

use winit::event::KeyEvent;
use winit::keyboard::{Key, ModifiersState, NamedKey};
use winit::platform::modifier_supplement::KeyEventExtModifierSupplement;
use wyvern_buffer::{CommentMode, Direction, Movement};

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
    NextProblem,
    Scroll(isize),
    FontSize(i32),
    FontSizeReset,
    /// Recognized but not built yet; the message says which phase brings it.
    NotYet(&'static str),
}

/// Keeps state between key presses: `Strg+K` chords.
#[derive(Default)]
pub struct Keymap {
    chord: bool,
}

impl Keymap {
    pub fn chord_pending(&self) -> bool {
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

        if self.chord {
            self.chord = false;
            if let Key::Character(c) = &key {
                return match (c.as_str(), ctrl) {
                    ("c", _) => Some(Command::Comment(CommentMode::Add)),
                    ("u", _) => Some(Command::Comment(CommentMode::Remove)),
                    ("d", _) => Some(Command::Format),
                    ("i", _) => Some(Command::QuickInfo),
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
                NamedKey::F5 | NamedKey::F9 | NamedKey::F10 | NamedKey::F11 => {
                    NotYet("Debuggen kommt in Phase 3")
                }
                NamedKey::F12 => GotoDefinition,
                NamedKey::F2 => Rename,
                NamedKey::F8 => NextProblem,
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
                    self.chord = true;
                    return None;
                }
                ("q", false) => OpenPalette(""),
                ("p", true) => OpenPalette(">"),
                ("g", false) => OpenPalette(":"),
                ("f", _) | ("h", false) => NotYet("Suchen und Ersetzen kommt in Phase 2"),
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
