//! Jupyter notebooks (`.ipynb`, nbformat 4) as notes: runnable runbooks shared through the repo.
//! Files stay standard so they open in Jupyter and VS Code. Each code cell's language is taken,
//! in order, from `metadata.devall.language`, `metadata.vscode.languageId`, a Jupyter cell magic
//! on the first line (`%%bash`, `%%sh`, `%%powershell`, `%%cmd`, `%%js`, `%%node`, `%%python`), or
//! the notebook's kernel language.

use serde_json::{Value, json};

pub const NOTEBOOK_EXT: &str = ".ipynb";

/// Languages /dev/all can run.
pub const LANGUAGES: &[&str] = &["shell", "powershell", "cmd", "javascript", "python"];

fn source_text(cell: &Value) -> String {
    match cell.get("source") {
        Some(Value::Array(lines)) => lines.iter().filter_map(Value::as_str).collect(),
        Some(Value::String(s)) => s.clone(),
        _ => String::new(),
    }
}

/// Normalizes a language name (`bash`, `sh`, `zsh` → `shell`; `js`, `node` → `javascript`; …).
pub fn normalize_language(lang: &str) -> Option<&'static str> {
    Some(match lang.trim().to_ascii_lowercase().as_str() {
        "shell" | "bash" | "sh" | "zsh" | "shellscript" => "shell",
        "powershell" | "pwsh" | "ps1" => "powershell",
        "cmd" | "bat" | "batch" => "cmd",
        "javascript" | "js" | "node" | "nodejs" => "javascript",
        "python" | "python3" | "py" => "python",
        _ => return None,
    })
}

/// The language of a code cell (see module docs) and the code with any cell magic stripped.
pub fn cell_language(cell: &Value, notebook: &Value) -> (Option<&'static str>, String) {
    let src = source_text(cell);
    if let Some(first) = src.lines().next()
        && let Some(magic) = first.trim().strip_prefix("%%")
        && let Some(lang) = normalize_language(magic.split_whitespace().next().unwrap_or(""))
    {
        let rest = src.split_once('\n').map(|(_, r)| r.to_string()).unwrap_or_default();
        return (Some(lang), rest);
    }
    let meta = cell.get("metadata");
    let explicit = meta
        .and_then(|m| m.pointer("/devall/language").or_else(|| m.pointer("/vscode/languageId")))
        .and_then(Value::as_str)
        .and_then(normalize_language);
    let kernel = notebook
        .pointer("/metadata/language_info/name")
        .or_else(|| notebook.pointer("/metadata/kernelspec/language"))
        .and_then(Value::as_str)
        .and_then(normalize_language);
    (explicit.or(kernel), src)
}

/// (title from the first `# ` heading, plain-text rendering for search and AI context).
pub fn notebook_text(json_text: &str) -> (Option<String>, String) {
    let Ok(nb) = serde_json::from_str::<Value>(json_text) else { return (None, String::new()) };
    let mut title = None;
    let mut out = String::new();
    for cell in nb.get("cells").and_then(Value::as_array).into_iter().flatten() {
        let kind = cell.get("cell_type").and_then(Value::as_str).unwrap_or("");
        if kind == "markdown" {
            let text = source_text(cell);
            if title.is_none() {
                title = text.lines().find_map(|l| l.trim().strip_prefix("# ").map(|t| t.trim().to_string()));
            }
            out.push_str(&text);
            out.push_str("\n\n");
        } else if kind == "code" {
            let (lang, code) = cell_language(cell, &nb);
            out.push_str(&format!("```{}\n{}\n```\n\n", lang.unwrap_or(""), code.trim_end()));
        }
    }
    (title, out)
}

/// A new notebook with a title cell and an example shell cell.
pub fn new_notebook(title: &str) -> String {
    let nb = json!({
        "cells": [
            { "cell_type": "markdown", "metadata": {}, "source": [format!("# {}\n", title.trim()), "\n", "Runnable runbook: click ▶ on a cell to run it on your machine (from the project folder)."] },
            { "cell_type": "code", "execution_count": null, "metadata": { "devall": { "language": "shell" } }, "outputs": [], "source": ["echo \"Hello from $(pwd)\""] }
        ],
        "metadata": {
            "kernelspec": { "display_name": "Polyglot (/dev/all)", "language": "shell", "name": "devall" },
            "language_info": { "name": "shell" }
        },
        "nbformat": 4,
        "nbformat_minor": 5
    });
    format!("{}\n", serde_json::to_string_pretty(&nb).unwrap_or_default())
}

/// Validates notebook JSON before saving (nbformat 4 with a `cells` array).
pub fn validate(json_text: &str) -> Result<(), String> {
    let v: Value = serde_json::from_str(json_text).map_err(|e| format!("not valid JSON: {e}"))?;
    if v.get("nbformat").and_then(Value::as_u64) != Some(4) {
        return Err("only nbformat 4 notebooks are supported".into());
    }
    if !v.get("cells").is_some_and(Value::is_array) {
        return Err("notebook has no cells array".into());
    }
    Ok(())
}

/// Sets the title in the first markdown `# ` heading (used when a notebook is renamed).
pub fn retitle(json_text: &str, title: &str) -> Result<String, String> {
    let mut nb: Value = serde_json::from_str(json_text).map_err(|e| e.to_string())?;
    let mut done = false;
    if let Some(cells) = nb.get_mut("cells").and_then(Value::as_array_mut) {
        for cell in cells.iter_mut().filter(|c| c.get("cell_type").and_then(Value::as_str) == Some("markdown")) {
            let text = source_text(cell);
            if text.lines().any(|l| l.trim_start().starts_with("# ")) {
                let mut replaced = false;
                let new_text: Vec<String> = text
                    .split_inclusive('\n')
                    .map(|l| {
                        if !replaced && l.trim_start().starts_with("# ") {
                            replaced = true;
                            format!("# {}{}", title.trim(), if l.ends_with('\n') { "\n" } else { "" })
                        } else {
                            l.to_string()
                        }
                    })
                    .collect();
                cell["source"] = Value::Array(new_text.into_iter().map(Value::String).collect());
                done = true;
                break;
            }
        }
        if !done {
            cells.insert(0, json!({ "cell_type": "markdown", "metadata": {}, "source": [format!("# {}", title.trim())] }));
        }
    }
    Ok(format!("{}\n", serde_json::to_string_pretty(&nb).map_err(|e| e.to_string())?))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn languages_and_text() {
        let nb = new_notebook("Run the app");
        validate(&nb).unwrap();
        let (title, text) = notebook_text(&nb);
        assert_eq!(title.as_deref(), Some("Run the app"));
        assert!(text.contains("```shell\necho"));

        let v: Value = serde_json::from_str(r#"{"nbformat":4,"cells":[],"metadata":{"kernelspec":{"language":"python"}}}"#).unwrap();
        let magic = json!({"cell_type":"code","source":["%%bash\n","npm run dev\n"]});
        assert_eq!(cell_language(&magic, &v), (Some("shell"), "npm run dev\n".to_string()));
        let vscode = json!({"cell_type":"code","metadata":{"vscode":{"languageId":"javascript"}},"source":"console.log(1)"});
        assert_eq!(cell_language(&vscode, &v).0, Some("javascript"));
        let kernel = json!({"cell_type":"code","source":"print(1)"});
        assert_eq!(cell_language(&kernel, &v).0, Some("python"));
        assert!(validate("{}").is_err());

        let renamed = retitle(&nb, "Start everything").unwrap();
        assert_eq!(notebook_text(&renamed).0.as_deref(), Some("Start everything"));
    }
}
