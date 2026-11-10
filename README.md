# Wyvern

Ein schneller, sparsamer Code-Editor in Rust, mit VS Code als Vorbild.
Der ganze Plan steht in [PLAN.md](PLAN.md).

**Stand: Phase 1 fertig, Phase 2 läuft (Terminal, Suchen/Ersetzen, Git, Minimap und Sticky Scroll fertig).** Wyvern öffnet Projektordner mit Explorer, Tabs und Suchfeld, bearbeitet Dateien mit Syntax-Highlighting und mehreren Cursorn, hat die Leader-Taste aus [SHORTCUTS.md](SHORTCUTS.md) und versteht C# über den Roslyn-Language-Server.
Noch offen in Phase 2: Code-Faltung (siehe [PLAN.md](PLAN.md)).

## Starten

```bash
cargo run --release -- pfad/zum/Projektordner
```

Mit einem Ordner öffnet sich dieser als Projekt, mit einer Datei die Datei samt ihrem Ordner. Ohne Pfad öffnet sich das zuletzt benutzte Projekt.
`cargo run` (ohne `--release`) baut schneller nach Änderungen, läuft aber langsamer und zeigt ein Konsolenfenster mit Logs.
Die fertige Exe liegt nach dem Bauen unter `target\release\wyvern.exe`.

## Was schon geht

- **Suchen in der Datei** (wie Visual Studio): `Strg+F` öffnet das Suchfeld oben rechts (mit Auswahl oder Wort unter dem Cursor vorbelegt), `Strg+H` zusätzlich Ersetzen.
  - `Enter` / `F3` nächster Treffer, `Shift+Enter` / `Shift+F3` vorheriger, `Tab` wechselt zwischen Suchen und Ersetzen, `Esc` schließt.
  - `Alt+C` Groß/Klein, `Alt+W` ganzes Wort, `Alt+E` regulärer Ausdruck (Ersetzen mit `$1`, `$2` …), `Alt+R` ersetzt den Treffer, `Alt+A` alle (ein Rückgängig-Schritt).
  - Alle Treffer sind im Editor hinterlegt, der aktuelle kräftiger.
- **Suchen im Projekt**: `Strg+Shift+F` (oder `Alt → s`) zeigt die Suche links statt des Explorers; `Strg+Shift+H` mit Ersetzen.
  - Sucht beim Tippen im Hintergrund, beachtet `.gitignore`, überspringt `bin`/`obj` und Binärdateien. Gleiche Optionen wie oben.
  - `↑`/`↓` + `Enter` oder Klick öffnet den Treffer. `Alt+A` ersetzt im ganzen Projekt: offene Dateien werden im Editor geändert (Rückgängig möglich), alle anderen sofort gespeichert.

- **Terminal** unter dem Editor: `Alt → t` öffnet es (beim ersten Mal startet die Shell im Projektordner), springt hinein oder schließt es. Start direkt mit Terminal: `wyvern <Ordner> --terminal`.
  - Alle Tasten gehen an die Shell, auch `Esc`, `Strg+C` und `Strg+V`. So laufen auch Programme wie Claude Code oder `dotnet watch` darin. `Alt` antippen öffnet weiterhin das Leader-Menü.
  - Kopieren/Einfügen: `Strg+Shift+C` / `Strg+Shift+V`, Auswahl mit der Maus. Mausrad scrollt zurück (bis 5000 Zeilen).
  - Volle Farben (256 und True Color), Shell und Höhe in `settings.toml` unter `[terminal]`. Standard: PowerShell 7, sonst Windows PowerShell.

- **Git** (nutzt das installierte `git`, alles läuft im Hintergrund):
  - **Balken im Zeilenrand**: grün neu, blau geändert, rote Marke für gelöschte Zeilen, verglichen mit dem letzten Commit und beim Tippen aktualisiert.
  - **Git-Panel** links statt des Explorers (`Alt → g → g`): Branch, ↑/↓ gegenüber dem Server, Commit-Nachricht und alle geänderten Dateien. Nachricht eintippen, `Strg+Enter` committet **alles** (vorher werden offene Dateien gespeichert). `↑`/`↓` + `Enter` oder Klick zeigt den Diff einer Datei, `Esc` zurück zum Explorer.
  - **Diff nebeneinander** (`Alt → g → d` für die aktuelle Datei): links der letzte Commit, rechts der aktuelle Stand, beide scrollen gemeinsam. `F8` / `Shift+F8` springen zur nächsten/vorigen Änderung, `Esc` schließt.
  - **Sync** (`Alt → g → s`): pull, dann push. Ein neuer Branch wird beim ersten Sync veröffentlicht. Alle 3 Minuten wird im Hintergrund `fetch` ausgeführt.
  - **Branches**: `Alt → g → b` wechseln (Ziffer oder Enter), `Alt → g → n` neuen Branch anlegen und wechseln.
  - **Blame auf Abruf** (`Alt → g → l`): Autor, Alter und Commit-Nachricht der Cursorzeile als Tooltip.
- **Dateien, die sich auf der Festplatte ändern** (Branch-Wechsel, Pull, Claude Code, Formatter), werden automatisch neu geladen, solange sie im Editor keine ungespeicherten Änderungen haben.

- **Einstellungen** in `settings.toml` (`%APPDATA%\Wyvern\settings.toml`, öffnen mit `Alt → ,`): Theme, Schriftart/-größe, Zeilenhöhe, Tab-Breite, Einrückung, Autovervollständigung und Info-Tooltip an/aus, Oberflächenschrift und Explorer-Breite. Die Datei wird beim ersten Start mit allen Optionen und deutschen Erklärungen angelegt. Speichern in Wyvern wendet sie sofort an. Tippfehler bei Optionsnamen werden in der Statusleiste gemeldet, die alten Einstellungen bleiben dann aktiv.
- **Themes** „Wyvern Dark“ und „Wyvern Light“: umschalten mit `Alt → v → t` oder im Suchfeld `>Theme`. Die Wahl wird in `settings.toml` gespeichert.
- **Minimap** rechts statt Scrollbalken: zeigt die Form des Codes als farbige Blöcke und einen Kasten für den sichtbaren Bereich (heller, wenn die Maus darüber ist). Ganz rechts liegt ein schmaler Streifen mit Markierungen für die ganze Datei: Git-Änderungen, Suchtreffer, Fehler/Warnungen und die Cursorzeile.
  - Klick springt an die Stelle, Ziehen des Kastens scrollt. Bei langen Dateien scrollt die Minimap mit.
  - `Alt → v → m` schaltet um: Blöcke → schmale Markierungsleiste → aus. Die Wahl steht in `settings.toml` unter `[editor] minimap`.
- **Sticky Scroll**: Beim Scrollen bleiben die Kopfzeilen von Namespace, Klasse/Struct/`impl` und Methode/Property oben stehen (höchstens 5, nie mehr als ein Drittel des Editors). Klick auf eine Kopfzeile springt zur Deklaration. `Alt → v → s` schaltet es an/aus (`[editor] sticky_scroll` in `settings.toml`).
- **Startoptionen**: `wyvern <Datei> --goto=120` springt zu Zeile 120; `--terminal`, `--git` und `--diff` öffnen gleich Terminal, Git-Panel bzw. Diff der Datei.
- **Oberfläche auf Englisch**: Leader-Menü, Befehle im Suchfeld (`File: Save`, `Git: Commit All` …), Spickzettel, Panels, Statusmeldungen und die Vorlage der `settings.toml`. Tasten heißen `Ctrl`, `PgUp` usw. In `settings.toml` gelten `theme = "dark"`/`"light"` und `indent = "spaces"`/`"tabs"`, die alten Werte `dunkel`/`hell`/`leerzeichen` funktionieren weiter.
- **Spickzettel** mit allen Kürzeln: `Alt → h`. Er wird aus denselben Listen erzeugt wie Suchfeld und Leader-Menü und ist deshalb immer aktuell.

- **C# mit Roslyn** (derselbe Server wie im C# Dev Kit von VS Code):
  - Fehler und Warnungen als Wellenlinien. Die Meldung erscheint in der Statusleiste, sobald der Cursor auf der Stelle steht, rechts stehen die Zähler `✖ / ⚠`.
  - Autovervollständigung beim Tippen und nach `.`, oder mit `Strg+Leertaste`. `Enter`/`Tab` übernimmt, `Esc` schließt.
  - `F12` oder `Strg+Klick`: Gehe zu Definition
  - Info zum Symbol: Maus kurz still halten oder `Strg+K, Strg+I`
  - Formatieren: `Strg+K, Strg+D`
  - Umbenennen: `F2` am Namen, neuen Namen tippen, `Enter`. Gilt für alle Stellen im Projekt. Betroffene Dateien öffnen sich als ungespeicherte Tabs, damit du alles prüfen und mit `Strg+Z` zurücknehmen kannst.
  - Quick Actions: `Strg+.` zeigt die Vorschläge von Roslyn (Tippfehler korrigieren, Feld/Methode erzeugen, `using` ergänzen …). Auswahl per Ziffer oder Pfeiltasten + `Enter`.
  - Nächster Fehler: `F8` (wie in Visual Studio)

  Der Server startet automatisch, sobald eine `.cs`-Datei offen ist. Wyvern meldet ihm die `.sln`/`.slnx` im Projektordner an, oder wenn es keine gibt, alle `.csproj` bis drei Ordner tief. Das Laden dauert ein paar Sekunden (beim ersten Mal inkl. `dotnet restore`), die Statusleiste zeigt den Fortschritt. Ohne Projektdatei arbeitet Roslyn im Einzeldatei-Modus.
  Woher der Server kommt, in dieser Reihenfolge: Umgebungsvariable `WYVERN_CSHARP_SERVER`, dann der Roslyn-Server aus der **C#-Extension von VS Code**, dann `csharp-ls` im PATH. Er braucht das .NET SDK.

- **Suchfeld** `Strg+Q`, ein Feld für alles:
  - ohne Präfix: Dateien im Projekt, unscharf (`prgcs` findet `Program.cs`). Beachtet `.gitignore`.
  - `>`: Befehle mit ihrem Kürzel (auch direkt mit `Strg+Shift+P`)
  - `@`: Klassen, Methoden, Eigenschaften … der aktuellen Datei (C#, JS/TS, Rust; auch `Alt → j → s`)
  - `:`: Gehe zu Zeile, z. B. `:42` oder `:42:7` (auch direkt mit `Strg+G`)

- **Explorer** links: Ordner auf/zu per Klick oder `→`/`←`, Datei öffnen per Klick oder `Enter`. Generierte Ordner (`bin`, `obj`, `.git`, `target`, `node_modules` …) werden ausgeblendet. Der Inhalt wird neu gelesen, wenn das Fenster wieder den Fokus bekommt.
- **Tabs:** `Strg+Tab` wechselt zur zuletzt benutzten Datei (Strg gedrückt halten und Tab mehrmals drücken geht weiter zurück). `Strg+W` schließt, bei ungespeicherten Änderungen erst nach einer Rückfrage. `Strg+Shift+T` öffnet den zuletzt geschlossenen Tab wieder, mittlere Maustaste schließt einen Tab.
- **Dateien:** `Strg+N` neu, `Strg+O` öffnen, `Strg+S` speichern (bei neuen Dateien mit „Speichern unter“), `Strg+Shift+S` speichern unter.
- **Leader-Taste:** `Alt` kurz antippen. Hält man kurz inne, zeigt ein Popup die nächsten Tasten. Schon belegt:
  - `Alt → e`: Explorer ein/aus
  - `Alt → p → o`: Ordner öffnen
  - `Alt → p → r`: zuletzt geöffnete Projekte
  - `Alt → p → e`: in den Explorer springen
  - `Alt → f → n / o / s / c`: new file, open file, save as, close all

  Alle anderen Einträge sagen, in welcher Phase sie kommen.

- Syntax-Highlighting mit Tree-sitter für C#, JavaScript, TypeScript/TSX, HTML, CSS, JSON, Markdown, TOML und Rust. Es parst bei jeder Änderung nur den geänderten Teil neu und färbt nur die sichtbaren Zeilen ein.
- Mehrere Cursor: `Strg+D` (nächstes gleiches Wort), `Alt+Klick`, `Esc` zurück zu einem Cursor.
- Zeilenbefehle: `Alt+↑/↓` verschieben, `Alt+Shift+↑/↓` duplizieren, `Strg+Shift+L` löschen.
- Kommentieren: `Strg+K, Strg+C` / `Strg+K, Strg+U` / `Strg+#`.
- Rückgängig/Wiederholen mit Gruppierung beim Tippen, Kopieren/Einfügen wie in Visual Studio (ohne Auswahl ganze Zeile), Einrückung bei Enter und zwischen `{}`.
- Speichern mit `Strg+S`: Die Datei wird zuerst in eine temporäre Datei geschrieben und dann ersetzt. CRLF/LF bleiben erhalten.
- Maus: Klicken, Ziehen, Doppelklick wählt ein Wort, Mausrad scrollt (`Shift` = seitlich).
- Schrift vergrößern/verkleinern mit `Strg++` / `Strg+-` / `Strg+0`.

## Aufbau

| Crate | Aufgabe |
|---|---|
| `crates/wyvern-app` | Startpunkt (`wyvern.exe`), Fenster, Tastenbelegung, Editor-Ansicht, Statusleiste |
| `crates/wyvern-buffer` | Textpuffer (Rope), Multi-Cursor, Transaktionen, Undo/Redo, Laden/Speichern |
| `crates/wyvern-syntax` | Tree-sitter: Sprachen erkennen, inkrementell parsen, Highlighting, Symbole |
| `crates/wyvern-search` | Suchen/Ersetzen: Abfragen (Text, Wort, Groß/Klein, Regex), Projektsuche |
| `crates/wyvern-git` | Git über die `git`-Kommandozeile: Status, Commit, Sync, Branches, Blame, Zeilen-Diffs |
| `crates/wyvern-terminal` | Terminal: Shell in ConPTY, Emulation mit `alacritty_terminal`, Tasten-Kodierung |
| `crates/wyvern-lsp` | Language-Server-Client: JSON-RPC über stdio, UTF-16-Positionen |
| `crates/wyvern-render` | GPU-Rendering (wgpu): Rechtecke und Text (glyphon / cosmic-text) |

Tests: `cargo test --workspace`

## Messwerte (Phase 0)

Gemessen auf Windows 11, Laptop mit Intel Arc + RTX 4050, Release-Build:

| | OpenGL (Standard) | DX12 |
|---|---|---|
| RAM (privat, wie im Task-Manager) | ~33 MB | ~285 MB |
| Start bis erstes Bild | ~0,9 s | ~1,6–2,9 s |
| Taste → Bild an GPU übergeben | ~2,5 ms | – |

Unter Windows lädt DX12 beim Start die Treiber *aller* Grafikkarten. Deshalb ist OpenGL der Standard, DX12 dient als Rückfallebene.

Mit geöffneter C#-Datei und Highlighting (Phase 1) liegt der private RAM bei ~42 MB, Taste → Bild bei ~3–4 ms.

Die Statuszeile im Fenster zeigt den *Working Set* (inkl. geteilter Treiber-Bibliotheken). Der Wert ist daher höher als „privat“.

## Umgebungsvariablen zum Testen

| Variable | Wirkung |
|---|---|
| `WGPU_BACKEND=dx12` / `vulkan` / `gl` | Grafik-Backend erzwingen |
| `WYVERN_GPU=high` | Die starke (dedizierte) Grafikkarte statt der integrierten nutzen |
| `RUST_LOG=info` | Log-Ausgabe, z. B. Startzeiten pro Phase |

## Lizenz

MIT oder Apache-2.0, nach Wahl.
