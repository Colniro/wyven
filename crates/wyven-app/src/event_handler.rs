//! C# event handlers like in Visual Studio: after `button1.Click +=` the name
//! `Button1_Click;` is offered as gray text, and Tab inserts it together with a method
//! `private void Button1_Click(object? sender, EventArgs e)` below the current method that
//! throws `NotImplementedException`.

use crate::runtime_check::code_only;

/// What Tab would insert after `… +=`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Offer {
    /// The handler's name, e.g. `CbxUsers_SelectedValueChanged`.
    pub name: String,
    /// The type of its second parameter, e.g. `EventArgs` or `MouseEventArgs`.
    pub args: &'static str,
    /// The text after the cursor (gray until Tab): `Name;`, with a space first if needed.
    pub ghost: String,
}

/// The handler offer when `before` (the line up to the cursor) ends with `object.Event +=`;
/// `text_before` is the whole file up to the cursor (for the class name with `this.Load +=`).
pub fn offer(before: &str, text_before: &str) -> Option<Offer> {
    let trimmed = before.trim_end();
    let lhs = trimmed.strip_suffix("+=")?.trim_end();
    // The event expression: `cbxUsers.SelectedValueChanged`, `this.Load` or `Load`.
    let start = lhs
        .char_indices()
        .rev()
        .find(|(_, c)| !(c.is_alphanumeric() || *c == '_' || *c == '.'))
        .map_or(0, |(i, c)| i + c.len_utf8());
    let expr = &lhs[start..];
    if expr.is_empty() || !lhs[..start].trim().is_empty() {
        return None; // more on the line than an event (`x = a +=` …)
    }
    let mut parts: Vec<&str> = expr.split('.').collect();
    let event = parts.pop()?;
    if !event.chars().next().is_some_and(char::is_uppercase) || parts.iter().any(|p| p.is_empty()) {
        return None;
    }
    let object = match parts.last() {
        None | Some(&"this") => class_name(text_before)?,
        Some(o) => capitalize(o),
    };
    let name = format!("{object}_{event}");
    let space = if before.ends_with(' ') { "" } else { " " };
    Some(Offer { ghost: format!("{space}{name};"), name, args: args_for(event) })
}

/// The method to add for `offer` in `text` (cursor at byte `cursor`): where (byte position)
/// and what. `None` when a method with that name exists already or the place is unclear.
pub fn stub(text: &str, cursor: usize, offer: &Offer, indent_unit: &str) -> Option<(usize, String)> {
    let code = code_only(text);
    if code.contains(&format!("void {}(", offer.name)) {
        return None;
    }
    let bytes = code.as_bytes();
    // The braces around the cursor, innermost first.
    let mut open = Vec::new();
    let mut depth = 0i32;
    for i in (0..cursor.min(bytes.len())).rev() {
        match bytes[i] {
            b'}' => depth += 1,
            b'{' if depth > 0 => depth -= 1,
            b'{' => open.push(i),
            _ => {}
        }
    }
    // The member (method, constructor) is the block right inside the class.
    let class_at = open.iter().position(|&b| is_type_header(&code[header_start(bytes, b)..b]))?;
    let member = *open.get(class_at.checked_sub(1)?)?;
    let close = matching_close(bytes, member)?;
    let line_start = code[..close].rfind('\n').map_or(0, |i| i + 1);
    let indent: String = text[line_start..close].chars().take_while(|c| *c == ' ' || *c == '\t').collect();
    let method = format!(
        "\n\n{indent}private void {}(object? sender, {} e)\n{indent}{{\n{indent}{indent_unit}throw new NotImplementedException();\n{indent}}}",
        offer.name, offer.args
    );
    Some((close + 1, method))
}

/// The name of the class the cursor is in (the last `class X` before it).
fn class_name(text_before: &str) -> Option<String> {
    let code = code_only(text_before);
    let at = code.rfind("class ")?;
    let name: String = code[at + 6..].trim_start().chars().take_while(|c| c.is_alphanumeric() || *c == '_').collect();
    (!name.is_empty()).then_some(name)
}

fn capitalize(s: &str) -> String {
    let mut c = s.chars();
    c.next().map_or_else(String::new, |f| f.to_uppercase().chain(c).collect())
}

/// Where the declaration before the `{` at `brace` starts (after the last `;`, `{` or `}`).
fn header_start(bytes: &[u8], brace: usize) -> usize {
    (0..brace).rev().find(|&i| matches!(bytes[i], b';' | b'{' | b'}')).map_or(0, |i| i + 1)
}

fn is_type_header(header: &str) -> bool {
    header
        .split(|c: char| !(c.is_alphanumeric() || c == '_'))
        .any(|w| matches!(w, "class" | "struct" | "record" | "interface"))
}

fn matching_close(bytes: &[u8], open: usize) -> Option<usize> {
    let mut depth = 0i32;
    for (i, &b) in bytes.iter().enumerate().skip(open) {
        match b {
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(i);
                }
            }
            _ => {}
        }
    }
    None
}

/// The event args type of common WinForms events; `EventArgs` for all others.
fn args_for(event: &str) -> &'static str {
    match event {
        "MouseClick" | "MouseDoubleClick" | "MouseDown" | "MouseUp" | "MouseMove" | "MouseWheel" => "MouseEventArgs",
        "KeyDown" | "KeyUp" => "KeyEventArgs",
        "KeyPress" => "KeyPressEventArgs",
        "PreviewKeyDown" => "PreviewKeyDownEventArgs",
        "FormClosing" => "FormClosingEventArgs",
        "FormClosed" => "FormClosedEventArgs",
        "Paint" => "PaintEventArgs",
        "DragDrop" | "DragEnter" | "DragOver" => "DragEventArgs",
        "Validating" => "System.ComponentModel.CancelEventArgs",
        "CellClick" | "CellContentClick" | "CellDoubleClick" | "CellEnter" | "CellLeave" | "CellValueChanged"
        | "CellEndEdit" | "CellBeginEdit" => "DataGridViewCellEventArgs",
        "ItemCheck" => "ItemCheckEventArgs",
        "AfterSelect" | "AfterCheck" | "AfterExpand" | "AfterCollapse" => "TreeViewEventArgs",
        "NodeMouseClick" | "NodeMouseDoubleClick" => "TreeNodeMouseClickEventArgs",
        "LinkClicked" => "LinkLabelLinkClickedEventArgs",
        "DoWork" => "System.ComponentModel.DoWorkEventArgs",
        "ProgressChanged" => "System.ComponentModel.ProgressChangedEventArgs",
        "RunWorkerCompleted" => "System.ComponentModel.RunWorkerCompletedEventArgs",
        _ => "EventArgs",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FORM: &str = "namespace App\n{\n    public partial class Form1 : Form\n    {\n        public Form1()\n        {\n            InitializeComponent();\n            cbxUsers.SelectedValueChanged += \n        }\n    }\n}\n";

    #[test]
    fn offers_the_handler_name_after_plus_equals() {
        let o = offer("            cbxUsers.SelectedValueChanged += ", "").unwrap();
        assert_eq!(o.name, "CbxUsers_SelectedValueChanged");
        assert_eq!(o.ghost, "CbxUsers_SelectedValueChanged;");
        assert_eq!(o.args, "EventArgs");
        assert_eq!(offer("btnOk.MouseDown +=", "").unwrap().ghost, " BtnOk_MouseDown;");
        assert_eq!(offer("btnOk.MouseDown +=", "").unwrap().args, "MouseEventArgs");
        // `this.Load` / `Load`: the class name.
        let before = "public partial class Form1 : Form { public Form1() { ";
        assert_eq!(offer("this.Load += ", before).unwrap().name, "Form1_Load");
        assert_eq!(offer("Load += ", before).unwrap().name, "Form1_Load");
        // Not an event subscription.
        assert_eq!(offer("count += ", ""), None);
        assert_eq!(offer("int x = a.B += ", ""), None);
        assert_eq!(offer("a.B + ", ""), None);
    }

    #[test]
    fn the_stub_goes_below_the_current_method() {
        let cursor = FORM.find("+= ").unwrap() + 3;
        let o = offer(&FORM[FORM[..cursor].rfind('\n').unwrap() + 1..cursor], &FORM[..cursor]).unwrap();
        let (at, method) = stub(FORM, cursor, &o, "    ").unwrap();
        let mut out = FORM.to_string();
        out.insert_str(at, &method);
        assert!(out.contains(
            "            cbxUsers.SelectedValueChanged += \n        }\n\n        private void CbxUsers_SelectedValueChanged(object? sender, EventArgs e)\n        {\n            throw new NotImplementedException();\n        }\n    }\n}"
        ), "{out}");
        // Already there: only the name.
        assert_eq!(stub(&out, cursor, &o, "    "), None);
    }
}
