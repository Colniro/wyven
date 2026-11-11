# Wyven User Guide

Wyven is a fast, lean code editor for Windows, written in Rust. It looks and feels like VS Code
and keeps the Visual Studio habits that live in your fingers (F5, F9–F12, `Ctrl+.`).

- [Install and update](#install-and-update)
- [First steps](#first-steps)
- [The leader key](#the-leader-key)
- [Editing](#editing)
- [Finding things](#finding-things)
- [Panels and layout](#panels-and-layout)
- [Workspaces with several folders](#workspaces-with-several-folders)
- [Languages](#languages)
- [Git](#git)
- [Terminal](#terminal)
- [Debugging (.NET)](#debugging-net)
- [AI suggestions](#ai-suggestions)
- [Claude Code](#claude-code)
- [Appearance](#appearance)
- [Settings](#settings)
- [Plugins](#plugins)
- [Files Wyven keeps](#files-wyven-keeps)

## Install and update

Run `wyven-setup-<version>.exe`. It installs for your user only (no admin rights) into
`%LOCALAPPDATA%\Programs\Wyven` and can add:

- a Start menu entry and optionally a desktop icon,
- **Open with Wyven** in the Explorer context menu (files, folders, folder background),
- `wyven` on the PATH, so `wyven .` opens the current folder from any terminal,
- opening `.code-workspace` files with Wyven.

Uninstall it from *Settings → Apps*. Your settings in `%APPDATA%\Wyven` stay.

**Updates:** set *Update address* on the settings page (`Alt → ,` → Updates) to the
`update.json` of your Wyven releases (a web address or a folder/network path). Wyven checks at
most once a day, asks before installing, verifies the installer's SHA-256 checksum and restarts
itself. *Help: Check for Updates* (`Ctrl+Q`, `>`) checks right away.

## First steps

```
wyven                       reopens the last project
wyven C:\code\my-app        opens a folder
wyven Program.cs            opens a file (and its folder)
wyven team.code-workspace   opens a workspace with several folders
```

Start options: `--terminal`, `--git`, `--diff`, `--preview`, `--settings`, `--goto=LINE`.

## The leader key

Tap **Alt** on its own, then type letters. A popup shows what comes next, so you never need to
learn the keys by heart. The letters follow the English command names:

| Keys | Meaning |
|---|---|
| `Alt e` / `Alt t` / `Alt s` | Explorer / terminal / search panel |
| `Alt g …` | **g**it: `g` panel, `c` commit all, `s` sync, `b` branch, `n` new branch, `d` diff, `l` blame |
| `Alt d …` | **d**ebug: `d` panel, `r` run, `s` stop, `b` breakpoint, `w` watch, `a` attach |
| `Alt j …` | **j**ump: `d` definition, `r` references, `s` symbol, `l` line |
| `Alt c …` | **c**ode: `r` rename, `f` format, `a` quick actions, `e` next error |
| `Alt f …` | **f**ile: `n` new, `o` open, `s` save as, `c` close all |
| `Alt p …` | **p**roject: `o` open folder, `r` recent, `a` add folder, `f` open workspace file, `w` save workspace |
| `Alt v …` | **v**iew: `m` minimap, `s` sticky scroll, `i` inlay hints, `l` CodeLens, `c` color theme, `t` light/dark, fold commands |
| `Alt a …` | **a**i: `t` suggestions on/off, `m` model, `k` API key, `c` Claude Code |
| `Alt w r` | reset the panel layout |
| `Alt m` | Markdown preview |
| `Alt ,` | settings |
| `Alt h` | cheat sheet with every shortcut |

## Editing

| Keys | Action |
|---|---|
| `Ctrl+D` | add the next occurrence as another cursor |
| `Alt+Click` / `Esc` | add a cursor / back to one cursor |
| `Alt+↑/↓`, `Alt+Shift+↑/↓` | move / duplicate lines |
| `Ctrl+Shift+L` | delete line |
| `Ctrl+K Ctrl+C` / `Ctrl+K Ctrl+U` / `Ctrl+#` | comment / uncomment / toggle |
| `Ctrl+K Ctrl+D` | format document |
| `Ctrl+M Ctrl+M`, `Ctrl+M Ctrl+L`, `Ctrl+M Ctrl+O` | fold block, fold all, fold to definitions |
| `Ctrl+Space`, `Ctrl+.` , `F2` | completion, quick actions, rename |
| `Alt c o` | organize usings / imports |
| snippet name + `Tab` | insert a snippet; `Tab` / `Shift+Tab` next / previous placeholder, `Esc` done |

**Snippets** work like in Visual Studio: type `prop`, `propfull`, `ctor`, `class`, `foreach`, `try`,
`cw` … in C# (also JavaScript/TypeScript, HTML, CSS and Rust) and press `Tab`, or pick them in the
completion list. Placeholders that appear several times change together. Your own snippets go
into `%APPDATA%\Wyven\snippets\` in VS Code's format ("Preferences: Configure User Snippets").

**Word wrap** (`Alt v w`) continues long lines in the next row at word boundaries, indented like
the line. **Zen mode** (`Alt v z`) shows only the text, fullscreen; `Esc` twice leaves it.
With a split, **drag a tab onto the other half** to move it there. **Split** the editor with `Alt w v` (side by side) or `Alt w h` (stacked): each half has its own
tabs, the same file can be open in both, `Alt w o` switches halves and `Alt w c` closes the split.

Files are saved safely (written to a temporary file first) and keep their line endings. Files
changed by other programs (git, formatters, Claude Code) reload automatically when they have no
unsaved changes.

## Finding things

- `Ctrl+Q` is the central search field: type a file name (fuzzy), `>` for commands, `@` for
  symbols in the file, `:` for a line number.
- `Ctrl+F` / `Ctrl+H`: find / replace in the file (`Alt+C` case, `Alt+W` whole word, `Alt+E` regex).
- `Ctrl+Shift+F` / `Ctrl+Shift+H`: search / replace in all files of the workspace.
- `F12` / `Ctrl+Click`: go to definition; `Ctrl+F12`: implementations; `Shift+F12`: all references.

## Panels and layout

The window is made of rounded cards with a small gap between them; the card with the focus has a
colored edge. The **activity bar** at the left edge holds colored icons for Explorer, Search, Git,
Debug and Terminal (and Settings at the bottom): a click opens the panel next to it, another click
closes it.

Docking works like in Visual Studio: the window is a tree of rows, columns and panes (a pane holds
panels as tabs). **Drag a panel's header** and the panel follows the mouse as a see-through card
while a **docking compass** appears. Over a pane it has five targets: left, top, right and bottom
split the pane, the middle adds the panel as a tab. Over the editor the panel docks next to the
editor; the four targets at the window edges put it along a whole edge. The hovered target shades
where the panel lands. Released anywhere else (or with `Ctrl` held), the panel gets a **window of
its own**: drag its tab strip back over the main window to dock it again, close the window to
close the panel (it comes back where it was docked). Dragging an editor tab splits the editor.
**Drag the gap between panes** to resize them. The layout is remembered; `Alt w r` resets it.

The activity bar shows the panels docked left of the editor above a line and the ones at the
bottom, right or floating below it; moving a panel moves its icon.

Files save by themselves shortly after you stop typing (settings "Auto save" and "Auto save
delay"); `Ctrl+Shift+S` saves all changed files at once.

**Right-click** explorer entries (new file/folder, rename, delete, copy path, reveal in File
Explorer, open in terminal), tabs (close others/to the right/all, split) and text (definition,
references, rename, quick actions, cut/copy/paste). Tabs have a × (middle-click closes too).

In the Explorer: `F2` renames (open tabs follow), `Del` moves to the recycle bin after asking,
`Ctrl+N` / `Ctrl+Shift+N` create a file / folder in the selected folder.

## Workspaces with several folders

`Alt p a` adds another folder to the window; each folder gets its own row in the Explorer, the ×
removes it again. File search, project search, Git, terminal, language servers and Claude Code
work across all folders (Git, terminal and language servers follow the folder of the active
file). Added folders are remembered with the main folder; `Alt p w` saves them as a VS Code
`.code-workspace` file that works in both editors.

## Languages

Syntax highlighting: C#, JavaScript, TypeScript/TSX, HTML, CSS, JSON, Markdown, TOML, Rust.
Language servers add errors, completion, hover info, go to definition, references, rename,
formatting, inlay hints and CodeLens:

- **C#** uses Roslyn from the VS Code C# extension (install that extension once).
- **TypeScript/JavaScript, HTML, CSS, JSON**: `cd %APPDATA%\Wyven\servers` and
  `npm install typescript vscode-langservers-extracted` (needs Node.js).
- **Markdown**: live preview next to the file (`Alt m`).

## Git

Change bars in the gutter, the Git panel (`Alt g g`) with commit message and changed files,
side-by-side diffs (`Alt g d`, `F8` jumps between changes), sync, branches and blame. Everything
runs in the background with the `git` you have installed.

## Start page, dropping files, closing

Without an open file Wyven shows a start page: new file, open file or folder, settings, shortcuts,
recent projects and tips; typing starts a new file. Drop files from Windows onto the window to open
them (folders join the workspace). Closing the window with unsaved files asks: save all and close,
close without saving, or cancel.

Brackets and quotes close themselves: typing `(` inserts `()`, typing `)` steps over it, with a
selection the pair goes around it, and `Backspace` in an empty pair removes both.

The status bar is clickable: the position opens "go to line", errors open Problems, the language
and the line breaks (LF/CRLF) can be switched there.

## Problems, build and test

The **Problems** panel (`Alt r p`) lists every error and warning of the project, from the language
servers and from the last build, grouped by file; a click or `Enter` jumps there, and the error
count shows on its icon in the activity bar. `Tab` switches to the output of the last task.

`Ctrl+Shift+B` (or `Alt r b`) builds the project in the background after saving all files: Wyven
finds `dotnet` (.sln/.csproj), `cargo` (Cargo.toml) or `npm` (package.json). `Alt r t` tests,
`Alt r c` cleans, `Alt r x` stops, `Alt r r` (or ▶ at the right end of the tab bar) runs the program
in a new terminal. Errors open the
Problems panel.

**Git changes:** click a change bar left of the text to see the lines of the last commit in a card
below; "Revert" puts them back.

The **Git panel** works with the mouse too: branch button (switch branches), sync/publish button,
message field and commit button; hover a changed file for open and discard buttons, right-click it
for more.

## Terminal

`Alt t` opens a terminal in the folder of the active file (PowerShell 7 if installed). All keys go
to the shell; `Ctrl+Shift+C` / `Ctrl+Shift+V` copy and paste. Choose another shell in the settings.

**Several terminals:** `+` in the terminal header starts another one. From two terminals on, a list
at the right edge switches between them; its × (or a middle click) ends one. The command palette
has "Terminal: New / Close / Next / Previous Terminal". `˅` next to `+` picks a shell (PowerShell,
Command Prompt, Git Bash, WSL); right-click a terminal in the list to rename, color or kill it.

## Debugging (.NET)

`F5` builds and starts the project with netcoredbg (downloaded once into `%APPDATA%\Wyven`),
`F9` toggles breakpoints, `F10` / `F11` / `Shift+F11` step, `Shift+F5` stops, `Ctrl+F5` runs
without the debugger. The Debug panel shows variables, watches and the call stack; `Alt d a`
attaches to a running process (for example Godot).

## AI suggestions

Suggestions are prepared quietly after a short pause and appear gray once you have not typed for
4 seconds (settings "Delay" and "Show after"); the switch in the status bar turns them on and off: `Tab` accepts, `Ctrl+→` takes one word,
`Esc` dismisses. By default they come from a free local model through
[Ollama](https://ollama.com) (`ollama pull qwen2.5-coder:7b-base`); nothing leaves your PC.
Optionally Claude (Haiku or Sonnet) with your own API key (`Alt a k`, stored in the Windows
Credential Manager).

## Claude Code

`Alt a c` starts Claude Code in Wyven's terminal and connects it like the VS Code extension:
Claude Code sees the open file, your selection and the errors, and its edits appear side by side
in Wyven – `Ctrl+Enter` accepts, `Esc` rejects. In another terminal, `/ide` finds Wyven.

## Appearance

`Alt v c` lists Wyven's themes and every VS Code theme you have installed; moving through the
list previews each one, `Enter` keeps it, `Esc` goes back. Theme files can also go into
`%APPDATA%\Wyven\themes`. Further options: minimap (blocks, bar, off), sticky scroll, inlay hints,
CodeLens, fonts and sizes.

## Settings

Your own shortcuts go into `%APPDATA%\Wyven\keymap.toml` (*Preferences: Open Keymap File*
creates it with a list of all command names): `[keys]` with `"ctrl+shift+d" = "view.toggle_word_wrap"`
or `"f12" = "none"`, and `[leader.g]` with `"x" = "git.sync"` for `Alt g x`.

`Alt ,` opens the settings page: all options in groups with a search field. Changes apply
immediately and are written to `%APPDATA%\Wyven\settings.toml`, keeping its comments. You can edit
that file directly too (*Open settings.toml* on the page).

## Plugins

Plugins are WebAssembly files in `%APPDATA%\Wyven\plugins`. They add commands to the search field
and can react to opening, switching and saving files. They run in a sandbox without access to
files, network or programs. See [plugins.md](plugins.md) for writing your own.

## Files Wyven keeps

Everything lives in `%APPDATA%\Wyven`: `settings.toml`, `layout.json` (panels),
`workspaces.json` (added folders), `recent-projects.txt`, `themes\`, `plugins\`, `servers\`
(web language servers), `netcoredbg\`, and `crash.log` if Wyven ever crashes.
