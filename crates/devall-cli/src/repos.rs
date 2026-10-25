//! `devall repos`: the git repositories of a (multi-repo) project — `list`, `status`, `add`,
//! `remove`, `locate`. Paths in `devall.yaml` stay relative; `locate` (and `add --local`) keep a
//! machine-specific location in the project data folder, never in the repo.

use std::path::PathBuf;
use std::process::ExitCode;

use anyhow::{Context, Result};
use clap::Subcommand;
use devall_core::repos::{self, Repo, RepoOrigin, RepoState, git};

#[derive(Subcommand)]
pub enum ReposCmd {
    /// The project's repositories: listed in devall.yaml, the one containing the project folder,
    /// and nested ones not listed yet.
    List {
        #[arg(short, long)]
        project: Option<PathBuf>,
        /// Machine-readable output.
        #[arg(long)]
        json: bool,
    },
    /// Branch, ahead/behind and uncommitted changes of every repository (concurrently).
    Status {
        #[arg(short, long)]
        project: Option<PathBuf>,
        #[arg(long)]
        json: bool,
    },
    /// Add a folder as a repository (stored relative to the project in devall.yaml).
    Add {
        /// The repository folder.
        folder: PathBuf,
        /// Name (default: the folder name); files in it are referenced as repo:<name>/<path>.
        #[arg(long)]
        name: Option<String>,
        /// Group shown in the app (e.g. Frontend, Backend).
        #[arg(long)]
        group: Option<String>,
        /// Keep the location on this machine only; devall.yaml gets ../<folder name>.
        #[arg(long)]
        local: bool,
        #[arg(short, long)]
        project: Option<PathBuf>,
    },
    /// Remove a repository from devall.yaml (its files are left alone).
    Remove {
        name: String,
        #[arg(short, long)]
        project: Option<PathBuf>,
    },
    /// Point a listed repository at a folder on this machine (kept outside the repo).
    Locate {
        name: String,
        folder: PathBuf,
        #[arg(short, long)]
        project: Option<PathBuf>,
    },
}

fn absolute(p: PathBuf) -> Result<PathBuf> {
    std::path::absolute(&p).with_context(|| format!("can't resolve {}", p.display()))
}

fn state_label(r: &Repo) -> &'static str {
    match (r.state, r.origin) {
        (RepoState::Missing, _) => "missing",
        (RepoState::NotARepo, _) => "not a repo",
        (_, RepoOrigin::Discovered) => "discovered",
        (_, RepoOrigin::Containing) => "contains project",
        (_, RepoOrigin::Local) => "worktree (this machine)",
        _ => "ok",
    }
}

pub fn run(cmd: ReposCmd) -> Result<ExitCode> {
    match cmd {
        ReposCmd::List { project, json } => {
            let p = crate::find_project(project)?;
            let list = p.repos();
            if json {
                println!("{}", serde_json::to_string_pretty(&list)?);
            } else if list.is_empty() {
                println!("No repositories (the project folder isn't in a git repo and lists none). Add one: devall repos add <folder>");
            } else {
                for r in &list {
                    let shared = r.rel_path.as_deref().map(|p| format!("  ({p})")).unwrap_or_default();
                    let local = if r.overridden { "  [this machine]" } else { "" };
                    println!("{:<20} {:<12} {:<16} {}{shared}{local}", r.name, r.group.as_deref().unwrap_or("-"), state_label(r), r.path.display());
                }
            }
            Ok(ExitCode::SUCCESS)
        }
        ReposCmd::Status { project, json } => {
            let p = crate::find_project(project)?;
            let rows = git::parallel(p.repos(), git::PARALLEL, |r| {
                let s = r.is_found().then(|| git::summary(&r.path));
                (r, s)
            });
            if json {
                let out: Vec<serde_json::Value> = rows.iter().map(|(r, s)| serde_json::json!({ "repo": r, "summary": s })).collect();
                println!("{}", serde_json::to_string_pretty(&out)?);
                return Ok(ExitCode::SUCCESS);
            }
            for (r, s) in &rows {
                let line = match s {
                    None => state_label(r).to_string(),
                    Some(s) if s.error.is_some() => format!("error: {}", s.error.as_deref().unwrap_or_default()),
                    Some(s) => {
                        let branch = s.branch.as_deref().unwrap_or("(detached)");
                        let sync = if s.upstream.is_some() { format!("↑{} ↓{}", s.ahead, s.behind) } else { "no upstream".into() };
                        let dirty = if s.changes == 0 { "clean".to_string() } else { format!("{} changed", s.changes) };
                        format!("{branch:<24} {sync:<12} {dirty}")
                    }
                };
                println!("{:<20} {line}", r.name);
            }
            Ok(ExitCode::SUCCESS)
        }
        ReposCmd::Add { folder, name, group, local, project } => {
            let mut p = crate::find_project(project)?;
            let folder = absolute(folder)?;
            let s = repos::suggest(&p, &folder);
            let r = repos::add(&mut p, &folder, name.as_deref(), group.as_deref(), local)?;
            if !s.is_git {
                eprintln!("note: {} isn't a git repository yet", folder.display());
            }
            if local || s.rel_path.is_none() {
                println!("Added {} (devall.yaml: {}; this machine: {})", r.name, r.path, folder.display());
            } else {
                println!("Added {} (devall.yaml: {})", r.name, r.path);
            }
            Ok(ExitCode::SUCCESS)
        }
        ReposCmd::Remove { name, project } => {
            let mut p = crate::find_project(project)?;
            repos::remove(&mut p, &name)?;
            println!("Removed {name} from devall.yaml (files untouched)");
            Ok(ExitCode::SUCCESS)
        }
        ReposCmd::Locate { name, folder, project } => {
            let p = crate::find_project(project)?;
            let folder = absolute(folder)?;
            repos::locate(&p, &name, &folder)?;
            println!("{name} → {} on this machine", folder.display());
            Ok(ExitCode::SUCCESS)
        }
    }
}
