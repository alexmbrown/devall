use super::*;
use crate::model::RepoRef;

/// A fake work tree: `.git/` with an origin URL (enough for resolution; no git needed).
fn fake_repo(dir: &Path, remote: Option<&str>) {
    fs::create_dir_all(dir.join(".git")).unwrap();
    fs::write(dir.join(".git/HEAD"), "ref: refs/heads/main\n").unwrap();
    if let Some(r) = remote {
        fs::write(dir.join(".git/config"), format!("[core]\n\tbare = false\n[remote \"origin\"]\n\turl = {r}\n\tfetch = +refs/heads/*:refs/remotes/origin/*\n")).unwrap();
    }
}

fn repo_ref(name: &str, path: &str, group: Option<&str>) -> RepoRef {
    RepoRef { name: name.into(), path: path.into(), remote: None, group: group.map(str::to_string) }
}

/// `<tmp>/ws/shop` (the project, not itself a repo) with nested repos, plus `<tmp>/ws/api` next to it.
fn workspace() -> (tempfile::TempDir, Project) {
    let tmp = tempfile::tempdir().unwrap();
    let ws = tmp.path().join("ws");
    let shop = ws.join("shop");
    fs::create_dir_all(&shop).unwrap();
    fake_repo(&shop.join("frontend/web"), Some("git@example.com:acme/web.git"));
    fake_repo(&shop.join("frontend/admin"), None);
    fake_repo(&shop.join("backend/orders"), None);
    fake_repo(&ws.join("api"), Some("https://example.com/acme/api.git"));
    let p = Project::init(&shop, "Shop").unwrap();
    (tmp, p)
}

#[test]
fn discovers_nested_repos_within_limits() {
    let (tmp, p) = workspace();
    let shop = p.root();
    // Too deep, inside skipped folders, inside a content folder, hidden, or inside another repo.
    fake_repo(&shop.join("a/b/c/d"), None);
    fake_repo(&shop.join("node_modules/pkg"), None);
    fake_repo(&shop.join("notes/vendored"), None);
    fake_repo(&shop.join(".cache/x"), None);
    fake_repo(&shop.join("frontend/web/packages/inner"), None);
    // Exactly at the depth limit.
    fake_repo(&shop.join("x/y/z"), None);
    assert_eq!(discover(shop, false), ["backend/orders", "frontend/admin", "frontend/web", "x/y/z"]);
    // A worktree/submodule (`.git` file) is skipped only when the project lives in a repo.
    fs::create_dir_all(shop.join("libs/sub")).unwrap();
    fs::write(shop.join("libs/sub/.git"), "gitdir: ../../.git/modules/sub\n").unwrap();
    assert!(discover(shop, false).contains(&"libs/sub".to_string()));
    assert!(!discover(shop, true).contains(&"libs/sub".to_string()));
    drop(tmp);
}

#[test]
fn resolves_listed_discovered_and_external_repos() {
    let (_tmp, mut p) = workspace();
    p.manifest.repos = vec![repo_ref("web", "frontend/web", Some("Frontend")), repo_ref("api", "../api", Some("Backend")), repo_ref("billing", "../billing", Some("Backend"))];
    p.save_manifest().unwrap();
    let p = Project::open(p.root()).unwrap();
    let repos = p.repos();
    let names: Vec<(&str, RepoOrigin, RepoState)> = repos.iter().map(|r| (r.name.as_str(), r.origin, r.state)).collect();
    assert_eq!(
        names,
        [
            ("web", RepoOrigin::Manifest, RepoState::Found),
            ("api", RepoOrigin::Manifest, RepoState::Found),
            ("billing", RepoOrigin::Manifest, RepoState::Missing),
            ("orders", RepoOrigin::Discovered, RepoState::Found),
            ("admin", RepoOrigin::Discovered, RepoState::Found),
        ]
    );
    let api = find(&repos, "api").unwrap();
    assert!(same(&api.path, &p.root().parent().unwrap().join("api")), "{}", api.path.display());
    assert_eq!(api.group.as_deref(), Some("Backend"));
    assert_eq!(api.project_prefix, None);
    // Discovered repos carry what saving them needs.
    let admin = find(&repos, "admin").unwrap();
    assert_eq!(admin.rel_path.as_deref(), Some("frontend/admin"));
    assert_eq!(find(&repos, "WEB").unwrap().name, "web", "case-insensitive lookup");
    // No repo contains the project folder, so there's no default "containing" repo.
    assert_eq!(default_repo(&repos).unwrap().name, "web");
    assert!(p.repo("orders").is_some(), "unlisted repos are found by name too");
}

#[test]
fn overrides_relocate_listed_repos_on_this_machine() {
    let (tmp, mut p) = workspace();
    let elsewhere = tmp.path().join("elsewhere").join("billing-svc");
    fake_repo(&elsewhere, None);
    p.manifest.repos = vec![repo_ref("billing", "../billing", None)];
    p.save_manifest().unwrap();
    assert_eq!(p.repo("billing").unwrap().state, RepoState::Missing);

    locate(&p, "billing", &elsewhere).unwrap();
    let r = p.repo("billing").unwrap();
    assert_eq!((r.state, r.overridden), (RepoState::Found, true));
    assert!(same(&r.path, &elsewhere));
    assert_eq!(r.rel_path.as_deref(), Some("../billing"), "devall.yaml keeps the shared path");
    let yaml = fs::read_to_string(p.root().join("devall.yaml")).unwrap();
    assert!(!yaml.contains("elsewhere"), "absolute locations never land in devall.yaml: {yaml}");

    // Renaming carries the location along; pointing back at the shared path clears it.
    update(&mut p, "billing", Some("billing-svc"), Some("Backend")).unwrap();
    assert!(read_overrides(p.root()).contains_key("billing-svc"));
    let shared = p.root().parent().unwrap().join("billing");
    fake_repo(&shared, None);
    locate(&p, "billing-svc", &shared).unwrap();
    assert!(read_overrides(p.root()).is_empty());
    remove(&mut p, "billing-svc").unwrap();
    assert!(Project::open(p.root()).unwrap().manifest.repos.is_empty());
}

#[test]
fn containing_repo_is_the_single_repo_case() {
    let tmp = tempfile::tempdir().unwrap();
    let mono = tmp.path().join("mono");
    fake_repo(&mono, Some("git@example.com:acme/mono.git"));
    let svc = mono.join("services").join("billing");
    fs::create_dir_all(&svc).unwrap();
    let p = Project::init(&svc, "Billing").unwrap();
    let repos = p.repos();
    assert_eq!(repos.len(), 1);
    let r = &repos[0];
    assert_eq!((r.name.as_str(), r.origin, r.state), ("mono", RepoOrigin::Containing, RepoState::Found));
    assert_eq!(r.project_prefix.as_deref(), Some("services/billing"));
    assert_eq!(r.remote.as_deref(), Some("git@example.com:acme/mono.git"));
    assert_eq!(default_repo(&repos).unwrap().name, "mono");
    assert_eq!(outline(&repos), "- mono contains the project (at services/billing) — branch main; remote git@example.com:acme/mono.git");
    assert_eq!(strip_credentials("https://bob:tok3n@github.com/acme/x.git"), "https://github.com/acme/x.git");
    assert_eq!(strip_credentials("git@github.com:acme/x.git"), "git@github.com:acme/x.git");

    // A project at a repo's root: prefix ''. Listing it (`path: .`) doesn't duplicate it.
    let solo = tmp.path().join("solo");
    fake_repo(&solo, None);
    let mut p = Project::init(&solo, "Solo").unwrap();
    assert_eq!(p.repos()[0].project_prefix.as_deref(), Some(""));
    p.manifest.repos = vec![repo_ref("self", ".", None)];
    p.save_manifest().unwrap();
    let repos = p.repos();
    assert_eq!(repos.len(), 1);
    assert_eq!((repos[0].name.as_str(), repos[0].origin), ("self", RepoOrigin::Manifest));
}

#[test]
fn names_stay_unique() {
    let (_tmp, mut p) = workspace();
    fake_repo(&p.root().join("backend/web"), None);
    p.manifest.repos = vec![repo_ref("orders", "../api", None)];
    let repos = resolve(p.root(), &p.manifest, &Overrides::new(), true);
    let names: Vec<&str> = repos.iter().map(|r| r.name.as_str()).collect();
    // The listed "orders" wins; the discovered one and the second "web" get their parent folder's name.
    assert_eq!(names, ["orders", "backend-orders", "web", "admin", "frontend-web"]);
    assert!(validate_name("api").is_ok());
    for bad in ["", "a/b", "a\\b", "x:y", ".hidden"] {
        assert!(validate_name(bad).is_err(), "{bad}");
    }
}

#[test]
fn relative_and_windows_style_paths() {
    let root = Path::new(if cfg!(windows) { r"C:\work\shop" } else { "/work/shop" });
    assert_eq!(resolve_rel(root, "../api"), resolve_rel(root, r"..\api"), "either separator");
    assert_eq!(resolve_rel(root, "./frontend//web/"), root.join("frontend").join("web"));
    let up = resolve_rel(root, "../../x");
    assert_eq!(up, root.parent().unwrap().parent().unwrap().join("x"));
    assert!(looks_absolute(r"C:\x") && looks_absolute("C:/x") && looks_absolute("/x") && looks_absolute(r"\\server\share"));
    assert!(!looks_absolute("../x") && !looks_absolute("web"));

    assert_eq!(relative_path(root, &root.join("frontend").join("web")).as_deref(), Some("frontend/web"));
    assert_eq!(relative_path(root, &root.parent().unwrap().join("api")).as_deref(), Some("../api"));
    assert_eq!(relative_path(root, root).as_deref(), Some("."));
    #[cfg(windows)]
    {
        assert_eq!(relative_path(Path::new(r"C:\work\shop"), Path::new(r"D:\src\api")), None, "another drive");
        assert_eq!(relative_path(Path::new(r"C:\Work\Shop"), Path::new(r"c:\work\API")).as_deref(), Some("../API"), "case-insensitive");
        assert_eq!(key(Path::new(r"C:\Work\")), key(Path::new("c:/work")));
    }
}

#[test]
fn repo_links_and_portable_paths() {
    let (_tmp, mut p) = workspace();
    p.manifest.repos = vec![repo_ref("api", "../api", None), repo_ref("web", "frontend/web", None)];
    p.save_manifest().unwrap();
    assert_eq!(parse_link("repo:api/openapi/spec.yaml"), Some(("api", "openapi/spec.yaml")));
    assert_eq!(parse_link("repo:api"), Some(("api", "")));
    assert_eq!(parse_link(r"repo:api\src\x.ts"), Some(("api", r"src\x.ts")));
    assert_eq!(parse_link("repo:"), None);
    assert_eq!(parse_link("http/x.req.yaml"), None);

    let api = p.root().parent().unwrap().join("api");
    fs::write(api.join("openapi.yaml"), "openapi: 3.0.0\n").unwrap();
    let resolved = resolve_link(&p, "repo:api/openapi.yaml").unwrap();
    assert!(same(&resolved, &api.join("openapi.yaml")));
    assert!(resolve_link(&p, "repo:api/../../etc/passwd").is_err(), "can't climb out of the repo");
    assert!(resolve_link(&p, "repo:nope/x").is_err());

    assert_eq!(portable_path(&p, &api.join("openapi.yaml")), "repo:api/openapi.yaml");
    // Nested listed repo: still repo-qualified (portable when a teammate relocates it).
    assert_eq!(portable_path(&p, &p.root().join("frontend/web/src/app.ts")), "repo:web/src/app.ts");
    assert_eq!(portable_path(&p, &p.root().join("specs/a.yaml")), "specs/a.yaml");
    let outside = p.root().parent().unwrap().join("loose.yaml");
    assert_eq!(portable_path(&p, &outside), outside.display().to_string());
}

#[test]
fn adding_folders_keeps_devall_yaml_portable() {
    let (tmp, mut p) = workspace();
    let api = p.root().parent().unwrap().join("api");
    let s = suggest(&p, &api);
    assert_eq!((s.name.as_str(), s.rel_path.as_deref(), s.far_away, s.is_git), ("api", Some("../api"), false, true));
    assert_eq!(s.remote.as_deref(), Some("https://example.com/acme/api.git"));
    let r = add(&mut p, &api, None, Some("Backend"), false).unwrap();
    assert_eq!((r.path.as_str(), r.group.as_deref()), ("../api", Some("Backend")));
    assert!(add(&mut p, &api, Some("api2"), None, false).is_err(), "already listed");

    // Far away: kept on this machine, devall.yaml only gets a sibling guess.
    let far = tmp.path().join("deep/er/still/tools");
    fake_repo(&far, None);
    assert!(suggest(&p, &far).far_away);
    let r = add(&mut p, &far, None, None, true).unwrap();
    assert_eq!(r.path, "../tools");
    let found = p.repo("tools").unwrap();
    assert!(found.overridden && found.is_found());

    // Discovered repos are listed with their relative path and remote.
    let added = save_discovered(&mut p, &["web".into()], &[("web".to_string(), "Frontend".to_string())].into_iter().collect()).unwrap();
    assert_eq!(added[0].path, "frontend/web");
    assert_eq!(added[0].remote.as_deref(), Some("git@example.com:acme/web.git"));
    let yaml = fs::read_to_string(p.root().join("devall.yaml")).unwrap();
    assert!(yaml.contains("repos:") && yaml.contains("path: ../api") && yaml.contains("group: Frontend"), "{yaml}");
    assert!(!yaml.contains(&tmp.path().display().to_string()), "{yaml}");
}

#[test]
fn manifest_without_repos_is_unchanged() {
    let (_tmp, p) = workspace();
    let yaml = fs::read_to_string(p.root().join("devall.yaml")).unwrap();
    assert!(!yaml.contains("repos"), "{yaml}");
    // Old files (no `repos:`) still parse.
    let m: ProjectManifest = serde_yaml_ng::from_str("id: x\nname: Old\n").unwrap();
    assert!(m.repos.is_empty());
}

#[test]
fn finds_compose_files_in_repos() {
    let (_tmp, mut p) = workspace();
    p.manifest.repos = vec![repo_ref("api", "../api", Some("Backend")), repo_ref("gone", "../gone", None)];
    p.save_manifest().unwrap();
    let api = p.root().parent().unwrap().join("api");
    fs::write(api.join("docker-compose.yml"), "services: {}\n").unwrap();
    fs::write(api.join("compose.yaml"), "services: {}\n").unwrap();
    fs::create_dir_all(api.join("deploy")).unwrap();
    fs::write(api.join("deploy/docker-compose.yaml"), "services: {}\n").unwrap();
    fs::create_dir_all(api.join("node_modules/x")).unwrap();
    fs::write(api.join("node_modules/x/compose.yml"), "services: {}\n").unwrap();
    let files = compose_files(&p.repos());
    let found: Vec<(&str, &str, &str)> = files.iter().map(|f| (f.repo.as_str(), f.rel_path.as_str(), f.project.as_str())).collect();
    assert_eq!(found, [("api", "compose.yaml", "api"), ("api", "deploy/docker-compose.yaml", "deploy")]);
}

#[test]
fn detect_lists_repos_under_a_folder() {
    let (_tmp, p) = workspace();
    let found: Vec<String> = detect(p.root()).into_iter().map(|r| r.rel_path.unwrap_or_default()).collect();
    assert_eq!(found, ["backend/orders", "frontend/admin", "frontend/web"]);
}

/// Real git: status, summary, switching branches across repos. Skipped without git.
#[test]
fn git_status_and_switch_across_repos() {
    if !git::available() {
        eprintln!("git not available; skipping");
        return;
    }
    let tmp = tempfile::tempdir().unwrap();
    let init = |dir: &Path| {
        fs::create_dir_all(dir).unwrap();
        let hooks = dir.join(".git").join("hooks").display().to_string();
        // Local hooks path: never the user's global hooks (core.hooksPath) in tests.
        for args in [&["init", "-q", "-b", "main"][..], &["config", "user.email", "t@example.com"], &["config", "user.name", "T"], &["config", "commit.gpgsign", "false"], &["config", "core.hooksPath", &hooks]] {
            git::run(dir, args).unwrap();
        }
        fs::write(dir.join("README.md"), "hi\n").unwrap();
        git::run(dir, &["add", "."]).unwrap();
        git::run(dir, &["commit", "-q", "-m", "init"]).unwrap();
    };
    let shop = tmp.path().join("shop");
    init(&shop.join("web"));
    init(&shop.join("api"));
    init(&tmp.path().join("ext"));
    let mut p = Project::init(&shop, "Shop").unwrap();
    p.manifest.repos = vec![repo_ref("web", "web", Some("Frontend")), repo_ref("api", "api", Some("Backend")), repo_ref("ext", "../ext", None)];
    p.save_manifest().unwrap();
    let repos: Vec<Repo> = p.repos().into_iter().filter(Repo::is_found).collect();
    assert_eq!(repos.len(), 3);

    fs::write(shop.join("web/new.txt"), "x").unwrap();
    let sums = git::parallel(repos.clone(), git::PARALLEL, |r| (r.name.clone(), git::summary(&r.path)));
    assert_eq!(sums[0].0, "web");
    assert_eq!((sums[0].1.branch.as_deref(), sums[0].1.changes), (Some("main"), 1));
    assert_eq!(sums[1].1.changes, 0);
    assert_eq!(sums[1].1.last_commit.as_ref().map(|c| c.subject.as_str()), Some("init"));

    // Feature branch across repos: create where missing, then switch back to an existing one.
    for r in &repos {
        assert_eq!(git::switch(&r.path, "feature/login", true).unwrap(), git::Switched::Created);
    }
    assert_eq!(git::switch(&repos[0].path, "feature/login", false).unwrap(), git::Switched::Already);
    assert_eq!(git::switch(&repos[1].path, "main", false).unwrap(), git::Switched::Local);
    assert!(git::switch(&repos[1].path, "nope", false).is_err());
    assert!(git::switch(&repos[1].path, "-bad", true).is_err());
    let b = git::branches(&repos[1].path).unwrap();
    assert_eq!(b.current.as_deref(), Some("main"));
    assert!(b.local.contains(&"feature/login".to_string()));

    // HEAD/index live in the git dir the watcher follows.
    assert!(git::git_dir(&repos[0].path).unwrap().join("HEAD").is_file());
    assert!(!git::has_scan_hook(&repos[0].path));
    git::install_scan_hook(&repos[0].path).unwrap();
    assert!(git::has_scan_hook(&repos[0].path) && !git::has_scan_hook(&repos[1].path), "hooks are per repo");
}

#[test]
fn local_only_repos_stay_out_of_devall_yaml() {
    let _iso = crate::local::isolate_for_tests();
    let (tmp, p) = workspace();
    let wt = tmp.path().join("ws").join("shop-feature");
    fake_repo(&wt, None);
    let name = add_local_repo(&p, &wt, None).unwrap();
    assert_eq!(name, "shop-feature");
    let r = p.repos().into_iter().find(|r| r.name == name).unwrap();
    assert_eq!(r.origin, RepoOrigin::Local);
    assert_eq!(r.state, RepoState::Found);
    assert!(p.repo(&name).is_some());
    assert!(!fs::read_to_string(p.root().join("devall.yaml")).unwrap().contains("shop-feature"));
    // Adding it again returns the same name; a folder that isn't a repo is refused.
    assert_eq!(add_local_repo(&p, &wt, Some("other")).unwrap(), name);
    assert!(add_local_repo(&p, tmp.path(), None).is_err());
    remove_local_repo(&p, &name).unwrap();
    assert!(p.repo(&name).is_none());
    assert!(remove_local_repo(&p, &name).is_err());
}

#[test]
fn containing_repo_without_a_manifest() {
    let dir = tempfile::tempdir().unwrap();
    fake_repo(dir.path(), None);
    let sub = dir.path().join("api");
    std::fs::create_dir_all(&sub).unwrap();
    let r = super::containing(&sub).expect("the folder is inside a repo");
    assert_eq!(r.origin, super::RepoOrigin::Containing);
    assert_eq!(r.project_prefix.as_deref(), Some("api"));
    let outside = tempfile::tempdir().unwrap();
    assert!(super::containing(outside.path()).is_none());
}
