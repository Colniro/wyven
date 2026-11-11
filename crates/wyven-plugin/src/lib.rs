//! SDK for Wyven plugins.
//!
//! A plugin is a WebAssembly module (`wasm32-unknown-unknown`) that Wyven runs in a sandbox: it
//! sees only what Wyven hands it (the current file's text, its selection, path and language) and
//! answers with [`Action`]s. No files, no network, no programs.
//!
//! ```ignore
//! use wyven_plugin::*;
//!
//! #[derive(Default)]
//! struct Shout;
//!
//! impl Plugin for Shout {
//!     fn info(&self) -> Info {
//!         Info::new("Shout").command("shout.upper", "Shout: Uppercase Selection")
//!     }
//!     fn command(&mut self, _id: &str, editor: &Editor) -> Vec<Action> {
//!         vec![Action::ReplaceSelection { text: editor.selected_text().to_uppercase() }]
//!     }
//! }
//!
//! export_plugin!(Shout);
//! ```
//!
//! Build with `cargo build --release --target wasm32-unknown-unknown` and copy the `.wasm` file
//! to `%APPDATA%\Wyven\plugins`.
//!
//! ## The interface (for other languages)
//!
//! The module exports `memory`, `wyven_alloc(len: i32) -> i32` (memory for a request) and
//! `wyven_handle(ptr: i32, len: i32) -> i64`. Wyven writes a JSON [`Request`] into the
//! allocated memory and calls `wyven_handle`; the result is a JSON [`Response`] at
//! `result >> 32` with length `result & 0xFFFF_FFFF`. The module must not import anything.

use serde::{Deserialize, Serialize};

/// The file the user is working on. Positions are character offsets into `text`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Editor {
    pub path: Option<String>,
    /// "Rust", "C#", "Markdown" …
    pub language: String,
    pub text: String,
    pub selection_start: usize,
    pub selection_end: usize,
}

impl Editor {
    /// The selected text (empty if nothing is selected).
    pub fn selected_text(&self) -> String {
        self.text.chars().skip(self.selection_start).take(self.selection_end.saturating_sub(self.selection_start)).collect()
    }

    pub fn has_selection(&self) -> bool {
        self.selection_end > self.selection_start
    }

    /// The character offset of the start of the line that contains `pos`.
    pub fn line_start(&self, pos: usize) -> usize {
        let before: Vec<char> = self.text.chars().take(pos).collect();
        before.iter().rposition(|&c| c == '\n').map_or(0, |i| i + 1)
    }

    /// The character offset of the end of the line that contains `pos` (before its `\n`).
    pub fn line_end(&self, pos: usize) -> usize {
        self.text.chars().skip(pos).position(|c| c == '\n').map_or(self.text.chars().count(), |i| pos + i)
    }
}

/// What Wyven should do after a command or event.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Action {
    /// Replace the whole file (one undo step).
    ReplaceAll { text: String },
    /// Replace the selection (or insert at the cursor).
    ReplaceSelection { text: String },
    /// Replace characters `start..end`.
    ReplaceRange { start: usize, end: usize, text: String },
    /// Select `start..end` (character offsets).
    SetSelection { start: usize, end: usize },
    /// Show a message in the status bar.
    Message { text: String },
}

/// Moments a plugin can react to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Event {
    /// A file was opened.
    Open,
    /// Another tab became active.
    Switch,
    /// Just before saving: actions change what gets written.
    BeforeSave,
    /// After saving.
    Save,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Command {
    /// Unique, e.g. "text-tools.sort".
    pub id: String,
    /// Shown in the search field, e.g. "Text Tools: Sort Lines".
    pub title: String,
}

/// What a plugin offers.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Info {
    pub name: String,
    #[serde(default)]
    pub commands: Vec<Command>,
    #[serde(default)]
    pub events: Vec<Event>,
}

impl Info {
    pub fn new(name: &str) -> Self {
        Self { name: name.into(), ..Self::default() }
    }

    pub fn command(mut self, id: &str, title: &str) -> Self {
        self.commands.push(Command { id: id.into(), title: title.into() });
        self
    }

    pub fn event(mut self, event: Event) -> Self {
        self.events.push(event);
        self
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Request {
    /// First call after loading: answer with [`Response::info`].
    Init,
    Command { id: String, editor: Editor },
    Event { event: Event, editor: Editor },
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Response {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub info: Option<Info>,
    #[serde(default)]
    pub actions: Vec<Action>,
}

/// A plugin. It lives as long as Wyven runs, so it can keep state between calls.
pub trait Plugin: Default {
    fn info(&self) -> Info;
    fn command(&mut self, id: &str, editor: &Editor) -> Vec<Action>;
    fn event(&mut self, _event: Event, _editor: &Editor) -> Vec<Action> {
        Vec::new()
    }
}

/// Answers one JSON request (used by [`export_plugin!`]; also handy for tests).
pub fn handle<P: Plugin>(plugin: &mut P, request: &[u8]) -> Vec<u8> {
    let response = match serde_json::from_slice::<Request>(request) {
        Ok(Request::Init) => Response { info: Some(plugin.info()), actions: Vec::new() },
        Ok(Request::Command { id, editor }) => Response { info: None, actions: plugin.command(&id, &editor) },
        Ok(Request::Event { event, editor }) => Response { info: None, actions: plugin.event(event, &editor) },
        Err(e) => Response { info: None, actions: vec![Action::Message { text: format!("plugin: bad request: {e}") }] },
    };
    serde_json::to_vec(&response).unwrap_or_default()
}

/// Exports the functions Wyven calls. Use once, with your plugin type.
#[macro_export]
macro_rules! export_plugin {
    ($plugin:ty) => {
        ::std::thread_local! {
            static WYVEN_PLUGIN: ::std::cell::RefCell<$plugin> = ::std::cell::RefCell::new(<$plugin>::default());
            static WYVEN_OUT: ::std::cell::RefCell<::std::vec::Vec<u8>> = ::std::cell::RefCell::new(::std::vec::Vec::new());
        }

        /// Memory for a request of `len` bytes (handed back in `wyven_handle`).
        #[unsafe(no_mangle)]
        pub extern "C" fn wyven_alloc(len: i32) -> i32 {
            let mut buf = ::std::vec::Vec::<u8>::with_capacity(len.max(0) as usize);
            let ptr = buf.as_mut_ptr();
            ::std::mem::forget(buf);
            ptr as i32
        }

        /// Handles the request at `ptr`; returns `(response ptr << 32) | response len`.
        #[unsafe(no_mangle)]
        pub extern "C" fn wyven_handle(ptr: i32, len: i32) -> i64 {
            // SAFETY: Wyven got `ptr` from `wyven_alloc(len)` and wrote exactly `len` bytes.
            let request = unsafe { ::std::vec::Vec::from_raw_parts(ptr as *mut u8, len as usize, len as usize) };
            let response = WYVEN_PLUGIN.with(|p| $crate::handle(&mut *p.borrow_mut(), &request));
            WYVEN_OUT.with(|out| {
                let mut out = out.borrow_mut();
                *out = response;
                ((out.as_ptr() as i64) << 32) | out.len() as i64
            })
        }
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Default)]
    struct Counter {
        calls: usize,
    }

    impl Plugin for Counter {
        fn info(&self) -> Info {
            Info::new("Counter").command("c.count", "Counter: Count").event(Event::Save)
        }
        fn command(&mut self, _id: &str, editor: &Editor) -> Vec<Action> {
            self.calls += 1;
            vec![Action::Message { text: format!("{} chars, call {}", editor.text.chars().count(), self.calls) }]
        }
    }

    #[test]
    fn requests_and_responses_round_trip() {
        let mut p = Counter::default();
        let init: Response = serde_json::from_slice(&handle(&mut p, br#"{"kind":"init"}"#)).unwrap();
        let info = init.info.unwrap();
        assert_eq!(info.commands[0].title, "Counter: Count");
        assert_eq!(info.events, vec![Event::Save]);
        let req = Request::Command { id: "c.count".into(), editor: Editor { text: "äbc".into(), ..Editor::default() } };
        let resp: Response = serde_json::from_slice(&handle(&mut p, &serde_json::to_vec(&req).unwrap())).unwrap();
        assert_eq!(resp.actions, vec![Action::Message { text: "3 chars, call 1".into() }]);
    }

    #[test]
    fn editor_helpers_use_characters() {
        let e = Editor { text: "ab\nçd\nef".into(), selection_start: 3, selection_end: 5, ..Editor::default() };
        assert_eq!(e.selected_text(), "çd");
        assert_eq!(e.line_start(4), 3);
        assert_eq!(e.line_end(4), 5);
        assert_eq!(e.line_end(7), 8);
    }
}
