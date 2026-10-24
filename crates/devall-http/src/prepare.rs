use std::path::{Path, PathBuf};

use base64::Engine;
use devall_core::{ApiKeyLocation, Auth, Body, HttpRequestFile, Resolver};
use serde::Serialize;

use crate::{HttpError, USER_AGENT};

/// A request with every template resolved, ready to send.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PreparedRequest {
    pub method: String,
    /// Full URL including query parameters.
    pub url: String,
    pub headers: Vec<(String, String)>,
    pub body: PreparedBody,
    pub timeout_ms: u64,
    pub follow_redirects: bool,
    pub verify_tls: bool,
    /// `None` = system proxy; `Some("none")` = no proxy; otherwise a proxy URL.
    pub proxy: Option<String>,
    /// Resolved client certificate (PEM) and optional separate key for mutual TLS.
    pub client_cert: Option<(PathBuf, Option<PathBuf>)>,
    /// Digest credentials: answered after the server's 401 challenge.
    #[serde(skip)]
    pub digest: Option<(String, String)>,
    /// NTLM credentials: negotiate → challenge → authenticate on one connection.
    #[serde(skip)]
    pub ntlm: Option<crate::ntlm::Credentials>,
}

/// Extra inputs to [`prepare_with`].
#[derive(Debug, Clone, Default)]
pub struct PrepareOptions {
    /// Access token for `oauth2` auth (fetch it with `oauth2::get_token` first).
    pub oauth2_token: Option<String>,
    /// Signing time for AWS SigV4 (defaults to now; tests pin it).
    pub now: Option<chrono::DateTime<chrono::Utc>>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", content = "data", rename_all = "camelCase")]
pub enum PreparedBody {
    None,
    Bytes(#[serde(serialize_with = "ser_bytes")] Vec<u8>),
    Form(Vec<(String, String)>),
    Multipart(Vec<PreparedPart>),
}

fn ser_bytes<S: serde::Serializer>(b: &[u8], s: S) -> Result<S::Ok, S::Error> {
    s.serialize_str(&String::from_utf8_lossy(b))
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PreparedPart {
    pub name: String,
    pub value: String,
    pub file: Option<PathBuf>,
    pub content_type: Option<String>,
}

impl PreparedRequest {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers.iter().find(|(k, _)| k.eq_ignore_ascii_case(name)).map(|(_, v)| v.as_str())
    }
    fn has_header(&self, name: &str) -> bool {
        self.header(name).is_some()
    }
}

/// Resolves `req` (which should already have folder inheritance applied — see
/// `Project::effective_request`). Relative file paths (binary bodies, multipart files) are
/// resolved against `project_root`.
pub fn prepare(req: &HttpRequestFile, r: &mut Resolver, project_root: &Path) -> Result<PreparedRequest, HttpError> {
    prepare_with(req, r, project_root, &PrepareOptions::default())
}

/// Like [`prepare`], with an OAuth2 access token and/or a fixed signing time.
pub fn prepare_with(req: &HttpRequestFile, r: &mut Resolver, project_root: &Path, opts: &PrepareOptions) -> Result<PreparedRequest, HttpError> {
    let method = r.resolve(req.method.trim()).to_uppercase();
    let method = if method.is_empty() { "GET".to_string() } else { method };

    let mut raw_url = r.resolve(req.url.trim());
    if raw_url.is_empty() {
        return Err(HttpError::InvalidUrl("URL is empty".into()));
    }
    if !raw_url.contains("://") {
        raw_url = format!("http://{raw_url}");
    }
    let mut url = url::Url::parse(&raw_url).map_err(|e| HttpError::InvalidUrl(format!("{raw_url} ({e})")))?;
    {
        let enabled: Vec<(String, String)> =
            req.params.iter().filter(|p| p.enabled && !p.name.is_empty()).map(|p| (r.resolve(&p.name), r.resolve(&p.value))).collect();
        if !enabled.is_empty() {
            let mut q = url.query_pairs_mut();
            for (k, v) in &enabled {
                q.append_pair(k, v);
            }
        }
    }

    let mut out = PreparedRequest {
        method,
        url: String::new(),
        headers: req
            .headers
            .iter()
            .filter(|h| h.enabled && !h.name.trim().is_empty())
            .map(|h| (r.resolve(h.name.trim()), r.resolve(&h.value)))
            .collect(),
        body: PreparedBody::None,
        timeout_ms: req.settings.timeout_ms,
        follow_redirects: req.settings.follow_redirects,
        verify_tls: req.settings.verify_tls,
        proxy: req.settings.proxy.as_ref().map(|p| r.resolve(p.trim())).filter(|p| !p.is_empty()),
        client_cert: req
            .settings
            .client_cert
            .as_ref()
            .filter(|c| !c.cert.trim().is_empty())
            .map(|c| (resolve_path(project_root, &r.resolve(c.cert.trim())), c.key.as_ref().filter(|k| !k.trim().is_empty()).map(|k| resolve_path(project_root, &r.resolve(k.trim()))))),
        digest: None,
        ntlm: None,
    };
    let mut sigv4: Option<(String, String, Option<String>, String, String)> = None;
    let mut plugin_auth: Option<(String, String, std::collections::BTreeMap<String, String>)> = None;

    match &req.auth {
        Auth::Inherit | Auth::None => {}
        Auth::Basic { username, password } => {
            if !out.has_header("authorization") {
                let token = base64::engine::general_purpose::STANDARD.encode(format!("{}:{}", r.resolve(username), r.resolve(password)));
                out.headers.push(("Authorization".into(), format!("Basic {token}")));
            }
        }
        Auth::Bearer { token, prefix } => {
            if !out.has_header("authorization") {
                let prefix = prefix.as_deref().map(|p| r.resolve(p)).unwrap_or_else(|| "Bearer".into());
                let token = r.resolve(token);
                let value = if prefix.is_empty() { token } else { format!("{prefix} {token}") };
                out.headers.push(("Authorization".into(), value));
            }
        }
        Auth::ApiKey { key, value, location } => {
            let (k, v) = (r.resolve(key), r.resolve(value));
            if !k.is_empty() {
                match location {
                    ApiKeyLocation::Header => {
                        if !out.has_header(&k) {
                            out.headers.push((k, v));
                        }
                    }
                    ApiKeyLocation::Query => {
                        url.query_pairs_mut().append_pair(&k, &v);
                    }
                }
            }
        }
        Auth::OAuth2(cfg) => {
            if !out.has_header("authorization") {
                let token = opts.oauth2_token.as_deref().ok_or_else(|| {
                    HttpError::InvalidRequest("OAuth 2.0: no access token yet — fetch one first (oauth2::get_token / “Get token”)".into())
                })?;
                let prefix = cfg.token_prefix.as_deref().map(|p| r.resolve(p)).unwrap_or_else(|| "Bearer".into());
                out.headers.push(("Authorization".into(), if prefix.is_empty() { token.to_string() } else { format!("{prefix} {token}") }));
            }
        }
        Auth::AwsSigV4 { access_key_id, secret_access_key, session_token, region, service } => {
            sigv4 = Some((
                r.resolve(access_key_id),
                r.resolve(secret_access_key),
                session_token.as_ref().map(|t| r.resolve(t)),
                r.resolve(region),
                r.resolve(service),
            ));
        }
        Auth::Digest { username, password } => {
            out.digest = Some((r.resolve(username), r.resolve(password)));
        }
        Auth::Ntlm { username, password, domain, workstation } => {
            let domain = domain.as_deref().map(|d| r.resolve(d));
            let workstation = workstation.as_deref().map(|w| r.resolve(w));
            out.ntlm = Some(crate::ntlm::Credentials::new(&r.resolve(username), &r.resolve(password), domain.as_deref(), workstation.as_deref()));
        }
        Auth::Plugin { plugin, provider, config } => {
            if plugin.trim().is_empty() || provider.trim().is_empty() {
                return Err(HttpError::InvalidRequest("plugin auth: pick a plugin and an auth provider".into()));
            }
            plugin_auth = Some((plugin.trim().to_string(), provider.trim().to_string(), config.iter().map(|(k, v)| (k.clone(), r.resolve(v))).collect()));
        }
    }
    out.url = url.to_string();

    let mut default_ct: Option<String> = None;
    out.body = match &req.body {
        Body::None => PreparedBody::None,
        Body::Json { content } => {
            default_ct = Some("application/json".into());
            PreparedBody::Bytes(r.resolve(content).into_bytes())
        }
        Body::Text { content, content_type } => {
            default_ct = Some(content_type.clone().unwrap_or_else(|| "text/plain; charset=utf-8".into()));
            PreparedBody::Bytes(r.resolve(content).into_bytes())
        }
        Body::Xml { content } => {
            default_ct = Some("application/xml".into());
            PreparedBody::Bytes(r.resolve(content).into_bytes())
        }
        Body::Form { fields } => {
            PreparedBody::Form(fields.iter().filter(|f| f.enabled && !f.name.is_empty()).map(|f| (r.resolve(&f.name), r.resolve(&f.value))).collect())
        }
        Body::Multipart { parts } => PreparedBody::Multipart(
            parts
                .iter()
                .filter(|p| p.enabled && !p.name.is_empty())
                .map(|p| PreparedPart {
                    name: r.resolve(&p.name),
                    value: r.resolve(&p.value),
                    file: p.file.as_ref().filter(|f| !f.is_empty()).map(|f| resolve_path(project_root, &r.resolve(f))),
                    content_type: p.content_type.as_ref().map(|c| r.resolve(c)),
                })
                .collect(),
        ),
        Body::Graphql { query, variables, operation_name } => {
            default_ct = Some("application/json".into());
            let vars_text = r.resolve(variables);
            let vars: serde_json::Value = if vars_text.trim().is_empty() {
                serde_json::Value::Object(Default::default())
            } else {
                serde_json::from_str(&vars_text).map_err(|e| HttpError::InvalidRequest(format!("GraphQL variables are not valid JSON: {e}")))?
            };
            let mut payload = serde_json::json!({ "query": r.resolve(query), "variables": vars });
            if let Some(op) = operation_name.as_ref().filter(|o| !o.is_empty()) {
                payload["operationName"] = serde_json::Value::String(r.resolve(op));
            }
            PreparedBody::Bytes(serde_json::to_vec(&payload).unwrap_or_default())
        }
        Body::Binary { path, content_type } => {
            let p = resolve_path(project_root, &r.resolve(path));
            let data = std::fs::read(&p).map_err(|e| HttpError::InvalidRequest(format!("cannot read {}: {e}", p.display())))?;
            default_ct = Some(content_type.clone().unwrap_or_else(|| "application/octet-stream".into()));
            PreparedBody::Bytes(data)
        }
    };
    if let Some(ct) = default_ct
        && !out.has_header("content-type")
    {
        out.headers.push(("Content-Type".into(), ct));
    }
    if !out.has_header("user-agent") {
        out.headers.push(("User-Agent".into(), USER_AGENT.into()));
    }
    if !out.has_header("accept") {
        out.headers.push(("Accept".into(), "*/*".into()));
    }
    if let Some((key_id, secret, token, region, service)) = sigv4 {
        if key_id.is_empty() || secret.is_empty() || region.is_empty() || service.is_empty() {
            return Err(HttpError::InvalidRequest("AWS SigV4 needs an access key id, secret key, region and service".into()));
        }
        let body: Vec<u8> = match &out.body {
            PreparedBody::None => Vec::new(),
            PreparedBody::Bytes(b) => b.clone(),
            PreparedBody::Form(pairs) => url::form_urlencoded::Serializer::new(String::new()).extend_pairs(pairs.iter()).finish().into_bytes(),
            PreparedBody::Multipart(_) => return Err(HttpError::InvalidRequest("AWS SigV4 signing of multipart bodies is not supported".into())),
        };
        if matches!(out.body, PreparedBody::Form(_)) && !out.has_header("content-type") {
            out.headers.push(("Content-Type".into(), "application/x-www-form-urlencoded".into()));
        }
        let params = crate::sigv4::SigV4Params {
            access_key_id: &key_id,
            secret_access_key: &secret,
            session_token: token.as_deref(),
            region: &region,
            service: &service,
        };
        let added = crate::sigv4::sign(&out.method, &url, &out.headers, &body, &params, opts.now.unwrap_or_else(chrono::Utc::now));
        out.headers.retain(|(k, _)| !added.iter().any(|(a, _)| a.eq_ignore_ascii_case(k)));
        out.headers.extend(added);
    }
    if let Some((plugin, provider, config)) = plugin_auth {
        let host = crate::plugins::installed().ok_or_else(|| {
            HttpError::InvalidRequest(format!("plugin auth ({plugin}/{provider}) needs plugins, which are available in the /dev/all app and the devall CLI"))
        })?;
        let added = host.authorize(project_root, &plugin, &provider, &config, &out).map_err(|e| HttpError::InvalidRequest(format!("plugin auth: {e}")))?;
        crate::plugins::apply_auth(&mut out, added).map_err(|e| HttpError::InvalidRequest(format!("plugin auth: {e}")))?;
    }
    Ok(out)
}

fn resolve_path(root: &Path, p: &str) -> PathBuf {
    let path = Path::new(p);
    if path.is_absolute() { path.to_path_buf() } else { root.join(path) }
}
