//! Generic resource access over `DynamicObject`: kinds, table rows, YAML (with Secret values
//! hidden), describe-like summaries and the few write actions (delete, restart, scale).

use std::collections::BTreeMap;

use k8s_openapi::api::core::v1::{Event, Namespace};
use k8s_openapi::apiextensions_apiserver::pkg::apis::apiextensions::v1::CustomResourceDefinition;
use kube::api::{Api, ApiResource, DeleteParams, DynamicObject, ListParams, Patch, PatchParams};
use kube::Client;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::error::{Error, Result};

/// What every masked secret value looks like (same mask as the rest of /dev/all).
pub const MASK: &str = "••••••";
const LAST_APPLIED: &str = "kubectl.kubernetes.io/last-applied-configuration";

/// Identifies a resource type (from the kinds list; CRDs included).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct KindRef {
    /// Empty for the core group.
    pub group: String,
    pub version: String,
    pub kind: String,
    pub plural: String,
    pub namespaced: bool,
}

impl KindRef {
    pub fn new(group: &str, version: &str, kind: &str, plural: &str, namespaced: bool) -> Self {
        Self { group: group.into(), version: version.into(), kind: kind.into(), plural: plural.into(), namespaced }
    }

    pub fn api_resource(&self) -> ApiResource {
        ApiResource {
            group: self.group.clone(),
            version: self.version.clone(),
            api_version: if self.group.is_empty() { self.version.clone() } else { format!("{}/{}", self.group, self.version) },
            kind: self.kind.clone(),
            plural: self.plural.clone(),
        }
    }

    fn is(&self, group: &str, kind: &str) -> bool {
        self.group == group && self.kind == kind
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct KindInfo {
    #[serde(flatten)]
    pub kind: KindRef,
    pub category: String,
}

/// The kinds shown in the sidebar before custom resources.
pub fn builtin_kinds() -> Vec<KindInfo> {
    let k = |cat: &str, g: &str, v: &str, kind: &str, plural: &str, ns: bool| KindInfo { kind: KindRef::new(g, v, kind, plural, ns), category: cat.into() };
    vec![
        k("Workloads", "", "v1", "Pod", "pods", true),
        k("Workloads", "apps", "v1", "Deployment", "deployments", true),
        k("Workloads", "apps", "v1", "StatefulSet", "statefulsets", true),
        k("Workloads", "apps", "v1", "DaemonSet", "daemonsets", true),
        k("Workloads", "apps", "v1", "ReplicaSet", "replicasets", true),
        k("Workloads", "batch", "v1", "Job", "jobs", true),
        k("Workloads", "batch", "v1", "CronJob", "cronjobs", true),
        k("Network", "", "v1", "Service", "services", true),
        k("Network", "networking.k8s.io", "v1", "Ingress", "ingresses", true),
        k("Config", "", "v1", "ConfigMap", "configmaps", true),
        k("Config", "", "v1", "Secret", "secrets", true),
        k("Storage", "", "v1", "PersistentVolumeClaim", "persistentvolumeclaims", true),
        k("Storage", "", "v1", "PersistentVolume", "persistentvolumes", false),
        k("Cluster", "", "v1", "Event", "events", true),
        k("Cluster", "", "v1", "Namespace", "namespaces", false),
        k("Cluster", "", "v1", "Node", "nodes", false),
    ]
}

/// Custom resource kinds from the cluster's CRDs (storage version preferred).
pub async fn custom_kinds(client: &Client) -> Result<Vec<KindInfo>> {
    let crds = Api::<CustomResourceDefinition>::all(client.clone()).list(&ListParams::default()).await?;
    let mut out: Vec<KindInfo> = crds
        .items
        .into_iter()
        .filter_map(|crd| {
            let spec = crd.spec;
            let version = spec.versions.iter().find(|v| v.storage && v.served).or_else(|| spec.versions.iter().find(|v| v.served))?.name.clone();
            Some(KindInfo {
                kind: KindRef { group: spec.group.clone(), version, kind: spec.names.kind.clone(), plural: spec.names.plural.clone(), namespaced: spec.scope == "Namespaced" },
                category: format!("Custom · {}", spec.group),
            })
        })
        .collect();
    out.sort_by(|a, b| (&a.category, &a.kind.kind).cmp(&(&b.category, &b.kind.kind)));
    Ok(out)
}

/// `Api<DynamicObject>` for `kind`; namespaced kinds without a namespace list across all.
pub fn dyn_api(client: Client, kind: &KindRef, namespace: Option<&str>) -> Api<DynamicObject> {
    let ar = kind.api_resource();
    match namespace.filter(|n| !n.is_empty() && kind.namespaced) {
        Some(ns) => Api::namespaced_with(client, ns, &ar),
        None => Api::all_with(client, &ar),
    }
}

fn require_ns<'a>(kind: &KindRef, namespace: Option<&'a str>) -> Result<Option<&'a str>> {
    let ns = namespace.filter(|n| !n.is_empty());
    if kind.namespaced && ns.is_none() {
        return Err(Error::Invalid(format!("{} is namespaced: a namespace is required", kind.kind)));
    }
    Ok(ns)
}

pub async fn namespaces(client: &Client, fallback: &str) -> Result<Vec<String>> {
    match Api::<Namespace>::all(client.clone()).list(&ListParams::default()).await {
        Ok(list) => {
            let mut names: Vec<String> = list.items.into_iter().filter_map(|n| n.metadata.name).collect();
            names.sort();
            Ok(names)
        }
        // RBAC often forbids listing namespaces; the context's namespace still works.
        Err(kube::Error::Api(s)) if s.code == 403 => Ok(vec![fallback.to_string()]),
        Err(e) => Err(e.into()),
    }
}

// ---------- table rows ----------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Health {
    Ok,
    Warn,
    Err,
    Idle,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PortInfo {
    pub port: u16,
    pub name: Option<String>,
    pub protocol: String,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ResourceRow {
    pub uid: String,
    pub name: String,
    pub namespace: Option<String>,
    /// RFC 3339 creation time (the UI renders age).
    pub created: Option<String>,
    /// Values for [`columns_for`], in order.
    pub cells: Vec<String>,
    pub health: Health,
    /// Pods: container names (init containers last) for logs/exec.
    pub containers: Vec<String>,
    /// Pods/Services: ports usable for port-forwarding.
    pub ports: Vec<PortInfo>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ResourceList {
    pub columns: Vec<String>,
    pub rows: Vec<ResourceRow>,
}

/// Extra columns per kind (name, namespace and age are always shown).
pub fn columns_for(kind: &KindRef) -> Vec<&'static str> {
    match (kind.group.as_str(), kind.kind.as_str()) {
        ("", "Pod") => vec!["Ready", "Status", "Restarts", "Node", "IP"],
        ("apps", "Deployment") => vec!["Ready", "Up-to-date", "Available"],
        ("apps", "StatefulSet") => vec!["Ready"],
        ("apps", "DaemonSet") => vec!["Desired", "Current", "Ready", "Available"],
        ("apps", "ReplicaSet") => vec!["Desired", "Current", "Ready"],
        ("batch", "Job") => vec!["Completions", "Status"],
        ("batch", "CronJob") => vec!["Schedule", "Suspend", "Active", "Last schedule"],
        ("", "Service") => vec!["Type", "Cluster IP", "External IP", "Ports"],
        ("networking.k8s.io", "Ingress") => vec!["Class", "Hosts", "Address"],
        ("", "ConfigMap") => vec!["Data"],
        ("", "Secret") => vec!["Type", "Data"],
        ("", "Event") => vec!["Type", "Reason", "Object", "Message", "Count", "Last seen"],
        ("", "Node") => vec!["Status", "Roles", "Version", "Internal IP"],
        ("", "Namespace") => vec!["Status"],
        ("", "PersistentVolumeClaim") => vec!["Status", "Volume", "Capacity", "Access modes", "Storage class"],
        ("", "PersistentVolume") => vec!["Capacity", "Access modes", "Reclaim", "Status", "Claim"],
        _ => vec!["Status"],
    }
}

fn st<'a>(v: &'a Value, ptr: &str) -> Option<&'a str> {
    v.pointer(ptr).and_then(Value::as_str)
}
fn int(v: &Value, ptr: &str) -> i64 {
    v.pointer(ptr).and_then(Value::as_i64).unwrap_or(0)
}
fn arr<'a>(v: &'a Value, ptr: &str) -> &'a [Value] {
    v.pointer(ptr).and_then(Value::as_array).map(Vec::as_slice).unwrap_or(&[])
}
fn count(v: &Value, ptr: &str) -> usize {
    v.pointer(ptr).and_then(Value::as_object).map(|m| m.len()).unwrap_or(0)
}
fn condition<'a>(v: &'a Value, ty: &str) -> Option<&'a Value> {
    arr(v, "/status/conditions").iter().find(|c| st(c, "/type") == Some(ty))
}
fn lb_address(v: &Value) -> String {
    arr(v, "/status/loadBalancer/ingress").iter().filter_map(|i| st(i, "/ip").or_else(|| st(i, "/hostname"))).collect::<Vec<_>>().join(",")
}

/// kubectl-style pod status: waiting/terminated reasons win over the phase.
fn pod_status(v: &Value, terminating: bool) -> (String, Health) {
    if terminating {
        return ("Terminating".into(), Health::Warn);
    }
    let phase = st(v, "/status/phase").unwrap_or("Unknown").to_string();
    let mut reason: Option<String> = st(v, "/status/reason").map(str::to_string);
    for cs in arr(v, "/status/initContainerStatuses") {
        if let Some(r) = st(cs, "/state/waiting/reason") {
            reason = Some(format!("Init:{r}"));
        }
    }
    for cs in arr(v, "/status/containerStatuses") {
        if let Some(r) = st(cs, "/state/waiting/reason").or_else(|| st(cs, "/state/terminated/reason")) {
            reason = Some(r.to_string());
        }
    }
    let total = arr(v, "/spec/containers").len();
    let ready = arr(v, "/status/containerStatuses").iter().filter(|c| c.get("ready").and_then(Value::as_bool) == Some(true)).count();
    let text = reason.clone().unwrap_or_else(|| phase.clone());
    let bad = ["CrashLoopBackOff", "Error", "ImagePullBackOff", "ErrImagePull", "OOMKilled", "CreateContainerConfigError", "InvalidImageName", "Evicted"];
    let health = if bad.iter().any(|b| text.contains(b)) || phase == "Failed" {
        Health::Err
    } else if phase == "Succeeded" || text == "Completed" {
        Health::Idle
    } else if phase == "Running" && ready == total {
        Health::Ok
    } else {
        Health::Warn
    };
    (text, health)
}

/// Builds the table row for one object of `kind`. `obj` is the whole object as JSON.
pub fn row_for(kind: &KindRef, obj: &Value) -> ResourceRow {
    let meta = |p: &str| st(obj, &format!("/metadata/{p}")).map(str::to_string);
    let terminating = obj.pointer("/metadata/deletionTimestamp").is_some_and(|t| !t.is_null());
    let mut containers = Vec::new();
    let mut ports = Vec::new();
    let ratio = |a: i64, b: i64| format!("{a}/{b}");
    let (cells, health): (Vec<String>, Health) = match (kind.group.as_str(), kind.kind.as_str()) {
        ("", "Pod") => {
            for c in arr(obj, "/spec/containers") {
                containers.extend(st(c, "/name").map(str::to_string));
                for p in arr(c, "/ports") {
                    if let Some(port) = p.get("containerPort").and_then(Value::as_u64).and_then(|n| u16::try_from(n).ok()) {
                        ports.push(PortInfo { port, name: st(p, "/name").map(str::to_string), protocol: st(p, "/protocol").unwrap_or("TCP").into() });
                    }
                }
            }
            for c in arr(obj, "/spec/initContainers") {
                containers.extend(st(c, "/name").map(str::to_string));
            }
            let total = arr(obj, "/spec/containers").len() as i64;
            let ready = arr(obj, "/status/containerStatuses").iter().filter(|c| c.get("ready").and_then(Value::as_bool) == Some(true)).count() as i64;
            let restarts: i64 = arr(obj, "/status/containerStatuses").iter().map(|c| int(c, "/restartCount")).sum();
            let (status, health) = pod_status(obj, terminating);
            (
                vec![ratio(ready, total), status, restarts.to_string(), st(obj, "/spec/nodeName").unwrap_or("").into(), st(obj, "/status/podIP").unwrap_or("").into()],
                health,
            )
        }
        ("apps", "Deployment") | ("apps", "StatefulSet") => {
            let want = obj.pointer("/spec/replicas").and_then(Value::as_i64).unwrap_or(1);
            let ready = int(obj, "/status/readyReplicas");
            let health = if want == 0 { Health::Idle } else if ready >= want { Health::Ok } else if ready == 0 { Health::Err } else { Health::Warn };
            let mut cells = vec![ratio(ready, want)];
            if kind.kind == "Deployment" {
                cells.push(int(obj, "/status/updatedReplicas").to_string());
                cells.push(int(obj, "/status/availableReplicas").to_string());
            }
            (cells, health)
        }
        ("apps", "DaemonSet") => {
            let desired = int(obj, "/status/desiredNumberScheduled");
            let ready = int(obj, "/status/numberReady");
            let health = if desired == 0 { Health::Idle } else if ready >= desired { Health::Ok } else { Health::Warn };
            (
                vec![desired.to_string(), int(obj, "/status/currentNumberScheduled").to_string(), ready.to_string(), int(obj, "/status/numberAvailable").to_string()],
                health,
            )
        }
        ("apps", "ReplicaSet") => {
            let desired = obj.pointer("/spec/replicas").and_then(Value::as_i64).unwrap_or(1);
            let ready = int(obj, "/status/readyReplicas");
            let health = if desired == 0 { Health::Idle } else if ready >= desired { Health::Ok } else { Health::Warn };
            (vec![desired.to_string(), int(obj, "/status/replicas").to_string(), ready.to_string()], health)
        }
        ("batch", "Job") => {
            let completions = obj.pointer("/spec/completions").and_then(Value::as_i64).unwrap_or(1);
            let succeeded = int(obj, "/status/succeeded");
            let done = |t: &str| condition(obj, t).and_then(|c| st(c, "/status")) == Some("True");
            let (status, health) = if done("Complete") {
                ("Complete", Health::Idle)
            } else if done("Failed") {
                ("Failed", Health::Err)
            } else if int(obj, "/status/active") > 0 {
                ("Running", Health::Ok)
            } else {
                ("Pending", Health::Warn)
            };
            (vec![ratio(succeeded, completions), status.into()], health)
        }
        ("batch", "CronJob") => {
            let suspended = obj.pointer("/spec/suspend").and_then(Value::as_bool).unwrap_or(false);
            (
                vec![
                    st(obj, "/spec/schedule").unwrap_or("").into(),
                    if suspended { "True".into() } else { "False".into() },
                    arr(obj, "/status/active").len().to_string(),
                    st(obj, "/status/lastScheduleTime").unwrap_or("").into(),
                ],
                if suspended { Health::Idle } else { Health::Ok },
            )
        }
        ("", "Service") => {
            let mut rendered = Vec::new();
            for p in arr(obj, "/spec/ports") {
                let port = p.get("port").and_then(Value::as_u64).unwrap_or(0);
                let proto = st(p, "/protocol").unwrap_or("TCP");
                let node = p.get("nodePort").and_then(Value::as_u64).map(|n| format!(":{n}")).unwrap_or_default();
                rendered.push(format!("{port}{node}/{proto}"));
                if let Ok(port) = u16::try_from(port) {
                    ports.push(PortInfo { port, name: st(p, "/name").map(str::to_string), protocol: proto.into() });
                }
            }
            let mut external = lb_address(obj);
            if external.is_empty() {
                external = arr(obj, "/spec/externalIPs").iter().filter_map(Value::as_str).collect::<Vec<_>>().join(",");
            }
            (
                vec![st(obj, "/spec/type").unwrap_or("ClusterIP").into(), st(obj, "/spec/clusterIP").unwrap_or("").into(), external, rendered.join(" ")],
                Health::Ok,
            )
        }
        ("networking.k8s.io", "Ingress") => {
            let hosts: Vec<&str> = arr(obj, "/spec/rules").iter().filter_map(|r| st(r, "/host")).collect();
            (vec![st(obj, "/spec/ingressClassName").unwrap_or("").into(), if hosts.is_empty() { "*".into() } else { hosts.join(",") }, lb_address(obj)], Health::Ok)
        }
        ("", "ConfigMap") => ((vec![(count(obj, "/data") + count(obj, "/binaryData")).to_string()]), Health::Ok),
        ("", "Secret") => (vec![st(obj, "/type").unwrap_or("Opaque").into(), count(obj, "/data").max(count(obj, "/stringData")).to_string()], Health::Ok),
        ("", "Event") => {
            let ty = st(obj, "/type").unwrap_or("Normal");
            let object = format!("{}/{}", st(obj, "/involvedObject/kind").unwrap_or(""), st(obj, "/involvedObject/name").unwrap_or(""));
            let last = st(obj, "/lastTimestamp").or_else(|| st(obj, "/eventTime")).or_else(|| st(obj, "/metadata/creationTimestamp")).unwrap_or("");
            (
                vec![ty.into(), st(obj, "/reason").unwrap_or("").into(), object, st(obj, "/message").unwrap_or("").trim().into(), int(obj, "/count").max(1).to_string(), last.into()],
                if ty == "Warning" { Health::Warn } else { Health::Ok },
            )
        }
        ("", "Node") => {
            let ready = condition(obj, "Ready").and_then(|c| st(c, "/status")) == Some("True");
            let unschedulable = obj.pointer("/spec/unschedulable").and_then(Value::as_bool).unwrap_or(false);
            let mut status = if ready { "Ready".to_string() } else { "NotReady".to_string() };
            if unschedulable {
                status.push_str(",SchedulingDisabled");
            }
            let roles: Vec<String> = obj
                .pointer("/metadata/labels")
                .and_then(Value::as_object)
                .map(|m| m.keys().filter_map(|k| k.strip_prefix("node-role.kubernetes.io/")).map(str::to_string).collect())
                .unwrap_or_default();
            let ip = arr(obj, "/status/addresses").iter().find(|a| st(a, "/type") == Some("InternalIP")).and_then(|a| st(a, "/address")).unwrap_or("");
            (
                vec![status, if roles.is_empty() { "<none>".into() } else { roles.join(",") }, st(obj, "/status/nodeInfo/kubeletVersion").unwrap_or("").into(), ip.into()],
                if !ready { Health::Err } else if unschedulable { Health::Warn } else { Health::Ok },
            )
        }
        ("", "Namespace") => {
            let phase = st(obj, "/status/phase").unwrap_or("Active");
            (vec![phase.into()], if phase == "Active" { Health::Ok } else { Health::Warn })
        }
        ("", "PersistentVolumeClaim") => {
            let phase = st(obj, "/status/phase").unwrap_or("Pending");
            (
                vec![
                    phase.into(),
                    st(obj, "/spec/volumeName").unwrap_or("").into(),
                    st(obj, "/status/capacity/storage").unwrap_or("").into(),
                    arr(obj, "/spec/accessModes").iter().filter_map(Value::as_str).collect::<Vec<_>>().join(","),
                    st(obj, "/spec/storageClassName").unwrap_or("").into(),
                ],
                if phase == "Bound" { Health::Ok } else { Health::Warn },
            )
        }
        ("", "PersistentVolume") => {
            let phase = st(obj, "/status/phase").unwrap_or("");
            let claim = match (st(obj, "/spec/claimRef/namespace"), st(obj, "/spec/claimRef/name")) {
                (Some(ns), Some(n)) => format!("{ns}/{n}"),
                _ => String::new(),
            };
            (
                vec![
                    st(obj, "/spec/capacity/storage").unwrap_or("").into(),
                    arr(obj, "/spec/accessModes").iter().filter_map(Value::as_str).collect::<Vec<_>>().join(","),
                    st(obj, "/spec/persistentVolumeReclaimPolicy").unwrap_or("").into(),
                    phase.into(),
                    claim,
                ],
                if phase == "Bound" || phase == "Available" { Health::Ok } else { Health::Warn },
            )
        }
        _ => {
            // Custom resources: the Ready condition when there is one.
            match condition(obj, "Ready").or_else(|| condition(obj, "Available")) {
                Some(c) => {
                    let ok = st(c, "/status") == Some("True");
                    let text = if ok { "Ready".to_string() } else { st(c, "/reason").unwrap_or("NotReady").to_string() };
                    (vec![text], if ok { Health::Ok } else { Health::Warn })
                }
                None => (vec![st(obj, "/status/phase").or_else(|| st(obj, "/status/state")).unwrap_or("").into()], Health::Idle),
            }
        }
    };
    ResourceRow {
        uid: meta("uid").unwrap_or_default(),
        name: meta("name").unwrap_or_default(),
        namespace: meta("namespace"),
        created: meta("creationTimestamp"),
        cells,
        health: if terminating { Health::Warn } else { health },
        containers,
        ports,
    }
}

pub fn to_json(obj: &DynamicObject) -> Value {
    serde_json::to_value(obj).unwrap_or(Value::Null)
}

pub async fn list(client: &Client, kind: &KindRef, namespace: Option<&str>, label_selector: Option<&str>) -> Result<ResourceList> {
    let api = dyn_api(client.clone(), kind, namespace);
    let mut lp = ListParams::default();
    if let Some(sel) = label_selector.filter(|s| !s.trim().is_empty()) {
        lp = lp.labels(sel);
    }
    let list = api.list(&lp).await?;
    let mut rows: Vec<ResourceRow> = list.items.iter().map(|o| row_for(kind, &to_json(o))).collect();
    sort_rows(kind, &mut rows);
    Ok(ResourceList { columns: columns_for(kind).into_iter().map(str::to_string).collect(), rows })
}

pub fn sort_rows(kind: &KindRef, rows: &mut [ResourceRow]) {
    if kind.is("", "Event") {
        // Newest first ("Last seen" is the last cell).
        rows.sort_by(|a, b| b.cells.last().cmp(&a.cells.last()));
    } else {
        rows.sort_by(|a, b| (&a.namespace, &a.name).cmp(&(&b.namespace, &b.name)));
    }
}

// ---------- YAML / secrets ----------

/// Masks Secret values in a whole Secret object: `data`, `stringData`, and the copy kubectl keeps
/// in the last-applied-configuration annotation.
pub fn redact_secret(obj: &mut Value) {
    for key in ["data", "stringData"] {
        if let Some(map) = obj.get_mut(key).and_then(Value::as_object_mut) {
            for v in map.values_mut() {
                *v = Value::String(MASK.into());
            }
        }
    }
    if let Some(ann) = obj.pointer_mut(&format!("/metadata/annotations/{}", LAST_APPLIED.replace('/', "~1")))
        && let Some(text) = ann.as_str()
    {
        *ann = Value::String(match serde_json::from_str::<Value>(text) {
            Ok(mut inner) => {
                redact_secret(&mut inner);
                serde_json::to_string(&inner).unwrap_or_else(|_| MASK.into())
            }
            Err(_) => MASK.into(),
        });
    }
}

/// YAML for one object, without `managedFields`. Secret values are masked unless `reveal`.
pub fn object_yaml(kind: &KindRef, mut obj: Value, reveal: bool) -> Result<String> {
    if let Some(meta) = obj.get_mut("metadata").and_then(Value::as_object_mut) {
        meta.remove("managedFields");
    }
    if kind.is("", "Secret") && !reveal {
        redact_secret(&mut obj);
    }
    serde_yaml_ng::to_string(&obj).map_err(|e| Error::Invalid(format!("cannot render YAML: {e}")))
}

pub async fn get_yaml(client: &Client, kind: &KindRef, namespace: Option<&str>, name: &str, reveal: bool) -> Result<String> {
    let ns = require_ns(kind, namespace)?;
    let obj = dyn_api(client.clone(), kind, ns).get(name).await?;
    object_yaml(kind, to_json(&obj), reveal)
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SecretEntry {
    pub key: String,
    /// Decoded UTF-8 value; `None` for binary data.
    pub value: Option<String>,
    pub size: usize,
}

/// Decoded values of a Secret (only on explicit reveal).
pub async fn secret_values(client: &Client, namespace: &str, name: &str) -> Result<Vec<SecretEntry>> {
    use base64::Engine;
    let kind = KindRef::new("", "v1", "Secret", "secrets", true);
    let obj = to_json(&dyn_api(client.clone(), &kind, Some(namespace)).get(name).await?);
    let mut out = Vec::new();
    if let Some(map) = obj.get("data").and_then(Value::as_object) {
        for (k, v) in map {
            let bytes = base64::engine::general_purpose::STANDARD.decode(v.as_str().unwrap_or("")).unwrap_or_default();
            out.push(SecretEntry { key: k.clone(), size: bytes.len(), value: String::from_utf8(bytes).ok() });
        }
    }
    out.sort_by(|a, b| a.key.cmp(&b.key));
    Ok(out)
}

// ---------- describe ----------

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConditionRow {
    #[serde(rename = "type")]
    pub type_: String,
    pub status: String,
    pub reason: Option<String>,
    pub message: Option<String>,
    pub last_transition: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ContainerInfo {
    pub name: String,
    pub image: String,
    pub ready: bool,
    pub restarts: i64,
    /// `running`, `waiting`, `terminated`.
    pub state: String,
    pub reason: Option<String>,
    pub message: Option<String>,
    pub init: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EventRow {
    #[serde(rename = "type")]
    pub type_: String,
    pub reason: String,
    pub message: String,
    pub count: i64,
    pub last_seen: Option<String>,
    pub source: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Describe {
    pub kind: String,
    pub name: String,
    pub namespace: Option<String>,
    pub uid: String,
    pub created: Option<String>,
    pub labels: BTreeMap<String, String>,
    pub annotations: BTreeMap<String, String>,
    pub owners: Vec<String>,
    /// Key facts (node, IP, replicas, selector, ports, …).
    pub fields: Vec<(String, String)>,
    pub conditions: Vec<ConditionRow>,
    pub containers: Vec<ContainerInfo>,
    pub events: Vec<EventRow>,
}

fn str_map(v: Option<&Value>) -> BTreeMap<String, String> {
    v.and_then(Value::as_object).map(|m| m.iter().map(|(k, v)| (k.clone(), v.as_str().map(str::to_string).unwrap_or_else(|| v.to_string()))).collect()).unwrap_or_default()
}

fn selector_text(v: Option<&Value>) -> String {
    str_map(v).into_iter().map(|(k, v)| format!("{k}={v}")).collect::<Vec<_>>().join(",")
}

/// The describe summary from an object (events are added by [`describe`]).
pub fn describe_object(kind: &KindRef, obj: &Value) -> Describe {
    let mut obj = obj.clone();
    if kind.is("", "Secret") {
        redact_secret(&mut obj);
    }
    let mut annotations = str_map(obj.pointer("/metadata/annotations"));
    if let Some(a) = annotations.get_mut(LAST_APPLIED)
        && a.len() > 200
    {
        *a = format!("{}… ({} chars)", &a[..a.floor_char_boundary(200)], a.len());
    }
    let mut fields: Vec<(String, String)> = Vec::new();
    let mut f = |k: &str, v: Option<String>| {
        if let Some(v) = v.filter(|v| !v.is_empty()) {
            fields.push((k.to_string(), v));
        }
    };
    let s = |p: &str| st(&obj, p).map(str::to_string);
    match (kind.group.as_str(), kind.kind.as_str()) {
        ("", "Pod") => {
            f("Node", s("/spec/nodeName"));
            f("Pod IP", s("/status/podIP"));
            f("Phase", s("/status/phase"));
            f("QoS class", s("/status/qosClass"));
            f("Service account", s("/spec/serviceAccountName"));
            f("Started", s("/status/startTime"));
        }
        ("apps", _) => {
            f("Replicas", obj.pointer("/spec/replicas").map(|v| v.to_string()));
            f("Ready", obj.pointer("/status/readyReplicas").map(|v| v.to_string()));
            f("Strategy", s("/spec/strategy/type").or_else(|| s("/spec/updateStrategy/type")));
            f("Selector", Some(selector_text(obj.pointer("/spec/selector/matchLabels"))));
            let images: Vec<String> = arr(&obj, "/spec/template/spec/containers").iter().filter_map(|c| st(c, "/image").map(str::to_string)).collect();
            f("Images", Some(images.join(", ")));
        }
        ("", "Service") => {
            f("Type", s("/spec/type"));
            f("Cluster IP", s("/spec/clusterIP"));
            f("Selector", Some(selector_text(obj.pointer("/spec/selector"))));
            let ports: Vec<String> = arr(&obj, "/spec/ports")
                .iter()
                .map(|p| format!("{} → {}/{}", p.get("port").map(|v| v.to_string()).unwrap_or_default(), p.get("targetPort").map(|v| v.as_str().map(str::to_string).unwrap_or_else(|| v.to_string())).unwrap_or_default(), st(p, "/protocol").unwrap_or("TCP")))
                .collect();
            f("Ports", Some(ports.join(", ")));
        }
        ("batch", "CronJob") => {
            f("Schedule", s("/spec/schedule"));
            f("Last schedule", s("/status/lastScheduleTime"));
        }
        ("", "Node") => {
            f("Kubelet", s("/status/nodeInfo/kubeletVersion"));
            f("OS image", s("/status/nodeInfo/osImage"));
            f("Container runtime", s("/status/nodeInfo/containerRuntimeVersion"));
            f("CPU", s("/status/capacity/cpu"));
            f("Memory", s("/status/capacity/memory"));
        }
        ("", "Secret") => {
            f("Type", s("/type"));
            f("Keys", obj.get("data").and_then(Value::as_object).map(|m| m.keys().cloned().collect::<Vec<_>>().join(", ")));
        }
        ("", "ConfigMap") => {
            f("Keys", obj.get("data").and_then(Value::as_object).map(|m| m.keys().cloned().collect::<Vec<_>>().join(", ")));
        }
        _ => {}
    }
    let conditions = arr(&obj, "/status/conditions")
        .iter()
        .map(|c| ConditionRow {
            type_: st(c, "/type").unwrap_or("").into(),
            status: st(c, "/status").unwrap_or("").into(),
            reason: st(c, "/reason").map(str::to_string),
            message: st(c, "/message").map(str::to_string),
            last_transition: st(c, "/lastTransitionTime").map(str::to_string),
        })
        .collect();
    let mut containers = Vec::new();
    if kind.is("", "Pod") {
        for (init, spec_ptr, status_ptr) in [(false, "/spec/containers", "/status/containerStatuses"), (true, "/spec/initContainers", "/status/initContainerStatuses")] {
            for c in arr(&obj, spec_ptr) {
                let name = st(c, "/name").unwrap_or("").to_string();
                let status = arr(&obj, status_ptr).iter().find(|s| st(s, "/name") == Some(name.as_str()));
                let state_obj = status.and_then(|s| s.get("state")).and_then(Value::as_object);
                let (state, detail) = state_obj.and_then(|m| m.iter().next()).map(|(k, v)| (k.clone(), Some(v))).unwrap_or(("waiting".into(), None));
                containers.push(ContainerInfo {
                    image: st(c, "/image").unwrap_or("").into(),
                    ready: status.and_then(|s| s.get("ready")).and_then(Value::as_bool).unwrap_or(false),
                    restarts: status.map(|s| int(s, "/restartCount")).unwrap_or(0),
                    state,
                    reason: detail.and_then(|d| st(d, "/reason")).map(str::to_string),
                    message: detail.and_then(|d| st(d, "/message")).map(str::to_string),
                    init,
                    name,
                });
            }
        }
    }
    Describe {
        kind: kind.kind.clone(),
        name: st(&obj, "/metadata/name").unwrap_or("").into(),
        namespace: st(&obj, "/metadata/namespace").map(str::to_string),
        uid: st(&obj, "/metadata/uid").unwrap_or("").into(),
        created: st(&obj, "/metadata/creationTimestamp").map(str::to_string),
        labels: str_map(obj.pointer("/metadata/labels")),
        annotations,
        owners: arr(&obj, "/metadata/ownerReferences").iter().map(|o| format!("{}/{}", st(o, "/kind").unwrap_or(""), st(o, "/name").unwrap_or(""))).collect(),
        fields,
        conditions,
        containers,
        events: Vec::new(),
    }
}

pub async fn describe(client: &Client, kind: &KindRef, namespace: Option<&str>, name: &str) -> Result<Describe> {
    let ns = require_ns(kind, namespace)?;
    let obj = to_json(&dyn_api(client.clone(), kind, ns).get(name).await?);
    let mut d = describe_object(kind, &obj);
    let lp = ListParams::default().fields(&format!("involvedObject.uid={}", d.uid));
    let events_api: Api<Event> = match ns {
        Some(ns) => Api::namespaced(client.clone(), ns),
        None => Api::all(client.clone()),
    };
    if let Ok(list) = events_api.list(&lp).await {
        let mut events: Vec<EventRow> = list
            .items
            .into_iter()
            .map(|e| EventRow {
                type_: e.type_.unwrap_or_else(|| "Normal".into()),
                reason: e.reason.unwrap_or_default(),
                message: e.message.unwrap_or_default().trim().to_string(),
                count: e.count.map(i64::from).unwrap_or(1),
                last_seen: e
                    .last_timestamp
                    .map(|t| t.0.to_string())
                    .or_else(|| e.event_time.map(|t| t.0.to_string()))
                    .or_else(|| e.metadata.creation_timestamp.map(|t| t.0.to_string())),
                source: e.source.and_then(|s| s.component),
            })
            .collect();
        events.sort_by(|a, b| b.last_seen.cmp(&a.last_seen));
        d.events = events;
    }
    Ok(d)
}

// ---------- actions ----------

pub async fn delete(client: &Client, kind: &KindRef, namespace: Option<&str>, name: &str) -> Result<()> {
    let ns = require_ns(kind, namespace)?;
    dyn_api(client.clone(), kind, ns).delete(name, &DeleteParams::background()).await?;
    Ok(())
}

/// `kubectl rollout restart` for Deployments/StatefulSets/DaemonSets; deletes a Pod so its
/// controller recreates it.
pub async fn restart(client: &Client, kind: &KindRef, namespace: Option<&str>, name: &str) -> Result<()> {
    let ns = require_ns(kind, namespace)?;
    let api = dyn_api(client.clone(), kind, ns);
    match (kind.group.as_str(), kind.kind.as_str()) {
        ("apps", "Deployment" | "StatefulSet" | "DaemonSet") => {
            let patch = restart_patch(&chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true));
            api.patch(name, &PatchParams::default(), &Patch::Merge(&patch)).await?;
            Ok(())
        }
        ("", "Pod") => {
            api.delete(name, &DeleteParams::default()).await?;
            Ok(())
        }
        _ => Err(Error::Invalid(format!("{} can't be restarted", kind.kind))),
    }
}

pub fn restart_patch(now: &str) -> Value {
    json!({ "spec": { "template": { "metadata": { "annotations": { "kubectl.kubernetes.io/restartedAt": now } } } } })
}

pub async fn scale(client: &Client, kind: &KindRef, namespace: Option<&str>, name: &str, replicas: i32) -> Result<()> {
    if !matches!((kind.group.as_str(), kind.kind.as_str()), ("apps", "Deployment" | "StatefulSet" | "ReplicaSet")) {
        return Err(Error::Invalid(format!("{} can't be scaled", kind.kind)));
    }
    if replicas < 0 {
        return Err(Error::Invalid("replicas must be ≥ 0".into()));
    }
    let ns = require_ns(kind, namespace)?;
    dyn_api(client.clone(), kind, ns).patch(name, &PatchParams::default(), &Patch::Merge(&json!({ "spec": { "replicas": replicas } }))).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pod() -> Value {
        json!({
            "apiVersion": "v1", "kind": "Pod",
            "metadata": { "name": "api-7d9f", "namespace": "shop", "uid": "u1", "creationTimestamp": "2026-10-21T08:00:00Z" },
            "spec": {
                "nodeName": "node-1",
                "containers": [
                    { "name": "api", "image": "shop/api:1", "ports": [ { "containerPort": 8080, "name": "http" } ] },
                    { "name": "sidecar", "image": "envoy" }
                ],
                "initContainers": [ { "name": "migrate", "image": "shop/migrate" } ]
            },
            "status": {
                "phase": "Running", "podIP": "10.0.0.7",
                "containerStatuses": [
                    { "name": "api", "ready": false, "restartCount": 4, "state": { "waiting": { "reason": "CrashLoopBackOff", "message": "back-off" } } },
                    { "name": "sidecar", "ready": true, "restartCount": 0, "state": { "running": {} } }
                ]
            }
        })
    }

    #[test]
    fn pod_row() {
        let kind = KindRef::new("", "v1", "Pod", "pods", true);
        let row = row_for(&kind, &pod());
        assert_eq!(row.cells, vec!["1/2", "CrashLoopBackOff", "4", "node-1", "10.0.0.7"]);
        assert_eq!(row.health, Health::Err);
        assert_eq!(row.containers, vec!["api", "sidecar", "migrate"]);
        assert_eq!(row.ports, vec![PortInfo { port: 8080, name: Some("http".into()), protocol: "TCP".into() }]);
        assert_eq!(columns_for(&kind).len(), row.cells.len());
        let d = describe_object(&kind, &pod());
        assert_eq!(d.containers.len(), 3);
        assert_eq!(d.containers[0].state, "waiting");
        assert_eq!(d.containers[0].reason.as_deref(), Some("CrashLoopBackOff"));
        assert!(d.containers[2].init);
    }

    #[test]
    fn deployment_and_service_rows() {
        let dep = KindRef::new("apps", "v1", "Deployment", "deployments", true);
        let row = row_for(&dep, &json!({ "metadata": { "name": "api" }, "spec": { "replicas": 3 }, "status": { "readyReplicas": 2, "updatedReplicas": 3, "availableReplicas": 2 } }));
        assert_eq!(row.cells, vec!["2/3", "3", "2"]);
        assert_eq!(row.health, Health::Warn);
        let svc = KindRef::new("", "v1", "Service", "services", true);
        let row = row_for(
            &svc,
            &json!({ "metadata": { "name": "api" }, "spec": { "type": "NodePort", "clusterIP": "10.96.0.10", "ports": [ { "port": 80, "targetPort": 8080, "nodePort": 30080, "protocol": "TCP", "name": "http" } ] } }),
        );
        assert_eq!(row.cells, vec!["NodePort", "10.96.0.10", "", "80:30080/TCP"]);
        assert_eq!(row.ports[0].port, 80);
    }

    #[test]
    fn secret_values_are_masked_everywhere() {
        let last_applied = json!({ "apiVersion": "v1", "kind": "Secret", "metadata": { "name": "db" }, "stringData": { "password": "hunter22" }, "data": { "user": "YWRtaW4=" } }).to_string();
        let secret = json!({
            "apiVersion": "v1", "kind": "Secret",
            "metadata": { "name": "db", "namespace": "shop", "managedFields": [ { "manager": "kubectl" } ], "annotations": { LAST_APPLIED: last_applied } },
            "type": "Opaque",
            "data": { "password": "aHVudGVyMjI=", "user": "YWRtaW4=" }
        });
        let kind = KindRef::new("", "v1", "Secret", "secrets", true);
        let yaml = object_yaml(&kind, secret.clone(), false).unwrap();
        assert!(!yaml.contains("aHVudGVyMjI="), "{yaml}");
        assert!(!yaml.contains("YWRtaW4="), "{yaml}");
        assert!(!yaml.contains("hunter22"), "{yaml}");
        assert!(yaml.contains(MASK));
        assert!(!yaml.contains("managedFields"));
        assert!(yaml.contains("password"), "keys stay visible");
        // Revealed YAML keeps values.
        let shown = object_yaml(&kind, secret.clone(), true).unwrap();
        assert!(shown.contains("aHVudGVyMjI="));
        // Other kinds aren't touched.
        let cm = KindRef::new("", "v1", "ConfigMap", "configmaps", true);
        let cm_yaml = object_yaml(&cm, json!({ "metadata": { "name": "c" }, "data": { "k": "v" } }), false).unwrap();
        assert!(cm_yaml.contains("k: v"));
        // Describe masks too, and the row never carries values.
        let d = describe_object(&kind, &secret);
        let text = serde_json::to_string(&d).unwrap();
        assert!(!text.contains("hunter22") && !text.contains("aHVudGVyMjI="), "{text}");
        let row = row_for(&kind, &secret);
        assert_eq!(row.cells, vec!["Opaque", "2"]);
    }

    #[test]
    fn unparseable_last_applied_is_fully_masked() {
        let mut s = json!({ "metadata": { "annotations": { LAST_APPLIED: "{not json" } }, "data": {} });
        redact_secret(&mut s);
        assert_eq!(s.pointer(&format!("/metadata/annotations/{}", LAST_APPLIED.replace('/', "~1"))).unwrap(), MASK);
    }

    #[test]
    fn restart_patch_shape() {
        let p = restart_patch("2026-10-21T10:00:00Z");
        assert_eq!(p.pointer("/spec/template/metadata/annotations/kubectl.kubernetes.io~1restartedAt").unwrap(), "2026-10-21T10:00:00Z");
    }

    #[test]
    fn custom_resource_ready_condition() {
        let kind = KindRef::new("cert-manager.io", "v1", "Certificate", "certificates", true);
        let row = row_for(&kind, &json!({ "metadata": { "name": "tls" }, "status": { "conditions": [ { "type": "Ready", "status": "False", "reason": "Issuing" } ] } }));
        assert_eq!(row.cells, vec!["Issuing"]);
        assert_eq!(row.health, Health::Warn);
        assert_eq!(kind.api_resource().api_version, "cert-manager.io/v1");
        assert_eq!(KindRef::new("", "v1", "Pod", "pods", true).api_resource().api_version, "v1");
    }
}
