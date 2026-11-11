//! Code snippets, like in Visual Studio and VS Code: type a short word (`prop`, `ctor`,
//! `foreach` …) and press `Tab` (or pick it in the completion list). The text appears with
//! placeholders; `Tab` jumps to the next one, `Shift+Tab` back, `Esc` ends. A placeholder that
//! appears several times is edited everywhere at once.
//!
//! The body uses the VS Code / LSP snippet syntax: `$1`, `${1:default}`, `${1|a,b|}` (first
//! choice), `$0` (where the cursor ends), variables like `$TM_FILENAME_BASE` and `$CLASS_NAME`
//! (the class around the cursor). Language servers' snippets use the same syntax.
//!
//! Own snippets: JSON files in `%APPDATA%\Wyven\snippets\`, in VS Code's format, so VS Code
//! snippet files can be copied there: `csharp.json` (one language, named by its VS Code id) or
//! `*.code-snippets` (with `"scope": "csharp,typescript"`; without scope for all languages).
//! Own snippets win over built-in ones with the same prefix.

use std::path::{Path, PathBuf};

use serde_json::Value;

/// A snippet as written in the tables below.
pub struct Builtin {
    pub prefix: &'static str,
    pub description: &'static str,
    pub body: &'static str,
}

const fn s(prefix: &'static str, description: &'static str, body: &'static str) -> Builtin {
    Builtin { prefix, description, body }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Snippet {
    pub prefix: String,
    pub description: String,
    pub body: String,
}

/// Snippets from the user's files, with the language (Wyven name) or `None` for all.
#[derive(Debug, Default)]
pub struct UserSnippets {
    pub list: Vec<(Option<String>, Snippet)>,
    /// Problems while reading the files.
    pub errors: Vec<String>,
}

pub fn dir() -> Option<PathBuf> {
    crate::config::config_dir().map(|d| d.join("snippets"))
}

/// VS Code's language id for a Wyven language name (and back).
const IDS: &[(&str, &str)] = &[
    ("csharp", "C#"),
    ("javascript", "JavaScript"),
    ("typescript", "TypeScript"),
    ("typescriptreact", "TSX"),
    ("html", "HTML"),
    ("css", "CSS"),
    ("json", "JSON"),
    ("markdown", "Markdown"),
    ("toml", "TOML"),
    ("rust", "Rust"),
    ("python", "Python"),
];

pub fn language_of_id(id: &str) -> Option<&'static str> {
    IDS.iter().find(|(i, _)| i.eq_ignore_ascii_case(id.trim())).map(|(_, n)| *n)
}

pub fn id_of_language(lang: &str) -> Option<&'static str> {
    IDS.iter().find(|(_, n)| *n == lang).map(|(i, _)| *i)
}

/// Removes `//` and `/* */` comments and trailing commas (VS Code's JSON allows them).
fn loose_json(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len());
    let (mut i, mut in_str) = (0, false);
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
        match c {
            '"' => {
                in_str = true;
                out.push(c);
            }
            '/' if chars.get(i + 1) == Some(&'/') => {
                while i < chars.len() && chars[i] != '\n' {
                    i += 1;
                }
                continue;
            }
            '/' if chars.get(i + 1) == Some(&'*') => {
                i += 2;
                while i + 1 < chars.len() && !(chars[i] == '*' && chars[i + 1] == '/') {
                    i += 1;
                }
                i += 2;
                continue;
            }
            ',' => {
                // A comma before } or ] is dropped.
                let next = chars[i + 1..].iter().find(|c| !c.is_whitespace());
                if !matches!(next, Some('}') | Some(']')) {
                    out.push(c);
                }
            }
            c => out.push(c),
        }
        i += 1;
    }
    out
}

/// Reads one snippet file. `lang`: the language from the file name (`csharp.json`).
pub fn parse_file(text: &str, lang: Option<&str>) -> Result<Vec<(Option<String>, Snippet)>, String> {
    let v: Value = serde_json::from_str(&loose_json(text)).map_err(|e| e.to_string())?;
    let obj = v.as_object().ok_or("expected an object of snippets")?;
    let mut out = Vec::new();
    let join = |v: &Value| -> String {
        match v {
            Value::Array(a) => a.iter().filter_map(Value::as_str).collect::<Vec<_>>().join("\n"),
            Value::String(s) => s.clone(),
            _ => String::new(),
        }
    };
    for (name, s) in obj {
        let body = join(&s["body"]);
        if body.is_empty() {
            continue;
        }
        let description = s["description"].as_str().unwrap_or(name).to_string();
        let prefixes: Vec<String> = match &s["prefix"] {
            Value::Array(a) => a.iter().filter_map(Value::as_str).map(str::to_string).collect(),
            Value::String(p) => vec![p.clone()],
            _ => continue,
        };
        let langs: Vec<Option<String>> = match (lang, s["scope"].as_str()) {
            (Some(l), _) => vec![Some(l.to_string())],
            (None, Some(scope)) => scope.split(',').filter_map(language_of_id).map(|l| Some(l.to_string())).collect(),
            (None, None) => vec![None],
        };
        for p in &prefixes {
            for l in &langs {
                out.push((l.clone(), Snippet { prefix: p.clone(), description: description.clone(), body: body.clone() }));
            }
        }
    }
    Ok(out)
}

impl UserSnippets {
    /// Reads every file in the snippets folder.
    pub fn load() -> Self {
        let mut me = Self::default();
        let Some(dir) = dir() else { return me };
        let Ok(entries) = std::fs::read_dir(&dir) else { return me };
        let mut files: Vec<PathBuf> = entries.flatten().map(|e| e.path()).collect();
        files.sort();
        for path in files {
            let name = path.file_name().map(|n| n.to_string_lossy().to_lowercase()).unwrap_or_default();
            let lang = if name.ends_with(".code-snippets") {
                None
            } else if let Some(stem) = name.strip_suffix(".json") {
                match language_of_id(stem) {
                    Some(l) => Some(l),
                    None => {
                        me.errors.push(format!("{name}: unknown language (use e.g. csharp.json)"));
                        continue;
                    }
                }
            } else {
                continue;
            };
            match std::fs::read_to_string(&path).map_err(|e| e.to_string()).and_then(|t| parse_file(&t, lang)) {
                Ok(list) => me.list.extend(list),
                Err(e) => me.errors.push(format!("{name}: {e}")),
            }
        }
        me
    }

    pub fn is_snippet_file(path: &Path) -> bool {
        dir().is_some_and(|d| path.parent() == Some(d.as_path()))
    }
}

const CSHARP: &[Builtin] = &[
    s("prop", "Auto-implemented property", "public ${1:int} ${2:MyProperty} { get; set; }$0"),
    s("propg", "Property with private setter", "public ${1:int} ${2:MyProperty} { get; private set; }$0"),
    s("propi", "Init-only property", "public ${1:int} ${2:MyProperty} { get; init; }$0"),
    s(
        "propfull",
        "Property with backing field",
        "private ${1:int} ${2:myVar};\n\npublic $1 ${3:MyProperty}\n{\n\tget { return $2; }\n\tset { $2 = value; }\n}$0",
    ),
    s("ctor", "Constructor", "public ${1:$CLASS_NAME}($2)\n{\n\t$0\n}"),
    s("class", "Class", "public class ${1:$TM_FILENAME_BASE}\n{\n\t$0\n}"),
    s("interface", "Interface", "public interface ${1:I$TM_FILENAME_BASE}\n{\n\t$0\n}"),
    s("struct", "Struct", "public struct ${1:MyStruct}\n{\n\t$0\n}"),
    s("record", "Record", "public record ${1:MyRecord}(${2:string Name});$0"),
    s("enum", "Enum", "public enum ${1:MyEnum}\n{\n\t$0\n}"),
    s("namespace", "Namespace", "namespace ${1:MyNamespace}\n{\n\t$0\n}"),
    s("method", "Method", "public ${1:void} ${2:MyMethod}($3)\n{\n\t$0\n}"),
    s("amethod", "Async method", "public async Task${1:<int>} ${2:MyMethodAsync}($3)\n{\n\t$0\n}"),
    s("svm", "static void Main", "static void Main(string[] args)\n{\n\t$0\n}"),
    s("sim", "static int Main", "static int Main(string[] args)\n{\n\t$0\n\treturn 0;\n}"),
    s("cw", "Console.WriteLine", "Console.WriteLine($1);$0"),
    s("for", "for loop", "for (int ${1:i} = 0; $1 < ${2:length}; $1++)\n{\n\t$0\n}"),
    s("forr", "Reverse for loop", "for (int ${1:i} = ${2:length} - 1; $1 >= 0; $1--)\n{\n\t$0\n}"),
    s("foreach", "foreach loop", "foreach (${1:var} ${2:item} in ${3:collection})\n{\n\t$0\n}"),
    s("if", "if statement", "if (${1:true})\n{\n\t$0\n}"),
    s("else", "else block", "else\n{\n\t$0\n}"),
    s("while", "while loop", "while (${1:true})\n{\n\t$0\n}"),
    s("do", "do-while loop", "do\n{\n\t$0\n} while (${1:true});"),
    s("switch", "switch statement", "switch (${1:value})\n{\n\tcase ${2:0}:\n\t\t$0\n\t\tbreak;\n\tdefault:\n\t\tbreak;\n}"),
    s("try", "try-catch", "try\n{\n\t$1\n}\ncatch (${2:Exception} ${3:ex})\n{\n\t$0\n}"),
    s("tryf", "try-finally", "try\n{\n\t$1\n}\nfinally\n{\n\t$0\n}"),
    s("using", "using statement", "using (${1:var} ${2:resource} = ${3:new()})\n{\n\t$0\n}"),
    s("lock", "lock statement", "lock (${1:this})\n{\n\t$0\n}"),
    s("indexer", "Indexer", "public ${1:object} this[${2:int} index]\n{\n\tget { $3 }\n\tset { $0 }\n}"),
    s("exception", "Exception class", "public class ${1:My}Exception : Exception\n{\n\tpublic $1Exception() { }\n\tpublic $1Exception(string message) : base(message) { }\n\tpublic $1Exception(string message, Exception inner) : base(message, inner) { }\n}$0"),
    s("tostring", "Override ToString", "public override string ToString()\n{\n\treturn $0;\n}"),
];

const SCRIPT: &[Builtin] = &[
    s("log", "console.log", "console.log($1);$0"),
    s("for", "for loop", "for (let ${1:i} = 0; $1 < ${2:array}.length; $1++) {\n\t$0\n}"),
    s("forof", "for…of loop", "for (const ${1:item} of ${2:items}) {\n\t$0\n}"),
    s("forin", "for…in loop", "for (const ${1:key} in ${2:object}) {\n\t$0\n}"),
    s("foreach", "forEach", "${1:array}.forEach((${2:item}) => {\n\t$0\n});"),
    s("function", "Function", "function ${1:name}($2) {\n\t$0\n}"),
    s("afn", "Arrow function", "const ${1:name} = ($2) => {\n\t$0\n};"),
    s("async", "Async function", "async function ${1:name}($2) {\n\t$0\n}"),
    s("class", "Class", "class ${1:$TM_FILENAME_BASE} {\n\tconstructor($2) {\n\t\t$0\n\t}\n}"),
    s("ctor", "Constructor", "constructor($1) {\n\t$0\n}"),
    s("if", "if statement", "if (${1:condition}) {\n\t$0\n}"),
    s("ifelse", "if-else", "if (${1:condition}) {\n\t$2\n} else {\n\t$0\n}"),
    s("switch", "switch statement", "switch (${1:key}) {\n\tcase ${2:value}:\n\t\t$0\n\t\tbreak;\n\tdefault:\n\t\tbreak;\n}"),
    s("try", "try-catch", "try {\n\t$1\n} catch (${2:error}) {\n\t$0\n}"),
    s("imp", "import", "import { $2 } from '${1:module}';$0"),
    s("timeout", "setTimeout", "setTimeout(() => {\n\t$0\n}, ${1:1000});"),
];

const TYPESCRIPT_ONLY: &[Builtin] = &[
    s("interface", "Interface", "interface ${1:Name} {\n\t$0\n}"),
    s("type", "Type alias", "type ${1:Name} = $0;"),
    s("enum", "Enum", "enum ${1:Name} {\n\t$0\n}"),
];

const HTML: &[Builtin] = &[
    s(
        "html5",
        "HTML document",
        "<!DOCTYPE html>\n<html lang=\"${1:en}\">\n<head>\n\t<meta charset=\"UTF-8\">\n\t<meta name=\"viewport\" content=\"width=device-width, initial-scale=1.0\">\n\t<title>${2:Document}</title>\n</head>\n<body>\n\t$0\n</body>\n</html>",
    ),
    s("link", "Stylesheet link", "<link rel=\"stylesheet\" href=\"${1:style.css}\">$0"),
    s("script", "Script tag", "<script src=\"${1:main.js}\"></script>$0"),
    s("div", "div with class", "<div class=\"$1\">$0</div>"),
    s("a", "Link", "<a href=\"${1:#}\">$0</a>"),
    s("img", "Image", "<img src=\"$1\" alt=\"$2\">$0"),
    s("ul", "List", "<ul>\n\t<li>$0</li>\n</ul>"),
];

const CSS: &[Builtin] = &[
    s("media", "Media query", "@media (max-width: ${1:600px}) {\n\t$0\n}"),
    s("flex", "Flex container", "display: flex;\njustify-content: ${1:center};\nalign-items: ${2:center};$0"),
    s("grid", "Grid container", "display: grid;\ngrid-template-columns: ${1:repeat(3, 1fr)};\ngap: ${2:1rem};$0"),
    s("keyframes", "Keyframes", "@keyframes ${1:name} {\n\tfrom { $2 }\n\tto { $0 }\n}"),
];

const RUST: &[Builtin] = &[
    s("fn", "Function", "fn ${1:name}($2) {\n\t$0\n}"),
    s("pfn", "Public function", "pub fn ${1:name}($2) {\n\t$0\n}"),
    s("struct", "Struct", "#[derive(Debug, Clone)]\npub struct ${1:Name} {\n\t$0\n}"),
    s("enum", "Enum", "#[derive(Debug, Clone, Copy, PartialEq, Eq)]\npub enum ${1:Name} {\n\t$0\n}"),
    s("impl", "impl block", "impl ${1:Type} {\n\t$0\n}"),
    s("test", "Test function", "#[test]\nfn ${1:it_works}() {\n\t$0\n}"),
    s("tests", "Test module", "#[cfg(test)]\nmod tests {\n\tuse super::*;\n\n\t#[test]\n\tfn ${1:it_works}() {\n\t\t$0\n\t}\n}"),
    s("match", "match", "match ${1:value} {\n\t${2:_} => $0,\n}"),
    s("iflet", "if let", "if let ${1:Some(x)} = ${2:value} {\n\t$0\n}"),
    s("for", "for loop", "for ${1:item} in ${2:items} {\n\t$0\n}"),
    s("println", "println!", "println!(\"$1\");$0"),
];

/// The snippets for a language (as named by the syntax highlighter): the user's first, then
/// the built-in ones whose prefix they do not take.
pub fn for_language(lang: &str, user: &UserSnippets) -> Vec<Snippet> {
    let mut out: Vec<Snippet> =
        user.list.iter().filter(|(l, _)| l.as_deref().is_none_or(|l| l == lang)).map(|(_, s)| s.clone()).collect();
    for b in builtin(lang) {
        if !out.iter().any(|s| s.prefix == b.prefix) {
            out.push(Snippet { prefix: b.prefix.into(), description: b.description.into(), body: b.body.into() });
        }
    }
    out
}

fn builtin(lang: &str) -> Vec<&'static Builtin> {
    let lists: &[&[Builtin]] = match lang {
        "C#" => &[CSHARP],
        "JavaScript" => &[SCRIPT],
        "TypeScript" | "TSX" => &[SCRIPT, TYPESCRIPT_ONLY],
        "HTML" => &[HTML],
        "CSS" => &[CSS],
        "Rust" => &[RUST],
        _ => &[],
    };
    lists.iter().flat_map(|l| l.iter()).collect()
}

pub fn find(lang: &str, prefix: &str, user: &UserSnippets) -> Option<Snippet> {
    for_language(lang, user).into_iter().find(|s| s.prefix == prefix)
}

/// A starting file for a language's own snippets.
pub fn template(lang: &str) -> String {
    let example = match lang {
        "C#" => r#""Public method": {
		"prefix": "pm",
		"body": [
			"public ${1:void} ${2:Name}($3)",
			"{",
			"\t$0",
			"}"
		],
		"description": "Public method"
	}"#,
        _ => r#""Example": {
		"prefix": "todo",
		"body": "// TODO: $0",
		"description": "A TODO comment"
	}"#,
    };
    format!(
        "{{\n\t// Your {lang} snippets, in VS Code's format: type the prefix and press Tab.\n\t// $1, $2 … are placeholders (Tab jumps), ${{1:text}} with a default, $0 is where the cursor ends.\n\t// Variables: $TM_FILENAME_BASE, $TM_FILENAME, $CLASS_NAME. Saving applies the changes.\n\t{example}\n}}\n"
    )
}

/// Values for `$NAME` variables.
pub struct Vars {
    pub file_base: String,
    pub file_name: String,
    pub class_name: String,
}

impl Vars {
    fn get(&self, name: &str) -> Option<String> {
        Some(match name {
            "TM_FILENAME_BASE" => self.file_base.clone(),
            "TM_FILENAME" => self.file_name.clone(),
            "CLASS_NAME" => self.class_name.clone(),
            _ => return None,
        })
    }
}

/// A snippet ready to insert: the text and its tab stops (char ranges in the text), in the
/// order `Tab` visits them ($1, $2, … and $0 last); each stop can have several places.
#[derive(Debug, PartialEq)]
pub struct Expanded {
    pub text: String,
    pub stops: Vec<Vec<(usize, usize)>>,
}

/// Turns a snippet body into text. `indent`: the current line's indentation (added after every
/// line break); `unit`: one indentation step (replaces `\t`).
pub fn expand(body: &str, indent: &str, unit: &str, vars: &Vars) -> Expanded {
    let mut p = Parser { chars: body.chars().collect(), i: 0, out: String::new(), len: 0, stops: Vec::new(), indent, unit, vars, pending: false };
    p.parse(None);
    // Defaults of placeholders that appear first without one ($1 … ${1:x}): use the default.
    let mut text = p.out;
    let mut stops = p.stops;
    let defaults: Vec<(u32, String)> = {
        let mut seen: Vec<(u32, String)> = Vec::new();
        for s in &stops {
            if !s.2.is_empty() && !seen.iter().any(|(n, _)| *n == s.0) {
                seen.push((s.0, s.2.clone()));
            }
        }
        seen
    };
    // Fill empty mirrors with their default, shifting everything after them.
    let mut chars: Vec<char> = text.chars().collect();
    stops.sort_by_key(|s| s.1);
    let mut shift = 0usize;
    for s in &mut stops {
        s.1 += shift;
        if s.2.is_empty()
            && let Some((_, d)) = defaults.iter().find(|(n, _)| *n == s.0)
        {
            let ins: Vec<char> = d.chars().collect();
            chars.splice(s.1..s.1, ins.iter().copied());
            shift += ins.len();
            s.2 = d.clone();
        }
    }
    text = chars.into_iter().collect();
    // Group by number: 1, 2, … then 0 (or the end of the text).
    let mut numbers: Vec<u32> = stops.iter().map(|s| s.0).filter(|&n| n > 0).collect();
    numbers.sort_unstable();
    numbers.dedup();
    let mut groups: Vec<Vec<(usize, usize)>> = numbers
        .iter()
        .map(|&n| stops.iter().filter(|s| s.0 == n).map(|s| (s.1, s.1 + s.2.chars().count())).collect())
        .collect();
    let end = stops.iter().find(|s| s.0 == 0).map_or(text.chars().count(), |s| s.1);
    groups.push(vec![(end, end)]);
    Expanded { text, stops: groups }
}

struct Parser<'a> {
    chars: Vec<char>,
    i: usize,
    out: String,
    /// Length of `out` in chars.
    len: usize,
    /// (number, position, text) of every placeholder.
    stops: Vec<(u32, usize, String)>,
    indent: &'a str,
    unit: &'a str,
    vars: &'a Vars,
    /// A line break was written; its indentation follows with the next character.
    pending: bool,
}

impl Parser<'_> {
    /// Writes the indentation owed after a line break (not on empty lines).
    fn flush(&mut self) {
        if self.pending {
            self.pending = false;
            self.out.push_str(self.indent);
            self.len += self.indent.chars().count();
        }
    }

    fn push(&mut self, c: char) {
        if c != '\n' {
            self.flush();
        }
        match c {
            '\n' => {
                self.out.push('\n');
                self.len += 1;
                self.pending = true;
            }
            '\t' => {
                self.out.push_str(self.unit);
                self.len += self.unit.chars().count();
            }
            c => {
                self.out.push(c);
                self.len += 1;
            }
        }
    }

    fn peek(&self) -> Option<char> {
        self.chars.get(self.i).copied()
    }

    fn number(&mut self) -> Option<u32> {
        let start = self.i;
        while self.peek().is_some_and(|c| c.is_ascii_digit()) {
            self.i += 1;
        }
        (self.i > start).then(|| self.chars[start..self.i].iter().collect::<String>().parse().ok()).flatten()
    }

    fn name(&mut self) -> String {
        let start = self.i;
        while self.peek().is_some_and(|c| c.is_ascii_alphanumeric() || c == '_') {
            self.i += 1;
        }
        self.chars[start..self.i].iter().collect()
    }

    /// Reads text until `stop` (a closing `}` of a placeholder) or the end.
    fn parse(&mut self, stop: Option<char>) {
        while let Some(c) = self.peek() {
            if Some(c) == stop {
                return;
            }
            self.i += 1;
            match c {
                '\\' => {
                    if let Some(n) = self.peek() {
                        self.i += 1;
                        self.push(n);
                    }
                }
                '$' => self.dollar(),
                c => self.push(c),
            }
        }
    }

    fn dollar(&mut self) {
        self.flush();
        match self.peek() {
            Some(c) if c.is_ascii_digit() => {
                let n = self.number().unwrap_or(0);
                self.stops.push((n, self.len, String::new()));
            }
            Some('{') => {
                self.i += 1;
                if self.peek().is_some_and(|c| c.is_ascii_digit()) {
                    let n = self.number().unwrap_or(0);
                    let at = self.len;
                    match self.peek() {
                        Some(':') => {
                            self.i += 1;
                            let from = self.out.len();
                            self.parse(Some('}'));
                            let text = self.out[from..].to_string();
                            self.stops.push((n, at, text));
                        }
                        Some('|') => {
                            // A choice: the first option.
                            self.i += 1;
                            let start = self.i;
                            while self.peek().is_some_and(|c| c != '|' && c != '}') {
                                self.i += 1;
                            }
                            let all: String = self.chars[start..self.i].iter().collect();
                            let first = all.split(',').next().unwrap_or("").to_string();
                            for ch in first.chars() {
                                self.push(ch);
                            }
                            while self.peek().is_some_and(|c| c != '}') {
                                self.i += 1;
                            }
                            self.stops.push((n, at, first));
                        }
                        _ => self.stops.push((n, at, String::new())),
                    }
                } else {
                    let name = self.name();
                    let value = self.vars.get(&name);
                    if self.peek() == Some(':') {
                        self.i += 1;
                        match value {
                            Some(v) if !v.is_empty() => {
                                for ch in v.chars() {
                                    self.push(ch);
                                }
                                // Skip the default.
                                let mut depth = 0;
                                while let Some(c) = self.peek() {
                                    if c == '}' && depth == 0 {
                                        break;
                                    }
                                    depth += (c == '{') as i32 - (c == '}') as i32;
                                    self.i += 1;
                                }
                            }
                            _ => self.parse(Some('}')),
                        }
                    } else {
                        for ch in value.unwrap_or_default().chars() {
                            self.push(ch);
                        }
                    }
                }
                if self.peek() == Some('}') {
                    self.i += 1;
                }
            }
            Some(c) if c.is_ascii_alphabetic() || c == '_' => {
                let name = self.name();
                match self.vars.get(&name) {
                    Some(v) => {
                        for ch in v.chars() {
                            self.push(ch);
                        }
                    }
                    None => {
                        self.push('$');
                        for ch in name.chars() {
                            self.push(ch);
                        }
                    }
                }
            }
            _ => self.push('$'),
        }
    }
}

/// The class (or struct/record/interface) the position is in: the last declaration before it.
pub fn enclosing_class(text_before: &str) -> Option<String> {
    let words: Vec<&str> = text_before.split(|c: char| !(c.is_alphanumeric() || c == '_')).filter(|w| !w.is_empty()).collect();
    words
        .windows(2)
        .rev()
        .find(|w| matches!(w[0], "class" | "struct" | "record"))
        .map(|w| w[1].to_string())
}

/// Tab stops of an inserted snippet while the user fills them in.
#[derive(Debug, Clone)]
pub struct Session {
    /// Absolute char ranges per stop.
    pub stops: Vec<Vec<(usize, usize)>>,
    pub current: usize,
    /// Document length when the current stop was entered.
    pub len_at: usize,
}

impl Session {
    /// Moves to stop `current + step`, accounting for what was typed into the current stop
    /// (every place of it changed by the same amount). Returns the ranges to select, or None
    /// when the snippet is done.
    pub fn advance(&mut self, doc_len: usize, step: isize) -> Option<Vec<(usize, usize)>> {
        let here = self.stops.get(self.current)?.clone();
        let n = here.len().max(1) as isize;
        let delta = (doc_len as isize - self.len_at as isize) / n;
        if delta != 0 {
            let shift = |p: usize, inclusive: bool| -> usize {
                let before = here.iter().filter(|r| if inclusive { r.0 <= p } else { r.0 < p }).count() as isize;
                (p as isize + delta * before).max(0) as usize
            };
            for (k, group) in self.stops.iter_mut().enumerate() {
                for r in group.iter_mut() {
                    if k == self.current {
                        // Its own places: start moves by the ones before, end by those up to it.
                        let s = shift(r.0, false);
                        *r = (s, (s as isize + (r.1 - r.0) as isize + delta).max(s as isize) as usize);
                    } else {
                        *r = (shift(r.0, false), shift(r.1, false).max(shift(r.0, false)));
                    }
                }
            }
        }
        let next = self.current as isize + step;
        if next < 0 || next as usize >= self.stops.len() {
            return None;
        }
        self.current = next as usize;
        self.len_at = doc_len;
        Some(self.stops[self.current].clone())
    }

    /// The last stop ($0) is reached: nothing more to fill in.
    pub fn at_end(&self) -> bool {
        self.current + 1 >= self.stops.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vars() -> Vars {
        Vars { file_base: "Person".into(), file_name: "Person.cs".into(), class_name: "Person".into() }
    }

    #[test]
    fn prop_has_two_placeholders_and_an_end() {
        let e = expand(&find("C#", "prop", &UserSnippets::default()).unwrap().body, "    ", "    ", &vars());
        assert_eq!(e.text, "public int MyProperty { get; set; }");
        assert_eq!(e.stops, vec![vec![(7, 10)], vec![(11, 21)], vec![(35, 35)]]);
    }

    #[test]
    fn mirrors_get_the_default_and_lines_the_indent() {
        let e = expand(&find("C#", "propfull", &UserSnippets::default()).unwrap().body, "  ", "\t", &vars());
        assert_eq!(e.text, "private int myVar;\n\n  public int MyProperty\n  {\n  \tget { return myVar; }\n  \tset { myVar = value; }\n  }");
        // $1 twice, $2 three times.
        assert_eq!(e.stops[0].len(), 2);
        assert_eq!(e.stops[1].len(), 3);
        let chars: Vec<char> = e.text.chars().collect();
        for &(a, b) in &e.stops[1] {
            assert_eq!(chars[a..b].iter().collect::<String>(), "myVar");
        }
    }

    #[test]
    fn variables_choices_and_escapes() {
        let e = expand(&find("C#", "ctor", &UserSnippets::default()).unwrap().body, "", "    ", &vars());
        assert!(e.text.starts_with("public Person()"));
        let e = expand("${1|a,b|} \\$x $UNKNOWN ${TM_FILENAME}", "", "  ", &vars());
        assert_eq!(e.text, "a $x $UNKNOWN Person.cs");
        assert_eq!(e.stops[0], vec![(0, 1)]);
        assert_eq!(enclosing_class("namespace A { public class Person : Base {\n int x;"), Some("Person".into()));
    }

    #[test]
    fn vs_code_snippet_files_with_comments_and_scopes() {
        let text = r#"{
            // a comment
            "Prop": { "prefix": ["prop", "p"], "body": ["public $1 X { get; set; }", "$0"], "description": "mine", },
            /* block */ "Log": { "prefix": "log", "body": "console.log(\"$1\");", "scope": "javascript,typescript" },
        }"#;
        let list = parse_file(text, None).unwrap();
        assert_eq!(list.iter().filter(|(l, _)| l.is_none()).count(), 2);
        assert!(list.iter().any(|(l, s)| l.as_deref() == Some("TypeScript") && s.prefix == "log" && s.body == "console.log(\"$1\");"));
        let user = UserSnippets { list, errors: vec![] };
        // The user's prop replaces the built-in one; the other built-ins stay.
        let cs = for_language("C#", &user);
        assert_eq!(cs.iter().find(|s| s.prefix == "prop").unwrap().description, "mine");
        assert!(cs.iter().any(|s| s.prefix == "ctor"));
        assert!(parse_file(&template("C#"), Some("C#")).unwrap().len() == 1);
        assert!(parse_file("[1]", None).is_err());
    }

    #[test]
    fn typing_in_a_mirrored_stop_shifts_the_later_ones() {
        // "private int myVar; ... return myVar;" with $1 = int (twice) and $2 = myVar.
        let e = expand("private ${1:int} ${2:x}; $1 $2", "", "    ", &vars());
        assert_eq!(e.text, "private int x; int x");
        let mut s = Session { stops: e.stops.clone(), current: 0, len_at: 20 };
        // "int" became "string" in both places: +3 each.
        let next = s.advance(26, 1).unwrap();
        assert_eq!(next, vec![(15, 16), (25, 26)]);
        // Back to $1: both now cover "string".
        let back = s.advance(26, -1).unwrap();
        assert_eq!(back, vec![(8, 14), (18, 24)]);
    }
}
