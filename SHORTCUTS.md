# Wyven – Tastenkürzel-Schema

Stand: 10.11.2026. Gehört zu [PLAN.md](PLAN.md).

## Grundregeln

1. **Mischung:** Was man ständig braucht, liegt direkt auf `Strg`. Alles andere erreicht man über die **Leader-Taste**.
2. **Leader = `Alt` kurz antippen.** Danach folgen ein bis zwei Buchstaben mit englischer Merkhilfe, z. B. `Alt` → `g` → `c` = **g**it **c**ommit.
3. **Betriebssystem-Standards bleiben:** `Strg+C/V/X/Z/Y/S/A/F` funktionieren wie überall.
4. **Visual-Studio-Gewohnheiten bleiben**, wo du sie im Muskelgedächtnis hast (F5, F9–F11, F12, `Strg+.`, `Strg+K Strg+C/U`).
5. **QWERTZ-optimiert:** Die Standard-Kürzel brauchen keine Tasten, für die man auf einer deutschen Tastatur Shift oder AltGr drücken muss.
6. **Bewegen mit Pfeiltasten** (`Strg` = wortweise, `Shift` = auswählen, `Pos1`/`Ende`).
7. **Alles ist umbelegbar** (siehe unten, `keymap.toml`).

Mit *Vorschlag* markierte Einträge habe ich ergänzt; sie wurden nicht einzeln abgefragt.

## Wie die Leader-Taste funktioniert

- `Alt` drücken und **ohne andere Taste** loslassen → Leader-Modus.
- Nach ~300 ms erscheint ein **Popup** mit allen möglichen nächsten Tasten (wie „which-key“). Wer die Folge kennt, tippt einfach weiter, dann erscheint gar kein Popup.
- `Esc` oder ein Klick bricht ab.
- `Alt+Klick` und `Alt+Taste` (z. B. `Alt+↑`) lösen **keinen** Leader-Modus aus.
- Technisch: Windows öffnet bei einem einzelnen `Alt` sonst das Systemmenü des Fensters. Das muss Wyven unterdrücken.

## Direkte Kürzel

### Grundlagen
| Kürzel | Aktion |
|---|---|
| `Strg+C` / `X` / `V` | Kopieren / Ausschneiden / Einfügen |
| `Strg+Z` / `Strg+Y` | Rückgängig / Wiederholen |
| `Strg+S` | Speichern |
| `Strg+Umschalt+B` | Projekt bauen (Fehler im Problems-Panel, wie VS) |
| `Strg+Umschalt+S` | Alle geänderten Dateien speichern (sonst auch automatisch kurz nach dem Tippen) |
| `Strg+A` | Alles auswählen |
| `Strg+F` / `Strg+H` | Suchen / Ersetzen in der Datei |
| `F3` / `Shift+F3` | Nächster / vorheriger Treffer |
| `Alt+C` / `Alt+W` / `Alt+E` | Im Suchfeld: Groß/Klein, ganzes Wort, Regex (wie VS) |
| `Alt+R` / `Alt+A` | Im Suchfeld: ersetzen / alle ersetzen (wie VS) |
| `Strg+Shift+F` / `Strg+Shift+H` | In allen Dateien suchen / ersetzen (wie VS) |
| `Strg+Q` | **Zentrales Suchfeld**: Dateien; `>` Befehle, `@` Symbole, `:` Zeile, `,` Einstellungen |
| `Strg+G` | Gehe zu Zeile (wie VS) |
| `Strg++` / `Strg+-` / `Strg+0` | Schrift größer / kleiner / zurücksetzen *(Vorschlag)* |

### Bearbeiten
| Kürzel | Aktion |
|---|---|
| `Strg+D` | Nächstes gleiches Wort als weiteren Cursor hinzufügen |
| `Alt+Klick` | Zusätzlichen Cursor setzen |
| `Esc` | Zurück zu einem Cursor |
| `Alt+↑` / `Alt+↓` | Zeile(n) verschieben |
| `Alt+Shift+↑` / `↓` | Zeile(n) duplizieren |
| `Strg+Shift+L` | Zeile löschen (wie VS) |
| `Strg+Enter` / `Strg+Shift+Enter` | Neue Zeile darunter / darüber (wie VS) *(Vorschlag)* |
| `Strg+K, Strg+C` | Auskommentieren (wie VS) |
| `Strg+K, Strg+U` | Kommentar entfernen (wie VS) |
| `Strg+#` | Kommentar umschalten (QWERTZ, eine Hand) *(Vorschlag)* |
| `Strg+K, Strg+D` | Dokument formatieren (wie VS) |
| `Strg+M, Strg+M` | Block am Cursor falten/öffnen (wie VS) |
| `Strg+M, Strg+L` | Alles falten / alles öffnen (wie VS) |
| `Strg+M, Strg+O` | Auf Definitionen falten (wie VS) |
| `Strg+M, Strg+P` | Alles öffnen (wie VS) |
| `Strg+Leertaste` | Autovervollständigung öffnen *(Vorschlag)* |
| Kürzel + `Tab` | Snippet einfügen (`prop`, `ctor`, `foreach` …); `Tab`/`Shift+Tab` nächster/voriger Platzhalter, `Esc` fertig (wie VS) |
| `Strg+.` | Quick Actions: Fixes, Usings, Refactorings (wie VS) |
| `F2` | Umbenennen *(Vorschlag; VS nutzt `Strg+R, Strg+R`)* |

### Navigation
| Kürzel | Aktion |
|---|---|
| `F12` / `Strg+Klick` | Gehe zu Definition |
| `Shift+F12` | Alle Referenzen |
| `Strg+F12` | Gehe zu Implementierung (wie VS) |
| `F2` / `Entf` / `Strg+N` / `Strg+Shift+N` | Im Explorer: umbenennen / in den Papierkorb / neue Datei / neuer Ordner |
| `Strg+Enter` / `Esc` | Im Diff von Claude Code: Änderung annehmen / ablehnen |
| `Strg+Tab` | Zuletzt benutzte Datei; gedrückt halten zeigt die Liste (wie VS) |
| `Strg+W` | Tab schließen |
| `Strg+Shift+T` | Zuletzt geschlossenen Tab wieder öffnen *(Vorschlag)* |
| `Alt+←` / `Alt+→` | Zurück / vorwärts zur letzten Cursor-Position *(Vorschlag)* |

### Debuggen (wie Visual Studio)
| Kürzel | Aktion |
|---|---|
| `F5` | Debuggen starten / fortsetzen |
| `Shift+F5` | Debuggen beenden |
| `Strg+F5` | Ohne Debugger starten (eigenes Konsolenfenster) |
| `F9` | Breakpoint umschalten |
| `F10` / `F11` / `Shift+F11` | Step Over / Step Into / Step Out |

### KI-Vorschläge
| Kürzel | Aktion |
|---|---|
| `Tab` | Vorschlag komplett annehmen (nur wenn einer angezeigt wird, sonst normales Tab) |
| `Strg+→` | Nur das nächste Wort annehmen (nur wenn ein Vorschlag angezeigt wird) |
| `Esc` | Vorschlag verwerfen |

## Leader-Menü (`Alt`, dann …)

Die Navigation ist englisch: Die Buchstaben sind Anfangsbuchstaben der englischen Befehle, Wyven zeigt die Beschriftungen ebenfalls auf Englisch.

| Taste | Gruppe | Folgetasten |
|---|---|---|
| `e` | **e**xplorer | Explorer ein/aus |
| `t` | **t**erminal | Terminal ein/aus |
| `s` | **s**earch | Suchpanel ein/aus |
| `g` | **g**it | `g` Panel · `c` alles committen · `s` Sync (pull + push) · `b` Branch wechseln · `n` neuer Branch · `d` Diff der Datei · `l` Blame (Zeile) |
| `d` | **d**ebug | `d` Panel · `r` starten/fortsetzen · `s` beenden · `b` Breakpoint umschalten · `w` Watch hinzufügen · `a` an Prozess anhängen (z. B. Godot) |
| `j` | **j**ump | `d` Definition · `i` Implementierung · `r` Referenzen · `s` Symbol in Datei · `l` Zeile |
| `c` | **c**ode | `r` umbenennen · `f` formatieren · `a` Quick Actions · `o` Usings ordnen · `e` nächster Fehler |
| `f` | **f**ile | `n` new file · `o` open file · `s` save as · `r` umbenennen · `d` in den Papierkorb · `f` neue Datei im Explorer-Ordner · `k` neuer Ordner · `c` close all |
| `p` | **p**roject | `o` Ordner öffnen · `r` zuletzt geöffnet (Projekt-Wechsler) · `a` Ordner zum Workspace hinzufügen · `f` Workspace-Datei öffnen · `w` Workspace speichern unter |
| `w` | **w**indow | `v` rechts teilen · `h` unten teilen · `o` / `←↑→↓` andere Hälfte · `c` Teilung schließen · `r` Panel-Anordnung zurücksetzen |
| `v` | **v**iew | `m` Minimap (Blöcke / Leiste / aus) · `s` Sticky Scroll an/aus · `i` Inlay Hints · `l` CodeLens · `c` Farbschema wählen · `t` hell/dunkel · `w` Zeilenumbruch · `z` Zen-Modus · `f` Faltung am Cursor · `a` alles falten/öffnen · `d` auf Definitionen falten · `e` alles öffnen |
| `r` | **r**un & build | `b` bauen · `t` testen · `r` im Terminal starten · `c` aufräumen · `x` abbrechen · `p` Problems · `o` Task-Ausgabe |
| `a` | **a**i | `t` Vorschläge an/aus · `m` Modell wechseln (Haiku/Sonnet) · `k` API-Key setzen · `c` Claude Code im Terminal öffnen |
| `m` | **m**arkdown | Vorschau ein/aus |
| `,` | Einstellungen | Einstellungsseite öffnen |
| `h` | **h**elp | Spickzettel mit allen Kürzeln |

## Maus

| Aktion | Wirkung |
|---|---|
| Klick auf ein Symbol der Aktivitätsleiste | Panel öffnen; nochmal: schließen |
| Klick auf × im Tab / mittlere Maustaste | Tab schließen |
| Rechtsklick auf Explorer-Eintrag, Tab oder Text | Kontextmenü |
| Panel-Kopf ziehen | Andock-Kompass: neben ein Pane (teilt es), als Tab hinein, neben den Editor oder an eine Fensterkante; ohne Ziel loslassen (oder `Strg`) = eigenes Fenster |
| Tab-Leiste eines schwebenden Panels aufs Hauptfenster ziehen | wieder andocken |
| Editor-Tab ziehen | Editor nach links / rechts / oben / unten teilen |
| Abstand zwischen zwei Panes ziehen | Größe ändern |

## Lernhilfen

- **Leader-Popup:** zeigt nach `Alt` alle nächsten Tasten.
- **Spickzettel** (`Alt` → `h`): durchsuchbare Übersicht aller Kürzel, auch selbst geänderter.
- **Tipps:** Wenn du einen Befehl per Maus oder Suchfeld startest, für den es ein Kürzel gibt, zeigt Wyven es einmal klein in der Statusleiste an. Tipps lassen sich abschalten.

## Eigene Belegung: `keymap.toml`

Liegt im Wyven-Konfigurationsordner (`%APPDATA%\Wyven\keymap.toml`, öffnen mit „Preferences: Open Keymap File“). Es stehen nur Abweichungen vom Standard drin; beim ersten Öffnen legt Wyven die Datei mit einer Liste aller Befehlsnamen an. Speichern in Wyven wendet sie sofort an.

```toml
# Direktes Kürzel ändern
[keys]
"ctrl+shift+d" = "view.toggle_word_wrap"
"f12" = "none"                  # Kürzel entfernen

# Leader-Menü erweitern (Alt → g → x)
[leader.g]
"x" = "git.sync"
```

Befehlsnamen entstehen aus den Titeln im Suchfeld: „View: Toggle Word Wrap“ → `view.toggle_word_wrap`. Belegbar ist alles, was im Suchfeld unter `>` steht, auch Befehle von Plugins.

Kürzel beziehen sich auf die **angezeigte Taste** (das Zeichen auf der Taste), nicht auf ihre Position. So steht in Doku und Spickzettel immer das, was auf deiner Tastatur aufgedruckt ist.

## Konflikte, die beim Bau beachtet werden müssen

- `Alt` antippen (Leader) und `Alt+Klick` / `Alt+Pfeil` dürfen sich nicht gegenseitig auslösen.
- Windows-Systemmenü bei einzelnem `Alt` unterdrücken.
- `Strg+→` heißt „Wort annehmen“ nur, solange ein KI-Vorschlag sichtbar ist, sonst „Wort weiter“.
- `Tab` heißt „KI annehmen“ nur bei sichtbarem Vorschlag, sonst einrücken.
- Im Terminal gehen `Strg+C` und Co. an das laufende Programm. Kopieren dort mit `Strg+Shift+C` *(Vorschlag)*.
- `Strg+Q` beendet unter Linux in vielen Programmen die App. In Wyven öffnet es das Suchfeld; beendet wird mit `Alt+F4` bzw. über das Fenster.
