# Plan: Wyvern – eigener Code-Editor in Rust

Stand: 10.11.2026

## 1. Ausgangslage

**Vorlage:** [microsoft/vscode](https://github.com/microsoft/vscode) ist das richtige Repo. Es enthält „Code – OSS“ (MIT-Lizenz).
Das fertige „Visual Studio Code“ von Microsoft baut darauf auf und fügt proprietäre Teile hinzu: Marketplace-Zugang, Telemetrie, Remote-Extensions und das C# Dev Kit.

Was im Repo steckt:

| Bereich | Ort im Repo | Inhalt |
|---|---|---|
| Basis | `src/vs/base` | Datenstrukturen, Events, UI-Grundbausteine |
| Plattform | `src/vs/platform` | Dienste: Dateien, Einstellungen, Tastenkürzel, Logging |
| Editor-Kern | `src/vs/editor` | Textmodell (Piece Table), Cursor, Rendering, ~55 Editor-Features (Multi-Cursor, Folding, Suggest, Inlay Hints, Sticky Scroll, Minimap …) |
| Workbench | `src/vs/workbench` | Die ganze Oberfläche: ~100 Bereiche (Explorer, Suche, SCM/Git, Debug, Terminal, Chat/KI, Notebooks, Tasks …) |
| Extensions | `extensions/` | Eingebaute Sprachen, Themes, Git, Markdown |
| CLI | `cli/` | Schon in Rust geschrieben (Tunnel, Server-Verwaltung) |

Insgesamt ~11.500 TypeScript-Dateien auf Electron (Chromium + Node.js). Daher kommen der hohe RAM-Verbrauch (oft 500 MB+) und die spürbare Eingabe-Latenz.

**Vorgehen:** Wir übersetzen nichts. Wir schreiben den Editor neu in Rust und nehmen VS Codes Architektur und Features als Vorlage. Wo es hilft, lesen wir den VS-Code-Quellcode direkt nach (z. B. Piece Table, Sticky Scroll, Diff-Algorithmus).

## 2. Deine Entscheidungen

| Thema | Entscheidung |
|---|---|
| Ansatz | Neu in Rust, VS Code als Vorlage |
| Ziel | Alltags-Editor für dich, später evtl. öffentlich |
| Lizenz | MIT / Apache-2.0 |
| Plattformen | Windows + Linux |
| UI | GPU-nativ (wgpu) |
| Look | Minimalistisch, aber anpassbar wie Visual Studio 2026 (andockbare Panels) |
| Bedienung | Eigenes Tastenkürzel-Schema, frei konfigurierbar |
| Command Palette | Ein zentrales Suchfeld: Befehle, Dateien, Symbole, Einstellungen |
| Highlighting | Tree-sitter |
| Plugins | Eigenes Plugin-System (WASM); Sprachen über LSP/DAP |
| Sprachen | C# zuerst (Godot, .NET-Apps/Web, Schule), dann Web (JS/TS/HTML/CSS) |
| Kern-Features | Integriertes Terminal, Git, Debugger (DAP) |
| Editor-Features | Multi-Cursor, Minimap, Sticky Scroll, Split-Ansichten, Inlay Hints, CodeLens |
| Weitere | Globale Suche/Ersetzen, Markdown-Vorschau |
| Projekte | Datei-Explorer, mehrere Ordner (Multi-Root), Projekt-Wechsler |
| Einstellungen | Datei (TOML) + grafische Einstellungsseite |
| Themes | Eigenes Format + Import von VS-Code-Themes |
| Performance | Wenig RAM, minimale Tipp-Latenz |
| KI inline | Claude-Vorschläge beim Tippen, per Button in der Statusleiste abschaltbar; braucht einen eigenen API-Key, ohne Key ist es aus |
| Claude Code | Läuft im integrierten Terminal; der Editor teilt Auswahl und offene Datei und zeigt Änderungen als Diff |
| Rust-Kenntnis | Keine, Claude schreibt den Code |
| MVP | Editor + Explorer + Command Palette + C#-LSP |

## 3. Architektur

Ein Cargo-Workspace mit klar getrennten Crates:

```
wyvern/
├─ crates/
│  ├─ app          Startpunkt, Fenster (winit), Event-Schleife
│  ├─ render       GPU-Rendering (wgpu: DX12 auf Windows, Vulkan auf Linux), Glyphen-Atlas
│  ├─ text         Text-Shaping und Fonts (cosmic-text / swash)
│  ├─ ui           Eigene Widget-Schicht: Layout, Docking, Tabs, Listen, Bäume
│  ├─ buffer       Textpuffer (Rope, ropey), Undo/Redo, Multi-Cursor-Auswahl
│  ├─ editor       Editor-Ansicht: Scrollen, Minimap, Sticky Scroll, Inlay Hints, CodeLens
│  ├─ syntax       Tree-sitter: Highlighting, Folding, Struktur-Auswahl
│  ├─ lsp          LSP-Client (tokio, lsp-types)
│  ├─ dap          Debug-Adapter-Client
│  ├─ terminal     Terminal-Emulator (alacritty_terminal) + PTY (ConPTY / Unix)
│  ├─ git          Git über gix (reines Rust)
│  ├─ search       Projektsuche mit den ripgrep-Crates (grep, ignore)
│  ├─ workspace    Ordner, Multi-Root, Dateibeobachtung (notify), zuletzt geöffnete Projekte
│  ├─ settings     TOML-Einstellungen, Schema, Tastenkürzel
│  ├─ theme        Theme-Format + VS-Code-Theme-Import
│  ├─ markdown     Markdown-Vorschau (pulldown-cmark, nativ gezeichnet, kein Browser)
│  ├─ ai           Inline-Vorschläge über die Anthropic-API (Streaming), Key im Windows-Tresor (keyring)
│  ├─ claude-bridge Verbindung zu Claude Code (Auswahl teilen, Diffs anzeigen)
│  └─ plugin       WASM-Plugin-Host (wasmtime, Component Model / WIT)
```

Grundprinzipien:
- **UI-Thread wird nie blockiert.** LSP, Git, Suche, KI und Dateizugriffe laufen im Hintergrund (tokio).
- **Nur zeichnen, was sich ändert.** Glyphen werden gecacht und nur Sichtbares neu gerendert. Das hält Latenz und RAM klein.
- **Budgets messen:** Leerlauf-RAM < 150 MB, Taste → Bild < 1 Frame (8 ms bei 120 Hz). Jede Phase bekommt einen Benchmark.

## 4. C#-Unterstützung

- **Language-Server: Roslyn LSP** (`Microsoft.CodeAnalysis.LanguageServer`), Microsofts offizieller Server, MIT-lizenziert. Der Editor lädt ihn automatisch herunter. Er braucht das .NET SDK, das du für C# ohnehin hast.
- **Debugger: netcoredbg** (MIT). Microsofts eigener Debugger `vsdbg` darf laut Lizenz nur in Microsoft-Produkten laufen.
- **Godot (C#):** Projekte als normale .NET-Projekte öffnen; Debuggen über netcoredbg mit „Attach“ an den laufenden Godot-Prozess. GDScript ließe sich später über Godots eingebauten LSP anbinden.
- **.sln/.csproj** liest Roslyn selbst. Ein Solution Explorer im Visual-Studio-Stil ist vorerst nicht geplant.

## 5. KI

- **Inline-Vorschläge** über die Anthropic-API mit eigenem API-Key, abgerechnet getrennt vom Pro-Abo.
  - Standard: **Claude Haiku 5.5**, weil Vorschläge beim Tippen schnell kommen müssen. Umschaltbar auf **Claude Sonnet 5.5** (bessere Qualität, langsamer).
  - Button in der Statusleiste: an/aus mit einem Klick. Ohne Key bleibt es aus.
- **Claude Code** nutzt dein Pro-Abo und läuft im integrierten Terminal. Die Brücke schickt die aktuelle Datei und Auswahl an Claude Code und zeigt dessen Änderungen als Diff zum Annehmen oder Ablehnen. Wie Claude Code mit IDEs spricht, prüfen wir beim Bau der Brücke.

## 6. Phasen

**Phase 0 – Grundgerüst**
Cargo-Workspace, Fenster, wgpu-Rendering, Text auf dem Bildschirm, Benchmarks für RAM und Latenz.

**Phase 1 – MVP (täglich nutzbar)** ✅ fertig
- Textpuffer mit Undo/Redo und Multi-Cursor
- Tree-sitter-Highlighting für C#, JS/TS, HTML, CSS, JSON, Markdown, TOML
- Tabs, Split-Ansichten
- Datei-Explorer, Projekt öffnen, Projekt-Wechsler
- Command Palette (Befehle + Dateien)
- C#-LSP: Autovervollständigung, Fehler, Hover, Go-to-Definition, Umbenennen, Formatieren
- TOML-Einstellungen, erstes Tastenkürzel-Schema, ein helles und ein dunkles Theme

**Phase 2 – Arbeitsumgebung**
Integriertes Terminal, Git (Status, Diff, Stagen, Commit, Branches, Blame), globale Suche/Ersetzen, Minimap, Sticky Scroll, Folding.

**Phase 3 – Profi-Features**
Debugger (netcoredbg), Inlay Hints, CodeLens, Markdown-Vorschau, KI-Inline-Vorschläge, Claude-Code-Brücke, Web-Sprachen per LSP.

**Phase 4 – Anpassbarkeit**
Andockbare Panels im VS-2026-Stil, grafische Einstellungsseite, Import von VS-Code-Themes, Multi-Root-Workspaces, WASM-Plugin-System.

**Phase 5 – Veröffentlichung**
Installer (Windows MSI/winget, Linux AppImage/Flatpak), Auto-Update, Doku, Website.

## 7. Name

**Wyvern**, ein geflügelter Drache aus der Wappenkunde.
Der Name `wyvern` ist auf crates.io schon vergeben. Die Rust-Pakete heißen deshalb `wyvern-*` (z. B. `wyvern-editor`), App und Programm heißen trotzdem „Wyvern“.

## 8. Offene Fragen für später

1. ~~Tastenkürzel-Schema~~ → festgelegt in [SHORTCUTS.md](SHORTCUTS.md).
2. **Panel-Layout im Detail:** Wo sollen Explorer, Terminal und Git standardmäßig liegen?
3. **Font:** Eigene Standard-Schrift mitliefern (z. B. JetBrains Mono, Cascadia Code)?
4. **Remote/WSL:** Vorerst raus; später nachrüsten?
