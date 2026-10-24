use serde_json::json;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

use super::*;

fn named(kind: &str, name: &str) -> Value {
    json!({ "kind": kind, "name": name, "ofType": null })
}
fn non_null(t: Value) -> Value {
    json!({ "kind": "NON_NULL", "name": null, "ofType": t })
}
fn list(t: Value) -> Value {
    json!({ "kind": "LIST", "name": null, "ofType": t })
}
fn scalar(n: &str) -> Value {
    named("SCALAR", n)
}
fn field(name: &str, ty: Value, args: Vec<Value>) -> Value {
    json!({ "name": name, "description": null, "args": args, "type": ty, "isDeprecated": false, "deprecationReason": null })
}
fn arg(name: &str, ty: Value) -> Value {
    json!({ "name": name, "description": null, "type": ty, "defaultValue": null })
}
fn object(name: &str, fields: Vec<Value>) -> Value {
    json!({ "kind": "OBJECT", "name": name, "description": format!("The {name} type"), "fields": fields, "inputFields": null, "interfaces": [], "enumValues": null, "possibleTypes": null })
}

/// A small but representative schema: arguments, lists, non-null, enums, input objects,
/// unions, a self-reference and a deprecated field.
fn canned() -> Value {
    let mut deprecated = field("legacyName", scalar("String"), vec![]);
    deprecated["isDeprecated"] = json!(true);
    deprecated["deprecationReason"] = json!("use name");
    json!({ "data": { "__schema": {
        "queryType": { "name": "Query" },
        "mutationType": { "name": "Mutation" },
        "subscriptionType": { "name": "Subscription" },
        "types": [
            object("Query", vec![
                field("user", named("OBJECT", "User"), vec![arg("id", non_null(scalar("ID")))]),
                field("users", non_null(list(non_null(named("OBJECT", "User")))), vec![arg("first", scalar("Int"))]),
                field("search", list(named("UNION", "SearchResult")), vec![arg("term", non_null(scalar("String")))]),
                field("version", non_null(scalar("String")), vec![]),
            ]),
            object("Mutation", vec![
                field("createUser", named("OBJECT", "User"), vec![arg("input", non_null(named("INPUT_OBJECT", "CreateUserInput")))]),
            ]),
            object("Subscription", vec![field("userCreated", named("OBJECT", "User"), vec![])]),
            object("User", vec![
                field("id", non_null(scalar("ID")), vec![]),
                field("name", scalar("String"), vec![]),
                field("role", named("ENUM", "Role"), vec![]),
                field("posts", list(named("OBJECT", "Post")), vec![]),
                field("best", named("OBJECT", "User"), vec![]),
                field("avatar", scalar("String"), vec![arg("size", non_null(scalar("Int")))]),
                deprecated,
            ]),
            object("Post", vec![
                field("id", non_null(scalar("ID")), vec![]),
                field("title", scalar("String"), vec![]),
                field("author", named("OBJECT", "User"), vec![]),
            ]),
            { "kind": "ENUM", "name": "Role", "description": null, "fields": null, "inputFields": null, "interfaces": null,
              "enumValues": [{ "name": "ADMIN", "description": null, "isDeprecated": false, "deprecationReason": null },
                             { "name": "USER", "description": null, "isDeprecated": false, "deprecationReason": null }],
              "possibleTypes": null },
            { "kind": "INPUT_OBJECT", "name": "CreateUserInput", "description": null, "fields": null, "interfaces": null, "enumValues": null, "possibleTypes": null,
              "inputFields": [arg("name", non_null(scalar("String"))), arg("role", non_null(named("ENUM", "Role"))), arg("tags", list(scalar("String")))] },
            { "kind": "UNION", "name": "SearchResult", "description": null, "fields": null, "inputFields": null, "interfaces": null, "enumValues": null,
              "possibleTypes": [named("OBJECT", "User"), named("OBJECT", "Post")] },
            scalar("ID"), scalar("String"), scalar("Int"), scalar("Boolean"),
            object("__Schema", vec![field("types", scalar("String"), vec![])]),
        ]
    } } })
}

#[test]
fn parses_types_and_root_operations() {
    let s = parse_introspection(&canned()).unwrap();
    assert_eq!(s.query_type.as_deref(), Some("Query"));
    assert!(s.types.iter().all(|t| !t.name.starts_with("__")));
    let user = s.types.iter().find(|t| t.name == "User").unwrap();
    assert_eq!(user.description.as_deref(), Some("The User type"));
    let users = s.types.iter().find(|t| t.name == "Query").unwrap().fields.iter().find(|f| f.name == "users").unwrap();
    assert_eq!((users.type_ref.as_str(), users.named_type.as_str()), ("[User!]!", "User"));
    let kinds: Vec<(OperationKind, &str)> = s.operations.iter().map(|o| (o.kind, o.field.as_str())).collect();
    assert_eq!(
        kinds,
        vec![
            (OperationKind::Query, "user"),
            (OperationKind::Query, "users"),
            (OperationKind::Query, "search"),
            (OperationKind::Query, "version"),
            (OperationKind::Mutation, "createUser"),
            (OperationKind::Subscription, "userCreated"),
        ]
    );
    let lp = s.types.iter().find(|t| t.name == "User").unwrap().fields.iter().find(|f| f.name == "legacyName").unwrap();
    assert!(lp.deprecated);
}

#[test]
fn generates_runnable_operations() {
    let s = parse_introspection(&canned()).unwrap();
    let op = |field: &str| s.operations.iter().find(|o| o.field == field).unwrap();

    let user = op("user");
    assert_eq!(user.query, "query User($id: ID!) {\n  user(id: $id) {\n    id\n    name\n    role\n    posts {\n      id\n      title\n    }\n  }\n}\n");
    assert_eq!(serde_json::from_str::<Value>(&user.variables).unwrap(), json!({ "id": "" }));
    // Deprecated fields, fields with required args and cycles are skipped.
    assert!(!user.query.contains("legacyName") && !user.query.contains("avatar") && !user.query.contains("best"));

    // Optional arguments are declared but not given placeholder values.
    let users = op("users");
    assert!(users.query.starts_with("query Users($first: Int) {\n  users(first: $first) {"), "{}", users.query);
    assert_eq!(users.variables, "");

    // Unions get __typename and inline fragments.
    let search = op("search");
    assert!(search.query.contains("__typename") && search.query.contains("... on User {") && search.query.contains("... on Post {"), "{}", search.query);

    // Scalars have no selection set.
    assert_eq!(op("version").query, "query Version {\n  version\n}\n");

    // Input objects get their required fields, enums their first value.
    let create = op("createUser");
    assert!(create.query.starts_with("mutation CreateUser($input: CreateUserInput!)"));
    assert_eq!(serde_json::from_str::<Value>(&create.variables).unwrap(), json!({ "input": { "name": "", "role": "ADMIN" } }));
    assert!(op("userCreated").query.starts_with("subscription UserCreated {"));

    // Deeper on request.
    assert!(!search.query.contains("posts {"), "{}", search.query);
    let (deep, _) = generate_operation(&s, OperationKind::Query, "search", 3).unwrap();
    assert!(deep.contains("posts {"), "{deep}");
    assert!(generate_operation(&s, OperationKind::Query, "nope", 2).is_none());
}

#[test]
fn introspection_errors_are_explained() {
    let e = parse_introspection(&json!({ "errors": [{ "message": "GraphQL introspection is not allowed" }] })).unwrap_err();
    assert!(e.to_string().contains("not allowed"), "{e}");
    assert!(parse_introspection(&json!({ "data": null })).is_err());
    assert!(parse_introspection(&json!({ "__schema": { "queryType": { "name": "Q" }, "types": [] } })).is_ok());
}

#[test]
fn introspection_query_survives_template_resolution() {
    let mut r = devall_core::Resolver::new(&devall_core::vars::NoSecrets);
    assert_eq!(r.resolve(INTROSPECTION_QUERY), INTROSPECTION_QUERY);
    assert!(r.unresolved.is_empty());
}

/// Serves one canned HTTP response per connection and reports each request it saw.
async fn http_server(status: u16, body: String) -> (String, tokio::sync::mpsc::UnboundedReceiver<String>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    tokio::spawn(async move {
        while let Ok((mut s, _)) = listener.accept().await {
            let (tx, body) = (tx.clone(), body.clone());
            tokio::spawn(async move {
                let mut buf = Vec::new();
                let mut chunk = [0u8; 8192];
                // Read headers + Content-Length body.
                loop {
                    let n = s.read(&mut chunk).await.unwrap_or(0);
                    if n == 0 {
                        break;
                    }
                    buf.extend_from_slice(&chunk[..n]);
                    let text = String::from_utf8_lossy(&buf).to_string();
                    if let Some(h) = text.find("\r\n\r\n") {
                        let len = text[..h]
                            .lines()
                            .find_map(|l| l.to_ascii_lowercase().strip_prefix("content-length:").map(|v| v.trim().parse::<usize>().unwrap_or(0)))
                            .unwrap_or(0);
                        if buf.len() >= h + 4 + len {
                            break;
                        }
                    }
                }
                let _ = tx.send(String::from_utf8_lossy(&buf).to_string());
                let resp = format!("HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len());
                let _ = s.write_all(resp.as_bytes()).await;
            });
        }
    });
    (format!("http://{addr}/graphql"), rx)
}

#[tokio::test]
async fn introspects_over_http_with_request_headers() {
    let (url, mut seen) = http_server(200, canned().to_string()).await;
    let mut req = HttpRequestFile::new("gql");
    req.method = "GET".into();
    req.url = "{{gql}}".into();
    req.headers.push(devall_core::KeyValue::new("Authorization", "Bearer {{secret.T}}"));
    req.body = Body::Graphql { query: "{ me { id } }".into(), variables: String::new(), operation_name: None };
    struct S;
    impl devall_core::SecretLookup for S {
        fn lookup(&self, _: &str) -> Option<String> {
            Some("tok-1234".into())
        }
    }
    let mut r = devall_core::Resolver::new(&S);
    r.push_layer([("gql".to_string(), url)].into_iter().collect());
    let prepared = devall_http::prepare(&introspection_request(&req), &mut r, std::path::Path::new(".")).unwrap();
    let pool = ClientPool::new();
    let schema = execute_introspection(&pool, &prepared).await.unwrap();
    assert_eq!(schema.operations.len(), 6);
    let raw = seen.recv().await.unwrap();
    assert!(raw.starts_with("POST /graphql"), "{raw}");
    assert!(raw.to_ascii_lowercase().contains("authorization: bearer tok-1234"), "{raw}");
    assert!(raw.contains("__schema") && raw.contains("\"operationName\":\"IntrospectionQuery\""), "{raw}");
}

#[tokio::test]
async fn http_errors_are_reported() {
    let (url, _seen) = http_server(400, json!({ "errors": [{ "message": "introspection disabled" }] }).to_string()).await;
    let mut req = HttpRequestFile::new("gql");
    req.url = url;
    let mut r = devall_core::Resolver::new(&devall_core::vars::NoSecrets);
    let prepared = devall_http::prepare(&introspection_request(&req), &mut r, std::path::Path::new(".")).unwrap();
    let e = execute_introspection(&ClientPool::new(), &prepared).await.unwrap_err();
    assert!(e.to_string().contains("HTTP 400") && e.to_string().contains("introspection disabled"), "{e}");

    let (url, _seen) = http_server(502, "bad gateway".into()).await;
    req.url = url;
    let prepared = devall_http::prepare(&introspection_request(&req), &mut r, std::path::Path::new(".")).unwrap();
    let e = execute_introspection(&ClientPool::new(), &prepared).await.unwrap_err();
    assert!(e.to_string().contains("502"), "{e}");
}
