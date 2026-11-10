# Wyvern

Ein schneller, sparsamer Code-Editor in Rust, mit VS Code als Vorbild.
Der ganze Plan steht in [PLAN.md](PLAN.md).

**Stand: Phase 0 (Grundgerüst).** Ein Fenster, das Text über die GPU zeichnet und dabei RAM und Eingabe-Latenz misst.

## Starten

```bash
cargo run --release
```

`cargo run` (ohne `--release`) startet schneller nach Änderungen, läuft aber langsamer und zeigt ein Konsolenfenster mit Logs.

## Aufbau

| Crate | Aufgabe |
|---|---|
| `crates/wyvern-app` | Startpunkt (`wyvern.exe`), Fenster, Eingabe, Messwerte |
| `crates/wyvern-render` | GPU-Rendering (wgpu) und Textdarstellung (glyphon / cosmic-text) |

## Messwerte (Phase 0)

Gemessen auf Windows 11, Laptop mit Intel Arc + RTX 4050, Release-Build:

| | OpenGL (Standard) | DX12 |
|---|---|---|
| RAM (privat, wie im Task-Manager) | ~33 MB | ~285 MB |
| Start bis erstes Bild | ~0,9 s | ~1,6–2,9 s |
| Taste → Bild an GPU übergeben | ~2,5 ms | – |

Unter Windows lädt DX12 beim Start die Treiber *aller* Grafikkarten. Deshalb ist OpenGL der Standard, DX12 dient als Rückfallebene.

Die Statuszeile im Fenster zeigt den *Working Set* (inkl. geteilter Treiber-Bibliotheken). Der Wert ist daher höher als „privat“.

## Umgebungsvariablen zum Testen

| Variable | Wirkung |
|---|---|
| `WGPU_BACKEND=dx12` / `vulkan` / `gl` | Grafik-Backend erzwingen |
| `WYVERN_GPU=high` | Die starke (dedizierte) Grafikkarte statt der integrierten nutzen |
| `RUST_LOG=info` | Log-Ausgabe, z. B. Startzeiten pro Phase |

## Lizenz

MIT oder Apache-2.0, nach Wahl.
