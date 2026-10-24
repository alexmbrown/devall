//! Import → apply into a temp project → read back through devall-core, for every format.

mod common;

use common::*;
use devall_core::{ApiKeyLocation, AssertOp, AssertTarget, Auth, Body, CaptureSource, OAuth2Grant};
use devall_import::{Format, ParseOptions, detect_path, parse_path};

#[test]
fn postman_collection() {
    let path = fixture("postman/acme.postman_collection.json");
    assert_eq!(detect_path(&path), Some(Format::Postman));
    let t = import(&path, &ParseOptions::default());
    let r = &t.report;
    assert_eq!(r.folder, "acme-store-api");
    assert_eq!(r.requests_created, 8);
    assert_eq!(r.folders_created, 4, "wrapper + Auth + Products + Reviews");

    // Collection settings → wrapper folder; collection variables → project (apiKey became a secret).
    let root = t.project.read_folder("acme-store-api").unwrap();
    assert_eq!(root.auth, Auth::Bearer { token: "{{accessToken}}".into(), prefix: None });
    assert_eq!(t.project.manifest.variables.get("baseUrl").map(String::as_str), Some("https://api.acme.test/v1"));
    assert!(!t.project.manifest.variables.contains_key("apiKey"));
    assert!(!t.project.manifest.variables.contains_key("unused"));
    let products = t.project.read_folder("acme-store-api/products").unwrap();
    assert_eq!(products.auth, Auth::ApiKey { key: "X-API-Key".into(), value: "{{secret.apiKey}}".into(), location: ApiKeyLocation::Header });
    assert_eq!(products.variables.get("productId").map(String::as_str), Some("42"), "path variable value");

    let (path, login) = t.request("Login");
    assert_eq!(path, "acme-store-api/auth/login.req.yaml");
    assert_eq!(login.method, "POST");
    assert_eq!(login.url, "{{baseUrl}}/auth/login");
    assert_eq!(login.auth, Auth::None);
    assert!(matches!(&login.body, Body::Json { content } if content.contains("{{password}}")));
    assert_eq!(login.tests.len(), 1);
    assert_eq!(login.tests[0].target, AssertTarget::Status);
    assert_eq!(login.captures[0].name, "accessToken");
    assert_eq!(login.captures[0].source, CaptureSource::JsonPath { path: "$.access_token".into() });
    let docs = login.docs.unwrap();
    assert!(docs.starts_with("Exchanges credentials") && docs.contains("pm.environment.set"), "{docs}");

    let (_, form) = t.request("Login (form)");
    assert_eq!(form.auth, Auth::Basic { username: "admin".into(), password: "{{secret.PASSWORD}}".into() });
    let Body::Form { fields } = &form.body else { panic!("{:?}", form.body) };
    assert!(fields[0].enabled && !fields[1].enabled);

    let (path, reviews) = t.request("List reviews");
    assert_eq!(path, "acme-store-api/products/reviews/list-reviews.req.yaml");
    assert_eq!(reviews.url, "{{baseUrl}}/products/{{productId}}/reviews");
    assert_eq!(reviews.params.len(), 2);
    assert!(reviews.params[0].enabled && !reviews.params[1].enabled);

    let (_, upload) = t.request("Upload product image");
    let Body::Multipart { parts } = &upload.body else { panic!() };
    assert_eq!(parts[0].file.as_deref(), Some("/Users/jane/Pictures/shoe.png"));
    assert_eq!(parts[1].value, "Red shoe");

    let (_, gql) = t.request("Search products (GraphQL)");
    assert!(matches!(&gql.body, Body::Graphql { query, variables, .. } if query.contains("products(q: $q)") && variables.contains("shoe")));

    let (_, health) = t.request("Health check");
    assert!(!health.settings.follow_redirects);
    assert_eq!(health.url, "https://status.acme.test/health");

    let (_, s3) = t.request("Signed S3 upload");
    let Auth::AwsSigV4 { secret_access_key, region, .. } = &s3.auth else { panic!("{:?}", s3.auth) };
    assert_eq!(secret_access_key, "{{secret.AWS_SECRET_ACCESS_KEY}}");
    assert_eq!(region, "eu-west-1");

    let (_, ntlm) = t.request("Legacy NTLM endpoint");
    let Auth::Ntlm { username, password, domain, workstation } = &ntlm.auth else { panic!("{:?}", ntlm.auth) };
    assert_eq!((username.as_str(), domain, workstation.as_deref()), ("CORP\\svc-legacy", &None, Some("BUILD-01")));
    assert!(password.starts_with("{{secret.PASSWORD"), "{password}");

    // Secrets come back to the caller and never touch the disk.
    let secrets: Vec<(&str, &str)> = r.secrets.iter().map(|s| (s.name.as_str(), s.value.as_str())).collect();
    assert!(secrets.contains(&("apiKey", "ak_live_51Habcdef0123456789")), "{secrets:?}");
    assert!(secrets.contains(&("PASSWORD", "S3cr3t!pass")), "{secrets:?}");
    assert!(secrets.contains(&("AWS_SECRET_ACCESS_KEY", "wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY")), "{secrets:?}");
    assert!(r.secrets.iter().all(|s| s.environment == "*"));
    assert!(secrets.iter().any(|(_, v)| *v == "Legacy#Pass1"), "{secrets:?}");
    t.assert_not_on_disk(&["ak_live_51Habcdef0123456789", "S3cr3t!pass", "wJalrXUtnFEMI", "Legacy#Pass1"]);

    let warnings = r.warnings.join("\n");
    assert!(!warnings.contains("ntlm"), "NTLM is supported: {warnings}");
    assert!(warnings.contains("scripts"), "{warnings}");
    assert!(warnings.contains("original machine"), "{warnings}");
}

#[test]
fn postman_environment() {
    let t = import(&fixture("postman/staging.postman_environment.json"), &ParseOptions::default());
    assert_eq!(t.report.requests_created, 0);
    assert_eq!(t.report.folders_created, 0, "no empty wrapper folder for environment-only imports");
    let env = t.env("Acme Staging");
    assert_eq!(env.file.variables.get("baseUrl").map(String::as_str), Some("https://staging.acme.test/v1"));
    assert!(!env.file.variables.contains_key("password") && !env.file.variables.contains_key("old"));
    assert!(env.file.secrets.contains(&"password".to_string()) && env.file.secrets.contains(&"accessToken".to_string()));
    let pw = t.report.secrets.iter().find(|s| s.name == "password").unwrap();
    assert_eq!((pw.environment.as_str(), pw.value.as_str()), (env.id.as_str(), "hunter2"));
    t.assert_not_on_disk(&["hunter2"]);
}

#[test]
fn insomnia_v4() {
    let t = import(&fixture("insomnia/weather.insomnia.json"), &ParseOptions::default());
    let r = &t.report;
    assert_eq!(r.folder, "weather-api");
    assert_eq!(r.requests_created, 4, "websocket request skipped");
    // Base environment → wrapper folder variables (nested keys flattened).
    let root = t.project.read_folder("weather-api").unwrap();
    assert_eq!(root.variables.get("api.version").map(String::as_str), Some("v2"));
    // Ordering by metaSortKey: Forecasts, Upload station data, Token.
    let order = t.project.read_folder("weather-api").unwrap().order;
    assert_eq!(order, vec!["forecasts", "upload-station-data.req.yaml", "token.req.yaml"]);

    let forecasts = t.project.read_folder("weather-api/forecasts").unwrap();
    assert_eq!(forecasts.variables.get("units").map(String::as_str), Some("metric"));
    assert_eq!(forecasts.variables.get("city").map(String::as_str), Some("Paris"));
    assert_eq!(forecasts.order, vec!["daily-forecast.req.yaml", "create-alert.req.yaml"]);

    let (_, daily) = t.request("Daily forecast");
    assert_eq!(daily.url, "{{base_url}}/{{api.version}}/forecast/{{city}}");
    assert_eq!(daily.auth, Auth::Bearer { token: "{{secret.token}}".into(), prefix: None });
    assert_eq!(daily.params[1].value, "{{units}}");
    assert!(!daily.params[1].enabled);

    let (_, alert) = t.request("Create alert");
    assert!(matches!(&alert.body, Body::Json { content } if content.contains("{{$uuid}}") && content.contains("{{$isoTimestamp}}")), "{:?}", alert.body);
    assert_eq!(alert.auth, Auth::ApiKey { key: "X-Api-Key".into(), value: "{{secret.X_API_KEY}}".into(), location: ApiKeyLocation::Header });

    let (_, upload) = t.request("Upload station data");
    assert!(matches!(&upload.body, Body::Multipart { parts } if parts[0].file.as_deref() == Some("C:\\data\\station.csv") && parts[1].value == "PAR-01"));

    let prod = t.env("Production");
    assert_eq!(prod.file.color.as_deref(), Some("#ff0000"));
    assert!(prod.file.secrets.contains(&"token".to_string()));
    assert!(r.secrets.iter().any(|s| s.name == "token" && s.environment == prod.id && s.value.starts_with("eyJ")));
    assert!(r.warnings.iter().any(|w| w.contains("WebSocket")));
    t.assert_not_on_disk(&["dozjgNryP4J3jVmNHl0w5N", "wk_9f8e7d6c5b4a3210fedcba98"]);
}

#[test]
fn insomnia_v5_yaml() {
    let t = import(&fixture("insomnia/books.insomnia.yaml"), &ParseOptions::default());
    assert_eq!(t.report.requests_created, 3);
    let (path, list) = t.request("List books");
    assert_eq!(path, "books-api/books/list-books.req.yaml");
    assert_eq!(list.url, "{{baseUrl}}/books");
    assert!(!list.settings.follow_redirects);
    assert_eq!(list.docs.as_deref(), Some("Returns every book."));
    let (_, add) = t.request("Add book");
    assert!(matches!(&add.body, Body::Json { content } if content.contains("Dune")));
    assert!(add.docs.unwrap().contains("insomnia.test"));
    let staging = t.env("Staging");
    assert_eq!(staging.file.variables.get("baseUrl").map(String::as_str), Some("https://books.staging.test"));
    assert!(t.report.secrets.iter().any(|s| s.name == "token" && s.value == "books-staging-token-123"));
    assert_eq!(t.project.read_folder("books-api").unwrap().variables.get("baseUrl").map(String::as_str), Some("http://localhost:4000"));
}

#[test]
fn bruno_folder() {
    let dir = fixture("bruno/pet-clinic");
    assert_eq!(detect_path(&dir), Some(Format::Bruno));
    let t = import(&dir, &ParseOptions::default());
    assert_eq!(t.report.folder, "pet-clinic");
    assert_eq!(t.report.requests_created, 6);
    let root = t.project.read_folder("pet-clinic").unwrap();
    assert_eq!(root.headers[0].name, "X-Client");
    assert_eq!(root.auth, Auth::Bearer { token: "{{secret.token}}".into(), prefix: None });
    assert_eq!(root.variables.get("apiVersion").map(String::as_str), Some("v3"));
    // seq order: Ping (1), Owners (2), Upload photo (3), then vets (no folder.bru).
    assert_eq!(root.order, vec!["ping.req.yaml", "owners", "upload-photo.req.yaml", "vets"]);
    let owners = t.project.read_folder("pet-clinic/owners").unwrap();
    assert_eq!(owners.order, vec!["list-owners.req.yaml", "create-owner.req.yaml", "get-owner.req.yaml"]);
    assert_eq!(owners.variables.get("ownerId").map(String::as_str), Some("1"));

    let (_, list) = t.request("List owners");
    assert_eq!(list.url, "{{baseUrl}}/api/owners");
    assert_eq!(list.params.len(), 2);
    assert_eq!(list.tests.len(), 2);
    assert_eq!(list.tests[1].op, AssertOp::IsType);
    let (_, create) = t.request("Create owner");
    assert!(matches!(&create.body, Body::Json { content } if content.contains("\"city\": \"Madison\"")), "{:?}", create.body);
    assert_eq!(create.captures[0].source, CaptureSource::JsonPath { path: "$.id".into() });
    assert!(create.docs.unwrap().contains("X-Request-Id"));
    let (_, get) = t.request("Get owner");
    assert_eq!(get.url, "{{baseUrl}}/api/owners/{{ownerId}}");
    let (_, ping) = t.request("Ping");
    assert!(!ping.settings.follow_redirects);
    let (_, vets) = t.request("List vets");
    assert_eq!(vets.auth, Auth::None);
    let (_, upload) = t.request("Upload photo");
    assert_eq!(upload.auth, Auth::Basic { username: "admin".into(), password: "{{secret.adminPassword}}".into() });
    assert!(matches!(&upload.body, Body::Multipart { parts } if parts[0].file.as_deref() == Some("photos/rex.jpg") && parts[0].content_type.as_deref() == Some("image/jpeg")));

    let local = t.env("Local (2)"); // the new project already has a "Local"
    assert_eq!(local.file.variables.get("baseUrl").map(String::as_str), Some("http://localhost:9966/petclinic"));
    assert!(!local.file.variables.contains_key("token"), "secret-looking variable lifted");
    assert!(local.file.secrets.contains(&"adminPassword".to_string()));
    let prod = t.env("Production");
    assert!(prod.file.secrets.contains(&"token".to_string()));
    assert!(t.report.secrets.iter().any(|s| s.name == "token" && s.environment == local.id && s.value == "local-dev-token"));
    assert!(t.report.secrets.iter().any(|s| s.name == "adminPassword" && s.value.is_empty()), "declared-only secrets are reported without value");
    t.assert_not_on_disk(&["local-dev-token"]);
}

#[test]
fn opencollection_folder() {
    let dir = fixture("opencollection/shop");
    assert_eq!(detect_path(&dir), Some(Format::OpenCollection));
    let t = import(&dir, &ParseOptions::default());
    assert_eq!(t.report.requests_created, 3);
    let root = t.project.read_folder("shop-api").unwrap();
    assert_eq!(root.headers[0].name, "X-Shop");
    assert_eq!(root.auth, Auth::Bearer { token: "{{secret.token}}".into(), prefix: None });
    assert_eq!(root.order, vec!["orders", "health.req.yaml"]);
    let (_, create) = t.request("Create order");
    assert_eq!(create.url, "{{baseUrl}}/shops/{{shop}}/orders");
    assert_eq!(create.settings.timeout_ms, 5000);
    assert_eq!(create.tests.len(), 2);
    assert_eq!(create.captures[0].name, "orderId");
    assert_eq!(create.docs.as_deref().map(|d| d.starts_with("Places an order.")), Some(true));
    let (_, list) = t.request("List orders");
    assert_eq!(list.params[0].name, "status");
    let (_, health) = t.request("Health");
    assert_eq!(health.auth, Auth::ApiKey { key: "X-Probe".into(), value: "{{secret.X_PROBE}}".into(), location: ApiKeyLocation::Header });
    let dev = t.env("Dev");
    assert_eq!(dev.file.color.as_deref(), Some("green"));
    assert!(dev.file.secrets.contains(&"token".to_string()));
}

#[test]
fn openapi_3_0_petstore() {
    let t = import(&fixture("openapi/petstore-3.0.yaml"), &ParseOptions::default());
    let r = &t.report;
    assert_eq!(r.folder, "swagger-petstore");
    assert_eq!(r.requests_created, 10);
    let root = t.project.read_folder("swagger-petstore").unwrap();
    assert_eq!(root.auth, Auth::ApiKey { key: "api_key".into(), value: "{{secret.API_KEY}}".into(), location: ApiKeyLocation::Header });
    assert_eq!(root.order, vec!["pet", "store", "user", "health.req.yaml"]);
    let pet = t.project.read_folder("swagger-petstore/pet").unwrap();
    assert_eq!(pet.variables.get("petId").map(String::as_str), Some("10"));

    let (_, add) = t.request("Add a new pet to the store");
    let Body::Json { content } = &add.body else { panic!("{:?}", add.body) };
    let body: serde_json::Value = serde_json::from_str(content).unwrap();
    assert_eq!(body["name"], "doggie");
    assert!(body.get("id").is_none(), "readOnly skipped");
    assert_eq!(body["category"]["name"], "Dogs");
    assert_eq!(body["category"]["parent"], serde_json::json!({}), "cycle cut");
    assert_eq!(body["photoUrls"][0], "https://example.com");
    assert_eq!(body["status"], "available");
    assert_eq!(add.auth, Auth::Inherit);

    let (_, update) = t.request("Update an existing pet");
    let Auth::OAuth2(cfg) = &update.auth else { panic!("{:?}", update.auth) };
    assert_eq!(cfg.grant, OAuth2Grant::ClientCredentials);
    assert_eq!(cfg.token_url, "https://petstore.test/oauth/token");
    assert_eq!(cfg.scope, "write:pets read:pets");
    assert_eq!(cfg.client_secret, "{{secret.PETSTORE_AUTH_CLIENT_SECRET}}");

    let (_, find) = t.request("Finds Pets by status");
    assert_eq!((find.params[0].name.as_str(), find.params[0].value.as_str(), find.params[0].enabled), ("status", "available", true));
    assert_eq!((find.params[1].name.as_str(), find.params[1].value.as_str(), find.params[1].enabled), ("limit", "20", false));

    let (_, get) = t.request("Find pet by ID");
    assert_eq!(get.url, "{{baseUrl}}/pet/{{petId}}");
    assert!(get.docs.as_deref().unwrap().contains("Deprecated"));
    assert_eq!(get.headers[0].name, "X-Request-ID");
    assert!(!get.headers[0].enabled);

    let (_, form) = t.request("Updates a pet in the store with form data");
    let Body::Form { fields } = &form.body else { panic!() };
    assert_eq!((fields[0].name.as_str(), fields[0].value.as_str(), fields[0].enabled), ("name", "doggie", true));
    assert!(!fields[1].enabled);

    let (_, delete) = t.request("Deletes a pet");
    assert_eq!(delete.auth, Auth::None);

    let (_, upload) = t.request("uploads an image");
    assert_eq!(upload.url, "{{baseUrl}}/pet/{{petId}}/uploadImage");
    let Body::Multipart { parts } = &upload.body else { panic!() };
    assert_eq!(parts[0].file.as_deref(), Some(""));
    assert_eq!(parts[0].content_type.as_deref(), Some("image/png"));

    let (_, order) = t.request("Place an order for a pet");
    let Body::Json { content } = &order.body else { panic!() };
    assert_eq!(serde_json::from_str::<serde_json::Value>(content).unwrap()["quantity"], 2);

    let (_, login) = t.request("Logs user into the system");
    assert_eq!(login.auth, Auth::Basic { username: "{{secret.BASIC_AUTH_USERNAME}}".into(), password: "{{secret.BASIC_AUTH_PASSWORD}}".into() });

    let (path, health) = t.request("health");
    assert_eq!(path, "swagger-petstore/health.req.yaml");
    assert_eq!(health.auth, Auth::None);

    let prod = t.env("Production");
    assert_eq!(prod.file.variables.get("baseUrl").map(String::as_str), Some("https://api.petstore.test/api/v3"));
    assert!(prod.file.secrets.contains(&"API_KEY".to_string()));
    assert_eq!(t.env("Local (2)").file.variables.get("baseUrl").map(String::as_str), Some("http://localhost:8080/api/v3"));
}

#[test]
fn openapi_3_1_refs_and_types() {
    let t = import(&fixture("openapi/graph-3.1.json"), &ParseOptions::default());
    assert_eq!(t.report.requests_created, 3);
    let (_, create) = t.request("Create node");
    let Body::Json { content } = &create.body else { panic!() };
    let body: serde_json::Value = serde_json::from_str(content).unwrap();
    assert_eq!(body["name"], "Platform", "schema `examples` array");
    assert_eq!(body["kind"], "team");
    assert_eq!(body["budget"], 0.5, "type array + minimum");
    assert_eq!(body["owner"], "user@example.com");
    assert_eq!(body["children"], serde_json::json!([{}]), "recursive items cut");
    assert_eq!(body["meta"], serde_json::json!({ "key": 0 }));
    assert!(create.headers.iter().any(|h| h.name == "Content-Type" && h.value == "application/merge-patch+json"));
    let (_, list) = t.request("listNodes");
    assert_eq!((list.params[0].value.as_str(), list.params[0].enabled), ("team", false));
    assert_eq!((list.params[1].value.as_str(), list.params[1].enabled), ("core", true));
    assert!(list.headers.iter().any(|h| h.name == "Cookie" && !h.enabled));
    let (_, children) = t.request("List children");
    assert_eq!(children.url, "{{baseUrl}}/nodes/{{nodeId}}/children");
    let root = t.project.read_folder("org-graph").unwrap();
    assert_eq!(root.auth, Auth::Bearer { token: "{{secret.BEARER}}".into(), prefix: None });
    assert!(!root.variables.contains_key("nodeId"), "no made-up path values");
    let warnings = t.report.warnings.join("\n");
    assert!(warnings.contains("relative") && warnings.contains("Webhooks"), "{warnings}");
    let env = t.env("localhost");
    assert_eq!(env.file.variables.get("baseUrl").map(String::as_str), Some("http://localhost/api"));
}

#[test]
fn swagger_2() {
    let t = import(&fixture("openapi/petstore-swagger-2.0.json"), &ParseOptions::default());
    assert_eq!(t.report.requests_created, 3);
    let root = t.project.read_folder("petstore-swagger-2").unwrap();
    let Auth::OAuth2(cfg) = &root.auth else { panic!("{:?}", root.auth) };
    assert_eq!(cfg.grant, OAuth2Grant::AuthorizationCode);
    assert_eq!(cfg.scope, "read:pets");
    let (_, add) = t.request("Add a new pet");
    let Body::Json { content } = &add.body else { panic!() };
    let body: serde_json::Value = serde_json::from_str(content).unwrap();
    assert_eq!(body["name"], "doggie");
    assert!(body.get("id").is_none());
    let (_, upload) = t.request("uploads an image");
    assert_eq!(upload.auth, Auth::ApiKey { key: "api_key".into(), value: "{{secret.API_KEY}}".into(), location: ApiKeyLocation::Query });
    assert!(matches!(&upload.body, Body::Multipart { parts } if parts.len() == 2 && parts[1].file.is_some()));
    let (_, tags) = t.request("Finds Pets by tags");
    assert_eq!(tags.params[0].value, "a,b");
    assert_eq!(t.env("petstore.swagger.test").file.variables.get("baseUrl").map(String::as_str), Some("https://petstore.swagger.test/v2"));
    assert_eq!(t.project.read_folder("petstore-swagger-2/pet").unwrap().variables.get("petId").map(String::as_str), Some("5"));
}

#[test]
fn har_capture() {
    let t = import(&fixture("har/app.acme.test.har"), &ParseOptions::default());
    let r = &t.report;
    // dashboard document, users GET (deduplicated), users POST, files upload, oauth token.
    assert_eq!(r.requests_created, 5, "{:?}", t.requests().iter().map(|(p, _)| p.clone()).collect::<Vec<_>>());
    let (path, users) = t.request("GET /v1/users");
    assert!(path.starts_with("har-import/api-acme-test/"), "{path}");
    assert_eq!(users.url, "https://api.acme.test/v1/users");
    assert_eq!(users.params.len(), 2);
    let names: Vec<&str> = users.headers.iter().map(|h| h.name.as_str()).collect();
    assert_eq!(names, vec!["accept", "authorization", "origin", "x-request-id"], "browser noise dropped");
    assert_eq!(users.headers[1].value, "Bearer {{secret.TOKEN}}");
    let (_, create) = t.request("POST /v1/users");
    assert!(matches!(&create.body, Body::Json { content } if content.contains("\n  \"name\": \"Grace Hopper\"")), "pretty-printed");
    let (_, files) = t.request("POST /v1/files");
    assert!(matches!(&files.body, Body::Multipart { parts } if parts[1].file.as_deref() == Some("q3.pdf")));
    assert!(!files.headers.iter().any(|h| h.name.eq_ignore_ascii_case("content-type")));
    let (_, token) = t.request("POST /oauth/token");
    let Body::Form { fields } = &token.body else { panic!() };
    assert_eq!(fields[1].name, "refresh_token");
    let (_, dash) = t.request("GET /dashboard");
    assert!(!dash.headers.iter().any(|h| h.name.eq_ignore_ascii_case("cookie")), "cookies dropped by default");
    let warnings = r.warnings.join("\n");
    assert!(warnings.contains("static asset") && warnings.contains("preflight") && warnings.contains("duplicate"), "{warnings}");
    t.assert_not_on_disk(&["c2lnbmF0dXJlLWJ5dGVz"]);

    // Opt-in: cookies kept (and lifted as secrets), assets kept.
    let opts = ParseOptions { include_cookies: true, include_assets: true };
    let t = import(&fixture("har/app.acme.test.har"), &opts);
    assert_eq!(t.report.requests_created, 8);
    let (_, dash) = t.request("GET /dashboard");
    assert_eq!(dash.headers.iter().find(|h| h.name == "cookie").map(|h| h.value.as_str()), Some("{{secret.COOKIE}}"));
    t.assert_not_on_disk(&["abc123def456ghi789"]);
}

#[test]
fn http_file_with_env_files() {
    let t = import(&fixture("http/users.http"), &ParseOptions::default());
    assert_eq!(t.report.requests_created, 6);
    let root = t.project.read_folder("users").unwrap();
    assert_eq!(root.variables.get("apiVersion").map(String::as_str), Some("v2"));
    assert_eq!(root.variables.get("region").map(String::as_str), Some("eu"), "$shared env");
    let (_, list) = t.request("List users");
    assert_eq!(list.url, "{{host}}/{{apiVersion}}/users");
    assert_eq!(list.params.len(), 2);
    assert_eq!(list.docs.as_deref(), Some("Returns a page of users."));
    assert_eq!(list.headers[1].value, "Bearer {{secret.token}}");
    let (_, create) = t.request("createUser");
    assert!(matches!(&create.body, Body::Json { content } if content.contains("{{$uuid}}") && !content.contains("client.test")));
    let (_, avatar) = t.request("Update avatar");
    assert!(!avatar.settings.follow_redirects);
    assert_eq!(avatar.settings.timeout_ms, 15_000);
    assert!(matches!(&avatar.body, Body::Multipart { parts } if parts[0].file.as_deref() == Some("./images/ada.png") && parts[1].value == "square"));
    let (_, login) = t.request("Login (form)");
    assert!(matches!(&login.body, Body::Form { fields } if fields[1].value == "{{secret.ADMIN_PASSWORD}}"), "{:?}", login.body);
    let (_, csv) = t.request("Import CSV");
    assert_eq!(csv.body, Body::Binary { path: "./data/users.csv".into(), content_type: Some("text/csv".into()) });
    let (_, me) = t.request("Me (GraphQL)");
    assert_eq!(me.method, "POST");
    assert!(matches!(&me.body, Body::Graphql { variables, .. } if variables == "{\"withPosts\": false}"));

    let dev = t.env("dev");
    assert_eq!(dev.file.variables.get("host").map(String::as_str), Some("http://localhost:8080"));
    assert!(dev.file.secrets.contains(&"token".to_string()));
    assert!(t.report.secrets.iter().any(|s| s.name == "token" && s.environment == dev.id && s.value == "dev-token-1234"));
    let prod = t.env("prod");
    assert!(!prod.file.variables.contains_key("SSLConfiguration"));
    t.assert_not_on_disk(&["dev-token-1234", "prod-token-9876"]);
}

#[test]
fn hurl_file() {
    let t = import(&fixture("hurl/checkout.hurl"), &ParseOptions::default());
    assert_eq!(t.report.requests_created, 4);
    let reqs = t.requests();
    let login = &reqs[0].1;
    assert_eq!(login.url, "https://shop.example.org/api/login");
    assert!(matches!(&login.body, Body::Json { content } if content.contains("\"user\": \"bob\"")));
    assert_eq!(login.captures.len(), 2);
    assert_eq!(login.captures[1].source, CaptureSource::Header { header: "X-CSRF-Token".into() });
    assert!(login.tests.iter().any(|a| a.target == AssertTarget::Status && a.value.as_deref() == Some("200")));
    assert!(login.tests.iter().any(|a| a.target == AssertTarget::ResponseTime));
    assert!(login.docs.as_deref().unwrap().contains("count == 2"));
    let cart = &reqs[1].1;
    assert!(cart.settings.follow_redirects, "location: true");
    assert!(matches!(&cart.body, Body::Form { fields } if fields.len() == 2));
    let receipt = &reqs[2].1;
    assert!(!receipt.settings.follow_redirects, "Hurl default");
    assert!(matches!(&receipt.body, Body::Multipart { parts } if parts[1].file.as_deref() == Some("receipt.pdf") && parts[1].content_type.as_deref() == Some("application/pdf")));
    assert_eq!(receipt.auth, Auth::Basic { username: "bob".into(), password: "{{password}}".into() });
    let search = &reqs[3].1;
    assert_eq!(search.params.len(), 2);
    assert_eq!(search.headers[0].value, "{{secret.COOKIE}}", "Cookie headers are treated as credentials");
    assert!(search.tests.iter().any(|a| a.target == AssertTarget::ContentType && a.op == AssertOp::Contains));
}

#[test]
fn curl_commands() {
    let t = import(&fixture("curl/commands.sh"), &ParseOptions::default());
    assert_eq!(t.report.requests_created, 3);
    let reqs = t.requests();
    assert_eq!(reqs[0].1.headers[1].value, "Bearer {{secret.TOKEN}}");
    assert!(matches!(&reqs[1].1.body, Body::Json { content } if content.contains("Grace")));
    assert_eq!(reqs[2].1.auth, Auth::Basic { username: "admin".into(), password: "{{secret.PASSWORD}}".into() });
    t.assert_not_on_disk(&["pa55word", "abc.def.ghi"]);
}

#[test]
fn previews_list_secret_names_but_never_values() {
    let plan = parse_path(&fixture("postman/acme.postman_collection.json"), &ParseOptions::default()).unwrap();
    let summary = plan.summary();
    assert_eq!(summary.request_count, 8);
    assert_eq!(summary.format, Format::Postman);
    assert!(summary.secrets.iter().any(|s| s.name == "apiKey" && s.has_value));
    let json = serde_json::to_string(&summary).unwrap();
    assert!(!json.contains("ak_live_51Habcdef0123456789") && !json.contains("S3cr3t!pass"), "{json}");
}
