use std::sync::{Arc, Mutex};

use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use super::*;

#[derive(Debug, Clone)]
struct Seen {
    method: String,
    path: String,
    headers: Vec<(String, String)>,
    body: Value,
}

type Handler = Arc<dyn Fn(&Seen) -> (u16, Value) + Send + Sync>;

/// A local HTTP server answering with `handler`; records every request.
async fn fake_api(handler: Handler) -> (String, Arc<Mutex<Vec<Seen>>>) {
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = l.local_addr().unwrap();
    let seen = Arc::new(Mutex::new(Vec::new()));
    let seen2 = seen.clone();
    tokio::spawn(async move {
        loop {
            let Ok((mut s, _)) = l.accept().await else { return };
            let handler = handler.clone();
            let seen = seen2.clone();
            tokio::spawn(async move {
                let mut buf = Vec::new();
                let mut tmp = [0u8; 8192];
                let header_end = loop {
                    let n = s.read(&mut tmp).await.unwrap_or(0);
                    if n == 0 {
                        return;
                    }
                    buf.extend_from_slice(&tmp[..n]);
                    if let Some(i) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
                        break i + 4;
                    }
                };
                let head = String::from_utf8_lossy(&buf[..header_end]).to_string();
                let mut lines = head.lines();
                let mut first = lines.next().unwrap_or("").split_whitespace();
                let method = first.next().unwrap_or("").to_string();
                let path = first.next().unwrap_or("/").to_string();
                let headers: Vec<(String, String)> = lines.filter_map(|l| l.split_once(':')).map(|(k, v)| (k.trim().to_ascii_lowercase(), v.trim().to_string())).collect();
                let len: usize = headers.iter().find(|(k, _)| k == "content-length").and_then(|(_, v)| v.parse().ok()).unwrap_or(0);
                while buf.len() < header_end + len {
                    let n = s.read(&mut tmp).await.unwrap_or(0);
                    if n == 0 {
                        break;
                    }
                    buf.extend_from_slice(&tmp[..n]);
                }
                let body = serde_json::from_slice(&buf[header_end..]).unwrap_or(Value::Null);
                let req = Seen { method, path, headers, body };
                let (status, reply) = handler(&req);
                seen.lock().unwrap().push(req);
                let text = reply.to_string();
                let head = format!("HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", text.len());
                let _ = s.write_all(head.as_bytes()).await;
                let _ = s.write_all(text.as_bytes()).await;
                let _ = s.shutdown().await;
            });
        }
    });
    (format!("http://{addr}"), seen)
}

fn header<'a>(s: &'a Seen, name: &str) -> Option<&'a str> {
    s.headers.iter().find(|(k, _)| k == name).map(|(_, v)| v.as_str())
}

const TOKEN: &str = "ghp_SuperSecretToken1234567890";

fn req() -> NewRequest {
    NewRequest { title: "Add login".into(), body: "## Summary\nlogin".into(), base: "main".into(), head: "feature/login".into(), draft: true, reviewers: vec!["octocat".into(), "acme/core".into()], labels: vec!["auth".into(), " ".into()] }
}

#[test]
fn detects_hosts_from_remote_urls() {
    let gh = detect("git@github.com:acme/web.git", &[]).unwrap();
    assert_eq!((gh.kind, gh.host.as_str(), gh.path.as_str()), (ForgeKind::Github, "github.com", "acme/web"));
    assert_eq!(gh.api_url, "https://api.github.com");
    assert_eq!(gh.web_url, "https://github.com/acme/web");
    let creds = detect("https://bob:s3cret@github.com/acme/web.git", &[]).unwrap();
    assert!(!format!("{creds:?}").contains("s3cret"));
    let ghe = detect("https://github.acme.dev/team/svc", &[]).unwrap();
    assert_eq!(ghe.api_url, "https://github.acme.dev/api/v3");
    let gl = detect("ssh://git@gitlab.com:2222/group/sub/proj.git", &[]).unwrap();
    assert_eq!((gl.kind, gl.host.as_str(), gl.path.as_str()), (ForgeKind::Gitlab, "gitlab.com", "group/sub/proj"));
    assert_eq!(gl.api_url, "https://gitlab.com/api/v4");
    let self_hosted = detect("https://code.acme.dev:8443/a/b.git", &[]);
    assert!(self_hosted.is_none(), "unknown host until connected");
    let acc = [Account { host: "code.acme.dev:8443".into(), kind: ForgeKind::Gitlab, user: None, api_url: None }];
    let sh = detect("https://code.acme.dev:8443/a/b.git", &acc).unwrap();
    assert_eq!(sh.kind, ForgeKind::Gitlab);
    assert_eq!(sh.api_url, "https://code.acme.dev:8443/api/v4");
    assert_eq!(sh.web_url, "https://code.acme.dev:8443/a/b");
    assert!(detect("C:/repos/local", &[]).is_none());
    assert!(detect("/srv/git/x.git", &[]).is_none());
}

#[test]
fn hosts_tokens_and_help() {
    assert_eq!(normalize_host("https://GitLab.Acme.dev/").unwrap(), "gitlab.acme.dev");
    assert_eq!(normalize_host("git@github.com").unwrap(), "github.com");
    assert!(normalize_host("  ").is_err());
    assert!(token_help(ForgeKind::Github, "github.com").url.contains("scopes=repo"));
    assert!(token_help(ForgeKind::Gitlab, "gitlab.com").scopes.contains("api"));
    let store = devall_core::MemorySecretStore::default();
    assert_eq!(get_token(&store, "github.com"), None);
    set_token(&store, "github.com", &format!(" {TOKEN} ")).unwrap();
    assert_eq!(get_token(&store, "github.com").as_deref(), Some(TOKEN));
    delete_token(&store, "github.com").unwrap();
    assert_eq!(get_token(&store, "github.com"), None);
    assert_eq!(clean(&format!("bad {TOKEN}!"), TOKEN), "bad ••••••!");
    let tmp = tempfile::tempdir().unwrap();
    let p = accounts_path(tmp.path());
    write_accounts(&p, &[Account { host: "github.com".into(), kind: ForgeKind::Github, user: Some("me".into()), api_url: None }]).unwrap();
    let text = std::fs::read_to_string(&p).unwrap();
    assert!(!text.contains("ghp_"));
    assert_eq!(read_accounts(&p)[0].user.as_deref(), Some("me"));
}

#[test]
fn finds_templates() {
    let tmp = tempfile::tempdir().unwrap();
    let r = tmp.path();
    assert!(templates(r, ForgeKind::Github).is_empty());
    std::fs::create_dir_all(r.join(".github/PULL_REQUEST_TEMPLATE")).unwrap();
    std::fs::write(r.join(".github/pull_request_template.md"), "## What\n").unwrap();
    std::fs::write(r.join(".github/PULL_REQUEST_TEMPLATE/bug.md"), "## Bug\n").unwrap();
    let t = templates(r, ForgeKind::Github);
    assert_eq!(t.iter().map(|x| x.path.as_str()).collect::<Vec<_>>(), [".github/pull_request_template.md", ".github/PULL_REQUEST_TEMPLATE/bug.md"]);
    std::fs::create_dir_all(r.join(".gitlab/merge_request_templates")).unwrap();
    std::fs::write(r.join(".gitlab/merge_request_templates/Feature.md"), "feature").unwrap();
    std::fs::write(r.join(".gitlab/merge_request_templates/Default.md"), "default").unwrap();
    let t = templates(r, ForgeKind::Gitlab);
    assert_eq!(t.iter().map(|x| x.name.as_str()).collect::<Vec<_>>(), ["Default", "Feature"]);
}

#[test]
fn cli_arguments_and_output() {
    let gh = detect("git@github.com:acme/web.git", &[]).unwrap();
    let a = cli::args(&gh, &req());
    assert_eq!(&a[..4], ["pr", "create", "--repo", "github.com/acme/web"]);
    assert!(a.windows(2).any(|w| w == ["--body-file", "-"]));
    assert!(a.contains(&"--draft".to_string()));
    assert!(a.windows(2).any(|w| w == ["--reviewer", "octocat"]));
    assert_eq!(a.iter().filter(|x| *x == "--label").count(), 1, "blank labels dropped");
    let gl = detect("git@gitlab.com:group/proj.git", &[]).unwrap();
    let a = cli::args(&gl, &req());
    assert!(a.windows(2).any(|w| w == ["--repo", "https://gitlab.com/group/proj"]));
    assert!(a.windows(2).any(|w| w == ["--reviewer", "octocat,acme/core"]));
    assert!(a.contains(&"--yes".to_string()));
    assert_eq!(cli::parse_output("Creating pull request\nhttps://github.com/acme/web/pull/42\n"), Some((42, "https://github.com/acme/web/pull/42".into())));
    assert_eq!(cli::parse_output("!5 Add login (feature)\n https://gitlab.com/g/p/-/merge_requests/5\n"), Some((5, "https://gitlab.com/g/p/-/merge_requests/5".into())));
    assert_eq!(cli::parse_output("nothing"), None);
    assert_eq!(gitlab::draft_title("Add x", true), "Draft: Add x");
    assert_eq!(gitlab::draft_title("Draft: Add x", true), "Draft: Add x");
    assert_eq!(gitlab::draft_title("Add x", false), "Add x");
}

#[tokio::test]
async fn github_request_bodies_and_headers() {
    let (url, seen) = fake_api(Arc::new(|s: &Seen| match (s.method.as_str(), s.path.as_str()) {
        ("POST", "/repos/acme/web/pulls") => (201, json!({ "number": 7, "html_url": "https://github.com/acme/web/pull/7" })),
        ("POST", "/repos/acme/web/pulls/7/requested_reviewers") => (201, json!({})),
        ("POST", "/repos/acme/web/issues/7/labels") => (422, json!({ "message": "Validation Failed", "errors": [{ "message": "label auth not found" }] })),
        ("GET", "/user") => (200, json!({ "login": "octocat" })),
        _ => (404, json!({ "message": "Not Found" })),
    }))
    .await;
    let remote = Remote { kind: ForgeKind::Github, host: "github.com".into(), path: "acme/web".into(), web_url: "https://github.com/acme/web".into(), api_url: url.clone() };
    let c = create(&remote, TOKEN, &req()).await.unwrap();
    assert_eq!((c.number, c.url.as_str()), (7, "https://github.com/acme/web/pull/7"));
    assert_eq!(c.warnings.len(), 1, "{:?}", c.warnings);
    assert!(c.warnings[0].contains("label auth not found"));
    let s = seen.lock().unwrap().clone();
    assert_eq!(s.len(), 3);
    assert_eq!(header(&s[0], "authorization"), Some(format!("Bearer {TOKEN}").as_str()));
    assert_eq!(header(&s[0], "accept"), Some("application/vnd.github+json"));
    assert_eq!(header(&s[0], "x-github-api-version"), Some("2022-11-28"));
    assert_eq!(s[0].body, json!({ "title": "Add login", "head": "feature/login", "base": "main", "body": "## Summary\nlogin", "draft": true }));
    assert_eq!(s[1].body, json!({ "reviewers": ["octocat"], "team_reviewers": ["core"] }));
    assert_eq!(s[2].body, json!({ "labels": ["auth"] }));
    assert_eq!(whoami(ForgeKind::Github, &url, TOKEN).await.unwrap(), "octocat");
}

#[tokio::test]
async fn gitlab_request_bodies_and_headers() {
    let (url, seen) = fake_api(Arc::new(|s: &Seen| {
        if s.method == "GET" && s.path.starts_with("/users?username=octocat") {
            (200, json!([{ "id": 99, "username": "octocat" }]))
        } else if s.method == "GET" && s.path.starts_with("/users?") {
            (200, json!([]))
        } else if s.method == "POST" && s.path == "/projects/group%2Fsub%2Fproj/merge_requests" {
            (201, json!({ "iid": 12, "web_url": "https://gitlab.com/group/sub/proj/-/merge_requests/12" }))
        } else {
            (404, json!({ "message": "404 Project Not Found" }))
        }
    }))
    .await;
    let remote = Remote { kind: ForgeKind::Gitlab, host: "gitlab.com".into(), path: "group/sub/proj".into(), web_url: "https://gitlab.com/group/sub/proj".into(), api_url: url.clone() };
    let c = create(&remote, TOKEN, &req()).await.unwrap();
    assert_eq!(c.number, 12);
    assert_eq!(c.warnings.len(), 1, "acme/core isn't a user: {:?}", c.warnings);
    let s = seen.lock().unwrap().clone();
    let post = s.iter().find(|x| x.method == "POST").unwrap();
    assert_eq!(header(post, "private-token"), Some(TOKEN));
    assert!(header(post, "authorization").is_none());
    assert_eq!(
        post.body,
        json!({ "source_branch": "feature/login", "target_branch": "main", "title": "Draft: Add login", "description": "## Summary\nlogin", "labels": "auth", "reviewer_ids": [99] })
    );
}

#[tokio::test]
async fn errors_never_contain_the_token() {
    // A server that echoes the token back in its error.
    let (url, _seen) = fake_api(Arc::new(|s: &Seen| {
        let auth = header(s, "authorization").or(header(s, "private-token")).unwrap_or_default().to_string();
        (401, json!({ "message": format!("Bad credentials: {auth}") }))
    }))
    .await;
    for kind in [ForgeKind::Github, ForgeKind::Gitlab] {
        let remote = Remote { kind, host: "h".into(), path: "a/b".into(), web_url: "https://h/a/b".into(), api_url: url.clone() };
        let err = create(&remote, TOKEN, &req()).await.unwrap_err().to_string();
        assert!(!err.contains(TOKEN), "{err}");
        assert!(err.contains("401") && err.contains("connect again"), "{err}");
        let err = whoami(kind, &url, TOKEN).await.unwrap_err().to_string();
        assert!(!err.contains(TOKEN), "{err}");
    }
    // Unreachable server: a network error, no token either.
    let remote = Remote { kind: ForgeKind::Github, host: "h".into(), path: "a/b".into(), web_url: String::new(), api_url: "http://127.0.0.1:9".into() };
    let err = create(&remote, TOKEN, &req()).await.unwrap_err().to_string();
    assert!(!err.contains(TOKEN));
    // Validation happens before any request.
    let mut bad = req();
    bad.title = " ".into();
    assert!(matches!(create(&remote, TOKEN, &bad).await, Err(ForgeError::Invalid(_))));
    bad = req();
    bad.base = bad.head.clone();
    assert!(create(&remote, TOKEN, &bad).await.is_err());
}
