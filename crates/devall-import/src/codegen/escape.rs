//! String literal escaping per target language.

use serde_json::Value;

/// A double-quoted literal with JSON escapes — valid as-is in JavaScript, Python, Go, Java and C#.
pub(crate) fn json_str(s: &str) -> String {
    serde_json::to_string(s).unwrap_or_else(|_| "\"\"".into())
}

/// Rust: `\u{…}` escapes, no `\b`/`\f`.
pub(crate) fn rust_str(s: &str) -> String {
    let mut out = String::from("\"");
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\0' => out.push_str("\\0"),
            c if c.is_control() => out.push_str(&format!("\\u{{{:x}}}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// Rust raw string `r#"…"#` with enough `#`s (for bodies; keeps them readable).
pub(crate) fn rust_raw(s: &str) -> String {
    if s.contains('\r') || s.chars().any(|c| c.is_control() && c != '\n' && c != '\t') {
        return rust_str(s);
    }
    let mut hashes = 0;
    while s.contains(&format!("\"{}", "#".repeat(hashes))) {
        hashes += 1;
    }
    let h = "#".repeat(hashes);
    format!("r{h}\"{s}\"{h}")
}

/// Swift: `\u{…}` escapes, no `\b`/`\f`.
pub(crate) fn swift_str(s: &str) -> String {
    let mut out = String::from("\"");
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\0' => out.push_str("\\0"),
            c if c.is_control() => out.push_str(&format!("\\u{{{:x}}}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// Kotlin: like Java but `$` starts a template and `\f` doesn't exist.
pub(crate) fn kotlin_str(s: &str) -> String {
    let mut out = String::from("\"");
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '$' => out.push_str("\\$"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{8}' => out.push_str("\\b"),
            c if c.is_control() => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// Single-quoted PHP / Ruby literal: only `\` and `'` are special.
pub(crate) fn single_quoted(s: &str) -> String {
    format!("'{}'", s.replace('\\', "\\\\").replace('\'', "\\'"))
}

/// PowerShell single-quoted literal (curly quotes are quote characters too).
pub(crate) fn ps_str(s: &str) -> String {
    let mut out = String::from("'");
    for c in s.chars() {
        if matches!(c, '\'' | '\u{2018}' | '\u{2019}' | '\u{201A}' | '\u{201B}') {
            out.push(c);
        }
        out.push(c);
    }
    out.push('\'');
    out
}

/// POSIX shell word (quoted only when needed).
pub(crate) fn sh_word(s: &str) -> String {
    if !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric() || "-_./:=@%+,".contains(c)) {
        return s.to_string();
    }
    format!("'{}'", s.replace('\'', "'\\''"))
}

/// A Python literal for a JSON value (`True`, `None`, …), indented by `indent` spaces.
pub(crate) fn py_value(v: &Value, indent: usize) -> String {
    let pad = " ".repeat(indent + 4);
    let end = " ".repeat(indent);
    match v {
        Value::Null => "None".into(),
        Value::Bool(true) => "True".into(),
        Value::Bool(false) => "False".into(),
        Value::Number(n) => n.to_string(),
        Value::String(s) => json_str(s),
        Value::Array(a) if a.is_empty() => "[]".into(),
        Value::Array(a) => format!("[\n{}\n{end}]", a.iter().map(|x| format!("{pad}{},", py_value(x, indent + 4))).collect::<Vec<_>>().join("\n")),
        Value::Object(m) if m.is_empty() => "{}".into(),
        Value::Object(m) => format!(
            "{{\n{}\n{end}}}",
            m.iter().map(|(k, x)| format!("{pad}{}: {},", json_str(k), py_value(x, indent + 4))).collect::<Vec<_>>().join("\n")
        ),
    }
}

/// Pretty JSON (2-space) whose continuation lines are indented by `indent` spaces.
pub(crate) fn json_block(v: &Value, indent: usize) -> String {
    let text = serde_json::to_string_pretty(v).unwrap_or_default();
    indent_tail(&text, &" ".repeat(indent))
}

/// Prefixes every line but the first with `prefix`.
pub(crate) fn indent_tail(s: &str, prefix: &str) -> String {
    let mut out = String::new();
    for (i, line) in s.lines().enumerate() {
        if i > 0 {
            out.push('\n');
            if !line.is_empty() {
                out.push_str(prefix);
            }
        }
        out.push_str(line);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escapes() {
        let s = "a\"b\\c\nd$e'f\u{1}";
        assert_eq!(json_str(s), r#""a\"b\\c\nd$e'f\u0001""#);
        assert_eq!(rust_str(s), r#""a\"b\\c\nd$e'f\u{1}""#);
        assert_eq!(kotlin_str(s), r#""a\"b\\c\nd\$e'f\u0001""#);
        assert_eq!(swift_str(s), r#""a\"b\\c\nd$e'f\u{1}""#);
        assert_eq!(single_quoted(s), "'a\"b\\\\c\nd$e\\'f\u{1}'");
        assert_eq!(ps_str("it's ‘x’"), "'it''s ‘‘x’’'");
        assert_eq!(sh_word("a'b"), r#"'a'\''b'"#);
        assert_eq!(sh_word("https://x.io/a"), "https://x.io/a");
        assert_eq!(rust_raw("{\"a\": \"#\"}"), "r##\"{\"a\": \"#\"}\"##");
    }

    #[test]
    fn python_literals() {
        let v: Value = serde_json::from_str(r#"{"a": [true, null, 1.5], "b": {}}"#).unwrap();
        assert_eq!(py_value(&v, 0), "{\n    \"a\": [\n        True,\n        None,\n        1.5,\n    ],\n    \"b\": {},\n}");
    }
}
