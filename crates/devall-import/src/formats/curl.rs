//! One or more cURL commands (bash, cmd or PowerShell style), parsed by `devall_core::curl`.

use crate::detect::Format;
use crate::error::{Error, Result};
use crate::plan::{ImportPlan, PlanItem};

pub(crate) fn parse(text: &str) -> Result<ImportPlan> {
    let mut plan = ImportPlan::new("cURL import", Format::Curl);
    let mut first_err = None;
    for (i, cmd) in split_commands(text).iter().enumerate() {
        match devall_core::curl::parse_curl(cmd) {
            Ok(r) => plan.items.push(PlanItem::Request(Box::new(r))),
            Err(e) => {
                plan.warnings.push(format!("Command {} was skipped: {e}", i + 1));
                first_err.get_or_insert(e.to_string());
            }
        }
    }
    if plan.items.is_empty() {
        return Err(Error::Parse(first_err.unwrap_or_else(|| "no cURL command found".into())));
    }
    Ok(plan)
}

/// Splits text into commands: a new command starts at a line beginning with `curl` that is
/// neither a continuation (`\`, `^`, `` ` ``) nor inside a quoted argument. Comment lines go.
pub(crate) fn split_commands(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let (mut single, mut double) = (false, false);
    for line in text.trim_start_matches('\u{feff}').lines() {
        let t = line.trim_start();
        let lower = t.to_ascii_lowercase();
        let in_quote = single || double;
        let continued = cur.trim_end().ends_with(['\\', '^', '`']);
        if !in_quote && !continued {
            if t.starts_with('#') {
                continue;
            }
            if (lower.starts_with("curl ") || lower == "curl" || lower.starts_with("curl.exe")) && !cur.trim().is_empty() {
                out.push(std::mem::take(&mut cur));
            }
        }
        let mut chars = line.chars().peekable();
        while let Some(c) = chars.next() {
            match c {
                '\\' if !single => {
                    chars.next();
                }
                '\'' if !double => single = !single,
                '"' if !single => double = !double,
                _ => {}
            }
        }
        cur.push_str(line);
        cur.push('\n');
    }
    if !cur.trim().is_empty() {
        out.push(cur);
    }
    out.into_iter().map(|c| c.trim().to_string()).filter(|c| !c.is_empty()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_commands() {
        let text = "# list\ncurl https://x.io/a\n\ncurl -X POST https://x.io/b \\\n  -d '{\n\n\"a\":1}'\ncurl https://x.io/c";
        let cmds = split_commands(text);
        assert_eq!(cmds.len(), 3, "{cmds:?}");
        assert!(cmds[1].contains("\n\n\"a\""));
        let plan = parse(text).unwrap();
        assert_eq!(plan.request_count(), 3);
    }

    #[test]
    fn bad_commands_become_warnings() {
        let plan = parse("curl https://x.io/a\ncurl -H 'A: b'").unwrap();
        assert_eq!(plan.request_count(), 1);
        assert_eq!(plan.warnings.len(), 1);
        assert!(parse("curl -H 'A: b'").is_err());
    }
}
