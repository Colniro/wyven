//! Text buffer for Wyven.
//!
//! - [`ropey::Rope`] stores the text, so edits stay fast even in huge files.
//! - All positions are **char indices** into the rope. Line endings are normalized to `\n`
//!   in memory and restored on save (see [`LineEnding`]).
//! - Every edit is a [`Transaction`]: a set of non-overlapping changes applied at once. That is
//!   what makes multi-cursor editing and undo simple: one transaction per user action.

mod document;
mod history;
mod selection;
mod text;
mod transaction;

pub use document::{CommentMode, Direction, Document, LineEnding, Movement};
pub use ropey::{Rope, RopeSlice};
pub use selection::{Range, Selection};
pub use text::{char_to_display_col, display_col_to_char, line_len_chars};
pub use transaction::{Assoc, Change, TextEdit, Transaction};
