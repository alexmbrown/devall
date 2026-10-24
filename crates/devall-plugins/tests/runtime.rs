//! Runtime and manager tests with hand-written WAT plugins (compiled with the `wat` crate, so no
//! wasm32 target is needed; see `common/mod.rs`).

mod common;

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use devall_plugins::contract::{Header, PluginRequest, PluginResponse};
use common::{Act, Wat, module};
use devall_plugins::{HostServices, Limits, PluginError, PluginManager, ProjectRef, Scope, Status};

#[derive(Default)]
struct Fake {
    secrets: HashMap<String, String>,
}

impl HostServices for Fake {
    fn http(&self, req: &PluginRequest, _timeout: Duration) -> Result<PluginResponse, String> {
        Ok(PluginResponse { status: 200, headers: vec![Header::new("x-url", req.url.clone())], body: Some("pong".into()), body_base64: false, time_ms: 1 })
    }
    fn secret(&self, plugin: &str, name: &str) -> Option<String> {
        self.secrets.get(&format!("{plugin}/{name}")).cloned()
    }
}

struct Env {
    _tmp: tempfile::TempDir,
    root: PathBuf,
    mgr: PluginManager,
}

fn env() -> Env {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().to_path_buf();
    let mut fake = Fake::default();
    fake.secrets.insert("caps/API_KEY".into(), "s3cr3t".into());
    fake.secrets.insert("caps/OTHER".into(), "nope".into());
    let mgr = PluginManager::new(root.join("app"), Arc::new(fake));
    Env { _tmp: tmp, root, mgr }
}

fn write_plugin(dir: &Path, manifest: &str, wasm: &[u8]) {
    std::fs::create_dir_all(dir).unwrap();
    std::fs::write(dir.join("plugin.yaml"), manifest).unwrap();
    std::fs::write(dir.join("plugin.wasm"), wasm).unwrap();
}

fn manifest(id: &str, contributes: &str, caps: &str) -> String {
    format!("id: {id}\nname: {id} plugin\nversion: 1.0.0\napiVersion: 1\ncapabilities: {caps}\ncontributes:\n{contributes}\n")
}

const UTIL: &str = "  utilities:\n    - id: echo\n      name: Echo\n      args: [{ name: mode, default: loud }]\n";

impl Env {
    fn install(&self, id: &str, contributes: &str, caps: &str, wasm: &[u8], grants: &[&str]) {
        let src = self.root.join("src").join(id);
        write_plugin(&src, &manifest(id, contributes, caps), wasm);
        let grants: Vec<String> = grants.iter().map(|s| s.to_string()).collect();
        self.mgr.install(&src, None, &grants).unwrap();
    }

    fn util(&self, id: &str, input: &str) -> Result<String, PluginError> {
        self.mgr.run_utility(None, id, input, &BTreeMap::new()).map(|(t, _)| t)
    }
}

#[test]
fn utility_contract_round_trip_and_logs() {
    let e = env();
    e.install("echo", UTIL, "{}", &module(&[("utility", Act::Echo)]), &[]);
    let out = e.util("echo:echo", "hello").unwrap();
    let v: serde_json::Value = serde_json::from_str(&out).unwrap();
    assert_eq!(v["apiVersion"], 1);
    assert_eq!(v["kind"], "utility");
    assert_eq!(v["id"], "echo");
    assert_eq!(v["input"]["input"], "hello");
    assert_eq!(v["input"]["args"]["mode"], "loud", "manifest defaults fill missing args");
    let cat = e.mgr.catalog(None);
    assert_eq!(cat.utilities.len(), 1);
    assert_eq!(cat.utilities[0].id, "echo:echo");
    assert_eq!(cat.utilities[0].category, "Plugins");

    // A plain string output is returned as-is; logs come back too.
    e.install("say", UTIL, "{}", &module(&[("utility", Act::LogThen("hi from wasm".into(), r#"{"ok":{"output":"done"}}"#.into()))]), &[]);
    let (text, logs) = e.mgr.run_utility(None, "say:echo", "", &BTreeMap::new()).unwrap();
    assert_eq!(text, "done");
    assert_eq!(logs[0].message, "hi from wasm");
    assert_eq!(logs[0].level, "info");

    // Errors reported by the plugin.
    e.install("bad", UTIL, "{}", &module(&[("utility", Act::Const(r#"{"error":"input must not be empty"}"#.into()))]), &[]);
    assert_eq!(e.util("bad:echo", "").unwrap_err().to_string(), "plugin bad: input must not be empty");
    assert!(matches!(e.util("bad:nope", ""), Err(PluginError::NoContribution { .. })));
    assert!(matches!(e.util("ghost:echo", ""), Err(PluginError::NotFound(_))));
}

#[test]
fn hooks_importers_codegen_and_auth() {
    let e = env();
    let contributes = "  hooks:\n    - { id: sign, name: Sign, events: [preRequest] }\n    - { id: check, name: Check, events: [postResponse], hosts: [api.example.com] }\n  importers:\n    - { id: fmt, name: My format, extensions: [.myfmt] }\n  codegen:\n    - { id: ktor, name: Kotlin Ktor, language: kotlin }\n  auth:\n    - { id: hmac, name: HMAC, fields: [{ name: keyId }] }\n";
    let wasm = module(&[
        ("hook.preRequest", Act::Const(r#"{"ok":{"request":{"method":"POST","url":"https://api.example.com/x","headers":[{"name":"X-Sig","value":"abc"}]},"logs":["signed"]}}"#.into())),
        ("hook.postResponse", Act::Const(r#"{"ok":{"tests":[{"name":"status ok","passed":true}],"variables":{"token":"t1"}}}"#.into())),
        ("import.detect", Act::Const(r#"{"ok":{"confidence":0.9}}"#.into())),
        ("import.parse", Act::Const(r#"{"ok":{"name":"Imported","requests":[{"name":"List","method":"GET","url":"https://api.example.com/items","folder":"items","body":{"type":"json","content":"{}"}}]}}"#.into())),
        ("codegen", Act::Const(r#"{"ok":{"code":"client.get(\"/x\")"}}"#.into())),
        ("auth", Act::Const(r#"{"ok":{"headers":[{"name":"Authorization","value":"HMAC k:sig"}],"query":[{"name":"ts","value":"1"}]}}"#.into())),
    ]);
    e.install("multi", contributes, "{}", &wasm, &[]);
    let req = PluginRequest { method: "GET".into(), url: "https://api.example.com/x".into(), ..Default::default() };

    let pre = e.mgr.run_pre_request(None, req.clone(), Some("List"), None);
    assert!(pre.errors.is_empty(), "{:?}", pre.errors);
    assert!(pre.changed);
    assert_eq!(pre.request.method, "POST");
    assert_eq!(pre.request.header("x-sig"), Some("abc"));
    assert_eq!(pre.logs, vec!["[multi/sign] signed"]);

    let resp = PluginResponse { status: 200, ..Default::default() };
    let post = e.mgr.run_post_response(None, &req, &resp, None);
    assert_eq!(post.tests[0].name, "Check: status ok");
    assert_eq!(post.variables.get("token").map(String::as_str), Some("t1"));
    // The postResponse hook is limited to api.example.com.
    let other = PluginRequest { url: "https://elsewhere.test/".into(), ..req.clone() };
    assert!(e.mgr.run_post_response(None, &other, &resp, None).tests.is_empty());

    // Disabling a hook skips it.
    e.mgr.set_hook_enabled(None, Scope::User, "multi", "sign", false).unwrap();
    assert!(!e.mgr.run_pre_request(None, req.clone(), None, None).changed);
    assert!(e.mgr.catalog(None).hooks.iter().any(|h| h.id == "sign" && !h.enabled));

    let m = e.mgr.detect_import(None, "anything", None).unwrap();
    assert_eq!((m.id.as_str(), m.confidence), ("multi:fmt", 0.9));
    let imported = e.mgr.import(None, "multi:fmt", "anything", Some("x.myfmt")).unwrap();
    assert_eq!(imported.requests[0].folder.as_deref(), Some("items"));
    assert!(matches!(imported.requests[0].body, Some(devall_plugins::ImportedBody::Json { .. })));

    assert_eq!(e.mgr.codegen(None, "multi:ktor", &req).unwrap(), "client.get(\"/x\")");

    let auth = e.mgr.auth(None, "multi", "hmac", &BTreeMap::new(), &req).unwrap();
    assert_eq!(auth.headers[0].value, "HMAC k:sig");
    assert_eq!(auth.query[0].name, "ts");
    assert!(matches!(e.mgr.auth(None, "multi", "nope", &BTreeMap::new(), &req), Err(PluginError::NoContribution { .. })));

    let cat = e.mgr.catalog(None);
    assert_eq!((cat.importers.len(), cat.codegen.len(), cat.auth.len(), cat.hooks.len()), (1, 1, 1, 2));
    assert_eq!(cat.codegen[0].language, "kotlin");

    // Output that doesn't match a contract is an ABI error, not a panic.
    e.install("wrong", "  codegen:\n    - { id: x, name: X }\n", "{}", &module(&[("codegen", Act::Const(r#"{"ok":{"nope":1}}"#.into()))]), &[]);
    let err = e.mgr.codegen(None, "wrong:x", &req).unwrap_err().to_string();
    assert!(err.contains("codegen output does not match the contract"), "{err}");
}

#[test]
fn fuel_exhaustion_and_timeout() {
    let e = env();
    e.install("spin", UTIL, "{}", &module(&[("utility", Act::Loop)]), &[]);
    let sha = e.mgr.list(None)[0].wasm_sha256.clone().unwrap();
    let limited = |limits: Limits| {
        let mut mgr = PluginManager::ephemeral(e.root.join("app"), Arc::new(Fake::default()));
        mgr.limits = limits;
        mgr.trust(None, Scope::User, "spin", &sha, &[]).unwrap();
        mgr
    };

    let mgr = limited(Limits { fuel: 50_000_000, timeout: Duration::from_secs(60), ..Limits::default() });
    let t0 = std::time::Instant::now();
    let err = mgr.run_utility(None, "spin:echo", "", &BTreeMap::new()).unwrap_err();
    assert_eq!(err.to_string(), "plugin spin: exceeded its instruction budget (50000000 fuel units)");
    assert!(t0.elapsed() < Duration::from_secs(30));

    // A huge budget hits the wall-clock limit instead.
    let mgr = limited(Limits { fuel: u64::MAX / 2, timeout: Duration::from_millis(150), ..Limits::default() });
    let t0 = std::time::Instant::now();
    let err = mgr.run_utility(None, "spin:echo", "", &BTreeMap::new()).unwrap_err();
    assert_eq!(err.to_string(), "plugin spin: timed out after 150 ms");
    assert!(t0.elapsed() < Duration::from_secs(5));
}

#[test]
fn memory_cap() {
    let e = env();
    e.install("hog", UTIL, "{}", &module(&[("utility", Act::Grow)]), &[]);
    let err = e.util("hog:echo", "").unwrap_err();
    assert_eq!(err.to_string(), "plugin hog: exceeded its memory limit (64 MiB)");

    // A module whose initial memory is already above the cap fails the same way.
    let mut w = Wat::new();
    w.memory_pages = 2000;
    e.install("big", UTIL, "{}", &w.module(&[("utility", Act::Echo)]), &[]);
    assert_eq!(e.util("big:echo", "").unwrap_err().to_string(), "plugin big: exceeded its memory limit (64 MiB)");
}

#[test]
fn capabilities_are_enforced() {
    let e = env();
    let caps = "{ http: { hosts: [api.example.com] }, secrets.read: [API_KEY, OTHER], fs.project.read: true }";
    let wasm = |call: &str| module(&[("utility", Act::Host { call: call.into(), prefix: r#"{"ok":{"output":"#.into(), suffix: "}}".into() })]);

    // Allowed host: the fake HTTP service answers.
    e.install("caps", UTIL, caps, &wasm(r#"{"op":"http.request","request":{"method":"GET","url":"https://api.example.com/ping"}}"#), &["API_KEY"]);
    let out: serde_json::Value = serde_json::from_str(&e.util("caps:echo", "").unwrap()).unwrap();
    assert_eq!(out["ok"]["body"], "pong");
    assert_eq!(out["ok"]["status"], 200);

    // Host not in the allow-list: the plugin gets an error reply…
    let evil = r#"{"op":"http.request","request":{"method":"GET","url":"https://evil.example/steal"}}"#;
    e.install("caps", UTIL, caps, &wasm(evil), &["API_KEY"]);
    assert!(e.util("caps:echo", "").unwrap().contains("capability denied: http request to evil.example is not allowed"));
    // …and when it then fails, the failure is the denial, deterministically.
    e.install("caps", UTIL, caps, &module(&[("utility", Act::HostThenFail(evil.into()))]), &["API_KEY"]);
    let err = e.util("caps:echo", "").unwrap_err();
    assert_eq!(err.to_string(), "plugin caps: capability denied: http request to evil.example is not allowed (allowed hosts: api.example.com)");

    // Granted secret is readable; a declared but not granted one is denied.
    e.install("caps", UTIL, caps, &wasm(r#"{"op":"secrets.get","name":"API_KEY"}"#), &["API_KEY"]);
    let out: serde_json::Value = serde_json::from_str(&e.util("caps:echo", "").unwrap()).unwrap();
    assert_eq!(out["ok"]["value"], "s3cr3t");
    e.install("caps", UTIL, caps, &module(&[("utility", Act::HostThenFail(r#"{"op":"secrets.get","name":"OTHER"}"#.into()))]), &["API_KEY"]);
    assert!(matches!(e.util("caps:echo", ""), Err(PluginError::CapabilityDenied { .. })));

    // No capabilities at all: everything but time/random is denied.
    e.install("plain", UTIL, "{}", &module(&[("utility", Act::HostThenFail(r#"{"op":"fs.read","path":"devall.yaml"}"#.into()))]), &[]);
    assert_eq!(e.util("plain:echo", "").unwrap_err().to_string(), "plugin plain: capability denied: fs.project.read was not requested or not granted");
    e.install("plain", UTIL, "{}", &wasm(r#"{"op":"time.now"}"#), &[]);
    let out: serde_json::Value = serde_json::from_str(&e.util("plain:echo", "").unwrap()).unwrap();
    assert!(out["ok"]["unixMs"].as_i64().unwrap() > 1_700_000_000_000);
    e.install("plain", UTIL, "{}", &wasm(r#"{"op":"random.bytes","len":16}"#), &[]);
    let out: serde_json::Value = serde_json::from_str(&e.util("plain:echo", "").unwrap()).unwrap();
    assert_eq!(out["ok"]["hex"].as_str().unwrap().len(), 32);

    // fs.project.read: inside the project only, never .git/.devall.
    let proj_root = e.root.join("proj");
    std::fs::create_dir_all(proj_root.join(".devall")).unwrap();
    std::fs::write(proj_root.join("notes.txt"), "project text").unwrap();
    std::fs::write(proj_root.join(".devall").join("x"), "local").unwrap();
    let proj = ProjectRef::new(&proj_root, "p1");
    e.install("caps", UTIL, caps, &wasm(r#"{"op":"fs.read","path":"notes.txt"}"#), &[]);
    let (text, _) = e.mgr.run_utility(Some(&proj), "caps:echo", "", &BTreeMap::new()).unwrap();
    assert!(text.contains("project text"));
    e.install("caps", UTIL, caps, &module(&[("utility", Act::HostThenFail(r#"{"op":"fs.read","path":"../outside.txt"}"#.into()))]), &[]);
    assert!(matches!(e.mgr.run_utility(Some(&proj), "caps:echo", "", &BTreeMap::new()), Err(PluginError::CapabilityDenied { .. })));
    e.install("caps", UTIL, caps, &module(&[("utility", Act::HostThenFail(r#"{"op":"fs.read","path":".devall/x"}"#.into()))]), &[]);
    assert!(matches!(e.mgr.run_utility(Some(&proj), "caps:echo", "", &BTreeMap::new()), Err(PluginError::CapabilityDenied { .. })));
}

#[test]
fn project_plugins_need_trust_pinned_to_the_hash() {
    let e = env();
    let proj_root = e.root.join("proj");
    let proj = ProjectRef::new(&proj_root, "project-1");
    let dir = proj_root.join("plugins").join("team");
    write_plugin(&dir, &manifest("team", UTIL, "{}"), &module(&[("utility", Act::Const(r#"{"ok":{"output":"v1"}}"#.into()))]));

    let info = e.mgr.list(Some(&proj)).into_iter().find(|p| p.id == "team").unwrap();
    assert_eq!((info.scope, info.status), (Scope::Project, Status::Untrusted));
    assert!(e.mgr.catalog(Some(&proj)).utilities.is_empty(), "untrusted plugins contribute nothing");
    assert!(matches!(e.mgr.run_utility(Some(&proj), "team:echo", "", &BTreeMap::new()), Err(PluginError::NotTrusted(_))));

    let sha = info.wasm_sha256.unwrap();
    assert!(e.mgr.trust(Some(&proj), Scope::Project, "team", "0000", &[]).is_err(), "must trust the reviewed hash");
    e.mgr.trust(Some(&proj), Scope::Project, "team", &sha, &[]).unwrap();
    assert_eq!(e.mgr.run_utility(Some(&proj), "team:echo", "", &BTreeMap::new()).unwrap().0, "v1");
    // Trust is per project id: the same folder opened as another project is untrusted.
    let other = ProjectRef::new(&proj_root, "project-2");
    assert!(matches!(e.mgr.run_utility(Some(&other), "team:echo", "", &BTreeMap::new()), Err(PluginError::NotTrusted(_))));

    // A `git pull` changes the module: re-prompt.
    std::thread::sleep(Duration::from_millis(20));
    std::fs::write(dir.join("plugin.wasm"), module(&[("utility", Act::Const(r#"{"ok":{"output":"v2"}}"#.into()))])).unwrap();
    assert!(matches!(e.mgr.run_utility(Some(&proj), "team:echo", "", &BTreeMap::new()), Err(PluginError::Changed(_))));
    let info = e.mgr.list(Some(&proj)).into_iter().find(|p| p.id == "team").unwrap();
    assert_eq!(info.status, Status::Changed);
    e.mgr.trust(Some(&proj), Scope::Project, "team", &info.wasm_sha256.unwrap(), &[]).unwrap();
    assert_eq!(e.mgr.run_utility(Some(&proj), "team:echo", "", &BTreeMap::new()).unwrap().0, "v2");

    // So does a manifest that asks for more capabilities.
    std::fs::write(dir.join("plugin.yaml"), manifest("team", UTIL, "{ fs.project.read: true }")).unwrap();
    assert!(matches!(e.mgr.run_utility(Some(&proj), "team:echo", "", &BTreeMap::new()), Err(PluginError::Changed(_))));

    // Disable / revoke.
    let sha = e.mgr.list(Some(&proj)).into_iter().find(|p| p.id == "team").unwrap().wasm_sha256.unwrap();
    e.mgr.trust(Some(&proj), Scope::Project, "team", &sha, &[]).unwrap();
    e.mgr.set_enabled(Some(&proj), Scope::Project, "team", false).unwrap();
    assert!(matches!(e.mgr.run_utility(Some(&proj), "team:echo", "", &BTreeMap::new()), Err(PluginError::Disabled(_))));
    e.mgr.revoke(Some(&proj), Scope::Project, "team").unwrap();
    assert!(matches!(e.mgr.run_utility(Some(&proj), "team:echo", "", &BTreeMap::new()), Err(PluginError::NotTrusted(_))));

    // Approvals persist in plugins.json (in the app data dir, never the project).
    let state = std::fs::read_to_string(e.root.join("app").join("plugins.json")).unwrap();
    assert!(!state.contains("project-1:team"), "revoked");
    assert!(!proj_root.join("plugins.json").exists());

    // An installed plugin with the same id shadows the project copy.
    e.install("team", UTIL, "{}", &module(&[("utility", Act::Const(r#"{"ok":{"output":"installed"}}"#.into()))]), &[]);
    assert_eq!(e.mgr.run_utility(Some(&proj), "team:echo", "", &BTreeMap::new()).unwrap().0, "installed");
    let list = e.mgr.list(Some(&proj));
    assert_eq!(list.iter().find(|p| p.scope == Scope::Project).unwrap().status, Status::Shadowed);
}

#[test]
fn install_from_zip_preview_and_uninstall() {
    let e = env();
    let wasm = module(&[("utility", Act::Const(r#"{"ok":{"output":"zipped"}}"#.into()))]);
    let zip_path = e.root.join("z.zip");
    {
        use std::io::Write;
        let f = std::fs::File::create(&zip_path).unwrap();
        let mut z = zip::ZipWriter::new(f);
        let o = zip::write::SimpleFileOptions::default();
        z.start_file("zipped-1.0/plugin.yaml", o).unwrap();
        z.write_all(manifest("zipped", UTIL, "{ secrets.read: [TOKEN] }").as_bytes()).unwrap();
        z.start_file("zipped-1.0/plugin.wasm", o).unwrap();
        z.write_all(&wasm).unwrap();
        z.finish().unwrap();
    }
    let preview = e.mgr.preview(&zip_path).unwrap();
    assert_eq!(preview.manifest.id, "zipped");
    assert_eq!(preview.capabilities, vec!["Read its secrets TOKEN"]);
    assert!(preview.replaces.is_none());
    assert!(e.mgr.install(&zip_path, Some("deadbeef"), &[]).is_err(), "hash mismatch");
    let info = e.mgr.install(&zip_path, Some(&preview.wasm_sha256), &["TOKEN".into(), "NOT_DECLARED".into()]).unwrap();
    assert_eq!(info.status, Status::Ready);
    assert_eq!(info.secrets.len(), 1);
    assert!(info.secrets[0].granted && !info.secrets[0].set);
    assert_eq!(e.util("zipped:echo", "").unwrap(), "zipped");
    assert!(e.root.join("app/plugins/zipped/plugin.wasm").is_file());

    // A WASI build is refused with a hint.
    let mut w = Wat::new();
    w.extra_imports = r#"(import "wasi_snapshot_preview1" "fd_write" (func (param i32 i32 i32 i32) (result i32)))"#.into();
    let src = e.root.join("src/wasi");
    write_plugin(&src, &manifest("wasi", UTIL, "{}"), &w.module(&[("", Act::Echo)]));
    let err = e.mgr.install(&src, None, &[]).unwrap_err().to_string();
    assert!(err.contains("wasi_snapshot_preview1.fd_write") && err.contains("wasm32-unknown-unknown"), "{err}");

    // Missing ABI exports.
    let src = e.root.join("src/noabi");
    write_plugin(&src, &manifest("noabi", UTIL, "{}"), &wat::parse_str(r#"(module (memory (export "memory") 1))"#).unwrap());
    assert!(e.mgr.install(&src, None, &[]).unwrap_err().to_string().contains("does not export `alloc`"));

    // Uninstall (to the OS trash) forgets the approval.
    e.mgr.uninstall("zipped").unwrap();
    assert!(matches!(e.util("zipped:echo", ""), Err(PluginError::NotFound(_))));
}

#[test]
fn hand_copied_plugins_are_untrusted() {
    let e = env();
    let dir = e.root.join("app/plugins/manual");
    write_plugin(&dir, &manifest("manual", UTIL, "{}"), &module(&[("utility", Act::Echo)]));
    let info = e.mgr.list(None).into_iter().find(|p| p.id == "manual").unwrap();
    assert_eq!((info.scope, info.status), (Scope::User, Status::Untrusted));
    assert!(matches!(e.util("manual:echo", ""), Err(PluginError::NotTrusted(_))));
    // A broken manifest is listed with its error.
    std::fs::create_dir_all(e.root.join("app/plugins/broken")).unwrap();
    std::fs::write(e.root.join("app/plugins/broken/plugin.yaml"), "id: Broken!").unwrap();
    let b = e.mgr.list(None).into_iter().find(|p| p.id == "broken").unwrap();
    assert_eq!(b.status, Status::Broken);
    assert!(b.error.unwrap().contains("invalid plugin manifest"));
}

#[test]
fn approvals_are_shared_through_plugins_json() {
    let e = env();
    // Another process (the CLI next to the app) has its own manager over the same folder.
    let other = PluginManager::new(e.root.join("app"), Arc::new(Fake::default()));
    e.install("shared", UTIL, "{}", &module(&[("utility", Act::Const(r#"{"ok":{"output":"hi"}}"#.into()))]), &[]);
    assert_eq!(other.run_utility(None, "shared:echo", "", &BTreeMap::new()).unwrap().0, "hi");
    std::thread::sleep(Duration::from_millis(20));
    e.mgr.set_enabled(None, Scope::User, "shared", false).unwrap();
    assert!(matches!(other.run_utility(None, "shared:echo", "", &BTreeMap::new()), Err(PluginError::Disabled(_))));
}
