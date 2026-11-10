# Wyvern – Tastenkürzel-Schema

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
- Technisch: Windows öffnet bei einem einzelnen `Alt` sonst das Systemmenü des Fensters. Das muss Wyvern unterdrücken.

## Direkte Kürzel

### Grundlagen
| Kürzel | Aktion |
|---|---|
| `Strg+C` / `X` / `V` | Kopieren / Ausschneiden / Einfügen |
| `Strg+Z` / `Strg+Y` | Rückgängig / Wiederholen |
| `Strg+S` | Speichern |
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
| `Strg+Leertaste` | Autovervollständigung öffnen *(Vorschlag)* |
| `Strg+.` | Quick Actions: Fixes, Usings, Refactorings (wie VS) |
| `F2` | Umbenennen *(Vorschlag; VS nutzt `Strg+R, Strg+R`)* |

### Navigation
| Kürzel | Aktion |
|---|---|
| `F12` / `Strg+Klick` | Gehe zu Definition |
| `Strg+Tab` | Zuletzt benutzte Datei; gedrückt halten zeigt die Liste (wie VS) |
| `Strg+W` | Tab schließen |
| `Strg+Shift+T` | Zuletzt geschlossenen Tab wieder öffnen *(Vorschlag)* |
| `Alt+←` / `Alt+→` | Zurück / vorwärts zur letzten Cursor-Position *(Vorschlag)* |

### Debuggen (wie Visual Studio)
| Kürzel | Aktion |
|---|---|
| `F5` | Debuggen starten / fortsetzen |
| `Shift+F5` | Debuggen beenden |
| `Strg+F5` | Ohne Debugger starten |
| `F9` | Breakpoint umschalten |
| `F10` / `F11` / `Shift+F11` | Step Over / Step Into / Step Out |

### KI-Vorschläge
| Kürzel | Aktion |
|---|---|
| `Tab` | Vorschlag komplett annehmen (nur wenn einer angezeigt wird, sonst normales Tab) |
| `Strg+→` | Nur das nächste Wort annehmen (nur wenn ein Vorschlag angezeigt wird) |
| `Esc` | Vorschlag verwerfen |

## Leader-Menü (`Alt`, dann …)

| Taste | Gruppe | Folgetasten |
|---|---|---|
| `e` | **e**xplorer | Explorer ein/aus |
| `t` | **t**erminal | Terminal ein/aus |
| `s` | **s**earch | Suchpanel ein/aus |
| `g` | **g**it | `g` Panel · `c` alles committen · `s` Sync (pull + push) · `b` Branch wechseln · `n` neuer Branch · `d` Diff der Datei · `l` Blame (Zeile) |
| `d` | **d**ebug | `d` Panel · `b` alle Breakpoints · `w` Watch hinzufügen · `a` an Prozess anhängen (z. B. Godot) |
| `j` | **j**ump | `d` Definition · `i` Implementierung · `r` Referenzen · `s` Symbol in Datei · `l` Zeile |
| `c` | **c**ode | `r` umbenennen · `f` formatieren · `a` Quick Actions · `o` Usings ordnen · `e` nächster Fehler |
| `f` | **f**ile | `n` neu · `o` öffnen · `s` speichern unter · `r` umbenennen · `w` alle schließen |
| `p` | **p**roject | `o` Ordner öffnen · `r` zuletzt geöffnet (Projekt-Wechsler) · `a` Ordner zum Workspace hinzufügen |
| `w` | **w**indow | `v` rechts teilen · `h` unten teilen · `←↑→↓` Fokus wechseln · `c` Teilung schließen |
| `v` | **v**iew | `m` Minimap · `s` Sticky Scroll · `i` Inlay Hints · `w` Zeilenumbruch · `z` Zen-Modus |
| `a` | **a**i | `t` Vorschläge an/aus · `m` Modell wechseln (Haiku/Sonnet) · `k` API-Key setzen · `c` Claude Code im Terminal öffnen |
| `m` | **m**arkdown | Vorschau ein/aus |
| `,` | Einstellungen | Einstellungsseite öffnen |
| `h` | **h**elp | Spickzettel mit allen Kürzeln |

## Lernhilfen

- **Leader-Popup:** zeigt nach `Alt` alle nächsten Tasten.
- **Spickzettel** (`Alt` → `h`): durchsuchbare Übersicht aller Kürzel, auch selbst geänderter.
- **Tipps:** Wenn du einen Befehl per Maus oder Suchfeld startest, für den es ein Kürzel gibt, zeigt Wyvern es einmal klein in der Statusleiste an. Tipps lassen sich abschalten.

## Eigene Belegung: `keymap.toml`

Liegt im Wyvern-Konfigurationsordner. Es stehen nur Abweichungen vom Standard drin:

```toml
# Direktes Kürzel ändern
[keys]
"ctrl+shift+d" = "edit.duplicate_line"
"f12" = "none"                  # Kürzel entfernen

# Leader-Menü erweitern
[leader.g]
"s" = "git.stash"

# Nur in bestimmten Situationen
[keys.when.terminal]
"ctrl+v" = "terminal.paste"
```

Kürzel beziehen sich auf die **angezeigte Taste** (das Zeichen auf der Taste), nicht auf ihre Position. So steht in Doku und Spickzettel immer das, was auf deiner Tastatur aufgedruckt ist.

## Konflikte, die beim Bau beachtet werden müssen

- `Alt` antippen (Leader) und `Alt+Klick` / `Alt+Pfeil` dürfen sich nicht gegenseitig auslösen.
- Windows-Systemmenü bei einzelnem `Alt` unterdrücken.
- `Strg+→` heißt „Wort annehmen“ nur, solange ein KI-Vorschlag sichtbar ist, sonst „Wort weiter“.
- `Tab` heißt „KI annehmen“ nur bei sichtbarem Vorschlag, sonst einrücken.
- Im Terminal gehen `Strg+C` und Co. an das laufende Programm. Kopieren dort mit `Strg+Shift+C` *(Vorschlag)*.
- `Strg+Q` beendet unter Linux in vielen Programmen die App. In Wyvern öffnet es das Suchfeld; beendet wird mit `Alt+F4` bzw. über das Fenster.
