# Wyven

Ein schneller, sparsamer Code-Editor in Rust, mit VS Code als Vorbild.
Der ganze Plan steht in [PLAN.md](PLAN.md).

**Stand: Phase 1 bis 4 fertig.** Wyven öffnet Projektordner mit Explorer, Tabs und Suchfeld, bearbeitet Dateien mit Syntax-Highlighting und mehreren Cursorn, hat die Leader-Taste aus [SHORTCUTS.md](SHORTCUTS.md) und versteht C# über den Roslyn-Language-Server.
Dazu kommen Debugger, KI-Vorschläge, Inlay Hints, CodeLens, Markdown-Vorschau, die Claude-Code-Brücke und Web-Sprachen per LSP. Phase 4 brachte andockbare Panels, eine Einstellungsseite, VS-Code-Themes, mehrere Ordner pro Fenster und Plugins. Als Nächstes kommt Phase 5 (siehe [PLAN.md](PLAN.md)).

## Starten

```bash
cargo run --release -- pfad/zum/Projektordner
```

Mit einem Ordner öffnet sich dieser als Projekt, mit einer Datei die Datei samt ihrem Ordner. Ohne Pfad öffnet sich das zuletzt benutzte Projekt.
`cargo run` (ohne `--release`) baut schneller nach Änderungen, läuft aber langsamer und zeigt ein Konsolenfenster mit Logs.
Die fertige Exe liegt nach dem Bauen unter `target\release\wyven.exe`.

## Installer und Updates

```powershell
powershell -File scripts\release.ps1 -Notes "Was neu ist"
```

baut `dist\wyven-setup-<version>.exe` (Inno Setup 6, installiert ohne Adminrechte nach `%LOCALAPPDATA%\Programs\Wyven`, mit Startmenü, „Open with Wyven“ im Explorer, optional `wyven` im PATH und `.code-workspace`-Verknüpfung) und `dist\update.json` (Version, Installer, SHA-256).
Zum Veröffentlichen beide Dateien an einen Ort legen (Webserver, GitHub-Release, Netzlaufwerk) und dessen `update.json`-Adresse in den Einstellungen unter *Updates* eintragen. Wyven prüft dann höchstens einmal am Tag, fragt vor dem Installieren, prüft die Prüfsumme und startet neu; „Help: Check for Updates“ prüft sofort.
Benutzerhandbuch (englisch): [docs/guide.md](docs/guide.md), Plugins: [docs/plugins.md](docs/plugins.md). `docs/` ist zugleich die Website (für GitHub Pages: Quelle „docs“-Ordner); das Icon erzeugt `python assets/make_icon.py`.

## Was schon geht

- **Suchen in der Datei** (wie Visual Studio): `Strg+F` öffnet das Suchfeld oben rechts (mit Auswahl oder Wort unter dem Cursor vorbelegt), `Strg+H` zusätzlich Ersetzen.
  - `Enter` / `F3` nächster Treffer, `Shift+Enter` / `Shift+F3` vorheriger, `Tab` wechselt zwischen Suchen und Ersetzen, `Esc` schließt.
  - `Alt+C` Groß/Klein, `Alt+W` ganzes Wort, `Alt+E` regulärer Ausdruck (Ersetzen mit `$1`, `$2` …), `Alt+R` ersetzt den Treffer, `Alt+A` alle (ein Rückgängig-Schritt).
  - Alle Treffer sind im Editor hinterlegt, der aktuelle kräftiger.
- **Oberfläche im Stil von Visual Studio 2026**: Explorer, Editor, Terminal usw. liegen als abgerundete Karten mit kleinem Abstand auf einem etwas dunkleren Rahmen; das Fenster mit dem Fokus hat einen farbigen Rand.
  - **Aktivitätsleiste** ganz links mit farbigen Symbolen (Explorer blau, Suche orange, Git rot, Debug grün, Terminal lila, unten die Einstellungen). Ein Klick öffnet das Panel daneben, ein zweiter Klick schließt es; beim Darüberfahren zeigt ein Hinweis das Tastenkürzel. Über einer Linie stehen die Panels, die links (oder oben) angedockt sind, darunter die unten, rechts oder schwebend – verschiebt man ein Panel, wandert sein Symbol mit.
  - **Panel-Köpfe** mit Knöpfen: im Explorer neue Datei, neuer Ordner, Aktualisieren, alles zuklappen; rechts und unten ein × zum Schließen.
  - **Tabs** mit farbigem Dateityp (C#, MD, {} …), × zum Schließen auf dem aktiven Tab und beim Darüberfahren (● bei ungespeicherten Änderungen), mittlere Maustaste schließt ebenfalls.
  - **Rechtsklick-Menüs**: im Explorer (neue Datei/neuer Ordner, Umbenennen, Löschen, Pfad kopieren, im Datei-Explorer zeigen, Terminal hier öffnen, Ordner aus dem Workspace nehmen), auf Tabs (Schließen, andere/rechts davon/alle schließen, Pfad kopieren, teilen) und im Text (Definition, Implementierung, Referenzen, Umbenennen, Quick Actions, Formatieren, Ausschneiden/Kopieren/Einfügen).
  - **Andocken wie in Visual Studio** (Layout als Baum aus Zeilen, Spalten und Panes): Beim Ziehen eines Panel-Kopfs folgt das Panel der Maus als halbdurchsichtige Karte, und ein Kompass erscheint. Über einem Pane hat er fünf Ziele: links, oben, rechts, unten teilen das Pane (das Panel bekommt die Hälfte), die Mitte legt es dort als Tab ab. Über dem Editor dockt es neben dem Editor an, die vier Ziele an den Fensterrändern legen es über die ganze Fensterkante. Das Ziel unter der Maus zeigt schattiert, wo das Panel landet. Wird es ohne Ziel losgelassen (oder mit `Strg`), bekommt es ein **eigenes Fenster**; dessen Tab-Leiste zurück aufs Hauptfenster ziehen dockt es wieder an, das Fenster schließen schließt das Panel (es kommt dann wieder dort, wo es angedockt war). Ein gezogener Editor-Tab teilt den Editor nach links, rechts, oben oder unten. Die Abstände zwischen Panes ziehen ändert die Größe; alles wird in `layout.json` gemerkt.
  - **Tastenmenü** (nach `Alt`, unten rechts): eine Karte mit farbigem Titel je Gruppe, Tasten als kleine Tastenkappen, Untermenüs (›) oben, Befehle darunter, bei langen Listen in zwei Spalten.
- **Geteilte Ansicht**: `Alt → w → v` teilt den Editor nebeneinander, `Alt → w → h` untereinander. Jede Hälfte hat eigene Tabs; dieselbe Datei kann in beiden offen sein, Änderungen erscheinen sofort in der anderen Hälfte. `Alt → w → o` oder `Alt → w → ←↑→↓` wechselt die Hälfte (oder ein Klick hinein), `Alt → w → c` schließt die Teilung.
- **Zeilenumbruch** (`Alt → v → w`, gespeichert in `settings.toml`): lange Zeilen gehen an Wortgrenzen in der nächsten Zeile weiter, eingerückt wie die Zeile selbst.
- **Zen-Modus** (`Alt → v → z`): Vollbild, nur noch der Text in einer Spalte in der Mitte; `Alt → v → z` oder zweimal `Esc` beendet ihn.
- **Dateien im Explorer**: `F2` umbenennen (offene Tabs folgen), `Entf` in den Papierkorb (mit Rückfrage), `Strg+N` / `Strg+Shift+N` neue Datei / neuer Ordner im gewählten Ordner. Auch über `Alt → f → r/d/f/k`.
- **Gehe zu Implementierung** (`Strg+F12`, `Alt → j → i`) und **Usings ordnen** (`Alt → c → o`: unnötige `using`s entfernen und sortieren; bei TypeScript „Organize Imports“).
- **Eigene Tastenkürzel** in `%APPDATA%\Wyven\keymap.toml` (siehe [SHORTCUTS.md](SHORTCUTS.md)).
- **Mehrere Ordner in einem Fenster** (Multi-Root):
  - `Alt → p → a` fügt einen Ordner hinzu. Im Explorer hat dann jeder Ordner eine eigene Zeile; das `×` dahinter nimmt ihn wieder heraus.
  - Dateisuche (`Ctrl+Q`) und Projektsuche durchsuchen alle Ordner; Dateien weiterer Ordner beginnen mit deren Namen (`lib/src/x.rs`).
  - Git zeigt das Repository der aktiven Datei, das Terminal startet in ihrem Ordner, Language-Server und Debugger nehmen ebenfalls ihren Ordner. Claude Code sieht alle Ordner.
  - Gemerkt wird das auf zwei Arten: automatisch beim Hauptordner (`%APPDATA%\Wyven\workspaces.json`; „zuletzt geöffnet“ bringt die weiteren Ordner zurück) und als VS-Code-Datei: `Alt → p → w` speichert eine `.code-workspace`-Datei, `Alt → p → f` öffnet eine (auch `wyven datei.code-workspace`). Änderungen an den Ordnern werden dann in die Datei geschrieben, ihre übrigen Einträge bleiben.
- **Plugins** (WebAssembly, in Rust geschrieben):
  - Plugins liegen als `.wasm`-Dateien in `%APPDATA%\Wyven\plugins` (auch in Unterordnern) und werden beim Start geladen. Ihre Befehle erscheinen im Suchfeld (`Ctrl+Q`, `>`), z. B. „Text Tools: Sort Lines“.
  - Sie können auf Ereignisse reagieren: Datei geöffnet, Tab gewechselt, vor und nach dem Speichern.
  - Sicher: Plugins laufen in einer Sandbox (wasmi) ohne Zugriff auf Dateien, Netzwerk oder Programme. Sie sehen nur Text, Auswahl, Pfad und Sprache der aktiven Datei und antworten mit Aktionen (Text ersetzen, auswählen, Meldung). Jeder Aufruf hat ein Rechenbudget, ein hängendes Plugin wird angehalten.
  - Befehle: „Plugins: Show Installed Plugins“, „Plugins: Reload Plugins“, „Plugins: Open Plugins Folder“.
  - Eigene Plugins: Crate `wyven-plugin` einbinden, `Plugin` implementieren, `export_plugin!(MeinPlugin)`, dann `cargo build --release --target wasm32-unknown-unknown`. Beispiel: [`plugins/text-tools`](plugins/text-tools) (Zeilen sortieren, Groß/Klein, Wörter zählen, Leerzeichen am Zeilenende beim Speichern entfernen). Die Schnittstelle (JSON über `wyven_alloc`/`wyven_handle`) ist in `crates/wyven-plugin/src/lib.rs` beschrieben und geht auch aus anderen Sprachen.
- **Problems-Panel** (wie die Fehlerliste in VS, `Alt → r → p`): alle Fehler und Warnungen des Projekts – von den Language-Servern und vom letzten Build –, nach Datei gruppiert; Klick oder `Enter` springt hin. Die Zahl der Fehler steht als Badge am Symbol in der Aktivitätsleiste. `Tab` (oder der Knopf im Kopf) wechselt zur Ausgabe des letzten Tasks.
- **▶-Knopf** rechts in der Tab-Leiste: startet das Projekt in einem neuen Terminal (wie `Alt → r → r`).
- **Build, Test, Run** (`Alt → r`): Wyven erkennt das Projekt (.sln/.csproj → `dotnet`, Cargo.toml → `cargo`, package.json → `npm`). `Strg+Shift+B` oder `Alt → r → b` baut (vorher wird alles gespeichert), `t` testet, `c` räumt auf, `x` bricht ab – im Hintergrund, Fehler landen im Problems-Panel, das bei Fehlern aufgeht. `r` startet das Programm in einem neuen Terminal.
- **Git-Änderungen am Rand**: Klick auf einen Balken (grün neu, blau geändert, rot gelöscht) zeigt die Zeilen aus dem letzten Commit in einer Karte darunter; „Revert“ stellt den Block wieder her, `Esc` schließt.
- **Andockbare Panels**: Explorer, Suche, Git, Debug und Terminal liegen in Panes um den Editor (frei anordenbar, siehe „Andocken wie in Visual Studio“).
  - Jedes Pane zeigt oben eine Leiste mit seinen Panels als Tabs; ein Klick wechselt das Panel. Die Knöpfe im Kopf werden beim Darüberfahren hervorgehoben, ihr Hinweistext erscheint erst nach kurzem Verweilen.
  - Einen Tab mit der Maus ziehen verschiebt das Panel: auf die Tab-Leiste eines Bereichs, an den linken oder rechten Fensterrand oder nach unten. Der Zielbereich leuchtet beim Ziehen auf.
  - Die Kante zwischen Bereich und Editor ziehen ändert die Größe.
  - Anordnung, Größen und was offen ist, merkt sich Wyven in `%APPDATA%\Wyven\layout.json` (für alle Projekte). `Alt → w → r` stellt die ursprüngliche Anordnung wieder her.
- **Suchen im Projekt**: `Strg+Shift+F` (oder `Alt → s`) zeigt die Suche im Bereich des Suche-Panels (anfangs links); `Strg+Shift+H` mit Ersetzen.
  - Sucht beim Tippen im Hintergrund, beachtet `.gitignore`, überspringt `bin`/`obj` und Binärdateien. Gleiche Optionen wie oben.
  - `↑`/`↓` + `Enter` oder Klick öffnet den Treffer. `Alt+A` ersetzt im ganzen Projekt: offene Dateien werden im Editor geändert (Rückgängig möglich), alle anderen sofort gespeichert.

- **Terminal** (anfangs unter dem Editor): `Alt → t` öffnet es (beim ersten Mal startet die Shell im Projektordner), springt hinein oder schließt es. Start direkt mit Terminal: `wyven <Ordner> --terminal`.
  - Alle Tasten gehen an die Shell, auch `Esc`, `Strg+C` und `Strg+V`. So laufen auch Programme wie Claude Code oder `dotnet watch` darin. `Alt` antippen öffnet weiterhin das Leader-Menü.
  - Kopieren/Einfügen: `Strg+Shift+C` / `Strg+Shift+V`, Auswahl mit der Maus. Mausrad scrollt zurück (bis 5000 Zeilen).
  - Volle Farben (256 und True Color), Shell und Höhe in `settings.toml` unter `[terminal]`. Standard: PowerShell 7, sonst Windows PowerShell.
  - **Profile**: `˅` neben dem `+` öffnet ein Menü mit den installierten Shells (PowerShell 7, Windows PowerShell, Eingabeaufforderung, Git Bash, WSL). Rechtsklick auf ein Terminal in der Liste: umbenennen, einfärben, beenden.
  - **Mehrere Terminals** wie in VS Code: `+` im Kopf startet ein weiteres; ab zwei Terminals steht am rechten Rand eine Liste zum Umschalten, das × (oder die mittlere Maustaste) beendet eines. Im Suchfeld: „Terminal: New / Close / Next / Previous Terminal“.

- **Git** (nutzt das installierte `git`, alles läuft im Hintergrund):
  - **Balken im Zeilenrand**: grün neu, blau geändert, rote Marke für gelöschte Zeilen, verglichen mit dem letzten Commit und beim Tippen aktualisiert.
  - **Git-Panel** (anfangs links neben dem Explorer, `Alt → g → g`): Branch, ↑/↓ gegenüber dem Server, Commit-Nachricht und alle geänderten Dateien. Nachricht eintippen, `Strg+Enter` committet **alles** (vorher werden offene Dateien gespeichert). `↑`/`↓` + `Enter` oder Klick zeigt den Diff einer Datei, `Esc` zurück zum Explorer.
  - **Diff nebeneinander** (`Alt → g → d` für die aktuelle Datei): links der letzte Commit, rechts der aktuelle Stand, beide scrollen gemeinsam. `F8` / `Shift+F8` springen zur nächsten/vorigen Änderung, `Esc` schließt.
  - **Sync** (`Alt → g → s`): pull, dann push. Ein neuer Branch wird beim ersten Sync veröffentlicht. Alle 3 Minuten wird im Hintergrund `fetch` ausgeführt.
  - **Branches**: `Alt → g → b` wechseln (Ziffer oder Enter), `Alt → g → n` neuen Branch anlegen und wechseln.
  - **Blame auf Abruf** (`Alt → g → l`): Autor, Alter und Commit-Nachricht der Cursorzeile als Tooltip.
- **Dateien, die sich auf der Festplatte ändern** (Branch-Wechsel, Pull, Claude Code, Formatter), werden automatisch neu geladen, solange sie im Editor keine ungespeicherten Änderungen haben.

- **Einstellungsseite** (`Alt → ,`): ein Tab wie in VS Code mit allen Optionen, nach Gruppen (Appearance, Font, Editor, Interface, AI, Terminal) und mit Suchfeld oben, das beim Tippen filtert. Per Maus: Häkchen, Auswahlknöpfe, `−`/`+` bei Zahlen, Klick in ein Feld zum Tippen (`Enter` übernimmt, `Esc` bricht ab). Jede Änderung wird sofort in `settings.toml` geschrieben und angewendet; die Kommentare der Datei bleiben erhalten. „Open settings.toml“ oben rechts öffnet die Datei selbst.
- **Einstellungen** in `settings.toml` (`%APPDATA%\Wyven\settings.toml`, im Suchfeld `>Open Settings File`): Theme, Schriftart/-größe, Zeilenhöhe, Tab-Breite, Einrückung, Autovervollständigung und Info-Tooltip an/aus, Oberflächenschrift und Explorer-Breite. Die Datei wird beim ersten Start mit allen Optionen und Erklärungen angelegt. Speichern in Wyven wendet sie sofort an. Tippfehler bei Optionsnamen werden in der Statusleiste gemeldet, die alten Einstellungen bleiben dann aktiv.
- **Themes** „Wyven Dark“ und „Wyven Light“: umschalten mit `Alt → v → t`. Die Wahl wird in `settings.toml` gespeichert.
- **VS-Code-Themes**: `Alt → v → c` (oder im Suchfeld `>Color Theme`) listet Wyvens Themes und alle in VS Code installierten Themes (One Dark, Dracula, Tokyo Night, Monokai Pro …). Tippen filtert, `↑`/`↓` zeigt jedes Theme sofort als Vorschau, `Enter` behält es, `Esc` geht zurück. Eigene Theme-Dateien (`.json` aus VS Code, auch entpackte Erweiterungen) kommen nach `%APPDATA%\Wyven	hemes`.
  - Übernommen werden Editor-, Panel-, Tab-, Statusleisten- und Terminalfarben sowie die Syntaxfarben (TextMate-Scopes). Was ein Theme nicht festlegt, wird aus seinem eigenen Hinter- und Vordergrund abgeleitet.
- **Minimap** rechts statt Scrollbalken: zeigt die Form des Codes als farbige Blöcke und einen Kasten für den sichtbaren Bereich (heller, wenn die Maus darüber ist). Ganz rechts liegt ein schmaler Streifen mit Markierungen für die ganze Datei: Git-Änderungen, Suchtreffer, Fehler/Warnungen und die Cursorzeile.
  - Klick springt an die Stelle, Ziehen des Kastens scrollt. Bei langen Dateien scrollt die Minimap mit.
  - `Alt → v → m` schaltet um: Blöcke → schmale Markierungsleiste → aus. Die Wahl steht in `settings.toml` unter `[editor] minimap`.
- **Sticky Scroll**: Beim Scrollen bleiben die Kopfzeilen von Namespace, Klasse/Struct/`impl` und Methode/Property oben stehen (höchstens 5, nie mehr als ein Drittel des Editors). Klick auf eine Kopfzeile springt zur Deklaration. `Alt → v → s` schaltet es an/aus (`[editor] sticky_scroll` in `settings.toml`).
- **Code-Faltung** wie in Visual Studio: `Ctrl+M Ctrl+M` faltet den Block am Cursor oder öffnet ihn, `Ctrl+M Ctrl+L` faltet alles bzw. öffnet alles, `Ctrl+M Ctrl+O` faltet alle Methoden-, Konstruktor- und Property-Körper, `Ctrl+M Ctrl+P` öffnet alles (auch über `Alt → v → f/a/d/e`).
  - Faltbar sind alle mehrzeiligen Blöcke aus dem Syntaxbaum (Typen, Methoden, `if`/`for`/`switch`, Objekte, Arrays, HTML-Elemente), `#region` … `#endregion` (auch als `// #region`) und Gruppen von `using`/`import`/`use`-Zeilen.
  - Steht `{` allein in der Zeile (C#-Stil), bleibt die Signatur sichtbar. Hinter einer gefalteten Zeile steht ein `…`.
  - Fährt die Maus über den Zeilenrand, erscheinen `▾`-Pfeile; gefaltete Stellen zeigen immer `▸`. Klick auf den Pfeil oder das `…` faltet bzw. öffnet.
  - Der Cursor springt mit den Pfeiltasten über gefaltete Blöcke. Springen Suche, „Gehe zu“ oder Gehe-zu-Definition hinein, öffnet sich der Block. Änderungen darüber verschieben die Faltung, Änderungen darin öffnen sie.
- **Debugger** für .NET mit [netcoredbg](https://github.com/Samsung/netcoredbg) (MIT; gesucht im `PATH` und in `%APPDATA%\Wyven\netcoredbg`):
  - `F5` baut das Projekt (`dotnet build`, vorher werden offene Dateien gespeichert) und startet es im **eigenen Konsolenfenster** mit Debugger; `Console.ReadLine` funktioniert dort. Bei mehreren startbaren Projekten fragt Wyven einmal. `Ctrl+F5` startet ohne Debugger.
  - Breakpoints mit `F9` oder Klick auf die Zeilennummer (roter Punkt); sie wandern beim Tippen mit und bleiben beim Schließen der Datei erhalten.
  - Angehalten: gelb hinterlegte Zeile mit Pfeil, Wyven kommt nach vorne. `F5` weiter, `F10` Step Over, `F11` Step Into, `Shift+F11` Step Out, `Shift+F5` beenden.
  - Debug-Panel links (`Alt → d → d`): Variablen (aufklappbar mit `Enter`/`→`), Watch-Ausdrücke (`Alt → d → w`), Aufrufliste (Klick wechselt den Frame) und alle Breakpoints (`Entf` löscht).
  - Maus kurz auf einem Namen: zeigt seinen Wert (auch `spieler.Position.X`).
  - `Alt → d → a` hängt sich an einen laufenden `dotnet`- oder Godot-Prozess.
  - Technik: netcoredbg leitet die Konsole eines selbst gestarteten Programms immer um. Deshalb startet Wyven das Programm selbst und hängt den Debugger an; ein kleiner Startup-Hook (einmalig nach `%APPDATA%\Wyven\debughook` gebaut) lässt das Programm so lange warten und hält das Fenster am Ende offen.
- **KI-Vorschläge** beim Tippen, standardmäßig **kostenlos und lokal**:
  - Nach ~400 ms Tipp-Pause wird der Vorschlag im Hintergrund vorbereitet und erscheint erst nach 4 s ohne Tippen grau am Cursor (Einstellungen „Delay“ und „Show after“), bis zu 10 Zeilen; weitere Zeilen schieben den Code darunter nach unten. `Tab` nimmt alles an, `Ctrl+→` ein Wort, `Esc` verwirft. Vorschläge kommen nur, wenn rechts vom Cursor höchstens schließende Klammern, `;` oder `,` stehen.
  - **Lokal (Standard):** das Code-Modell **Qwen2.5-Coder 7B (Base)** läuft über [Ollama](https://ollama.com) auf der eigenen Grafikkarte. Nichts verlässt den PC, es kostet nichts und geht auch offline. Einrichtung einmalig: Ollama installieren (`winget install Ollama.Ollama`), dann `ollama pull qwen2.5-coder:7b-base`. Wichtig ist die *Base*-Variante: Die Chat-Variante antwortet mit Erklärungen statt nur mit Code. Läuft Ollama nicht, sagt Wyven es in der Statusleiste.
  - **Claude (optional):** Claude Haiku 5.5 oder Sonnet 5.5 über die Anthropic-API mit eigenem, kostenpflichtigem API-Key (zählt nicht zum Pro-Abo). Key setzen mit `Alt → a → k`; er liegt im Windows-Tresor. Dann wechselt `Alt → a → m` zwischen lokal, Haiku und Sonnet. Gesendet wird nur die offene Datei; der Dateianfang wird zwischengespeichert (Prompt Caching), bei Sonnet springt bei einer Ablehnung durch die Sicherheitsfilter ein anderes Modell ein.
  - An/aus mit `Alt → a → t` oder Klick auf den Schalter in der Statusleiste („✦ Qwen2.5-Coder 7B (local)“). Einstellungen in `settings.toml` unter `[ai]`: `provider`, `local_model`, `model`, `delay_ms`.
- **Claude Code** arbeitet mit Wyven zusammen wie mit VS Code:
  - `Alt → a → c` öffnet das Terminal und startet dort `claude`; es verbindet sich von selbst. Läuft Claude Code in einem anderen Terminal, findet es Wyven über `/ide`.
  - Claude Code sieht die offene Datei, die Auswahl (auch ihre Änderungen) und die Fehler der Language-Server und kann Dateien öffnen und speichern.
  - Ein Schalter unten in der Statusleiste (vor dem Modellnamen) schaltet die Vorschläge per Klick ein und aus.
  - Will Claude etwas ändern, zeigt Wyven die Änderung nebeneinander: links der jetzige Stand, rechts der Vorschlag. `Strg+Enter` nimmt an, `Esc` lehnt ab; Claude Code wartet auf die Antwort und schreibt die Datei nach dem Annehmen selbst.
  - Technik: Wyven startet einen MCP-Server über WebSocket, nur auf `127.0.0.1` und mit Zufalls-Token, und meldet ihn in `~/.claude/ide/<port>.lock` an. Das Terminal bekommt `CLAUDE_CODE_SSE_PORT`.
- **Inlay Hints** (`Alt → v → i`): Typen und Parameternamen grau im Code, für C# und TypeScript/JavaScript.
- **CodeLens** (`Alt → v → l`): „N references“ über Typen und Methoden; ein Klick listet sie auf. `Shift+F12` bzw. `Alt → j → r` zeigt alle Referenzen am Cursor.
- **Markdown-Vorschau** (`Alt → m`): rechts neben der Datei, aktualisiert sich beim Tippen und scrollt mit.
- **Web-Sprachen per LSP**: TypeScript/JavaScript (TypeScript 7, `tsc --lsp`), HTML, CSS/SCSS/Less und JSON (die Server aus VS Code) mit Fehlern, Vervollständigung, Hover, Gehe zu Definition, Umbenennen und Formatieren. Einrichtung einmalig: `cd %APPDATA%\Wyven\servers` und `npm install typescript vscode-langservers-extracted` (braucht Node.js).
- **Startoptionen**: `wyven <Datei> --goto=120` springt zu Zeile 120; `--terminal`, `--git`, `--diff` und `--preview` öffnen gleich Terminal, Git-Panel, Diff bzw. Markdown-Vorschau.
- **Oberfläche auf Englisch**: Leader-Menü, Befehle im Suchfeld (`File: Save`, `Git: Commit All` …), Spickzettel, Panels, Statusmeldungen und die Vorlage der `settings.toml`. Tasten heißen `Ctrl`, `PgUp` usw. In `settings.toml` gelten `theme = "dark"`/`"light"` und `indent = "spaces"`/`"tabs"`, die alten Werte `dunkel`/`hell`/`leerzeichen` funktionieren weiter.
- **Spickzettel** mit allen Kürzeln: `Alt → h`. Er wird aus denselben Listen erzeugt wie Suchfeld und Leader-Menü und ist deshalb immer aktuell.

- **C# mit Roslyn** (derselbe Server wie im C# Dev Kit von VS Code):
  - Fehler und Warnungen als Wellenlinien. Die Meldung erscheint in der Statusleiste, sobald der Cursor auf der Stelle steht, rechts stehen die Zähler `✖ / ⚠`.
  - Autovervollständigung beim Tippen und nach `.`, oder mit `Strg+Leertaste`. `Enter`/`Tab` übernimmt, `Esc` schließt.
  - **Snippets** wie in Visual Studio: Kürzel tippen und `Tab` drücken (oder in der Vervollständigung wählen), z. B. in C# `prop`, `propfull`, `propg`, `ctor`, `class`, `for`, `foreach`, `if`, `try`, `cw`, `svm`, `switch`; dazu Snippets für JavaScript/TypeScript (`log`, `forof`, `afn` …), HTML (`html5` …), CSS und Rust. `Tab`/`Shift+Tab` springen zwischen den Platzhaltern, gleiche Platzhalter ändern sich gemeinsam, `Esc` beendet. Snippets der Language-Server (z. B. Methodenaufrufe mit Parametern) funktionieren genauso.
  - **Eigene Snippets** im VS-Code-Format in `%APPDATA%\Wyven\snippets\` (`csharp.json`, `typescript.json` … oder `*.code-snippets` mit `scope`); VS-Code-Snippet-Dateien lassen sich einfach hineinkopieren. Im Suchfeld „Preferences: Configure User Snippets“ legt die Datei der aktuellen Sprache mit Beispiel an; Speichern lädt sie neu.
  - `F12` oder `Strg+Klick`: Gehe zu Definition
  - Info zum Symbol: Maus kurz still halten oder `Strg+K, Strg+I`
  - Formatieren: `Strg+K, Strg+D`
  - Umbenennen: `F2` am Namen, neuen Namen tippen, `Enter`. Gilt für alle Stellen im Projekt. Betroffene Dateien öffnen sich als ungespeicherte Tabs, damit du alles prüfen und mit `Strg+Z` zurücknehmen kannst.
  - Quick Actions: `Strg+.` zeigt die Vorschläge von Roslyn (Tippfehler korrigieren, Feld/Methode erzeugen, `using` ergänzen …). Auswahl per Ziffer oder Pfeiltasten + `Enter`.
  - Nächster Fehler: `F8` (wie in Visual Studio)

  Der Server startet automatisch, sobald eine `.cs`-Datei offen ist. Wyven meldet ihm die `.sln`/`.slnx` im Projektordner an, oder wenn es keine gibt, alle `.csproj` bis drei Ordner tief. Das Laden dauert ein paar Sekunden (beim ersten Mal inkl. `dotnet restore`), die Statusleiste zeigt den Fortschritt. Ohne Projektdatei arbeitet Roslyn im Einzeldatei-Modus.
  Woher der Server kommt, in dieser Reihenfolge: Umgebungsvariable `WYVEN_CSHARP_SERVER`, dann der Roslyn-Server aus der **C#-Extension von VS Code**, dann `csharp-ls` im PATH. Er braucht das .NET SDK.

- **Suchfeld** `Strg+Q`, ein Feld für alles:
  - ohne Präfix: Dateien im Projekt, unscharf (`prgcs` findet `Program.cs`). Beachtet `.gitignore`.
  - `>`: Befehle mit ihrem Kürzel (auch direkt mit `Strg+Shift+P`)
  - `@`: Klassen, Methoden, Eigenschaften … der aktuellen Datei (C#, JS/TS, Rust; auch `Alt → j → s`)
  - `:`: Gehe zu Zeile, z. B. `:42` oder `:42:7` (auch direkt mit `Strg+G`)

- **Explorer** links: Ordner auf/zu per Klick oder `→`/`←`, Datei öffnen per Klick oder `Enter`. Generierte Ordner (`bin`, `obj`, `.git`, `target`, `node_modules` …) werden ausgeblendet. Der Inhalt wird neu gelesen, wenn das Fenster wieder den Fokus bekommt.
- **Tabs:** `Strg+Tab` wechselt zur zuletzt benutzten Datei (Strg gedrückt halten und Tab mehrmals drücken geht weiter zurück). `Strg+W` schließt, bei ungespeicherten Änderungen erst nach einer Rückfrage. `Strg+Shift+T` öffnet den zuletzt geschlossenen Tab wieder, mittlere Maustaste schließt einen Tab.
- **Dateien:** `Strg+N` neu, `Strg+O` öffnen, `Strg+S` speichern (bei neuen Dateien mit „Speichern unter“), `Strg+Shift+S` alle speichern, `Alt → f → s` speichern unter. Geänderte Dateien speichern sich außerdem kurz nach dem letzten Tastendruck von selbst (Einstellung „Auto save“, Verzögerung „Auto save delay“).
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
| `crates/wyven-app` | Startpunkt (`wyven.exe`), Fenster, Tastenbelegung, Editor-Ansicht, Statusleiste |
| `crates/wyven-buffer` | Textpuffer (Rope), Multi-Cursor, Transaktionen, Undo/Redo, Laden/Speichern |
| `crates/wyven-syntax` | Tree-sitter: Sprachen erkennen, inkrementell parsen, Highlighting, Symbole |
| `crates/wyven-search` | Suchen/Ersetzen: Abfragen (Text, Wort, Groß/Klein, Regex), Projektsuche |
| `crates/wyven-ai` | KI-Vorschläge: lokal über Ollama oder über die Anthropic-API (beides Streaming über HTTP), Prompt-Aufbau, API-Key im Windows-Tresor |
| `crates/wyven-dap` | Debug-Adapter-Client (DAP) für netcoredbg |
| `crates/wyven-plugin` | SDK für Plugins (Rust → WebAssembly): Anfragen, Antworten, `export_plugin!` |
| `crates/wyven-git` | Git über die `git`-Kommandozeile: Status, Commit, Sync, Branches, Blame, Zeilen-Diffs |
| `crates/wyven-terminal` | Terminal: Shell in ConPTY, Emulation mit `alacritty_terminal`, Tasten-Kodierung |
| `crates/wyven-lsp` | Language-Server-Client: JSON-RPC über stdio, UTF-16-Positionen |
| `crates/wyven-render` | GPU-Rendering (wgpu): Rechtecke und Text (glyphon / cosmic-text) |

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
| `WYVEN_GPU=high` | Die starke (dedizierte) Grafikkarte statt der integrierten nutzen |
| `RUST_LOG=info` | Log-Ausgabe, z. B. Startzeiten pro Phase |

## Lizenz

MIT oder Apache-2.0, nach Wahl.
