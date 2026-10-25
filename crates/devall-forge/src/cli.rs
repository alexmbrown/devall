//! Fallback without a token: an installed, signed-in GitHub CLI (`gh pr create`) or GitLab CLI
//! (`glab mr create`). Their own credentials apply; /dev/all never sees a token.

use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};

use serde::Serialize;

use crate::{Created, ForgeError, ForgeKind, NewRequest, Remote, Result};

pub fn program(kind: ForgeKind) -> &'static str {
    match kind {
        ForgeKind::Github => "gh",
        ForgeKind::Gitlab => "glab",
    }
}

fn command(program: &str) -> Command {
    let mut cmd = Command::new(program);
    cmd.env("GH_PROMPT_DISABLED", "1").env("GH_NO_UPDATE_NOTIFIER", "1").env("NO_PROMPT", "1").env("NO_COLOR", "1").env("GIT_TERMINAL_PROMPT", "0");
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000);
    }
    cmd
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CliStatus {
    pub program: String,
    pub installed: bool,
}

/// Is `gh` / `glab` installed? (No network: whether it's signed in shows when it runs.)
pub fn status(kind: ForgeKind) -> CliStatus {
    let p = program(kind);
    let installed = command(p).arg("--version").stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).status().is_ok_and(|s| s.success());
    CliStatus { program: p.into(), installed }
}

/// The arguments for one CLI (pure, so tests can check them).
pub fn args(remote: &Remote, req: &NewRequest) -> Vec<String> {
    let mut a: Vec<String> = Vec::new();
    let list = |v: &[String]| v.iter().map(|s| s.trim()).filter(|s| !s.is_empty()).map(str::to_string).collect::<Vec<_>>();
    match remote.kind {
        ForgeKind::Github => {
            a.extend(["pr", "create", "--repo"].map(String::from));
            a.push(format!("{}/{}", remote.host, remote.path));
            a.extend(["--base".into(), req.base.clone(), "--head".into(), req.head.clone(), "--title".into(), req.title.trim().to_string(), "--body-file".into(), "-".into()]);
            if req.draft {
                a.push("--draft".into());
            }
            for r in list(&req.reviewers) {
                a.extend(["--reviewer".into(), r]);
            }
            for l in list(&req.labels) {
                a.extend(["--label".into(), l]);
            }
        }
        ForgeKind::Gitlab => {
            a.extend(["mr", "create", "--repo"].map(String::from));
            a.push(remote.web_url.clone());
            a.extend(["--source-branch".into(), req.head.clone(), "--target-branch".into(), req.base.clone(), "--title".into(), req.title.trim().to_string(), "--description".into(), req.body.clone(), "--yes".into()]);
            if req.draft {
                a.push("--draft".into());
            }
            let reviewers = list(&req.reviewers);
            if !reviewers.is_empty() {
                a.extend(["--reviewer".into(), reviewers.join(",")]);
            }
            let labels = list(&req.labels);
            if !labels.is_empty() {
                a.extend(["--label".into(), labels.join(",")]);
            }
        }
    }
    a
}

/// The request's URL in the CLI's output, and its number (the URL's last segment).
pub fn parse_output(out: &str) -> Option<(u64, String)> {
    let url = out.split_whitespace().filter(|w| w.starts_with("https://") || w.starts_with("http://")).find(|w| w.contains("/pull/") || w.contains("/merge_requests/"))?.trim_end_matches(['.', ',']);
    let n = url.trim_end_matches('/').rsplit('/').next()?.parse().ok()?;
    Some((n, url.to_string()))
}

/// Runs `gh pr create` / `glab mr create` in `dir`.
pub fn create(dir: &Path, remote: &Remote, req: &NewRequest) -> Result<Created> {
    let p = program(remote.kind);
    let mut child = command(p)
        .args(args(remote, req))
        .current_dir(dir)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| ForgeError::Cli(format!("{p} isn't installed or not on PATH ({e})")))?;
    if let Some(mut stdin) = child.stdin.take() {
        // gh reads the body from stdin (`--body-file -`); glab ignores it.
        let body = if remote.kind == ForgeKind::Github { req.body.clone() } else { String::new() };
        std::thread::spawn(move || {
            let _ = stdin.write_all(body.as_bytes());
        });
    }
    let out = child.wait_with_output().map_err(|e| ForgeError::Cli(e.to_string()))?;
    let text = format!("{}\n{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    if !out.status.success() {
        let msg = text.trim();
        let hint = if msg.contains("auth login") || msg.contains("not logged") || msg.contains("authentication") { format!(" — run `{p} auth login` in a terminal, or connect a token") } else { String::new() };
        return Err(ForgeError::Cli(format!("{p} failed: {}{hint}", msg.lines().rev().take(4).collect::<Vec<_>>().into_iter().rev().collect::<Vec<_>>().join("\n"))));
    }
    let (number, url) = parse_output(&text).ok_or_else(|| ForgeError::Cli(format!("{p} finished but didn't print the {} URL:\n{}", remote.kind.noun(), text.trim())))?;
    Ok(Created { number, url, warnings: vec![], via_cli: true })
}
