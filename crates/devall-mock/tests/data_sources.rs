//! CSV / JSON / SQLite data sources (`mocks/data/`) used from response templates and from
//! `transformerParameters.dataSource`.

mod common;

use std::path::Path;

use common::{start_with, write_mapping};
use serde_json::json;

fn data(root: &Path) {
    let dir = root.join("mocks").join("data");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("users.csv"), "id,name,role\n1,Ann,admin\n2,Bob,user\n3,Cy,user\n").unwrap();
    let conn = rusqlite::Connection::open(dir.join("shop.db")).unwrap();
    conn.execute_batch("CREATE TABLE orders (id INTEGER, user_id INTEGER, total REAL); INSERT INTO orders VALUES (10, 1, 9.5), (11, 2, 20), (12, 2, 1.25);").unwrap();
}

#[tokio::test]
async fn helpers_and_transformer_parameters() {
    let f = start_with(
        |root| {
            data(root);
            write_mapping(
                root,
                "user.json",
                &json!({
                    "request": { "method": "GET", "urlPathTemplate": "/users/{id}" },
                    "response": {
                        "status": 200,
                        "headers": { "Content-Type": "application/json" },
                        "body": "{{#if row}}{\"id\": {{row.id}}, \"name\": \"{{row.name}}\", \"orders\": {{dataCount 'shop.orders' 'user_id' row.id}}}{{else}}{\"error\": \"no user {{request.path.id}}\"}{{/if}}",
                        "transformerParameters": { "dataSource": { "name": "users", "where": { "id": "{{request.path.id}}" }, "notFoundStatus": 404 } }
                    }
                }),
            );
            write_mapping(
                root,
                "inline.json",
                &json!({
                    "request": { "method": "GET", "urlPath": "/by-role" },
                    "response": {
                        "status": 200,
                        "body": "{{lookup (dataSource 'users' 'role' request.query.role) 'name'}}|{{dataRows 'users' 'role' request.query.role limit=5}}|{{size (dataRows 'users')}}",
                        "transformers": ["response-template"]
                    }
                }),
            );
            write_mapping(
                root,
                "sql.json",
                &json!({
                    "request": { "method": "GET", "urlPath": "/orders" },
                    "response": {
                        "status": 200,
                        "body": "{{#each data}}{{id}}={{total}};{{/each}}",
                        "transformerParameters": { "dataSource": { "name": "shop", "sql": "SELECT id, total FROM orders WHERE total >= ?1 ORDER BY id", "params": ["{{request.query.min}}"] } }
                    }
                }),
            );
            write_mapping(
                root,
                "missing.json",
                &json!({
                    "request": { "urlPath": "/missing" },
                    "response": { "body": "{{dataSource 'nope' 'id' 1}}", "transformers": ["response-template"] }
                }),
            );
        },
        |_| {},
    )
    .await;

    let ann = f.get("/users/1").await;
    assert_eq!(ann.status, 200, "{}", ann.text);
    assert_eq!(ann.json(), json!({ "id": 1, "name": "Ann", "orders": 1 }));
    let bob = f.get("/users/2").await;
    assert_eq!(bob.json()["orders"], json!(2));
    let none = f.get("/users/99").await;
    assert_eq!(none.status, 404);
    assert_eq!(none.json(), json!({ "error": "no user 99" }));

    let role = f.get("/by-role?role=user").await;
    let parts: Vec<&str> = role.text.split('|').collect();
    assert_eq!(parts[0], "Bob");
    let rows: serde_json::Value = serde_json::from_str(parts[1]).unwrap();
    assert_eq!(rows.as_array().unwrap().len(), 2);
    assert_eq!(parts[2], "3");

    let orders = f.get("/orders?min=5").await;
    assert_eq!(orders.text, "10=9.5;11=20;");

    let missing = f.get("/missing").await;
    assert!(missing.text.contains("[ERROR: no data source 'nope'"), "{}", missing.text);

    assert_eq!(f.handle.data_sources().len(), 2);
    let project = devall_mock::load_project(f.root());
    assert_eq!(project.mappings.len(), 4);
    assert!(project.problems().is_empty());
    assert!(project.mappings.iter().all(|m| m.file.starts_with("mocks/mappings/")));
    f.handle.stop().await;
}
