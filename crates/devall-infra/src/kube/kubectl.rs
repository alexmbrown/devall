//! A small `kubectl` command-line parser and a validator that never changes the cluster: read
//! commands are checked by looking the objects up; `delete`, `scale`, `rollout restart`, `label`
//! and `annotate` are sent as server-side dry runs (`dryRun=All`); other changing verbs are only
//! parsed. Used to check AI-suggested commands before the user copies (or runs) them.

use std::collections::BTreeMap;

use kube::Client;
use kube::api::{DeleteParams, ListParams, Patch, PatchParams};
use serde::Serialize;
use serde_json::{Value, json};

use super::resources::{KindRef, builtin_kinds, dyn_api, restart_patch};

/// Verbs that only read.
pub const READ_VERBS: &[&str] = &["get", "describe", "logs", "top", "explain", "api-resources", "api-versions", "version", "cluster-info", "events", "auth"];
/// Verbs that change something (only some can be dry-run here).
pub const CHANGE_VERBS: &[&str] = &[
    "delete", "scale", "rollout", "label", "annotate", "patch", "apply", "create", "replace", "edit", "set", "expose", "autoscale", "cordon", "uncordon", "drain", "taint", "run",
    "exec", "cp", "port-forward", "attach", "debug",
];

#[derive(Debug, Clone, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct KubectlCommand {
    pub verb: String,
    /// `rollout restart` → `restart`, `auth can-i` → `can-i`, `set image` → `image`.
    pub sub: Option<String>,
    /// Resource type token as written (`pods`, `deploy`, `deployment.apps`).
    pub resource: Option<String>,
    pub name: Option<String>,
    pub namespace: Option<String>,
    pub all_namespaces: bool,
    /// `--flag=value` / `-f value` (normalized to long names where known).
    pub flags: BTreeMap<String, String>,
    /// Remaining positional arguments (`key=value` labels…).
    pub args: Vec<String>,
}

impl KubectlCommand {
    pub fn mutating(&self) -> bool {
        CHANGE_VERBS.contains(&self.verb.as_str()) && !(self.verb == "rollout" && matches!(self.sub.as_deref(), Some("status" | "history")))
    }
}

/// Splits like a POSIX shell (single/double quotes, backslash escapes), without expansion.
fn tokenize(s: &str) -> Result<Vec<String>, String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut has = false;
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\'' => {
                has = true;
                loop {
                    match chars.next() {
                        Some('\'') => break,
                        Some(x) => cur.push(x),
                        None => return Err("unterminated quote".into()),
                    }
                }
            }
            '"' => {
                has = true;
                loop {
                    match chars.next() {
                        Some('"') => break,
                        Some('\\') => {
                            if let Some(x) = chars.next() {
                                cur.push(x);
                            }
                        }
                        Some(x) => cur.push(x),
                        None => return Err("unterminated quote".into()),
                    }
                }
            }
            '\\' => {
                if let Some(x) = chars.next() {
                    cur.push(x);
                    has = true;
                }
            }
            c if c.is_whitespace() => {
                if has {
                    out.push(std::mem::take(&mut cur));
                    has = false;
                }
            }
            c => {
                cur.push(c);
                has = true;
            }
        }
    }
    if has {
        out.push(cur);
    }
    Ok(out)
}

/// Short flags that take a value, with their long names.
const SHORT_WITH_VALUE: &[(&str, &str)] = &[("-n", "namespace"), ("-l", "selector"), ("-o", "output"), ("-c", "container"), ("-f", "filename"), ("-L", "label-columns")];
/// Long flags that take a value when written without `=`.
const LONG_WITH_VALUE: &[&str] = &["namespace", "selector", "output", "container", "replicas", "tail", "since", "context", "filename", "field-selector", "sort-by", "image", "cluster", "user", "kubeconfig", "timeout", "grace-period"];

/// Parses `kubectl …` (no shell syntax).
pub fn parse(cmd: &str) -> Result<KubectlCommand, String> {
    let toks = tokenize(cmd.trim())?;
    let mut it = toks.into_iter().peekable();
    match it.next().as_deref() {
        Some("kubectl") => {}
        _ => return Err("the command must start with `kubectl`".into()),
    }
    let mut c = KubectlCommand::default();
    let mut positional = Vec::new();
    while let Some(t) = it.next() {
        if t == "-A" || t == "--all-namespaces" {
            c.all_namespaces = true;
        } else if let Some(long) = t.strip_prefix("--") {
            match long.split_once('=') {
                Some((k, v)) => {
                    c.flags.insert(k.to_string(), v.to_string());
                }
                None if LONG_WITH_VALUE.contains(&long) => {
                    let v = it.next().ok_or_else(|| format!("--{long} needs a value"))?;
                    c.flags.insert(long.to_string(), v);
                }
                None => {
                    c.flags.insert(long.to_string(), "true".into());
                }
            }
        } else if t.starts_with('-') && t.len() > 1 && !t[1..].chars().all(|x| x.is_ascii_digit()) {
            let (flag, inline) = if t.len() > 2 && SHORT_WITH_VALUE.iter().any(|(s, _)| t.starts_with(s)) { (&t[..2], Some(t[2..].trim_start_matches('=').to_string())) } else { (t.as_str(), None) };
            match SHORT_WITH_VALUE.iter().find(|(s, _)| *s == flag) {
                Some((_, long)) => {
                    let v = match inline {
                        Some(v) => v,
                        None => it.next().ok_or_else(|| format!("{flag} needs a value"))?,
                    };
                    c.flags.insert(long.to_string(), v);
                }
                None => {
                    c.flags.insert(flag.trim_start_matches('-').to_string(), "true".into());
                }
            }
        } else {
            positional.push(t);
        }
    }
    c.namespace = c.flags.remove("namespace");
    let mut pos = positional.into_iter();
    c.verb = pos.next().ok_or("no kubectl verb")?.to_ascii_lowercase();
    if !READ_VERBS.contains(&c.verb.as_str()) && !CHANGE_VERBS.contains(&c.verb.as_str()) {
        return Err(format!("unknown kubectl verb `{}`", c.verb));
    }
    if matches!(c.verb.as_str(), "rollout" | "auth" | "set" | "top") {
        c.sub = pos.next().map(|s| s.to_ascii_lowercase());
    }
    let rest: Vec<String> = pos.collect();
    let takes_resource = !matches!(c.verb.as_str(), "logs" | "version" | "cluster-info" | "api-resources" | "api-versions" | "events" | "auth" | "exec" | "cp" | "attach" | "run" | "port-forward");
    let mut rest = rest.into_iter();
    if c.verb == "logs" || c.verb == "exec" || c.verb == "attach" || c.verb == "port-forward" {
        // `logs pod/x`, `logs x`, `logs deploy/x`.
        if let Some(t) = rest.next() {
            match t.split_once('/') {
                Some((r, n)) => {
                    c.resource = Some(r.to_string());
                    c.name = Some(n.to_string());
                }
                None => {
                    c.resource = Some("pods".into());
                    c.name = Some(t);
                }
            }
        }
    } else if takes_resource && let Some(t) = rest.next() {
        match t.split_once('/') {
            Some((r, n)) => {
                c.resource = Some(r.to_string());
                c.name = Some(n.to_string());
            }
            None => {
                c.resource = Some(t);
                if let Some(n) = rest.next() {
                    if n.contains('=') || (n.ends_with('-') && !matches!(c.verb.as_str(), "get" | "describe" | "delete")) {
                        c.args.push(n);
                    } else {
                        c.name = Some(n);
                    }
                }
            }
        }
    }
    c.args.extend(rest);
    Ok(c)
}

/// Common short names (`po`, `svc`, `deploy`…).
const SHORT_NAMES: &[(&str, &str)] = &[
    ("po", "pods"),
    ("svc", "services"),
    ("deploy", "deployments"),
    ("sts", "statefulsets"),
    ("ds", "daemonsets"),
    ("rs", "replicasets"),
    ("cm", "configmaps"),
    ("ns", "namespaces"),
    ("no", "nodes"),
    ("pvc", "persistentvolumeclaims"),
    ("pv", "persistentvolumes"),
    ("ing", "ingresses"),
    ("cj", "cronjobs"),
    ("ev", "events"),
];

/// The kind a resource token names: plural, singular, short name, `kind` or `plural.group`.
pub fn resolve_kind(token: &str, kinds: &[KindRef]) -> Option<KindRef> {
    let t = token.to_ascii_lowercase();
    let (name, group) = match t.split_once('.') {
        Some((n, g)) => (n.to_string(), Some(g.to_string())),
        None => (t.clone(), None),
    };
    let name = SHORT_NAMES.iter().find(|(s, _)| *s == name).map(|(_, p)| p.to_string()).unwrap_or(name);
    let all: Vec<KindRef> = builtin_kinds().into_iter().map(|k| k.kind).chain(kinds.iter().cloned()).collect();
    all.into_iter().find(|k| {
        (k.plural == name || k.kind.to_ascii_lowercase() == name || format!("{}s", k.kind.to_ascii_lowercase()) == name) && group.as_deref().is_none_or(|g| k.group == g || k.group.starts_with(&format!("{g}.")))
    })
}

fn labels_patch(args: &[String], field: &str) -> Result<Value, String> {
    let mut map = serde_json::Map::new();
    for a in args.iter().filter(|a| !a.starts_with('-')) {
        if let Some(k) = a.strip_suffix('-') {
            map.insert(k.to_string(), Value::Null);
        } else if let Some((k, v)) = a.split_once('=') {
            map.insert(k.to_string(), Value::String(v.to_string()));
        } else {
            return Err(format!("`{a}` is not key=value or key-"));
        }
    }
    if map.is_empty() {
        return Err(format!("no {field} given"));
    }
    Ok(json!({ "metadata": { field: map } }))
}

/// Checks a parsed command against the cluster without changing anything. `Ok` is a note for the
/// user (`verb`, `kind`, `name`, `namespace`, `mutating`, `dryRun`, and `action` when the app can
/// run it itself); `Err` explains why the command won't work.
pub async fn validate(client: &Client, cmd: &KubectlCommand, custom: &[KindRef], default_ns: &str) -> Result<Value, String> {
    let ns = cmd.namespace.clone().unwrap_or_else(|| default_ns.to_string());
    let mut note = json!({ "verb": cmd.verb, "sub": cmd.sub, "namespace": if cmd.all_namespaces { Value::Null } else { json!(ns) }, "mutating": cmd.mutating() });
    let kind = match cmd.resource.as_deref() {
        Some(r) => {
            // `get pods,svc` lists several kinds: check each.
            let mut first = None;
            for part in r.split(',') {
                let k = resolve_kind(part, custom).ok_or_else(|| format!("unknown resource type `{part}` in this cluster"))?;
                first.get_or_insert(k);
            }
            first
        }
        None => None,
    };
    if let Some(k) = &kind {
        note["kind"] = json!(k);
    }
    let name = cmd.name.clone();
    note["name"] = json!(name);
    let k8s = |e: kube::Error| crate::error::Error::from(e).to_string();
    let ns_opt = if cmd.all_namespaces { None } else { Some(ns.as_str()) };
    match (cmd.verb.as_str(), &kind, &name) {
        ("get" | "describe" | "logs", Some(k), Some(n)) => {
            dyn_api(client.clone(), k, ns_opt).get(n).await.map_err(|e| format!("{} {n}: {}", k.kind, k8s(e)))?;
            note["exists"] = json!(true);
        }
        ("get" | "describe", Some(k), None) => {
            let sel = cmd.flags.get("selector").cloned();
            let mut lp = ListParams::default().limit(1);
            if let Some(s) = &sel {
                lp = lp.labels(s);
            }
            dyn_api(client.clone(), k, ns_opt).list(&lp).await.map_err(k8s)?;
        }
        ("delete", Some(k), Some(n)) => {
            let dp = DeleteParams { dry_run: true, ..DeleteParams::background() };
            dyn_api(client.clone(), k, ns_opt).delete(n, &dp).await.map_err(|e| format!("dry run failed: {}", k8s(e)))?;
            note["dryRun"] = json!(format!("the API server accepted deleting {} {n} (dry run, nothing deleted)", k.kind));
            note["action"] = json!({ "type": "delete", "kind": k, "name": n, "namespace": ns });
        }
        ("scale", Some(k), Some(n)) => {
            let replicas: i32 = cmd.flags.get("replicas").and_then(|r| r.parse().ok()).ok_or("scale needs --replicas=N")?;
            let pp = PatchParams { dry_run: true, ..Default::default() };
            dyn_api(client.clone(), k, ns_opt).patch(n, &pp, &Patch::Merge(&json!({ "spec": { "replicas": replicas } }))).await.map_err(|e| format!("dry run failed: {}", k8s(e)))?;
            note["dryRun"] = json!(format!("the API server accepted scaling {} {n} to {replicas} (dry run, nothing changed)", k.kind));
            note["action"] = json!({ "type": "scale", "kind": k, "name": n, "namespace": ns, "replicas": replicas });
        }
        ("rollout", Some(k), Some(n)) if cmd.sub.as_deref() == Some("restart") => {
            let pp = PatchParams { dry_run: true, ..Default::default() };
            let patch = restart_patch(&chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true));
            dyn_api(client.clone(), k, ns_opt).patch(n, &pp, &Patch::Merge(&patch)).await.map_err(|e| format!("dry run failed: {}", k8s(e)))?;
            note["dryRun"] = json!(format!("the API server accepted restarting {} {n} (dry run, nothing restarted)", k.kind));
            note["action"] = json!({ "type": "restart", "kind": k, "name": n, "namespace": ns });
        }
        ("rollout", Some(k), Some(n)) => {
            dyn_api(client.clone(), k, ns_opt).get(n).await.map_err(|e| format!("{} {n}: {}", k.kind, k8s(e)))?;
            if cmd.mutating() {
                note["dryRun"] = json!(null);
                note["unverified"] = json!(format!("`rollout {}` can't be dry-run here; the target exists", cmd.sub.as_deref().unwrap_or("")));
            }
        }
        ("label" | "annotate", Some(k), Some(n)) => {
            let field = if cmd.verb == "label" { "labels" } else { "annotations" };
            let patch = labels_patch(&cmd.args, field)?;
            let pp = PatchParams { dry_run: true, ..Default::default() };
            dyn_api(client.clone(), k, ns_opt).patch(n, &pp, &Patch::Merge(&patch)).await.map_err(|e| format!("dry run failed: {}", k8s(e)))?;
            note["dryRun"] = json!(format!("the API server accepted the {field} change on {} {n} (dry run, nothing changed)", k.kind));
        }
        ("delete" | "scale" | "label" | "annotate" | "rollout", _, None) => {
            return Err(format!("`{}` needs a resource type and name (e.g. deployment/api)", cmd.verb));
        }
        _ if cmd.mutating() => {
            note["dryRun"] = json!(null);
            note["unverified"] = json!(format!("`{}` changes the cluster and can't be dry-run here — check it carefully before running it", cmd.verb));
        }
        _ => {}
    }
    Ok(note)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_common_commands() {
        let c = parse("kubectl get pods -n shop -l app=api -o wide").unwrap();
        assert_eq!((c.verb.as_str(), c.resource.as_deref(), c.name.as_deref(), c.namespace.as_deref()), ("get", Some("pods"), None, Some("shop")));
        assert_eq!(c.flags["selector"], "app=api");
        assert!(!c.mutating());
        let c = parse("kubectl scale deployment/api --replicas=3 --namespace=shop").unwrap();
        assert_eq!((c.resource.as_deref(), c.name.as_deref(), c.flags["replicas"].as_str()), (Some("deployment"), Some("api"), "3"));
        assert!(c.mutating());
        let c = parse("kubectl rollout restart deploy api -n shop").unwrap();
        assert_eq!((c.sub.as_deref(), c.resource.as_deref(), c.name.as_deref()), (Some("restart"), Some("deploy"), Some("api")));
        assert!(!parse("kubectl rollout status deploy/api").unwrap().mutating());
        let c = parse("kubectl logs api-7d9 -c app --tail 50").unwrap();
        assert_eq!((c.resource.as_deref(), c.name.as_deref(), c.flags["tail"].as_str(), c.flags["container"].as_str()), (Some("pods"), Some("api-7d9"), "50", "app"));
        let c = parse("kubectl label pod web 'tier=front end' old-").unwrap();
        assert_eq!(c.args, ["tier=front end", "old-"]);
        assert_eq!(labels_patch(&c.args, "labels").unwrap(), json!({ "metadata": { "labels": { "tier": "front end", "old": null } } }));
        assert!(parse("kubectl get pods -A").unwrap().all_namespaces);
        assert!(parse("kubectl frobnicate pods").unwrap_err().contains("unknown"));
        assert!(parse("helm list").is_err());
        assert!(parse("kubectl get 'pods").is_err());
    }

    #[test]
    fn resolves_kinds() {
        assert_eq!(resolve_kind("po", &[]).unwrap().kind, "Pod");
        assert_eq!(resolve_kind("Deployment", &[]).unwrap().plural, "deployments");
        assert_eq!(resolve_kind("deployments.apps", &[]).unwrap().group, "apps");
        assert_eq!(resolve_kind("svc", &[]).unwrap().kind, "Service");
        assert!(resolve_kind("widgets", &[]).is_none());
        let crd = KindRef::new("example.com", "v1", "Widget", "widgets", true);
        assert_eq!(resolve_kind("widget", std::slice::from_ref(&crd)).unwrap().group, "example.com");
    }
}
