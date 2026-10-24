use super::*;

const SDL: &str = r#"
    "The root"
    type Query {
      user(id: ID!): User
      users(first: Int = 2, role: Role): [User!]!
      search(term: String!): [SearchResult!]!
      node(id: ID!): Node
      settings(input: SettingsInput): Boolean
    }
    type Mutation { createUser(input: UserInput!): User! }
    interface Node { id: ID! }
    type User implements Node { id: ID! name: String! email: String! age: Int role: Role! posts(first: Int): [Post!]! createdAt: String legacy: String @deprecated(reason: "gone") }
    type Post implements Node { id: ID! title: String! }
    union SearchResult = User | Post
    enum Role { ADMIN MEMBER }
    input UserInput { name: String!, role: Role = MEMBER }
    input SettingsInput { dark: Boolean }
"#;

fn entry() -> SchemaEntry {
    SchemaEntry { name: "schema".into(), endpoint: "/graphql".into(), files: vec![], schema: Arc::new(Schema::from_sdl(&[SDL]).unwrap()), options: execute::ExecOptions::default() }
}

fn run(query: &str, vars: Json) -> Json {
    let p = Payload { query: query.into(), variables: vars.as_object().cloned().unwrap_or_default(), operation_name: None };
    execute_payload(&entry(), &p, None, &[], false)
}

fn errors(query: &str) -> Vec<String> {
    let v = run(query, json!({}));
    v["errors"].as_array().map(|a| a.iter().map(|e| e["message"].as_str().unwrap_or_default().to_string()).collect()).unwrap_or_default()
}

#[test]
fn automock_follows_the_selection_set() {
    let v = run("query Q($id: ID!) { user(id: $id) { id name email role posts(first: 3) { title } who: name __typename } }", json!({"id": "u-1"}));
    assert!(v.get("errors").is_none(), "{v}");
    let u = &v["data"]["user"];
    assert_eq!(u["id"], "u-1", "the id argument flows into the object's id");
    assert!(u["email"].as_str().unwrap().contains('@'));
    assert!(["ADMIN", "MEMBER"].contains(&u["role"].as_str().unwrap()));
    assert_eq!(u["posts"].as_array().unwrap().len(), 3, "first: 3");
    assert!(u["posts"][0]["title"].is_string());
    assert!(u["who"].is_string());
    assert_eq!(u["__typename"], "User");
    let keys: Vec<&String> = u.as_object().unwrap().keys().collect();
    assert_eq!(keys, vec!["id", "name", "email", "role", "posts", "who", "__typename"], "response keys in selection order");
    // Deterministic.
    assert_eq!(v, run("query Q($id: ID!) { user(id: $id) { id name email role posts(first: 3) { title } who: name __typename } }", json!({"id": "u-1"})));
    // Default list length and arguments' defaults.
    assert_eq!(run("{ users { id } }", json!({}))["data"]["users"].as_array().unwrap().len(), 2);
}

#[test]
fn abstract_types_fragments_and_directives() {
    let v = run(
        "{ search(term: \"x\") { __typename ... on Post { title } ...U } node(id: 1) { id ... on User { name } } } fragment U on User { email }",
        json!({}),
    );
    let first = &v["data"]["search"][0];
    assert_eq!(first["__typename"], "Post", "the first type condition is preferred");
    assert!(first["title"].is_string());
    assert!(first.get("email").is_none());
    assert!(v["data"]["node"]["id"].is_string());
    assert!(v["data"]["node"].get("__typename").is_none(), "not selected");
    let v = run("query($skip: Boolean!) { user(id: 1) { name @skip(if: $skip) email @include(if: false) id } }", json!({"skip": true}));
    assert_eq!(v["data"]["user"].as_object().unwrap().keys().collect::<Vec<_>>(), vec!["id"]);
}

#[test]
fn overrides_shape_the_data() {
    let p = Payload { query: "{ user(id: 1) { name posts { title } } search(term: \"a\") { ... on User { name } ... on Post { title } } }".into(), variables: Map::new(), operation_name: None };
    let over = json!({ "user": { "name": "Ada", "posts": [{ "title": "One" }, {}, {}] }, "search": [{ "__typename": "User", "name": "Grace" }] });
    let v = execute_payload(&entry(), &p, Some(&over), &[json!({"message": "partial"})], false);
    assert_eq!(v["data"]["user"]["name"], "Ada");
    let posts = v["data"]["user"]["posts"].as_array().unwrap();
    assert_eq!(posts.len(), 3, "override list length wins");
    assert_eq!(posts[0]["title"], "One");
    assert!(posts[1]["title"].is_string(), "missing fields are generated");
    assert_eq!(v["data"]["search"][0]["name"], "Grace");
    assert_eq!(v["errors"][0]["message"], "partial");
    let v = execute_payload(&entry(), &p, Some(&json!({ "user": null })), &[], false);
    assert!(v["data"]["user"].is_null());
}

#[test]
fn validation_errors_read_like_graphql_js() {
    assert_eq!(errors("{ user(id: 1) { nope } }"), vec!["Cannot query field \"nope\" on type \"User\"."]);
    assert_eq!(errors("{ user { id } }"), vec!["Field \"user\" argument \"id\" of type \"ID!\" is required, but it was not provided."]);
    assert_eq!(errors("{ user(id: 1, x: 2) { id } }"), vec!["Unknown argument \"x\" on field \"Query.user\"."]);
    assert_eq!(errors("{ user(id: 1) }"), vec!["Field \"user\" of type \"User\" must have a selection of subfields. Did you mean \"user { ... }\"?"]);
    assert_eq!(errors("{ user(id: 1) { name { x } } }"), vec!["Field \"name\" must not have a selection since type \"String!\" has no subfields."]);
    assert_eq!(errors("{ users(role: OWNER) { id } }"), vec!["Value \"OWNER\" does not exist in \"Role\" enum."]);
    assert_eq!(errors("{ users(first: \"two\") { id } }"), vec!["Int cannot represent non-integer value: \"two\""]);
    assert_eq!(errors("{ settings(input: {dark: true, x: 1}) }"), vec!["Field \"x\" is not defined by type \"SettingsInput\"."]);
    assert_eq!(errors("mutation { createUser(input: {role: ADMIN}) { id } }"), vec!["Field \"UserInput.name\" of required type \"String!\" was not provided."]);
    assert_eq!(errors("{ user(id: 1) { ...F } }"), vec!["Unknown fragment \"F\"."]);
    assert_eq!(errors("{ user(id: 1) { id } } fragment F on User { id }"), vec!["Fragment \"F\" is never used."]);
    assert_eq!(errors("{ user(id: 1) { ...F } } fragment F on User { ...G } fragment G on User { ...F }"), vec!["Cannot spread fragment \"F\" within itself via \"G\"."]);
    assert_eq!(errors("{ user(id: 1) { ... on Post { title } } }"), vec!["Fragment cannot be spread here as objects of type \"User\" can never be of type \"Post\"."]);
    assert_eq!(errors("query($id: ID) { user(id: $id) { id } }"), vec!["Variable \"$id\" of type \"ID\" used in position expecting type \"ID!\"."]);
    assert_eq!(errors("query Q { user(id: $x) { id } }"), vec!["Variable \"$x\" is not defined by operation \"Q\"."]);
    assert_eq!(errors("query Q($x: ID!) { users { id } }"), vec!["Variable \"$x\" is never used in operation \"Q\"."]);
    assert_eq!(errors("query($u: User) { users { id } }")[0], "Variable \"$u\" cannot be non-input type \"User\".");
    assert_eq!(errors("{ users { id @foo } }"), vec!["Unknown directive \"@foo\"."]);
    assert_eq!(errors("{ a: users { id } } { b: users { id } }"), vec!["This anonymous operation must be the only defined operation."; 2]);
    assert_eq!(errors("subscription { x }"), vec!["Schema is not configured for subscriptions."]);
    let v = run("{ user(id: 1) { ", json!({}));
    assert_eq!(v["errors"][0]["extensions"]["code"], "GRAPHQL_PARSE_FAILED");
    assert_eq!(v["errors"][0]["locations"][0]["line"], 1);
    // Variables are coerced before execution.
    let v = run("query($id: ID!) { user(id: $id) { id } }", json!({}));
    assert_eq!(v["errors"][0]["message"], "Variable \"$id\" of required type \"ID!\" was not provided.");
    assert_eq!(v["errors"][0]["extensions"]["code"], "BAD_USER_INPUT");
    let v = run("query($n: Int) { users(first: $n) { id } }", json!({"n": "x"}));
    assert!(v["errors"][0]["message"].as_str().unwrap().starts_with("Variable \"$n\" got invalid value \"x\""));
}

#[test]
fn introspection_matches_the_schema_explorer() {
    let v = run(devall_proto::graphql::INTROSPECTION_QUERY, json!({}));
    assert!(v.get("errors").is_none(), "{}", v["errors"]);
    let schema = devall_proto::graphql::parse_introspection(&v).expect("devall's own GraphQL explorer understands the mock");
    let user = schema.types.iter().find(|t| t.name == "User").unwrap();
    assert!(user.fields.iter().any(|f| f.name == "posts"));
    assert!(user.fields.iter().any(|f| f.name == "legacy"), "includeDeprecated: true");
    // Without includeDeprecated deprecated fields are hidden.
    let v = run("{ __type(name: \"User\") { name kind fields { name type { kind ofType { name } } } } }", json!({}));
    let fields = v["data"]["__type"]["fields"].as_array().unwrap();
    assert!(!fields.iter().any(|f| f["name"] == "legacy"));
    assert_eq!(v["data"]["__type"]["kind"], "OBJECT");
    assert_eq!(fields[0]["type"]["kind"], "NON_NULL");
    assert_eq!(fields[0]["type"]["ofType"]["name"], "ID");
    assert!(run("{ __type(name: \"Nope\") { name } }", json!({}))["data"]["__type"].is_null());
    assert_eq!(run("{ __schema { queryType { name } mutationType { name } } }", json!({}))["data"]["__schema"]["mutationType"]["name"], "Mutation");
}

#[test]
fn http_requests_batches_and_methods() {
    let e = entry();
    let req = MockRequest::new("POST", "/graphql", vec![("Content-Type".into(), "application/json".into())], r#"[{"query":"{ users { id } }"},{"query":"{ nope }"}]"#);
    let (status, body) = respond(&e, &req, None);
    assert_eq!(status, 200);
    assert_eq!(body.as_array().unwrap().len(), 2);
    assert!(body[1]["errors"].is_array());
    let get = MockRequest::new("GET", "/graphql?query=%7Busers%7Bid%7D%7D", vec![], "");
    assert_eq!(respond(&e, &get, None).1["data"]["users"].as_array().unwrap().len(), 2);
    let get_mut = MockRequest::new("GET", "/graphql?query=mutation%7BcreateUser(input%3A%7Bname%3A%22a%22%7D)%7Bid%7D%7D", vec![], "");
    assert!(respond(&e, &get_mut, None).1["errors"][0]["message"].as_str().unwrap().contains("POST"));
    let bad = MockRequest::new("POST", "/graphql", vec![], "not json");
    assert_eq!(respond(&e, &bad, None).0, 400);
    assert_eq!(respond(&e, &MockRequest::new("PUT", "/graphql", vec![], ""), None).0, 405);
}

#[test]
fn request_matchers() {
    let req = |q: &str, vars: Json, op: Option<&str>| {
        let mut body = json!({ "query": q, "variables": vars });
        if let Some(o) = op {
            body["operationName"] = json!(o);
        }
        MockRequest::new("POST", "/graphql", vec![("Content-Type".into(), "application/json".into())], body.to_string())
    };
    let ok = |p: &GqlPattern, r: &MockRequest| p.eval(r).iter().all(|(_, _, _, e)| e.matched);
    let p = GqlPattern::from_json(&json!({ "operationName": "GetUser", "operationType": "query", "field": "user", "variables": { "id": "1" } })).unwrap();
    assert!(ok(&p, &req("query GetUser($id: ID!) { user(id: $id) { id } }", json!({"id": "1", "x": 2}), None)));
    assert!(!ok(&p, &req("query GetUser($id: ID!) { user(id: $id) { id } }", json!({"id": "2"}), None)));
    assert!(!ok(&p, &req("query Other { user(id: 1) { id } }", json!({"id": "1"}), None)));
    assert!(!ok(&p, &req("mutation GetUser { user(id: 1) { id } }", json!({"id": "1"}), None)));
    // The operation is selected by operationName in multi-operation documents.
    assert!(ok(&p, &req("query A { users { id } } query GetUser { user(id: 1) { id } }", json!({"id": "1"}), Some("GetUser"))));
    let p = GqlPattern::from_json(&json!({ "variables": { "matchesJsonPath": "$.input.name" }, "query": { "contains": "createUser" } })).unwrap();
    assert!(ok(&p, &req("mutation { createUser(input: $input) { id } }", json!({"input": {"name": "a"}}), None)));
    // gql-matcher: semantically equal queries.
    let p = GqlPattern::from_custom_matcher(&json!({ "query": "{ user(id: 1) { name id } }", "variables": {} })).unwrap();
    assert!(ok(&p, &req("query {\n  user(id: 1) {\n    id   # the id\n    name\n  }\n}", json!({}), None)));
    assert!(!ok(&p, &req("{ user(id: 2) { id name } }", json!({}), None)));
    assert!(GqlPattern::from_json(&json!({ "nope": 1 })).is_err());
}

#[test]
fn loads_schemas_from_the_project() {
    let dir = tempfile::tempdir().unwrap();
    let g = dir.path().join("mocks/graphql");
    std::fs::create_dir_all(g.join("billing")).unwrap();
    std::fs::write(g.join("schema.graphql"), SDL).unwrap();
    std::fs::write(g.join("billing/a.graphql"), "type Query { invoices: [Invoice!]! }").unwrap();
    std::fs::write(g.join("billing/b.graphql"), "type Invoice { id: ID! total: Float! }").unwrap();
    std::fs::write(g.join("broken.graphql"), "type Query { a: Missing }").unwrap();
    let intro = run(devall_proto::graphql::INTROSPECTION_QUERY, json!({}));
    std::fs::write(g.join("remote.json"), intro.to_string()).unwrap();
    std::fs::write(g.join("config.json"), r#"{"remote": {"endpoint": "api/remote", "seed": 7, "listLength": 1}}"#).unwrap();
    let set = GraphqlSet::load(dir.path());
    let names: Vec<(&str, &str)> = set.schemas.iter().map(|s| (s.name.as_str(), s.endpoint.as_str())).collect();
    assert_eq!(names, vec![("billing", "/graphql/billing"), ("remote", "/api/remote"), ("schema", "/graphql")]);
    assert_eq!(set.errors.len(), 1);
    assert!(set.errors[0].error.contains("Unknown type \"Missing\""), "{:?}", set.errors);
    assert_eq!(set.for_path("/graphql/").unwrap().name, "schema");
    assert_eq!(set.by_name("remote").unwrap().options.list_length, 1);
    assert_eq!(set.info()[0].summary["query"][0], "invoices: [Invoice!]!");
}
