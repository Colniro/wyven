//! VS Code color themes: found in the installed VS Code extensions (`~/.vscode/extensions`) and in
//! `%APPDATA%\Wyven\themes` (loose `.json` files or unpacked extensions), turned into a [`Theme`].
//!
//! A VS Code theme colors the editor (`colors`) and the syntax (`tokenColors`, TextMate scopes).
//! Parts it does not define are derived from its own background and foreground, so everything
//! matches. Theme files are JSON with comments and trailing commas, and may `include` a base file.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde_json::Value;
use wyven_render::Rgba;

use crate::theme::{SyntaxColors, Theme};

/// A theme that can be picked.
#[derive(Debug, Clone, PartialEq)]
pub struct ThemeEntry {
    /// Shown in the picker and stored in settings.toml (`theme = "Dracula Theme"`).
    pub label: String,
    pub path: PathBuf,
    /// Where it comes from ("Dracula Official", "own folder").
    pub source: String,
}

fn vscode_extensions() -> Option<PathBuf> {
    let home = std::env::var_os("USERPROFILE").or_else(|| std::env::var_os("HOME"))?;
    Some(PathBuf::from(home).join(".vscode").join("extensions"))
}

/// Wyven's own theme folder: `%APPDATA%\Wyven\themes`.
pub fn own_dir() -> Option<PathBuf> {
    crate::config::config_dir().map(|d| d.join("themes"))
}

/// The themes an extension folder contributes (from its package.json).
fn extension_themes(dir: &Path) -> Vec<ThemeEntry> {
    let Ok(text) = std::fs::read_to_string(dir.join("package.json")) else { return Vec::new() };
    let Ok(pkg) = serde_json::from_str::<Value>(&strip_jsonc(&text)) else { return Vec::new() };
    let source = pkg["displayName"]
        .as_str()
        .filter(|s| !s.starts_with('%'))
        .or(pkg["name"].as_str())
        .unwrap_or("")
        .to_string();
    pkg["contributes"]["themes"]
        .as_array()
        .map(|list| {
            list.iter()
                .filter_map(|t| {
                    let label = t["label"].as_str().or(t["id"].as_str())?.to_string();
                    let path = dir.join(t["path"].as_str()?);
                    path.is_file().then(|| ThemeEntry { label, path, source: source.clone() })
                })
                .collect()
        })
        .unwrap_or_default()
}

/// All VS Code themes, sorted by name (several versions of an extension: the newest wins).
pub fn discover() -> Vec<ThemeEntry> {
    let mut found: Vec<ThemeEntry> = Vec::new();
    let mut ext_dirs: Vec<PathBuf> = vscode_extensions()
        .and_then(|d| std::fs::read_dir(d).ok())
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.is_dir())
        .collect();
    // "name-1.2.0" before "name-1.10.0" would be wrong, but later folders overwrite earlier
    // ones with the same label, and VS Code removes old versions anyway.
    ext_dirs.sort();
    for dir in ext_dirs {
        for t in extension_themes(&dir) {
            found.retain(|f| f.label != t.label);
            found.push(t);
        }
    }
    if let Some(own) = own_dir().and_then(|d| std::fs::read_dir(d).ok()) {
        for e in own.flatten() {
            let p = e.path();
            if p.is_dir() {
                found.extend(extension_themes(&p));
            } else if p.extension().is_some_and(|x| x.eq_ignore_ascii_case("json")) {
                let label = std::fs::read_to_string(&p)
                    .ok()
                    .and_then(|t| serde_json::from_str::<Value>(&strip_jsonc(&t)).ok())
                    .and_then(|v| v["name"].as_str().map(str::to_string))
                    .unwrap_or_else(|| p.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default());
                found.retain(|f| f.label != label);
                found.push(ThemeEntry { label, path: p, source: "own folder".into() });
            }
        }
    }
    found.sort_by_key(|t| t.label.to_lowercase());
    found
}

/// JSON with comments (`//`, `/* */`) and trailing commas → plain JSON.
pub fn strip_jsonc(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    let mut in_str = false;
    while i < chars.len() {
        let c = chars[i];
        if in_str {
            out.push(c);
            if c == '\\' && i + 1 < chars.len() {
                out.push(chars[i + 1]);
                i += 2;
                continue;
            }
            if c == '"' {
                in_str = false;
            }
            i += 1;
            continue;
        }
        match (c, chars.get(i + 1)) {
            ('"', _) => {
                in_str = true;
                out.push(c);
                i += 1;
            }
            ('/', Some('/')) => {
                while i < chars.len() && chars[i] != '\n' {
                    i += 1;
                }
            }
            ('/', Some('*')) => {
                i += 2;
                while i + 1 < chars.len() && !(chars[i] == '*' && chars[i + 1] == '/') {
                    i += 1;
                }
                i += 2;
            }
            (',', _) => {
                // Drop a comma that only has whitespace (or comments, already gone) before } or ].
                let mut j = i + 1;
                while j < chars.len() && chars[j].is_whitespace() {
                    j += 1;
                }
                let comment_next = chars.get(j) == Some(&'/');
                if comment_next || !matches!(chars.get(j), Some('}' | ']')) {
                    out.push(c);
                }
                i += 1;
            }
            _ => {
                out.push(c);
                i += 1;
            }
        }
    }
    // A comma before a comment before } is only visible now.
    if out.contains(',') {
        let again: Vec<char> = out.chars().collect();
        let mut fixed = String::with_capacity(out.len());
        let mut in_str = false;
        let mut k = 0;
        while k < again.len() {
            let c = again[k];
            if in_str {
                fixed.push(c);
                if c == '\\' && k + 1 < again.len() {
                    fixed.push(again[k + 1]);
                    k += 2;
                    continue;
                }
                if c == '"' {
                    in_str = false;
                }
            } else if c == '"' {
                in_str = true;
                fixed.push(c);
            } else if c == ',' {
                let mut j = k + 1;
                while j < again.len() && again[j].is_whitespace() {
                    j += 1;
                }
                if !matches!(again.get(j), Some('}' | ']')) {
                    fixed.push(c);
                }
            } else {
                fixed.push(c);
            }
            k += 1;
        }
        return fixed;
    }
    out
}

/// One `tokenColors` rule: its scope selectors and foreground.
struct Rule {
    scopes: Vec<String>,
    foreground: Option<Rgba>,
    background: Option<Rgba>,
}

/// A theme file with its includes merged: `colors` and the token rules in order.
#[derive(Default)]
struct Raw {
    colors: HashMap<String, Rgba>,
    rules: Vec<Rule>,
    kind: Option<bool>,
}

fn read_json(path: &Path) -> Result<Value, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    serde_json::from_str(&strip_jsonc(&text)).map_err(|e| format!("{}: {e}", path.display()))
}

fn rules_from(list: &Value) -> Vec<Rule> {
    list.as_array()
        .map(|rules| {
            rules
                .iter()
                .map(|r| {
                    let scopes = match &r["scope"] {
                        Value::String(s) => s.split(',').map(|x| x.trim().to_string()).collect(),
                        Value::Array(a) => a.iter().filter_map(|x| x.as_str()).map(|x| x.trim().to_string()).collect(),
                        _ => Vec::new(),
                    };
                    let color = |k: &str| r["settings"][k].as_str().and_then(Rgba::parse);
                    Rule { scopes, foreground: color("foreground"), background: color("background") }
                })
                .collect()
        })
        .unwrap_or_default()
}

fn load_raw(path: &Path, depth: usize) -> Result<Raw, String> {
    let v = read_json(path)?;
    let dir = path.parent().unwrap_or(Path::new("."));
    let mut raw = match v["include"].as_str() {
        Some(inc) if depth < 5 => load_raw(&dir.join(inc), depth + 1).unwrap_or_default(),
        _ => Raw::default(),
    };
    if let Some(colors) = v["colors"].as_object() {
        for (k, c) in colors {
            if let Some(c) = c.as_str().and_then(Rgba::parse) {
                raw.colors.insert(k.clone(), c);
            }
        }
    }
    match &v["tokenColors"] {
        Value::String(file) if file.ends_with(".json") => {
            if let Ok(t) = read_json(&dir.join(file)) {
                let list = if t.is_array() { t } else { t["tokenColors"].clone() };
                raw.rules.extend(rules_from(&list));
            }
        }
        list => raw.rules.extend(rules_from(list)),
    }
    if let Some(t) = v["type"].as_str() {
        raw.kind = Some(t != "light" && t != "hc-light");
    }
    Ok(raw)
}

// ----- color helpers -----

fn mix(a: Rgba, b: Rgba, t: f32) -> Rgba {
    let m = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t).round().clamp(0.0, 255.0) as u8;
    Rgba(m(a.0, b.0), m(a.1, b.1), m(a.2, b.2), 255)
}

/// A color with alpha painted onto an opaque background.
fn over(c: Rgba, bg: Rgba) -> Rgba {
    mix(bg, Rgba(c.0, c.1, c.2, 255), c.3 as f32 / 255.0)
}

fn luminance(c: Rgba) -> f32 {
    let [r, g, b, _] = c.to_linear();
    0.2126 * r + 0.7152 * g + 0.0722 * b
}

fn contrast(a: Rgba, b: Rgba) -> f32 {
    let (la, lb) = (luminance(a), luminance(b));
    (la.max(lb) + 0.05) / (la.min(lb) + 0.05)
}

/// Does selector `sel` (e.g. "keyword") cover scope `scope` (e.g. "keyword.control.cs")?
/// Returns how specific the match is (number of parts), 0 for no match.
fn selector_matches(sel: &str, scope: &str) -> usize {
    // Descendant selectors ("meta.class entity.name") and exclusions are not supported.
    if sel.is_empty() || sel.contains(' ') || sel.contains('-') && sel.starts_with('-') {
        return 0;
    }
    if scope == sel || scope.starts_with(&format!("{sel}.")) {
        sel.split('.').count()
    } else {
        0
    }
}

/// The foreground the theme gives the first of `scopes` it has a rule for.
fn token_color(rules: &[Rule], scopes: &[&str]) -> Option<Rgba> {
    for scope in scopes {
        let mut best: Option<(usize, Rgba)> = None;
        for r in rules {
            let Some(fg) = r.foreground else { continue };
            let spec = r.scopes.iter().map(|s| selector_matches(s, scope)).max().unwrap_or(0);
            if spec > 0 && best.is_none_or(|(b, _)| spec >= b) {
                best = Some((spec, fg));
            }
        }
        if let Some((_, c)) = best {
            return Some(c);
        }
    }
    None
}

/// Loads a VS Code theme file as a Wyven theme called `label`.
pub fn load(label: &str, path: &Path) -> Result<Theme, String> {
    let raw = load_raw(path, 0)?;
    Ok(build(label, &raw))
}

fn build(label: &str, raw: &Raw) -> Theme {
    let c = |k: &str| raw.colors.get(k).copied();
    let first = |keys: &[&str]| keys.iter().find_map(|k| c(k));
    let global = raw.rules.iter().find(|r| r.scopes.is_empty());
    let guess_dark = raw.kind.unwrap_or(true);
    let bg = over(
        c("editor.background").or(global.and_then(|g| g.background)).unwrap_or(if guess_dark {
            Rgba::rgb(0x1E, 0x1E, 0x1E)
        } else {
            Rgba::rgb(0xFF, 0xFF, 0xFF)
        }),
        Rgba::rgb(0, 0, 0),
    );
    let dark = luminance(bg) < 0.25;
    let base = if dark { Theme::dark() } else { Theme::light() };
    let fg = over(
        first(&["editor.foreground", "foreground"])
            .or(global.and_then(|g| g.foreground))
            .unwrap_or(base.text),
        bg,
    );
    let tint = |t: f32| mix(bg, fg, t);
    let on = |k: &[&str], under: Rgba| first(k).map(|x| over(x, under));
    let shade = if dark { Rgba::rgb(0, 0, 0) } else { Rgba::rgb(0x60, 0x60, 0x60) };

    let syntax_color = |scopes: &[&str], fallback: Rgba| token_color(&raw.rules, scopes).map_or(fallback, |x| over(x, bg));
    let syntax = SyntaxColors {
        keyword: syntax_color(&["keyword.control", "keyword", "storage.type", "storage.modifier"], base.syntax.keyword),
        type_: syntax_color(
            &["entity.name.type", "entity.name.class", "support.type", "support.class", "storage.type"],
            base.syntax.type_,
        ),
        function: syntax_color(&["entity.name.function", "support.function", "meta.function-call"], base.syntax.function),
        string: syntax_color(&["string.quoted", "string"], base.syntax.string),
        escape: syntax_color(&["constant.character.escape", "constant.character"], base.syntax.escape),
        comment: syntax_color(&["comment.line", "comment"], base.syntax.comment),
        number: syntax_color(&["constant.numeric"], base.syntax.number),
        constant: syntax_color(&["constant.language", "variable.other.constant", "constant"], base.syntax.constant),
        property: syntax_color(
            &["variable.other.property", "variable.other.object.property", "support.variable.property", "meta.object-literal.key", "variable"],
            fg,
        ),
        punctuation: syntax_color(&["punctuation"], tint(0.7)),
        attribute: syntax_color(&["entity.other.attribute-name", "meta.attribute"], base.syntax.attribute),
        link: syntax_color(&["markup.underline.link", "string.other.link"], base.syntax.link),
    };

    let accent = on(&["tab.activeBorderTop", "activityBarBadge.background", "focusBorder", "button.background"], bg)
        .filter(|a| contrast(*a, bg) > 1.8)
        .unwrap_or(syntax.keyword);
    let panel_bg = on(&["sideBar.background"], bg).unwrap_or_else(|| mix(bg, shade, if dark { 0.18 } else { 0.04 }));
    let panel_text = on(&["sideBar.foreground", "foreground"], panel_bg).unwrap_or_else(|| mix(panel_bg, fg, 0.85));
    let panel_dim = on(&["descriptionForeground"], panel_bg)
        .filter(|d| contrast(*d, panel_bg) > 2.0)
        .unwrap_or_else(|| mix(panel_bg, panel_text, 0.55));
    let status_bg = on(&["statusBar.background"], bg).unwrap_or(panel_bg);
    let status_text = on(&["statusBar.foreground"], status_bg).unwrap_or(panel_dim);
    let selection = c("editor.selectionBackground").unwrap_or(base.selection);
    let ansi_keys = [
        "Black", "Red", "Green", "Yellow", "Blue", "Magenta", "Cyan", "White", "BrightBlack", "BrightRed", "BrightGreen",
        "BrightYellow", "BrightBlue", "BrightMagenta", "BrightCyan", "BrightWhite",
    ];
    let ansi: Vec<Rgba> = ansi_keys.iter().filter_map(|k| c(&format!("terminal.ansi{k}"))).map(|x| over(x, panel_bg)).collect();

    Theme {
        name: Box::leak(label.to_string().into_boxed_str()),
        dark,
        background: bg,
        text: fg,
        gutter_text: on(&["editorLineNumber.foreground"], bg).unwrap_or_else(|| tint(0.35)),
        gutter_current: on(&["editorLineNumber.activeForeground"], bg).unwrap_or_else(|| tint(0.8)),
        current_line: on(&["editor.lineHighlightBackground"], bg).filter(|l| *l != bg).unwrap_or_else(|| tint(0.05)),
        selection: if selection.3 == 255 { selection.with_alpha(0xC0) } else { selection },
        cursor: on(&["editorCursor.foreground"], bg).unwrap_or(fg),
        status_bg,
        status_text,
        status_accent: if contrast(accent, status_bg) > 3.0 { accent } else { status_text },
        panel_bg,
        panel_text,
        panel_dim,
        row_selected: on(&["list.activeSelectionBackground"], panel_bg).unwrap_or_else(|| mix(panel_bg, accent, 0.3)),
        row_selected_unfocused: on(&["list.inactiveSelectionBackground"], panel_bg)
            .unwrap_or_else(|| mix(panel_bg, panel_text, 0.12)),
        accent,
        tab_bar: on(&["editorGroupHeader.tabsBackground"], bg).unwrap_or(panel_bg),
        tab_active: on(&["tab.activeBackground"], bg).unwrap_or(bg),
        tab_text: on(&["tab.inactiveForeground"], panel_bg).unwrap_or(panel_dim),
        tab_text_active: on(&["tab.activeForeground"], bg).unwrap_or(fg),
        divider: on(&["editorGroup.border", "sideBar.border", "panel.border", "contrastBorder"], bg)
            .filter(|d| *d != bg)
            .unwrap_or_else(|| tint(0.12)),
        popup_bg: on(&["editorWidget.background", "editorSuggestWidget.background", "quickInput.background"], bg)
            .unwrap_or_else(|| tint(0.06)),
        popup_border: on(&["editorWidget.border", "widget.border", "editorSuggestWidget.border"], bg)
            .unwrap_or_else(|| tint(0.22)),
        popup_key: accent,
        error: on(&["editorError.foreground", "errorForeground"], bg).unwrap_or(base.error),
        warning: on(&["editorWarning.foreground"], bg).unwrap_or(base.warning),
        info: on(&["editorInfo.foreground"], bg).unwrap_or(base.info),
        git_added: on(&["editorGutter.addedBackground", "gitDecoration.addedResourceForeground"], bg).unwrap_or(base.git_added),
        git_modified: on(&["editorGutter.modifiedBackground", "gitDecoration.modifiedResourceForeground"], bg)
            .unwrap_or(base.git_modified),
        git_deleted: on(&["editorGutter.deletedBackground", "gitDecoration.deletedResourceForeground"], bg)
            .unwrap_or(base.git_deleted),
        diff_removed: c("diffEditor.removedTextBackground").or(c("diffEditor.removedLineBackground")).unwrap_or(base.diff_removed),
        diff_added: c("diffEditor.insertedTextBackground").or(c("diffEditor.insertedLineBackground")).unwrap_or(base.diff_added),
        diff_filler: base.diff_filler,
        debug_line: c("editor.stackFrameHighlightBackground").unwrap_or(base.debug_line),
        frame: on(&["activityBar.background", "titleBar.activeBackground"], bg)
            .filter(|f| *f != bg)
            .unwrap_or_else(|| if dark { mix(bg, Rgba::rgb(0, 0, 0), 0.35) } else { mix(bg, Rgba::rgb(0x80, 0x80, 0x80), 0.15) }),
        ansi: (ansi.len() == 16).then(|| std::array::from_fn(|i| ansi[i])),
        syntax,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn jsonc_comments_and_trailing_commas_are_removed() {
        let text = r#"{
            // a comment with "quotes"
            "a": "http://x.y/z", /* block */
            "b": [1, 2, ],
            "c": { "d": "e", // trailing
            },
        }"#;
        let v: Value = serde_json::from_str(&strip_jsonc(text)).unwrap();
        assert_eq!(v["a"], "http://x.y/z");
        assert_eq!(v["b"], serde_json::json!([1, 2]));
        assert_eq!(v["c"]["d"], "e");
    }

    #[test]
    fn selectors_match_by_prefix_and_the_most_specific_rule_wins() {
        assert_eq!(selector_matches("keyword", "keyword.control.cs"), 1);
        assert_eq!(selector_matches("keyword.control", "keyword.control"), 2);
        assert_eq!(selector_matches("key", "keyword.control"), 0);
        assert_eq!(selector_matches("meta.class keyword", "keyword"), 0);
        let rule = |s: &str, c: &str| Rule { scopes: vec![s.into()], foreground: Rgba::parse(c), background: None };
        let rules = [rule("keyword", "#111111"), rule("keyword.control", "#222222"), rule("keyword", "#333333")];
        assert_eq!(token_color(&rules, &["keyword.control"]), Rgba::parse("#222222"));
        assert_eq!(token_color(&rules, &["keyword.other"]), Rgba::parse("#333333"));
        assert_eq!(token_color(&rules, &["string"]), None);
    }

    #[test]
    fn a_theme_with_includes_builds_and_fills_gaps() {
        let dir = std::env::temp_dir().join(format!("wyven-theme-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("base.json"),
            r##"{ "colors": { "editor.background": "#202020", "editor.foreground": "#e0e0e0" },
                  "tokenColors": [ { "scope": "comment", "settings": { "foreground": "#6a9955" } } ] }"##,
        )
        .unwrap();
        std::fs::write(
            dir.join("theme.json"),
            r##"{ "name": "Test", "include": "./base.json", "type": "dark",
                  // own colors win
                  "colors": { "editor.background": "#101820", },
                  "tokenColors": [ { "scope": ["keyword", "storage.type"], "settings": { "foreground": "#ff00ff" } } ] }"##,
        )
        .unwrap();
        let t = load("Test", &dir.join("theme.json")).unwrap();
        assert_eq!(t.name, "Test");
        assert!(t.dark);
        assert_eq!(t.background, Rgba::rgb(0x10, 0x18, 0x20));
        assert_eq!(t.text, Rgba::rgb(0xE0, 0xE0, 0xE0));
        assert_eq!(t.syntax.keyword, Rgba::rgb(0xFF, 0x00, 0xFF));
        assert_eq!(t.syntax.comment, Rgba::rgb(0x6A, 0x99, 0x55));
        // Not in the theme: derived from its own colors, never the Wyven defaults' background.
        assert_ne!(t.current_line, t.background);
        assert!(contrast(t.panel_text, t.panel_bg) > 4.0);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Every installed theme loads (only runs where VS Code themes are installed).
    #[test]
    fn installed_themes_load() {
        for e in discover() {
            let t = load(&e.label, &e.path).unwrap_or_else(|err| panic!("{}: {err}", e.label));
            assert!(contrast(t.text, t.background) > 2.0, "{}: text hard to read", e.label);
        }
    }
}
