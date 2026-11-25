//! Runtime checks for C#: code that builds but would fail when it runs. The language server
//! only reports what the compiler sees; these patterns are found in the text itself:
//!
//! - unboxing to the wrong type: `object o = 5; long l = (long)o;` → InvalidCastException
//! - parsing a literal that is no number: `int.Parse("abc")` → FormatException / OverflowException
//! - dividing by a literal zero: `x / 0`, `x % 0` → DivideByZeroException (whole numbers)
//! - a literal index past the end of a fixed array: `new int[3]` … `a[3]` → IndexOutOfRangeException
//! - using a variable right after setting it to `null`: `s = null; s.Length` → NullReferenceException
//!
//! They are heuristics on one file (no types from the compiler), so they only report clear cases.

use std::sync::OnceLock;

use regex::Regex;

/// A place that would fail at runtime: char range in the text and what would happen.
#[derive(Debug, Clone, PartialEq)]
pub struct Finding {
    pub start: usize,
    pub end: usize,
    pub message: String,
}

fn re(cell: &'static OnceLock<Regex>, pattern: &str) -> &'static Regex {
    cell.get_or_init(|| Regex::new(pattern).expect("valid pattern"))
}

/// The text with comments and the contents of strings and chars replaced by spaces (same
/// length), so patterns do not match inside them. Quotes stay.
pub(crate) fn code_only(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    // Blanked characters keep their byte length, so byte positions match the text.
    let blank = |c: char| if c == '\n' { "\n".to_string() } else { " ".repeat(c.len_utf8()) };
    while i < chars.len() {
        let c = chars[i];
        let next = chars.get(i + 1).copied();
        if c == '/' && next == Some('/') {
            while i < chars.len() && chars[i] != '\n' {
                out.push_str(&blank(chars[i]));
                i += 1;
            }
        } else if c == '/' && next == Some('*') {
            while i < chars.len() && !(chars[i] == '*' && chars.get(i + 1) == Some(&'/')) {
                out.push_str(&blank(chars[i]));
                i += 1;
            }
            if i < chars.len() {
                out.push_str("  ");
                i += 2;
            }
        } else if c == '"' || c == '\'' {
            out.push(c);
            i += 1;
            let verbatim = c == '"' && i >= 2 && chars[i - 2] == '@';
            while i < chars.len() && chars[i] != c && chars[i] != '\n' {
                if chars[i] == '\\' && !verbatim && i + 1 < chars.len() {
                    out.push_str(&blank(chars[i]));
                    i += 1;
                }
                out.push_str(&blank(chars[i]));
                i += 1;
            }
            if i < chars.len() {
                out.push(chars[i]);
                i += 1;
            }
        } else {
            out.push(c);
            i += 1;
        }
    }
    out
}

/// Char index of a byte index in `s`.
fn ch(s: &str, byte: usize) -> usize {
    s[..byte].chars().count()
}

/// The type of a literal value, if it is one.
fn literal_type(value: &str) -> Option<&'static str> {
    let v = value.trim();
    static INT: OnceLock<Regex> = OnceLock::new();
    static REAL: OnceLock<Regex> = OnceLock::new();
    if v.starts_with('"') || v.starts_with("@\"") || v.starts_with("$\"") {
        return Some("string");
    }
    if v.starts_with('\'') {
        return Some("char");
    }
    if v == "true" || v == "false" {
        return Some("bool");
    }
    if re(&INT, r"^-?\d+([uU]?[lL]?|[lL][uU])$").is_match(v) {
        let lower = v.to_ascii_lowercase();
        return Some(if lower.ends_with("ul") || lower.ends_with("lu") {
            "ulong"
        } else if lower.ends_with('l') {
            "long"
        } else if lower.ends_with('u') {
            "uint"
        } else {
            "int"
        });
    }
    if let Some(c) = re(&REAL, r"^-?\d*\.?\d+([fFdDmM])?$").captures(v) {
        return Some(match c.get(1).map(|m| m.as_str().to_ascii_lowercase()) {
            Some(s) if s == "f" => "float",
            Some(s) if s == "m" => "decimal",
            _ => "double",
        });
    }
    None
}

const BUILTIN: &[&str] = &["int", "uint", "long", "ulong", "short", "ushort", "byte", "sbyte", "float", "double", "decimal", "char", "bool", "string"];

/// Places in a C# file that would fail when the program runs.
pub fn check_csharp(text: &str) -> Vec<Finding> {
    let code = code_only(text);
    let mut out = Vec::new();
    unboxing(text, &code, &mut out);
    parsing(text, &code, &mut out);
    division(text, &code, &mut out);
    indexes(text, &code, &mut out);
    nulls(text, &code, &mut out);
    out.sort_by_key(|f| f.start);
    out.dedup_by_key(|f| f.start);
    out
}

/// `object o = 5;` … `(long)o` (an object holding an int can only be unboxed as int).
fn unboxing(text: &str, code: &str, out: &mut Vec<Finding>) {
    static DECL: OnceLock<Regex> = OnceLock::new();
    static ASSIGN: OnceLock<Regex> = OnceLock::new();
    let decl = re(&DECL, r"\b(?:object|dynamic)\s+([A-Za-z_]\w*)\s*=\s*([^;]+);");
    for d in decl.captures_iter(code) {
        let name = &d[1];
        let whole = d.get(0).expect("match");
        let value_span = d.get(2).expect("value");
        // The value from the real text (string contents are blanked in `code`).
        let value = &text[value_span.start()..value_span.end()];
        let Some(held) = literal_type(value) else { continue };
        // Up to the next assignment to the same name.
        let rest_start = whole.end();
        let assign = re(&ASSIGN, r"\b([A-Za-z_]\w*)\s*=[^=]");
        let until = assign
            .captures_iter(&code[rest_start..])
            .find(|c| &c[1] == name)
            .map_or(code.len(), |c| rest_start + c.get(0).expect("match").start());
        let cast = Regex::new(&format!(r"\(\s*([A-Za-z_]\w*)\s*\)\s*{}\b", regex::escape(name))).expect("pattern");
        for c in cast.captures_iter(&code[rest_start..until]) {
            let target = &c[1];
            if target == held || !BUILTIN.contains(&target) {
                continue;
            }
            let m = c.get(0).expect("match");
            let hint = if held != "string" && target != "string" {
                format!(" – convert instead: Convert.To{}({name})", dotnet_name(target))
            } else {
                String::new()
            };
            out.push(Finding {
                start: ch(text, rest_start + m.start()),
                end: ch(text, rest_start + m.end()),
                message: format!("Runtime: InvalidCastException – {name} holds a {held}, which cannot be cast to {target}{hint}"),
            });
        }
    }
}

fn dotnet_name(t: &str) -> &'static str {
    match t {
        "int" => "Int32",
        "uint" => "UInt32",
        "long" => "Int64",
        "ulong" => "UInt64",
        "short" => "Int16",
        "ushort" => "UInt16",
        "byte" => "Byte",
        "sbyte" => "SByte",
        "float" => "Single",
        "double" => "Double",
        "decimal" => "Decimal",
        "char" => "Char",
        "bool" => "Boolean",
        _ => "String",
    }
}

/// `int.Parse("abc")`, `Convert.ToInt32("1.5")`, `byte.Parse("300")`.
fn parsing(text: &str, code: &str, out: &mut Vec<Finding>) {
    static PARSE: OnceLock<Regex> = OnceLock::new();
    let parse = re(
        &PARSE,
        r#"\b(?:(int|uint|long|ulong|short|ushort|byte|sbyte|float|double|decimal|bool)\.Parse|Convert\.To(Int16|Int32|Int64|UInt16|UInt32|UInt64|Byte|SByte|Single|Double|Decimal|Boolean))\(\s*"([^"\n]*)"\s*\)"#,
    );
    for c in parse.captures_iter(code) {
        let m = c.get(0).expect("match");
        let ty = c.get(1).or(c.get(2)).map_or("", |t| t.as_str());
        let lit = c.get(3).expect("literal");
        let value = text[lit.start()..lit.end()].trim();
        let problem = match ty {
            "bool" | "Boolean" => (!value.eq_ignore_ascii_case("true") && !value.eq_ignore_ascii_case("false")).then_some("FormatException"),
            "float" | "double" | "decimal" | "Single" | "Double" | "Decimal" => value.parse::<f64>().is_err().then_some("FormatException"),
            _ => match value.parse::<i128>() {
                Err(_) => Some("FormatException"),
                Ok(n) => {
                    let (lo, hi): (i128, i128) = match ty {
                        "int" | "Int32" => (i32::MIN.into(), i32::MAX.into()),
                        "uint" | "UInt32" => (0, u32::MAX.into()),
                        "long" | "Int64" => (i64::MIN.into(), i64::MAX.into()),
                        "ulong" | "UInt64" => (0, u64::MAX.into()),
                        "short" | "Int16" => (i16::MIN.into(), i16::MAX.into()),
                        "ushort" | "UInt16" => (0, u16::MAX.into()),
                        "byte" | "Byte" => (0, 255),
                        _ => (-128, 127),
                    };
                    (n < lo || n > hi).then_some("OverflowException")
                }
            },
        };
        if let Some(p) = problem {
            let why = if p == "OverflowException" { "is out of range for" } else { "is no valid" };
            out.push(Finding {
                start: ch(text, m.start()),
                end: ch(text, m.end()),
                message: format!("Runtime: {p} – \"{value}\" {why} {}", if ty.is_empty() { "number" } else { ty }),
            });
        }
    }
}

/// `x / 0`, `x % 0` (whole numbers throw; 0.0 is fine for double).
fn division(text: &str, code: &str, out: &mut Vec<Finding>) {
    static DIV: OnceLock<Regex> = OnceLock::new();
    let div = re(&DIV, r"[\w)\]]\s*([/%])\s*(0)\b(?:[^.\dfdmFDM]|$)");
    for c in div.captures_iter(code) {
        let op = c.get(1).expect("op");
        let zero = c.get(2).expect("zero");
        out.push(Finding {
            start: ch(text, op.start()),
            end: ch(text, zero.end()),
            message: "Runtime: DivideByZeroException – dividing a whole number by 0 (with double you get Infinity)".into(),
        });
    }
}

/// `int[] a = new int[3];` … `a[3]` or `a[-1]`.
fn indexes(text: &str, code: &str, out: &mut Vec<Finding>) {
    static NEW: OnceLock<Regex> = OnceLock::new();
    static INIT: OnceLock<Regex> = OnceLock::new();
    let sized = re(&NEW, r"\b([A-Za-z_]\w*)\s*=\s*new\s+\w+\s*\[\s*(\d+)\s*\]\s*;");
    let init = re(&INIT, r"\b([A-Za-z_]\w*)\s*=\s*(?:new\s*\w*\s*\[\s*\]\s*)?\{([^{}]*)\}\s*;");
    let mut arrays: Vec<(String, usize, usize)> = Vec::new();
    for c in sized.captures_iter(code) {
        if let Ok(n) = c[2].parse::<usize>() {
            arrays.push((c[1].to_string(), n, c.get(0).expect("match").end()));
        }
    }
    for c in init.captures_iter(code) {
        let items = c[2].trim();
        let n = if items.is_empty() { 0 } else { items.split(',').filter(|s| !s.trim().is_empty()).count() };
        arrays.push((c[1].to_string(), n, c.get(0).expect("match").end()));
    }
    for (name, size, from) in arrays {
        let reassigned = Regex::new(&format!(r"\b{}\s*=[^=]", regex::escape(&name))).expect("pattern");
        let until = reassigned.find(&code[from..]).map_or(code.len(), |m| from + m.start());
        let index = Regex::new(&format!(r"\b{}\s*\[\s*(-?\d+)\s*\]", regex::escape(&name))).expect("pattern");
        for c in index.captures_iter(&code[from..until]) {
            let Ok(i) = c[1].parse::<i64>() else { continue };
            if i < 0 || i as usize >= size {
                let m = c.get(0).expect("match");
                out.push(Finding {
                    start: ch(text, from + m.start()),
                    end: ch(text, from + m.end()),
                    message: format!(
                        "Runtime: IndexOutOfRangeException – {name} has {size} element{} (indexes 0 to {})",
                        if size == 1 { "" } else { "s" },
                        size as i64 - 1
                    ),
                });
            }
        }
    }
}

/// `x = null;` and then `x.Something` or `x[…]` before x gets another value.
fn nulls(text: &str, code: &str, out: &mut Vec<Finding>) {
    static NULL: OnceLock<Regex> = OnceLock::new();
    let set_null = re(&NULL, r"\b([A-Za-z_]\w*)\s*=\s*null\s*;");
    for c in set_null.captures_iter(code) {
        let name = &c[1];
        let from = c.get(0).expect("match").end();
        // Only the next few lines, and only until the name gets a value or is checked.
        let window_end = code[from..].match_indices('\n').nth(25).map_or(code.len(), |(i, _)| from + i);
        let part = &code[from..window_end];
        let esc = regex::escape(name);
        let stop = Regex::new(&format!(r"\b{esc}\s*(=[^=]|\?\?=|!=|==)|\bif\s*\(|\bout\s+{esc}\b|\bref\s+{esc}\b")).expect("pattern");
        let until = stop.find(part).map_or(part.len(), |m| m.start());
        let use_ = Regex::new(&format!(r"\b{esc}\s*(\.|\[)")).expect("pattern");
        if let Some(m) = use_.find(&part[..until]) {
            // `x?.` is safe: the regex needs `.` or `[` right after the name.
            out.push(Finding {
                start: ch(text, from + m.start()),
                end: ch(text, from + m.start()) + name.chars().count(),
                message: format!("Runtime: NullReferenceException – {name} is null here"),
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn messages(code: &str) -> Vec<String> {
        check_csharp(code).into_iter().map(|f| f.message).collect()
    }

    #[test]
    fn wrong_unboxing_is_found() {
        let m = messages("object o = 5;\nlong l = (long)o;\nint i = (int)o;");
        assert_eq!(m.len(), 1);
        assert!(m[0].contains("InvalidCastException") && m[0].contains("holds a int") && m[0].contains("Convert.ToInt64(o)"));
        let m = messages("object s = \"text\";\nint n = (int)s;");
        assert!(m[0].contains("holds a string"));
        // After a new value, nothing.
        assert!(messages("object o = 5;\no = 5L;\nlong l = (long)o;").is_empty());
    }

    #[test]
    fn parse_literals_are_checked() {
        let m = messages("int a = int.Parse(\"abc\");\nint b = int.Parse(\"42\");\nbyte c = byte.Parse(\"300\");\ndouble d = double.Parse(\"1.5\");");
        assert_eq!(m.len(), 2);
        assert!(m[0].contains("FormatException") && m[1].contains("OverflowException"));
        assert!(messages("var x = Convert.ToInt32(\"1.5\");")[0].contains("FormatException"));
    }

    #[test]
    fn division_index_and_null() {
        assert!(messages("int r = total / 0;")[0].contains("DivideByZeroException"));
        assert!(messages("double r = total / 0.0;").is_empty());
        let m = messages("int[] a = new int[3];\na[2] = 1;\na[3] = 1;");
        assert_eq!(m.len(), 1);
        assert!(m[0].contains("has 3 elements"));
        assert!(messages("var b = new[] { 1, 2 };\nConsole.WriteLine(b[2]);")[0].contains("2 elements"));
        let m = messages("string s = null;\nint n = s.Length;");
        assert!(m[0].contains("NullReferenceException"));
        assert!(messages("string s = null;\nif (s != null) { s.Trim(); }").is_empty());
        assert!(messages("string s = null;\nvar n = s?.Length;").is_empty());
        // Nothing inside comments or strings.
        assert!(messages("// int r = x / 0;\nvar t = \"a / 0\";").is_empty());
    }
}
