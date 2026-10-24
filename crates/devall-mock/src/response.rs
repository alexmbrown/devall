//! Turning a matched stub into a concrete response: templating, body sources, proxying, delays.

use bytes::Bytes;
use serde_json::{Value, json};

use crate::proxy;
use crate::request::MockRequest;
use crate::server::{ServerState, read};
use crate::store::Matched;
use crate::stub::{BodySpec, DelayDistribution, Dribble, Fault};
use crate::template;

#[derive(Debug, Clone)]
pub(crate) struct Prepared {
    pub status: u16,
    pub reason: Option<String>,
    pub headers: Vec<(String, String)>,
    pub body: Bytes,
    pub fault: Option<Fault>,
    pub delay_ms: u64,
    pub dribble: Option<Dribble>,
    pub response_definition: Value,
    pub proxied_from: Option<String>,
}

impl Prepared {
    pub fn simple(status: u16, content_type: &str, body: impl Into<Bytes>) -> Self {
        Self {
            status,
            reason: None,
            headers: vec![("Content-Type".into(), content_type.into())],
            body: body.into(),
            fault: None,
            delay_ms: 0,
            dribble: None,
            response_definition: json!({ "status": status, "fromConfiguredStub": false }),
            proxied_from: None,
        }
    }

    fn error(message: &str, definition: Value) -> Self {
        let mut p = Self::simple(500, "text/plain", message.to_string());
        p.response_definition = definition;
        p
    }
}

/// A response injected by the chaos layer.
pub(crate) fn chaos_error(status: u16) -> Prepared {
    let body = json!({ "error": "Injected by /dev/all chaos", "status": status }).to_string();
    let mut p = Prepared::simple(status, "application/json", body);
    p.headers.push(("X-Devall-Chaos".into(), "error".into()));
    p.response_definition = json!({ "status": status, "fromConfiguredStub": false, "devallChaos": true });
    p
}

/// Fixed delay (stub, else global setting) plus a sample from the delay distribution (stub, else
/// global setting), as WireMock does.
fn delay_for(st: &ServerState, fixed: Option<u64>, dist: Option<&DelayDistribution>) -> u64 {
    let settings = read(&st.settings);
    let fixed = fixed.or_else(|| settings.get("fixedDelay").and_then(Value::as_u64)).unwrap_or(0);
    let random = match dist {
        Some(d) => d.sample(),
        None => settings.get("delayDistribution").and_then(|v| serde_json::from_value::<DelayDistribution>(v.clone()).ok()).map_or(0, |d| d.sample()),
    };
    fixed + random
}

pub(crate) async fn prepare(st: &ServerState, m: &Matched, req: &MockRequest) -> Prepared {
    let def = &m.stub.response;
    let mut definition = def.raw.clone();
    if let Value::Object(o) = &mut definition {
        o.insert("fromConfiguredStub".into(), Value::Bool(true));
    }
    let data_spec = def.transformer_parameters.get("dataSource").filter(|v| !v.is_null());
    let mut model = (st.config.global_templating || def.templated() || data_spec.is_some()).then(|| template::model(req, &m.path_params, &def.transformer_parameters));
    // `transformerParameters.dataSource`: rows go into the model as `data` / `row`.
    let mut status = def.status;
    if let (Some(spec), Some(md)) = (data_spec, model.as_mut()) {
        let looked_up = {
            let snapshot: &Value = md;
            st.data.lookup_parameters(spec, &|s: &str| st.templates.render(s, snapshot))
        };
        match looked_up {
            Ok((rows, not_found)) => {
                if rows.is_empty()
                    && let Some(nf) = not_found
                {
                    status = nf;
                }
                md["row"] = rows.first().cloned().map_or(Value::Null, Value::Object);
                md["data"] = Value::Array(rows.into_iter().map(Value::Object).collect());
            }
            Err(e) => return Prepared::error(&format!("data source: {e}"), definition),
        }
    }
    let render = |s: &str| -> Result<String, String> {
        match &model {
            Some(md) => st.templates.render(s, md),
            None => Ok(s.to_string()),
        }
    };
    let mut headers = Vec::with_capacity(def.headers.len());
    for (k, v) in &def.headers {
        if k.eq_ignore_ascii_case("content-length") || k.eq_ignore_ascii_case("transfer-encoding") {
            continue;
        }
        match render(v) {
            Ok(v) => headers.push((k.clone(), v)),
            Err(e) => return Prepared::error(&e, definition),
        }
    }
    let delay_ms = delay_for(st, def.fixed_delay_ms, def.delay.as_ref());
    let base = Prepared {
        status,
        reason: def.status_message.clone(),
        headers,
        body: Bytes::new(),
        fault: def.fault,
        delay_ms,
        dribble: def.dribble,
        response_definition: definition.clone(),
        proxied_from: None,
    };
    if def.fault.is_some() {
        return base;
    }

    if let Some(spec) = &def.proxy {
        let base_url = match render(&spec.base_url) {
            Ok(u) => u,
            Err(e) => return Prepared::error(&e, definition),
        };
        let mut extra = Vec::new();
        for (k, v) in &spec.additional_headers {
            match render(v) {
                Ok(v) => extra.push((k.clone(), v)),
                Err(e) => return Prepared::error(&e, definition),
            }
        }
        return match proxy::forward(&st.http, req, &base_url, spec.prefix_to_remove.as_deref(), &extra, &spec.remove_headers).await {
            Ok(resp) => {
                let mut headers = resp.headers;
                // Headers configured on the stub override the upstream ones.
                for (k, _) in &base.headers {
                    headers.retain(|(hk, _)| !hk.eq_ignore_ascii_case(k));
                }
                headers.extend(base.headers.iter().cloned());
                Prepared { status: resp.status, reason: None, headers, body: resp.body, proxied_from: Some(base_url), ..base }
            }
            Err(e) => {
                let mut p = Prepared::error(&format!("Network failure trying to make a proxied request from /dev/all to {base_url}: {e}"), definition);
                p.proxied_from = Some(base_url);
                p
            }
        };
    }

    let body: Result<Bytes, String> = match &def.body {
        BodySpec::Empty => Ok(Bytes::new()),
        BodySpec::Text(t) => render(t).map(Bytes::from),
        BodySpec::Json(v) => render(&v.to_string()).map(Bytes::from),
        BodySpec::Binary(b) => Ok(b.clone()),
        BodySpec::File(name) => match render(name) {
            Err(e) => Err(e),
            Ok(name) => match st.files.read_body_file(&name) {
                Err(e) => Err(format!("body file {name} could not be read: {e}")),
                Ok(bytes) => match (&model, String::from_utf8(bytes)) {
                    (Some(_), Ok(text)) => render(&text).map(Bytes::from),
                    (_, Ok(text)) => Ok(Bytes::from(text)),
                    (_, Err(e)) => Ok(Bytes::from(e.into_bytes())),
                },
            },
        },
    };
    match body {
        Ok(body) => Prepared { body, ..base },
        Err(e) => Prepared::error(&e, definition),
    }
}

/// WireMock-style 404 with a plain-text diff against the closest stub.
pub(crate) fn not_found(st: &ServerState, req: &MockRequest) -> Prepared {
    let mut out = String::from("\n                                               Request was not matched\n                                               =======================\n\n");
    let closest = {
        let store = read(&st.store);
        store
            .all()
            .iter()
            .map(|s| (s.clone(), s.request.evaluate(req)))
            .min_by(|a, b| a.1.distance.partial_cmp(&b.1.distance).unwrap_or(std::cmp::Ordering::Equal))
    };
    match closest {
        None => out.push_str("No stub mappings exist in this server.\n"),
        Some((stub, result)) => {
            let label = stub.name.clone().unwrap_or_else(|| stub.summary());
            out.push_str(&format!("Closest stub: {label} ({})\n\n", stub.id));
            out.push_str(&format!("{:<24} {:<40} {}\n", "Field", "Expected", "Actual"));
            out.push_str(&format!("{}\n", "-".repeat(100)));
            for f in &result.fields {
                let actual = f.actual.clone().unwrap_or_else(|| "(absent)".into());
                let marker = if f.matched { "" } else { "   <<<<< does not match" };
                out.push_str(&format!("{:<24} {:<40} {}{marker}\n", f.field, one_line(&f.expected), one_line(&actual)));
            }
            if result.matched
                && let (Some(name), Some(required)) = (&stub.scenario_name, &stub.required_state)
            {
                let current = read(&st.store).scenario_state(name).unwrap_or("Started").to_string();
                out.push_str(&format!("{:<24} {:<40} {current}   <<<<< scenario is in a different state\n", format!("Scenario {name}"), required));
            }
        }
    }
    let mut p = Prepared::simple(404, "text/plain", out);
    p.response_definition = json!({ "status": 404, "fromConfiguredStub": false });
    p
}

fn one_line(s: &str) -> String {
    let flat: String = s.chars().map(|c| if c == '\n' || c == '\r' { ' ' } else { c }).collect();
    if flat.chars().count() > 60 { format!("{}…", flat.chars().take(59).collect::<String>()) } else { flat }
}
