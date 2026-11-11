# Wyven – Oberfläche (UI-Leitfaden)

Stand: 10.11.2026. Gilt für alles, was ab jetzt gebaut oder umgebaut wird.
Gehört zu [PLAN.md](PLAN.md) und [SHORTCUTS.md](SHORTCUTS.md).

## Grundstil

| Thema | Entscheidung |
|---|---|
| Vorbild | Visual Studio 2026: minimalistisch, aber anpassbar |
| Schrift Oberfläche | **Segoe UI** (Explorer, Tabs, Menüs, Popups, Statusleiste) |
| Schrift Code | Cascadia Mono (einstellbar), ebenso im Terminal |
| Titelleiste | **Eigene Titelleiste**: Projektname, Suchfeld (Strg+Q) in der Mitte, Fensterknöpfe rechts; ersetzt die Windows-Titelleiste |
| Dichte | **Ausgewogen**: etwas mehr Luft in Tabs, Explorer und Popups; Code bleibt dicht |
| Ecken | **Leicht abgerundet** (~6 px) für Popups, Suchfeld, Menüs, Tabs |
| Hintergrund | **Deckend**, kein Mica/Transparenz |
| Akzentfarbe | **Bernstein** (aktiver Tab, Treffer, Hervorhebungen) |
| Scrollbalken | **Keine**; Orientierung über Minimap bzw. Markierungsleiste |

## Animationen

Alle Animationen sind **kurz** (Richtwert 80–120 ms) und dürfen das Tippen nie verzögern.

- Weiches Scrollen (Mausrad, Bild↑/↓, Sprünge zu Zeilen/Fehlern)
- Blinkender Cursor (blinkt nicht, solange getippt wird)
- Sanftes Ein-/Ausblenden von Popups und Panels

## Bereiche

### Seitenleiste
- Schmale **Icon-Leiste (Activity Bar)** ganz links: Explorer, Suche, Git, Debug.
- **Einklappbar** per Kürzel (Leader, z. B. `Alt → v → a`).

### Explorer und Tabs
- **Farbige Datei-Icons** pro Dateityp (C# lila, JSON gelb, Ordner …), im Explorer und in den Tabs.
- **Tabs mit Schließen-×**: Icon, Name, × beim Überfahren; ungespeichert = Punkt statt ×. Tabs per Maus verschiebbar.

### Statusleiste
Nur das Nötigste:
- **Fehler/Warnungen** (`✖ n ⚠ m`), klickbar → springt zum nächsten Fehler
- **Sprache** der aktuellen Datei (bei C# mit Server-Status)
- **RAM und Taste→Bild-Latenz**
- Kurze Meldungen (gespeichert, Hinweise, Fehler beim Speichern)

Nicht in der Statusleiste: Zeile/Spalte, Zeilenende, Git-Branch.

### Editor
- **Einrückungslinien** (dünne senkrechte Linien pro Ebene)
- **Bunte Klammerpaare** (Farbe je Verschachtelungsebene)
- **Passende Klammer hervorheben**, wenn der Cursor an einer Klammer steht
- **Gleiches Wort hervorheben**: alle Vorkommen des Worts unter dem Cursor dezent hinterlegt
- **Git-Farbbalken am Rand** neben den Zeilennummern: grün = neu, blau = geändert, rot = gelöscht
- **Minimap umschaltbar**: Standard farbige Blöcke (wie VS Code) mit Markierungen für Fehler/Treffer/Git; per Einstellung schmale Markierungsleiste

## Umsetzungsreihenfolge

Die Punkte werden nach und nach in die laufende Arbeit eingebaut, nicht alle auf einmal:

1. Segoe UI für die Oberfläche, ausgewogene Abstände, abgerundete Popups, Statusleiste verschlanken
2. Datei-Icons und Tabs mit ×
3. Code-Hilfen (Einrückungslinien, Klammerpaare, Wort-Hervorhebung)
4. Eigene Titelleiste und Activity Bar
5. Animationen (weiches Scrollen, Cursor-Blinken, Ein-/Ausblenden)
6. Git-Randbalken und Minimap mit den jeweiligen Phase-2-Teilen
