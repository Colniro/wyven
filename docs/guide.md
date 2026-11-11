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

Files are saved safely (written to a temporary file first) and keep their line endings. Files
changed by other programs (git, formatters, Claude Code) reload automatically when they have no
unsaved changes.

## Finding things

- `Ctrl+Q` is the central search field: type a file name (fuzzy), `>` for commands, `@` for
  symbols in the file, `:` for a line number.
- `Ctrl+F` / `Ctrl+H`: find / replace in the file (`Alt+C` case, `Alt+W` whole word, `Alt+E` regex).
- `Ctrl+Shift+F` / `Ctrl+Shift+H`: search / replace in all files of the workspace.
- `F12` / `Ctrl+Click`: go to definition; `Shift+F12`: all references.

## Panels and layout

Explorer, Search, Git, Debug and Terminal live in three areas around the editor: left, right and
bottom. Each area shows its panels as tabs. **Drag a tab** to another area (the target lights up),
**drag an area's edge** to resize it. The layout is remembered; `Alt w r` resets it.

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

## Terminal

`Alt t` opens a terminal in the folder of the active file (PowerShell 7 if installed). All keys go
to the shell; `Ctrl+Shift+C` / `Ctrl+Shift+V` copy and paste. Choose another shell in the settings.

## Debugging (.NET)

`F5` builds and starts the project with netcoredbg (downloaded once into `%APPDATA%\Wyven`),
`F9` toggles breakpoints, `F10` / `F11` / `Shift+F11` step, `Shift+F5` stops, `Ctrl+F5` runs
without the debugger. The Debug panel shows variables, watches and the call stack; `Alt d a`
attaches to a running process (for example Godot).

## AI suggestions

Gray suggestions appear after a short pause while typing: `Tab` accepts, `Ctrl+→` takes one word,
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
