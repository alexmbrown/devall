//! `http://127.0.0.1:{port}` style templates for bound environment variables.

pub const DEFAULT_TEMPLATE: &str = "http://127.0.0.1:{port}";

#[derive(Debug, Clone, Default)]
pub struct TemplateCtx<'a> {
    /// Local port the forward listens on (or the container's published port).
    pub port: u16,
    pub host: &'a str,
    pub remote_port: u16,
    pub name: &'a str,
    pub namespace: &'a str,
    pub target: &'a str,
    pub context: &'a str,
}

/// Replaces `{port}` (alias `{localPort}`), `{host}`, `{remotePort}`, `{name}`, `{namespace}`,
/// `{target}` and `{context}`. `{{…}}` references (the app's own variables/secrets) and unknown
/// placeholders are left untouched, so a template may still reference `{{secret.X}}`.
pub fn render_template(template: &str, ctx: &TemplateCtx<'_>) -> String {
    let template = if template.trim().is_empty() { DEFAULT_TEMPLATE } else { template };
    let mut out = String::with_capacity(template.len() + 8);
    let mut rest = template;
    while let Some(i) = rest.find('{') {
        out.push_str(&rest[..i]);
        let tail = &rest[i..];
        if let Some(inner) = tail.strip_prefix("{{") {
            // Copy a whole `{{…}}` reference verbatim.
            match inner.find("}}") {
                Some(end) => {
                    out.push_str(&tail[..end + 4]);
                    rest = &tail[end + 4..];
                }
                None => {
                    out.push_str(tail);
                    rest = "";
                }
            }
            continue;
        }
        let Some(end) = tail.find('}') else {
            out.push_str(tail);
            rest = "";
            break;
        };
        let key = &tail[1..end];
        let value = match key {
            "port" | "localPort" => Some(ctx.port.to_string()),
            "host" => Some(if ctx.host.is_empty() { "127.0.0.1".to_string() } else { ctx.host.to_string() }),
            "remotePort" => Some(ctx.remote_port.to_string()),
            "name" => Some(ctx.name.to_string()),
            "namespace" => Some(ctx.namespace.to_string()),
            "target" => Some(ctx.target.to_string()),
            "context" => Some(ctx.context.to_string()),
            _ => None,
        };
        match value {
            Some(v) => out.push_str(&v),
            None => out.push_str(&tail[..=end]),
        }
        rest = &tail[end + 1..];
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx() -> TemplateCtx<'static> {
        TemplateCtx { port: 18080, host: "127.0.0.1", remote_port: 8080, name: "orders-api", namespace: "orders", target: "svc/orders-api", context: "dev" }
    }

    #[test]
    fn default_and_placeholders() {
        assert_eq!(render_template("", &ctx()), "http://127.0.0.1:18080");
        assert_eq!(render_template(DEFAULT_TEMPLATE, &ctx()), "http://127.0.0.1:18080");
        assert_eq!(render_template("http://{host}:{localPort}/api/v1", &ctx()), "http://127.0.0.1:18080/api/v1");
        assert_eq!(render_template("grpc://localhost:{port}", &ctx()), "grpc://localhost:18080");
        assert_eq!(render_template("{name}.{namespace}:{remotePort} via {target}@{context}", &ctx()), "orders-api.orders:8080 via svc/orders-api@dev");
    }

    #[test]
    fn leaves_app_references_and_unknowns_alone() {
        assert_eq!(render_template("http://{{host}}:{port}", &ctx()), "http://{{host}}:18080");
        assert_eq!(render_template("http://u:{{secret.PASS}}@{host}:{port}", &ctx()), "http://u:{{secret.PASS}}@127.0.0.1:18080");
        assert_eq!(render_template("http://{host}:{port}/{unknown}", &ctx()), "http://127.0.0.1:18080/{unknown}");
        assert_eq!(render_template("broken {port", &ctx()), "broken {port");
        assert_eq!(render_template("open {{ref", &ctx()), "open {{ref");
        let mut c = ctx();
        c.host = "";
        assert_eq!(render_template("{host}", &c), "127.0.0.1");
    }
}
