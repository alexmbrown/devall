//! The request-path bridge: `auth: { type: plugin }` through `devall_http::prepare` and hooks on a
//! prepared request, with the bridge installed as the process-wide plugin host.

mod common;

use std::sync::Arc;

use common::{Act, module};
use devall_core::{Auth, HttpRequestFile, Resolver};
use devall_http::plugins::RequestPlugins;
use devall_plugins::{HttpBridge, NoServices, PluginManager};

#[test]
fn plugin_auth_and_hooks_through_devall_http() {
    let tmp = tempfile::tempdir().unwrap();
    let project = devall_core::Project::init(tmp.path().join("proj"), "Proj").unwrap();
    let mgr = Arc::new(PluginManager::new(tmp.path().join("app"), Arc::new(NoServices)));
    let src = tmp.path().join("src/signer");
    std::fs::create_dir_all(&src).unwrap();
    std::fs::write(
        src.join("plugin.yaml"),
        "id: signer\nname: Signer\nversion: 1.0.0\napiVersion: 1\ncontributes:\n  auth:\n    - { id: hmac, name: HMAC, fields: [{ name: keyId }] }\n  hooks:\n    - { id: trace, name: Trace, events: [preRequest] }\n",
    )
    .unwrap();
    let wasm = module(&[
        ("auth", Act::Const(r#"{"ok":{"headers":[{"name":"Authorization","value":"HMAC k1:abc"}],"query":[{"name":"sig","value":"abc"}]}}"#.into())),
        ("hook.preRequest", Act::Const(r#"{"ok":{"request":{"method":"GET","url":"https://api.example.com/items?sig=abc","headers":[{"name":"X-Trace","value":"t-1"}]}}}"#.into())),
    ]);
    std::fs::write(src.join("plugin.wasm"), wasm).unwrap();
    mgr.install(&src, None, &[]).unwrap();
    let bridge = Arc::new(HttpBridge::new(mgr.clone()));
    devall_http::plugins::install(bridge.clone());

    let mut req = HttpRequestFile::new("Items");
    req.url = "https://api.example.com/items".into();
    req.auth = Auth::Plugin { plugin: "signer".into(), provider: "hmac".into(), config: [("keyId".to_string(), "{{key}}".to_string())].into_iter().collect() };
    struct NoSecrets;
    impl devall_core::SecretLookup for NoSecrets {
        fn lookup(&self, _: &str) -> Option<String> {
            None
        }
    }
    let mut r = Resolver::new(&NoSecrets);
    r.push_layer([("key".to_string(), "k1".to_string())].into_iter().collect());
    let mut prepared = devall_http::prepare(&req, &mut r, project.root()).unwrap();
    assert_eq!(prepared.header("authorization"), Some("HMAC k1:abc"));
    assert_eq!(prepared.url, "https://api.example.com/items?sig=abc");

    let out = bridge.pre_request(project.root(), &mut prepared, "Items", None);
    assert!(out.errors.is_empty(), "{:?}", out.errors);
    assert_eq!(prepared.header("x-trace"), Some("t-1"));
    assert_eq!(prepared.header("authorization"), None, "the hook replaced the headers");

    // An unknown provider is a clear prepare error.
    req.auth = Auth::Plugin { plugin: "signer".into(), provider: "nope".into(), config: Default::default() };
    let err = devall_http::prepare(&req, &mut r, project.root()).unwrap_err().to_string();
    assert!(err.contains("plugin signer has no auth provider contribution `nope`"), "{err}");
}
