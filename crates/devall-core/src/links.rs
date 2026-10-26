//! Links between /dev/all items: `devall:<kind>/<id>`.
//!
//! One format for everything a note, a to-do or an agent can point at — a saved request, a note,
//! a personal to-do, an environment, a mock stub, a load test, a fake-data schema, a saved query, a
//! database connection, a webhook inbox, a utility, a repository or any page of the app:
//!
//! ```text
//! [Create order](devall:request/orders/create.req.yaml)
//! [Auth flow](devall:note/design/auth.md)
//! [Fix refunds](devall:todo/01JC8Z6Q3W7Y2V5T9R4N0M1K8H)
//! ```
//!
//! The id is the item's path inside its project folder (requests, notes, load tests…), its id
//! (to-dos, stubs, environments, webhook inboxes, utilities) or name (repositories). Each
//! `/`-separated segment is percent-encoded where Markdown needs it (spaces, parentheses, `#`, `?`,
//! non-ASCII), so a link stays a valid Markdown link destination. The desktop app resolves links
//! against the open project and only ever navigates inside the app: a link never runs anything or
//! writes a file. The TypeScript side (`apps/desktop/src/lib/links/syntax.ts`) mirrors this module.
//!
//! Links copied for use outside the app ("Copy link") name the project they belong to with
//! `?project=<devall.yaml id>`: `devall:note/design/auth.md?project=01JC8Z…`. The operating system
//! hands such links to the app (checked by [`parse_incoming`]), which opens that project first.
//! Links inside a project's notes and to-dos stay unqualified — relative to the project, so they
//! survive clones — and every parser here accepts the qualifier wherever it doesn't need it.

use std::sync::OnceLock;

use regex::Regex;
use serde::Serialize;

use crate::error::Result;
use crate::notes::title_of;
use crate::project::Project;

pub const SCHEME: &str = "devall:";

/// The query parameter naming a link's project (its `devall.yaml` id).
pub const PROJECT_PARAM: &str = "project";

/// Longest link accepted from outside the app (see [`parse_incoming`]).
pub const MAX_INCOMING_LEN: usize = 2048;

/// Longest project id a qualifier may carry (ids are ULIDs; anything far longer is not one).
const MAX_PROJECT_ID_LEN: usize = 128;

/// Every kind of item a link can point at, with what its id is.
pub const KINDS: &[(&str, &str)] = &[
    ("request", "a saved request under http/ (e.g. orders/create.req.yaml; also .ws/.grpc/.mqtt/.sio.yaml)"),
    ("folder", "a request folder under http/ (opens its settings)"),
    ("note", "a note under notes/ (e.g. design/auth.md)"),
    ("todo", "a personal to-do by id (from list_todos; only resolves on the user's machine)"),
    ("env", "an environment by id (its file name under environments/ without .env.yaml)"),
    ("variables", "the project variables (no id)"),
    ("stub", "a mock stub by id"),
    ("load", "a load test under load/"),
    ("fakedata", "a fake-data schema under data/"),
    ("query", "a saved query under queries/"),
    ("connection", "a database connection under connections/"),
    ("webhook", "a webhook inbox by id (its file name under webhooks/ without .webhook.yaml)"),
    ("utility", "a utility by operation id (e.g. jwt.decode)"),
    ("repo", "a repository of the project by name"),
    ("commit", "a commit: <repo>/<sha> (opens it in the commit graph)"),
    ("compare", "a branch comparison: <repo>/<base>...<head> (three dots: changes since the head branched off, what a merge request shows; two dots: tip to tip)"),
    ("page", "any page of the app: <section>[/<param>] (e.g. page/settings)"),
];

/// A parsed `devall:` link.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Link {
    pub kind: String,
    /// Decoded id (`/`-separated); empty for `variables`.
    pub id: String,
    /// The `?project=` qualifier: the `devall.yaml` id of the project the link belongs to. `None`
    /// for links inside a project (relative to it) and when the qualifier is malformed.
    pub project: Option<String>,
}

impl Link {
    /// The canonical text form (`devall:<kind>/<encoded id>`), without the project qualifier.
    pub fn to_link(&self) -> String {
        format(&self.kind, &self.id)
    }

    /// The canonical form with the project qualifier, if any (`devall:<kind>/<id>?project=<id>`).
    pub fn to_qualified_link(&self) -> String {
        match &self.project {
            Some(p) => format!("{}?{PROJECT_PARAM}={}", self.to_link(), encode_segment(p)),
            None => self.to_link(),
        }
    }
}

pub fn is_kind(kind: &str) -> bool {
    KINDS.iter().any(|(k, _)| *k == kind)
}

/// Bytes kept as they are in an id segment (RFC 3986 `pchar` minus what Markdown or our own
/// parsing would trip over: `(`, `)`, `%`, `#`, `?`, `/`).
fn keep(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b"-._~!$&'*+,;=:@".contains(&b)
}

fn encode_segment(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        if keep(b) {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

fn hex(b: u8) -> Option<u8> {
    (b as char).to_digit(16).map(|d| d as u8)
}

fn decode_segment(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && i + 2 < bytes.len()
            && let (Some(h), Some(l)) = (hex(bytes[i + 1]), hex(bytes[i + 2]))
        {
            out.push(h * 16 + l);
            i += 3;
            continue;
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Starts with `devall:` (any case).
pub fn has_scheme(s: &str) -> bool {
    s.get(..SCHEME.len()).is_some_and(|p| p.eq_ignore_ascii_case(SCHEME))
}

/// `devall:<kind>/<id>` with each id segment percent-encoded where needed.
pub fn format(kind: &str, id: &str) -> String {
    let id = id.trim_matches('/');
    if id.is_empty() {
        return format!("{SCHEME}{kind}");
    }
    let enc: Vec<String> = id.split('/').map(encode_segment).collect();
    format!("{SCHEME}{kind}/{}", enc.join("/"))
}

/// A plausible project id: 1–128 characters, no whitespace or control characters.
fn valid_project_id(p: &str) -> bool {
    !p.is_empty() && p.len() <= MAX_PROJECT_ID_LEN && !p.chars().any(|c| c.is_control() || c.is_whitespace())
}

/// The decoded `project=` value of a link's query (`a=1&project=…`), when there is one.
fn project_param(query: &str) -> Option<String> {
    query.split('&').find_map(|kv| kv.split_once('=').filter(|(k, _)| *k == PROJECT_PARAM).map(|(_, v)| decode_segment(v)))
}

/// What follows the scheme, split into `<kind>/<id>` and the query (before any `#anchor`).
fn split_rest(s: &str) -> (&str, &str) {
    let rest = s[SCHEME.len()..].trim_start_matches('/');
    let rest = rest.split('#').next().unwrap_or_default();
    rest.split_once('?').unwrap_or((rest, ""))
}

/// Parses a `devall:` link (scheme case-insensitive, `devall://…` tolerated, `#anchor` and query
/// parameters other than `project` ignored). `None` for other links, unknown kinds, missing ids
/// and ids that climb (`..`). A malformed `?project=` qualifier is dropped, not an error.
pub fn parse(s: &str) -> Option<Link> {
    let s = s.trim();
    if !has_scheme(s) {
        return None;
    }
    let (rest, query) = split_rest(s);
    let project = project_param(query).filter(|p| valid_project_id(p));
    let (kind, id) = rest.split_once('/').unwrap_or((rest, ""));
    // `devall://notes/<path>` is how the MCP server names note resources.
    let kind = match kind.to_ascii_lowercase().as_str() {
        "notes" => "note".to_string(),
        k => k.to_string(),
    };
    if !is_kind(&kind) {
        return None;
    }
    let mut parts = Vec::new();
    for seg in id.split('/') {
        let d = decode_segment(seg);
        if d.is_empty() {
            continue;
        }
        if d == "." || d == ".." || d.contains(['/', '\\', '\0']) {
            return None;
        }
        parts.push(d);
    }
    let id = parts.join("/");
    match kind.as_str() {
        "variables" => Some(Link { kind, id: String::new(), project }),
        _ if id.is_empty() => None,
        _ => Some(Link { kind, id, project }),
    }
}

/// Checks a link that arrived from outside the app — an OS deep link from a browser, a chat, an
/// email or another editor. Anyone can craft one, so this is stricter than [`parse`]: at most
/// [`MAX_INCOMING_LEN`] bytes, the `devall:` scheme, a known kind, no control characters or
/// backslashes, no `.`/`..` part in the id (between `/` or `:`), no `|` (pages use it for one-off
/// values such as a utility's input), no absolute or drive path, and a well-formed `?project=`
/// qualifier when there is one. The app still only ever navigates to what passes.
pub fn parse_incoming(raw: &str) -> std::result::Result<Link, String> {
    let s = raw.trim();
    if s.len() > MAX_INCOMING_LEN {
        return Err(format!("the link is longer than {MAX_INCOMING_LEN} characters"));
    }
    if !has_scheme(s) {
        return Err("not a devall: link".into());
    }
    if s.chars().any(char::is_control) || s.contains('\\') {
        return Err("the link contains control characters or backslashes".into());
    }
    let (rest, query) = split_rest(s);
    if project_param(query).is_some_and(|p| !valid_project_id(&p)) {
        return Err("the project id is not valid".into());
    }
    let (kind, id) = rest.split_once('/').unwrap_or((rest, ""));
    let kind = kind.to_ascii_lowercase();
    if !is_kind(&kind) && kind != "notes" {
        let kinds: Vec<&str> = KINDS.iter().map(|(k, _)| *k).collect();
        return Err(format!("unknown kind “{kind}” (use one of {})", kinds.join(", ")));
    }
    let parts: Vec<String> = id.split('/').map(decode_segment).collect();
    if parts.iter().any(|p| p.chars().any(char::is_control) || p.contains(['|', '\\', '/'])) {
        return Err("the id contains characters links never use".into());
    }
    if parts.iter().flat_map(|p| p.split(':')).any(|p| p == "." || p == "..") {
        return Err("the id climbs out of its folder".into());
    }
    let Some(link) = parse(s) else {
        return Err("the link has no id".into());
    };
    let first = link.id.split('/').next().unwrap_or_default();
    let drive = first.len() == 2 && first.ends_with(':') && first.as_bytes()[0].is_ascii_alphabetic();
    if drive || first.starts_with('~') {
        return Err("the id is an absolute path".into());
    }
    Ok(link)
}

/// Checks a link given to an agent tool: other links (paths, URLs, `repo:…`) pass; a `devall:`
/// link must parse, else the error lists the kinds.
pub fn check(link: &str) -> std::result::Result<(), String> {
    let l = link.trim();
    if !has_scheme(l) || parse(l).is_some() {
        return Ok(());
    }
    let kinds: Vec<&str> = KINDS.iter().map(|(k, _)| *k).collect();
    Err(format!("`{l}` is not a valid /dev/all link: use devall:<kind>/<id> with kind one of {}", kinds.join(", ")))
}

/// One line per kind, for tool descriptions and agent instructions.
pub fn syntax_help() -> String {
    KINDS.iter().map(|(k, what)| format!("devall:{k}{} — {what}", if *k == "variables" { "" } else { "/…" })).collect::<Vec<_>>().join("; ")
}

fn re(cell: &'static OnceLock<Regex>, pattern: &str) -> &'static Regex {
    cell.get_or_init(|| Regex::new(pattern).expect("valid regex"))
}

/// Link targets in Markdown that may point at project items: `devall:` links (inline, autolinks,
/// reference definitions and bare), project-relative paths (`http/x.req.yaml`, `../ops/run.md`) and
/// `repo:` links. Web/mail URLs, anchors and anything inside code are skipped. In order of
/// appearance, without duplicates; project-qualified links (`…?project=…`) as written.
pub fn extract_targets(markdown: &str) -> Vec<String> {
    static INLINE: OnceLock<Regex> = OnceLock::new();
    static AUTO: OnceLock<Regex> = OnceLock::new();
    static REFDEF: OnceLock<Regex> = OnceLock::new();
    static BARE: OnceLock<Regex> = OnceLock::new();
    static CODE: OnceLock<Regex> = OnceLock::new();
    let inline = re(&INLINE, r#"\]\(\s*(?:<([^>\n]+)>|([^\s)]+))(?:\s+"[^"\n]*")?\s*\)"#);
    let auto = re(&AUTO, r"<([a-zA-Z][a-zA-Z0-9+.-]{1,31}:[^\s<>]+)>");
    let refdef = re(&REFDEF, r"^\s{0,3}\[[^\]]+\]:\s*<?([^\s>]+)>?");
    let bare = re(&BARE, r#"(?i)devall:[^\s)\]>"'`<]+"#);
    let code = re(&CODE, r"`[^`\n]*`");

    let mut out: Vec<String> = Vec::new();
    let mut push = |t: &str| {
        let t = t.trim();
        let wanted = !t.is_empty() && !t.starts_with('#') && (!has_scheme(t) || parse(t).is_some()) && !is_external(t);
        if wanted && !out.iter().any(|x| x == t) {
            out.push(t.to_string());
        }
    };
    let mut fence: Option<&str> = None;
    for line in markdown.lines() {
        let trimmed = line.trim_start();
        if let Some(f) = fence {
            if trimmed.starts_with(f) {
                fence = None;
            }
            continue;
        }
        if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
            fence = Some(&trimmed[..3]);
            continue;
        }
        let text = code.replace_all(line, "");
        if let Some(c) = refdef.captures(&text) {
            push(&c[1]);
        }
        // Each form is cut out once taken, so a bare match never re-reads part of a link.
        let text = inline.replace_all(&text, |c: &regex::Captures| {
            push(c.get(1).or_else(|| c.get(2)).map(|m| m.as_str()).unwrap_or_default());
            " "
        });
        let text = auto.replace_all(&text, |c: &regex::Captures| {
            push(&c[1]);
            " "
        });
        for m in bare.find_iter(&text) {
            push(m.as_str().trim_end_matches(['.', ',', ';', ':', '!', '?']));
        }
    }
    out
}

/// A URL with a scheme other than ours or `repo:` (web, mail, data…), or a Windows drive path.
fn is_external(t: &str) -> bool {
    let Some((scheme, _)) = t.split_once(':') else { return false };
    let lower = scheme.to_ascii_lowercase();
    if lower == "devall" || lower == "repo" {
        return false;
    }
    lower.len() > 1 && lower.chars().all(|c| c.is_ascii_alphanumeric() || "+.-".contains(c)) || lower.len() == 1
}

/// The link targets of one note (see [`extract_targets`]).
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct NoteLinks {
    /// Relative to `notes/`.
    pub path: String,
    pub title: String,
    pub targets: Vec<String>,
}

impl Project {
    /// Every note that links somewhere inside the project, with its link targets — what the app's
    /// "Linked from" (backlinks) index is built from.
    pub fn note_links(&self) -> Result<Vec<NoteLinks>> {
        Ok(self
            .all_notes()?
            .into_iter()
            .filter_map(|(path, text)| {
                let targets = extract_targets(&text);
                (!targets.is_empty()).then(|| NoteLinks { title: title_of(&text, &path).trim_end_matches(".ipynb").to_string(), path, targets })
            })
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_and_parses_every_kind() {
        assert_eq!(format("request", "orders/create.req.yaml"), "devall:request/orders/create.req.yaml");
        assert_eq!(format("note", "Design notes/Auth (v2).md"), "devall:note/Design%20notes/Auth%20%28v2%29.md");
        assert_eq!(format("utility", "my-plugin:hash"), "devall:utility/my-plugin:hash");
        assert_eq!(format("note", "café.md"), "devall:note/caf%C3%A9.md");
        assert_eq!(format("variables", ""), "devall:variables");
        for (kind, _) in KINDS {
            let id = if *kind == "variables" { "" } else { "a b/c(1).yaml" };
            let link = format(kind, id);
            assert_eq!(parse(&link), Some(Link { kind: kind.to_string(), id: id.to_string(), project: None }), "{link}");
        }
    }

    #[test]
    fn project_qualifier_is_parsed_and_never_part_of_the_id() {
        let l = parse("devall:note/design/auth.md?project=01JC8Z6Q3W7Y2V5T9R4N0M1K8H#pkce").unwrap();
        assert_eq!((l.kind.as_str(), l.id.as_str(), l.project.as_deref()), ("note", "design/auth.md", Some("01JC8Z6Q3W7Y2V5T9R4N0M1K8H")));
        assert_eq!(l.to_link(), "devall:note/design/auth.md");
        assert_eq!(l.to_qualified_link(), "devall:note/design/auth.md?project=01JC8Z6Q3W7Y2V5T9R4N0M1K8H");
        // Other parameters are ignored; the qualifier is percent-decoded and round-trips.
        let l = parse("DEVALL:Request/a%3Fb.req.yaml?x=1&project=detached%3Ademo").unwrap();
        assert_eq!((l.id.as_str(), l.project.as_deref()), ("a?b.req.yaml", Some("detached:demo")));
        assert_eq!(parse(&l.to_qualified_link()), Some(l.clone()));
        assert_eq!(parse("devall:variables?project=P1").unwrap().project.as_deref(), Some("P1"));
        // `?` inside an id is always encoded, so the first raw `?` starts the query.
        assert_eq!(format("query", "top?.sql"), "devall:query/top%3F.sql");
        assert_eq!(parse("devall:query/top%3F.sql?project=P").unwrap().id, "top?.sql");
        // A `?` after the anchor belongs to the anchor; malformed qualifiers are dropped.
        assert_eq!(parse("devall:note/a.md#x?project=P").unwrap().project, None);
        assert_eq!(parse("devall:note/a.md?project=").unwrap().project, None);
        assert_eq!(parse("devall:note/a.md?project=a%20b").unwrap().project, None);
        assert_eq!(parse(&format!("devall:note/a.md?project={}", "x".repeat(129))).unwrap().project, None);
        assert_eq!(parse("devall:note/a.md?projectx=P").unwrap().project, None);
        // Qualified and unqualified spellings are the same item.
        assert_eq!(parse("devall:note/a.md?project=P").unwrap().to_link(), parse("devall:note/a.md").unwrap().to_link());
    }

    #[test]
    fn incoming_links_are_checked_strictly() {
        let ok = |s: &str| parse_incoming(s).unwrap_or_else(|e| panic!("{s}: {e}"));
        assert_eq!(ok("devall:page/settings").to_link(), "devall:page/settings");
        assert_eq!(ok("  devall:request/orders/create.req.yaml?project=01JC8Z  ").project.as_deref(), Some("01JC8Z"));
        assert_eq!(ok("devall://notes/My%20Note.md").id, "My Note.md");
        assert_eq!(ok("devall:compare/web/origin/main...feature/x").id, "web/origin/main...feature/x");
        assert_eq!(ok("devall:compare/web/main..dev").id, "web/main..dev");
        assert_eq!(ok("devall:utility/my-plugin:hash").id, "my-plugin:hash");
        assert_eq!(ok("devall:variables").id, "");

        let bad = |s: &str| parse_incoming(s).expect_err(s);
        assert!(bad(&format!("devall:note/{}.md", "a".repeat(MAX_INCOMING_LEN))).contains("longer"));
        assert!(bad("https://example.com").contains("not a devall: link"));
        assert!(bad("file:///C:/Windows/win.ini").contains("not a devall: link"));
        assert!(bad("devall:bogus/x").contains("unknown kind “bogus” (use one of request, folder, note"));
        assert!(bad("devall:request").contains("no id"));
        assert!(bad("devall:request/../secrets.env").contains("climbs"));
        assert!(bad("devall:request/a/%2e%2E/b").contains("climbs"));
        assert!(bad("devall:note/a\\b.md").contains("backslashes"));
        assert!(bad("devall:note/a%5Cb.md").contains("never use"));
        assert!(bad("devall:note/a%2Fb.md").contains("never use"));
        assert!(bad("devall:note/a\nb.md").contains("control"));
        assert!(bad("devall:note/a%0Ab.md").contains("never use"));
        assert!(bad("devall:utility/base64.decode%7Csecret").contains("never use"));
        assert!(bad("devall:page/mocks/stubs/file:../.env").contains("climbs"));
        assert!(bad("devall:page/mocks/stubs/file:%2E%2E/.env").contains("climbs"));
        assert!(bad("devall:note/C:/Windows/win.ini").contains("absolute"));
        assert!(bad("devall:note/~/notes.md").contains("absolute"));
        assert!(bad("devall:note/a.md?project=").contains("project id"));
        assert!(bad("devall:note/a.md?project=a%20b").contains("project id"));
    }

    #[test]
    fn parse_is_lenient_about_form_but_strict_about_paths() {
        let l = parse("  DEVALL://Note/design/auth.md#pkce ").unwrap();
        assert_eq!((l.kind.as_str(), l.id.as_str()), ("note", "design/auth.md"));
        assert_eq!(parse("devall:request/a//b.req.yaml?x=1").unwrap().id, "a/b.req.yaml");
        assert_eq!(parse("devall:variables/anything").unwrap().id, "");
        assert_eq!(parse("devall://notes/design/auth.md").unwrap().to_link(), "devall:note/design/auth.md");
        assert!(parse("devall:request/../secrets.env").is_none());
        assert!(parse("devall:request/a/%2E%2E/b").is_none());
        assert!(parse("devall:note/a%2Fb.md").is_none());
        assert!(parse("devall:request").is_none());
        assert!(parse("devall:bogus/x").is_none());
        assert!(parse("http/x.req.yaml").is_none());
        assert_eq!(parse("devall:todo/01JC8Z6Q3W").unwrap().to_link(), "devall:todo/01JC8Z6Q3W");
    }

    #[test]
    fn check_only_complains_about_bad_devall_links() {
        assert!(check("http/users/list.req.yaml").is_ok());
        assert!(check("https://example.com").is_ok());
        assert!(check("repo:api/src/main.rs").is_ok());
        assert!(check("devall:note/runbook.md").is_ok());
        let e = check("devall:requests/x.req.yaml").unwrap_err();
        assert!(e.contains("request, folder, note"), "{e}");
        assert!(syntax_help().contains("devall:todo/…"));
    }

    #[test]
    fn extracts_project_link_targets() {
        let md = "# Runbook\n\
            See [Create order](devall:request/orders/create.req.yaml \"the request\") and <devall:note/design/auth.md>.\n\
            Old style: [GET users](http/users/list.req.yaml), [ops](../ops/run.md), [api](repo:api/src/main.rs).\n\
            Bare devall:todo/01JC8Z6Q3W. Web: [docs](https://example.com) <https://x.dev> [top](#top) ![img](C:/x.png)\n\
            Twice [again](devall:request/orders/create.req.yaml), bad [x](devall:nope/1), `devall:note/in-code.md`\n\
            [ref]: devall:env/staging\n\
            ```md\n[in a fence](devall:note/skip.md)\n```\n\
            [spaced](<devall:note/My Note.md>)\n\
            Copied: [auth](devall:note/design/auth.md?project=01JC8Z) and devall:env/prod?project=01JC8Z.\n";
        assert_eq!(
            extract_targets(md),
            vec![
                "devall:request/orders/create.req.yaml",
                "devall:note/design/auth.md",
                "http/users/list.req.yaml",
                "../ops/run.md",
                "repo:api/src/main.rs",
                "devall:todo/01JC8Z6Q3W",
                "devall:env/staging",
                "devall:note/My Note.md",
                // Qualified links are taken as written; `parse` reads (or ignores) the qualifier.
                "devall:note/design/auth.md?project=01JC8Z",
                "devall:env/prod?project=01JC8Z",
            ]
        );
    }

    #[test]
    fn note_links_lists_linking_notes() {
        let dir = tempfile::tempdir().unwrap();
        let p = Project::init(dir.path(), "L").unwrap();
        let a = p.create_note("", "Runbook").unwrap();
        p.save_note(&a, "# Runbook\n\nCall [refund](devall:request/payments/refund.req.yaml).\n").unwrap();
        let b = p.create_note("", "Plain").unwrap();
        p.save_note(&b, "# Plain\n\nNo links, only [the web](https://example.com).\n").unwrap();
        let links = p.note_links().unwrap();
        assert_eq!(links, vec![NoteLinks { path: a, title: "Runbook".into(), targets: vec!["devall:request/payments/refund.req.yaml".into()] }]);
    }
}
