# Writing Wyven Plugins

A Wyven plugin is a WebAssembly module. Wyven runs it with [wasmi](https://github.com/wasmi-labs/wasmi)
in a sandbox: the module may not import anything, so it cannot read files, use the network or
start programs. It only receives what Wyven sends – the active file's text, selection, path and
language – and answers with actions. Each call has a computing budget and memory is limited; a
plugin that hangs or crashes is stopped until *Plugins: Reload Plugins*.

## A plugin in Rust

```toml
# Cargo.toml
[package]
name = "shout"
version = "0.1.0"
edition = "2024"

[lib]
crate-type = ["cdylib"]

[dependencies]
wyven-plugin = { path = "path/to/wyven/crates/wyven-plugin" }

[profile.release]
opt-level = "s"
lto = true
strip = true
panic = "abort"
```

```rust
// src/lib.rs
use wyven_plugin::*;

#[derive(Default)]
struct Shout {
    count: usize, // plugins keep their state while Wyven runs
}

impl Plugin for Shout {
    fn info(&self) -> Info {
        Info::new("Shout")
            .command("shout.upper", "Shout: Uppercase Selection")
            .event(Event::Save)
    }

    fn command(&mut self, _id: &str, editor: &Editor) -> Vec<Action> {
        if !editor.has_selection() {
            return vec![Action::Message { text: "Select some text first".into() }];
        }
        vec![Action::ReplaceSelection { text: editor.selected_text().to_uppercase() }]
    }

    fn event(&mut self, _event: Event, _editor: &Editor) -> Vec<Action> {
        self.count += 1;
        vec![Action::Message { text: format!("Saved {} times", self.count) }]
    }
}

export_plugin!(Shout);
```

Build and install:

```
rustup target add wasm32-unknown-unknown
cargo build --release --target wasm32-unknown-unknown
copy target\wasm32-unknown-unknown\release\shout.wasm %APPDATA%\Wyven\plugins\
```

Then *Plugins: Reload Plugins* (or restart Wyven). The commands appear in the search field
(`Ctrl+Q`, `>`). A complete example is [`plugins/text-tools`](../plugins/text-tools).

## What a plugin gets

`Editor`: `path` (if saved), `language` ("Rust", "C#", "Markdown" …), `text`, and
`selection_start` / `selection_end` as **character** offsets (not bytes). Helpers:
`selected_text()`, `has_selection()`, `line_start(pos)`, `line_end(pos)`.

Events: `Open` (a file was opened), `Switch` (another tab became active), `BeforeSave` (actions
change what gets written), `Save`.

## What a plugin can do

| Action | Effect |
|---|---|
| `ReplaceAll { text }` | replace the whole file |
| `ReplaceSelection { text }` | replace the selection (or insert at the cursor) |
| `ReplaceRange { start, end, text }` | replace characters `start..end` |
| `SetSelection { start, end }` | select characters `start..end` |
| `Message { text }` | show a message in the status bar |

Every edit is one undo step.

## Other languages

The SDK is a convenience; any language that compiles to WebAssembly works. The module must
export:

- `memory`
- `wyven_alloc(len: i32) -> i32` – memory for a request of `len` bytes
- `wyven_handle(ptr: i32, len: i32) -> i64` – handles the JSON request at `ptr` and returns
  `(response_ptr << 32) | response_len`

Requests (JSON): `{"kind":"init"}`, `{"kind":"command","id":"…","editor":{…}}`,
`{"kind":"event","event":"before_save","editor":{…}}`. The answer to `init` is
`{"info":{"name":"…","commands":[{"id":"…","title":"…"}],"events":["save"]}}`, to the others
`{"actions":[{"type":"replace_selection","text":"…"}]}`. The exact types are in
`crates/wyven-plugin/src/lib.rs`.
