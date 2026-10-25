//! Remotes: list (URLs shown without embedded credentials), add, rename, change URL, remove,
//! prune stale remote-tracking branches, fetch. Fetch and prune talk to the network and only run
//! on explicit user actions; their output has credentials scrubbed.

use std::path::Path;

use serde::Serialize;

use super::git::{self, GitResult};
use super::ops;

#[derive(Debug, Clone, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoteInfo {
    pub name: String,
    /// Credentials stripped.
    pub fetch_url: Option<String>,
    pub push_url: Option<String>,
    /// Remote-tracking branches this clone has.
    pub branches: usize,
}

/// Parses `git remote -v`: `origin\thttps://… (fetch)`.
pub fn parse_remote_v(out: &str) -> Vec<RemoteInfo> {
    let mut list: Vec<RemoteInfo> = Vec::new();
    for line in out.lines() {
        let Some((name, rest)) = line.split_once('\t') else { continue };
        let (url, kind) = match rest.rsplit_once(' ') {
            Some((u, k)) => (u.trim(), k.trim()),
            None => (rest.trim(), "(fetch)"),
        };
        let url = super::strip_credentials(url);
        let i = match list.iter().position(|r| r.name == name) {
            Some(i) => i,
            None => {
                list.push(RemoteInfo { name: name.to_string(), ..Default::default() });
                list.len() - 1
            }
        };
        if kind == "(push)" {
            list[i].push_url = Some(url);
        } else {
            list[i].fetch_url = Some(url);
        }
    }
    list
}

pub fn list(dir: &Path) -> GitResult<Vec<RemoteInfo>> {
    let mut list = parse_remote_v(&git::run(dir, &["remote", "-v"])?);
    let tracking = git::run(dir, &["for-each-ref", "--format=%(refname)", "refs/remotes"]).unwrap_or_default();
    for r in &mut list {
        let prefix = format!("refs/remotes/{}/", r.name);
        r.branches = tracking.lines().filter(|l| l.starts_with(&prefix) && !l.ends_with("/HEAD")).count();
    }
    Ok(list)
}

fn check_name(name: &str) -> GitResult<()> {
    if git::valid_branch(name) { Ok(()) } else { Err(format!("“{name}” isn't a valid remote name")) }
}

fn check_url(url: &str) -> GitResult<()> {
    let u = url.trim();
    if u.is_empty() || u.starts_with('-') || u.chars().any(|c| c.is_whitespace() || c.is_control()) { Err("that isn't a remote URL".into()) } else { Ok(()) }
}

fn exists(dir: &Path, name: &str) -> GitResult<()> {
    if super::compare::remotes(dir).iter().any(|r| r == name) { Ok(()) } else { Err(format!("no remote “{name}”")) }
}

pub fn add(dir: &Path, name: &str, url: &str) -> GitResult<()> {
    check_name(name)?;
    check_url(url)?;
    git::mutate(dir, || git::run(dir, &["remote", "add", name, url.trim()]).map(|_| ()).map_err(|e| ops::scrub(&e)))
}

pub fn rename(dir: &Path, old: &str, new: &str) -> GitResult<()> {
    exists(dir, old)?;
    check_name(new)?;
    git::mutate(dir, || git::run(dir, &["remote", "rename", old, new]).map(|_| ()))
}

pub fn set_url(dir: &Path, name: &str, url: &str) -> GitResult<()> {
    exists(dir, name)?;
    check_url(url)?;
    git::mutate(dir, || git::run(dir, &["remote", "set-url", name, url.trim()]).map(|_| ()).map_err(|e| ops::scrub(&e)))
}

/// `git remote remove`: the remote, its remote-tracking branches and the upstream settings that
/// point at it go (the branches themselves stay).
pub fn remove(dir: &Path, name: &str) -> GitResult<()> {
    exists(dir, name)?;
    git::mutate(dir, || git::run(dir, &["remote", "remove", name]).map(|_| ()))
}

/// `git remote prune <name>`: drops remote-tracking branches deleted on the remote. Network.
pub fn prune(dir: &Path, name: &str) -> GitResult<String> {
    exists(dir, name)?;
    git::mutate(dir, || ops::network(dir, &["remote", "prune", name]))
}

/// `git fetch [--prune] <remote>` or `--all`. Network.
pub fn fetch(dir: &Path, remote: Option<&str>, prune: bool) -> GitResult<String> {
    if let Some(r) = remote {
        exists(dir, r)?;
    }
    git::mutate(dir, || {
        let mut args = vec!["fetch"];
        if prune {
            args.push("--prune");
        }
        match remote {
            Some(r) => args.push(r),
            None => args.push("--all"),
        }
        ops::network(dir, &args)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::repos::testgit::repo;

    #[test]
    fn parses_and_strips_credentials() {
        let out = "origin\thttps://user:s3cret@github.com/acme/web.git (fetch)\norigin\thttps://user:s3cret@github.com/acme/web.git (push)\nfork\tgit@github.com:me/web.git (fetch)\nfork\tgit@github.com:me/web.git (push)\n";
        let l = parse_remote_v(out);
        assert_eq!(l.len(), 2);
        assert_eq!(l[0].fetch_url.as_deref(), Some("https://github.com/acme/web.git"));
        assert!(!format!("{l:?}").contains("s3cret"));
        assert_eq!(l[1].push_url.as_deref(), Some("git@github.com:me/web.git"));
    }

    #[test]
    fn add_rename_prune_fetch_remove() {
        let Some(up) = repo() else { return };
        up.write("a.txt", "a\n");
        up.commit("base");
        up.git(&["branch", "gone-soon"]);
        let Some(r) = repo() else { return };
        let url = up.root().display().to_string();
        assert!(add(r.root(), "-x", &url).is_err());
        assert!(add(r.root(), "up", "--upload-pack=evil").is_err());
        add(r.root(), "up", &url).unwrap();
        assert!(fetch(r.root(), Some("up"), false).is_ok());
        let l = list(r.root()).unwrap();
        assert_eq!(l[0].name, "up");
        assert_eq!(l[0].branches, 2);
        up.git(&["branch", "-D", "gone-soon"]);
        prune(r.root(), "up").unwrap();
        assert_eq!(list(r.root()).unwrap()[0].branches, 1);
        rename(r.root(), "up", "origin").unwrap();
        assert!(fetch(r.root(), None, true).is_ok());
        assert_eq!(list(r.root()).unwrap()[0].name, "origin");
        assert!(remove(r.root(), "nope").is_err());
        remove(r.root(), "origin").unwrap();
        assert!(list(r.root()).unwrap().is_empty());
    }
}
