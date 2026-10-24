//! Exporters: build a project, export it, import the export again and compare.

mod common;

use common::*;
use devall_core::{AssertOp, AssertTarget, Assertion, Auth, Body, EnvironmentFile, FolderFile, HttpRequestFile, KeyValue, MultipartPart, Project};
use devall_import::export::{to_http_file, to_openapi, to_postman};
use devall_import::{ApplyOptions, ParseOptions, apply, parse_path, parse_text};

fn kv(name: &str, value: &str, enabled: bool) -> KeyValue {
    let mut k = KeyValue::new(name, value);
    k.enabled = enabled;
    k
}

/// A small but varied project: nested folders, folder auth/headers/variables, every body type.
fn sample_project() -> (tempfile::TempDir, Project) {
    let (dir, mut p) = new_project();
    p.manifest.variables.insert("apiVersion".into(), "v1".into());
    p.save_manifest().unwrap();
    let mut local = p.read_environment("local").unwrap();
    local.variables.insert("baseUrl".into(), "http://localhost:3000".into());
    local.secrets.push("TOKEN".into());
    p.save_environment("local", &local).unwrap();
    let prod_id = p.create_environment("Production").unwrap();
    let mut prod = EnvironmentFile::new("Production");
    prod.variables.insert("baseUrl".into(), "https://api.example.com".into());
    prod.secrets.push("TOKEN".into());
    p.save_environment(&prod_id, &prod).unwrap();

    let users = p.create_folder("", "Users").unwrap();
    p.save_folder(
        &users,
        &FolderFile {
            name: Some("Users".into()),
            headers: vec![KeyValue::new("X-Team", "core")],
            auth: Auth::Bearer { token: "{{secret.TOKEN}}".into(), prefix: None },
            variables: [("userId".to_string(), "42".to_string())].into_iter().collect(),
            ..Default::default()
        },
    )
    .unwrap();
    let admin = p.create_folder(&users, "Admin").unwrap();

    let mut list = HttpRequestFile::new("List users");
    list.url = "{{baseUrl}}/users".into();
    list.params = vec![kv("page", "1", true), kv("sort", "-created", false)];
    list.headers = vec![KeyValue::new("Accept", "application/json"), kv("X-Debug", "1", false)];
    p.create_request(&users, list).unwrap();

    let mut get = HttpRequestFile::new("Get user");
    get.url = "{{baseUrl}}/users/{{userId}}".into();
    get.tests.push(Assertion { target: AssertTarget::Status, op: AssertOp::Eq, value: Some("200".into()), enabled: true });
    p.create_request(&users, get).unwrap();

    let mut create = HttpRequestFile::new("Create user");
    create.method = "POST".into();
    create.url = "{{baseUrl}}/users".into();
    create.body = Body::Json { content: "{\n  \"name\": \"Ada\",\n  \"roles\": [\n    \"admin\"\n  ],\n  \"active\": true\n}".into() };
    create.tests.push(Assertion { target: AssertTarget::Status, op: AssertOp::Eq, value: Some("201".into()), enabled: true });
    p.create_request(&users, create).unwrap();

    let mut ban = HttpRequestFile::new("Ban user");
    ban.method = "PUT".into();
    ban.url = "{{baseUrl}}/admin/users/{{userId}}/ban".into();
    ban.body = Body::Form { fields: vec![KeyValue::new("reason", "spam links"), kv("notify", "true", false)] };
    p.create_request(&admin, ban).unwrap();

    let mut avatar = HttpRequestFile::new("Upload avatar");
    avatar.method = "POST".into();
    avatar.url = "{{baseUrl}}/users/{{userId}}/avatar".into();
    avatar.body = Body::Multipart {
        parts: vec![
            MultipartPart { name: "file".into(), value: String::new(), file: Some("fixtures/ada.png".into()), content_type: Some("image/png".into()), enabled: true },
            MultipartPart { name: "caption".into(), value: "Ada".into(), file: None, content_type: None, enabled: true },
        ],
    };
    p.create_request(&users, avatar).unwrap();

    let mut health = HttpRequestFile::new("Health");
    health.url = "{{baseUrl}}/health".into();
    health.auth = Auth::None;
    health.settings.follow_redirects = false;
    p.create_request("", health).unwrap();

    let mut gql = HttpRequestFile::new("Search");
    gql.method = "POST".into();
    gql.url = "{{baseUrl}}/graphql".into();
    gql.body = Body::Graphql { query: "query { users { id } }".into(), variables: "{\"first\": 10}".into(), operation_name: None };
    p.create_request("", gql).unwrap();
    (dir, p)
}

fn by_name(list: &[(String, HttpRequestFile)], name: &str) -> HttpRequestFile {
    list.iter().find(|(_, r)| r.name == name || r.name.ends_with(&format!(" / {name}"))).map(|(_, r)| r.clone()).unwrap_or_else(|| {
        panic!("no {name:?} in {:?}", list.iter().map(|(_, r)| r.name.clone()).collect::<Vec<_>>())
    })
}

fn json(s: &str) -> serde_json::Value {
    serde_json::from_str(s).unwrap()
}

fn reimport(text: &str) -> (tempfile::TempDir, Project, devall_import::ApplyReport) {
    let plan = parse_text(text, &ParseOptions::default()).unwrap();
    let (dir, mut p) = new_project();
    let report = apply(plan, &mut p, "", &ApplyOptions::default()).unwrap();
    (dir, p, report)
}

#[test]
fn openapi_round_trip() {
    let (_d, p) = sample_project();
    let out = to_openapi(&p, None, None).unwrap();
    assert_eq!(out.file_name, "test-project.openapi.yaml");
    let doc: serde_json::Value = serde_yaml_ng::from_str(&out.content).unwrap();
    assert_eq!(doc["openapi"], "3.1.0");
    assert_eq!(doc["servers"][0]["url"], "http://localhost:3000");
    assert_eq!(doc["servers"][1]["url"], "https://api.example.com");
    assert!(doc["paths"]["/users/{userId}"]["get"].is_object(), "{}", out.content);
    assert_eq!(doc["paths"]["/users/{userId}"]["get"]["parameters"][0]["in"], "path");
    assert_eq!(doc["paths"]["/users"]["post"]["responses"]["201"]["description"], "Successful response");
    assert_eq!(doc["components"]["securitySchemes"]["TOKEN"]["scheme"], "bearer");
    assert_eq!(doc["paths"]["/health"]["get"]["security"], serde_json::json!([]));
    assert!(!out.content.contains("{{secret"), "{}", out.content);

    let (_d2, p2, report) = reimport(&out.content);
    assert_eq!(report.requests_created, 7);
    let before = requests_of(&p);
    let after = requests_of(&p2);
    for (_, orig) in &before {
        let got = by_name(&after, &orig.name);
        assert_eq!(got.method, orig.method, "{}", orig.name);
        assert_eq!(got.url, orig.url, "{}", orig.name);
        let names = |r: &HttpRequestFile| r.params.iter().map(|k| (k.name.clone(), k.value.clone(), k.enabled)).collect::<Vec<_>>();
        assert_eq!(names(&got), names(orig), "{}", orig.name);
    }
    // Tags = top-level folders (nested folders flatten into their top-level tag).
    let ban = after.iter().find(|(_, r)| r.name == "Ban user").unwrap();
    assert!(ban.0.starts_with("test-project/users/"), "{}", ban.0);
    let create = by_name(&after, "Create user");
    let Body::Json { content } = &create.body else { panic!() };
    assert_eq!(json(content), json("{\"name\": \"Ada\", \"roles\": [\"admin\"], \"active\": true}"));
    let list = by_name(&after, "List users");
    assert!(list.headers.iter().any(|h| h.name == "X-Team"), "folder headers become header parameters");
    assert!(list.headers.iter().any(|h| h.name == "X-Debug" && !h.enabled));
    let ban = by_name(&after, "Ban user");
    assert!(matches!(&ban.body, Body::Form { fields } if fields[0].name == "reason" && fields[0].value == "spam links" && fields[0].enabled && !fields[1].enabled));
    let avatar = by_name(&after, "Upload avatar");
    assert!(matches!(&avatar.body, Body::Multipart { parts } if parts[0].file.is_some() && parts[0].content_type.as_deref() == Some("image/png")));
    // Auth survives as a security scheme referencing the same secret name.
    let eff = |p: &Project, list: &[(String, HttpRequestFile)], name: &str| {
        let (path, r) = list.iter().find(|(_, r)| r.name == name).unwrap();
        p.effective_request(path, r).unwrap().request.auth
    };
    assert_eq!(eff(&p2, &after, "Get user"), Auth::Bearer { token: "{{secret.TOKEN}}".into(), prefix: None });
    assert_eq!(eff(&p2, &after, "Health"), Auth::None);
    // Servers → environments with baseUrl.
    let envs = p2.environments().unwrap();
    assert!(envs.iter().any(|e| e.file.variables.get("baseUrl").map(String::as_str) == Some("https://api.example.com")));
}

#[test]
fn openapi_reexport_of_petstore_keeps_operations() {
    let t = import(&fixture("openapi/petstore-3.0.yaml"), &ParseOptions::default());
    let out = to_openapi(&t.project, Some("swagger-petstore"), None).unwrap();
    let doc: serde_json::Value = serde_yaml_ng::from_str(&out.content).unwrap();
    let ops: Vec<String> = doc["paths"]
        .as_object()
        .unwrap()
        .iter()
        .flat_map(|(path, item)| item.as_object().unwrap().keys().map(move |m| format!("{} {path}", m.to_uppercase())))
        .collect();
    for expected in [
        "PUT /pet",
        "POST /pet",
        "GET /pet/findByStatus",
        "GET /pet/{petId}",
        "POST /pet/{petId}",
        "DELETE /pet/{petId}",
        "POST /pet/{petId}/uploadImage",
        "POST /store/order",
        "GET /user/login",
        "GET /health",
    ] {
        assert!(ops.contains(&expected.to_string()), "{expected} missing from {ops:?}");
    }
    assert_eq!(doc["info"]["title"], "Swagger Petstore");
    assert_eq!(doc["tags"].as_array().unwrap().len(), 3);
    assert_eq!(doc["components"]["securitySchemes"]["PETSTORE_AUTH"]["type"], "oauth2");
    // And it imports again.
    let (_d, _p, report) = reimport(&out.content);
    assert_eq!(report.requests_created, 10);
}

#[test]
fn postman_round_trip() {
    let (_d, p) = sample_project();
    let out = to_postman(&p, None).unwrap();
    let doc = json(&out.content);
    assert_eq!(doc["info"]["schema"], "https://schema.getpostman.com/json/collection/v2.1.0/collection.json");
    assert_eq!(doc["item"][0]["name"], "Users");
    assert_eq!(doc["item"][0]["auth"]["type"], "bearer");
    assert_eq!(doc["item"][0]["auth"]["bearer"][0]["value"], "{{TOKEN}}", "secret refs become plain variables");
    assert!(doc["variable"].as_array().unwrap().iter().any(|v| v["key"] == "userId"));
    assert_eq!(out.attachments.len(), 2);
    let env = json(&out.attachments[0].content);
    assert!(env["values"].as_array().unwrap().iter().any(|v| v["key"] == "TOKEN" && v["type"] == "secret" && v["value"] == ""));
    assert!(!out.content.contains("secret."));

    let (_d2, p2, report) = reimport(&out.content);
    assert_eq!(report.requests_created, 7);
    let after = requests_of(&p2);
    let paths: Vec<&str> = after.iter().map(|(p, _)| p.as_str()).collect();
    assert!(paths.contains(&"test-project/users/admin/ban-user.req.yaml"), "nested folders survive: {paths:?}");
    for (_, orig) in requests_of(&p) {
        let got = by_name(&after, &orig.name);
        assert_eq!((got.method.as_str(), got.url.as_str()), (orig.method.as_str(), orig.url.as_str()), "{}", orig.name);
        assert_eq!(got.params, orig.params, "{}", orig.name);
        assert_eq!(got.settings.follow_redirects, orig.settings.follow_redirects, "{}", orig.name);
        match (&orig.body, &got.body) {
            (Body::Json { content: a }, Body::Json { content: b }) => assert_eq!(json(a), json(b)),
            (a, b) => assert_eq!(a, b, "{}", orig.name),
        }
        // Status checks travel as a Postman test script and come back as assertions.
        assert_eq!(got.tests, orig.tests, "{}", orig.name);
    }
    let users = p2.read_folder("test-project/users").unwrap();
    assert_eq!(users.auth, Auth::Bearer { token: "{{TOKEN}}".into(), prefix: None });
    let health = by_name(&after, "Health");
    assert_eq!(health.auth, Auth::None);
}

#[test]
fn http_file_round_trip() {
    let (_d, p) = sample_project();
    let out = to_http_file(&p, None).unwrap();
    assert!(out.content.contains("### Users / Get user\n"), "{}", out.content);
    assert!(out.content.contains("Authorization: Bearer {{TOKEN}}"), "{}", out.content);
    assert!(out.content.contains("@userId = 42"), "{}", out.content);
    assert!(out.content.contains("GRAPHQL {{baseUrl}}/graphql"), "{}", out.content);
    assert!(out.content.contains("# @no-redirect"), "{}", out.content);
    assert!(out.content.contains("client.assert(response.status == 201"), "{}", out.content);
    let names: Vec<&str> = out.attachments.iter().map(|a| a.file_name.as_str()).collect();
    assert_eq!(names, vec!["http-client.env.json", "http-client.private.env.json"]);
    let private = json(&out.attachments[1].content);
    assert_eq!(private["Production"]["TOKEN"], "");

    // Write the files side by side, as the export dialog does, and import the folder's .http file.
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join(&out.file_name), &out.content).unwrap();
    for a in &out.attachments {
        std::fs::write(dir.path().join(&a.file_name), &a.content).unwrap();
    }
    let plan = parse_path(&dir.path().join(&out.file_name), &ParseOptions::default()).unwrap();
    let (_d2, mut p2) = new_project();
    let report = apply(plan, &mut p2, "", &ApplyOptions::default()).unwrap();
    assert_eq!(report.requests_created, 7);
    let after = requests_of(&p2);
    let list = by_name(&after, "List users");
    assert_eq!(list.url, "{{baseUrl}}/users");
    assert_eq!(list.params, vec![KeyValue::new("page", "1")], "disabled params are not exported");
    assert!(list.headers.iter().any(|h| h.name == "Authorization" && h.value == "Bearer {{secret.TOKEN}}"), "{:?}", list.headers);
    let create = by_name(&after, "Create user");
    let Body::Json { content } = &create.body else { panic!("{:?}", create.body) };
    assert_eq!(json(content)["name"], "Ada");
    let ban = by_name(&after, "Ban user");
    assert!(matches!(&ban.body, Body::Form { fields } if fields.len() == 1 && fields[0].value == "spam links"), "{:?}", ban.body);
    let avatar = by_name(&after, "Upload avatar");
    assert!(matches!(&avatar.body, Body::Multipart { parts } if parts[0].file.as_deref() == Some("fixtures/ada.png") && parts[1].value == "Ada"), "{:?}", avatar.body);
    let search = by_name(&after, "Search");
    assert!(matches!(&search.body, Body::Graphql { query, variables, .. } if query == "query { users { id } }" && variables == "{\"first\": 10}"));
    assert!(!by_name(&after, "Health").settings.follow_redirects);
    let envs = p2.environments().unwrap();
    let prod = envs.iter().find(|e| e.file.name == "Production").unwrap();
    assert_eq!(prod.file.variables.get("baseUrl").map(String::as_str), Some("https://api.example.com"));
    assert!(prod.file.secrets.contains(&"TOKEN".to_string()));
}

#[test]
fn exporting_one_folder() {
    let (_d, p) = sample_project();
    let out = to_openapi(&p, Some("users/admin"), Some("local")).unwrap();
    let doc: serde_json::Value = serde_yaml_ng::from_str(&out.content).unwrap();
    assert_eq!(doc["info"]["title"], "Admin");
    assert_eq!(doc["paths"].as_object().unwrap().len(), 1);
    assert_eq!(doc["servers"].as_array().unwrap().len(), 1);
    // Auth inherited from the parent folder outside the exported scope still applies.
    assert_eq!(doc["security"][0]["TOKEN"], serde_json::json!([]));
    let pm = json(&to_postman(&p, Some("users/admin")).unwrap().content);
    assert_eq!(pm["auth"]["type"], "bearer");
    assert!(to_openapi(&p, Some("nope"), None).is_err());
}
