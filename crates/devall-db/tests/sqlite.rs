//! End-to-end tests against SQLite (temp files): sessions, schema introspection, multi-statement
//! scripts, paging, read-only enforcement (classifier *and* SQLITE_OPEN_READ_ONLY), parameterized
//! edits, cancel, seeding from the fake-data engine, query → JSON, EXPLAIN.

use std::path::Path;
use std::time::Duration;

use devall_core::Resolver;
use devall_core::vars::NoSecrets;
use devall_db::sql::edit::{EditRequest, RowEdit};
use devall_db::sql::{ExecOptions, QueryProgress, SqlConn, TableKind};
use devall_db::{ConnectOptions, ConnectionFile, DbError, DbKind, Session, TunnelManager, resolve_connection};
use serde_json::{Map, Value, json};

async fn open(root: &Path, file: &str, read_only: bool) -> Session {
    let mut c = ConnectionFile::new("Test DB", DbKind::Sqlite);
    c.database = Some(file.into());
    c.read_only = read_only;
    c.options.insert("create".into(), "true".into());
    let mut r = Resolver::new(&NoSecrets);
    let rc = resolve_connection(&c, &mut r, root).unwrap();
    Session::connect(rc, &TunnelManager::new(), ConnectOptions::default()).await.unwrap()
}

fn obj(v: Value) -> Map<String, Value> {
    v.as_object().unwrap().clone()
}

async fn setup(root: &Path) -> Session {
    let s = open(root, "app.db", false).await;
    let out = s
        .query(
            "CREATE TABLE users (id INTEGER PRIMARY KEY, name TEXT NOT NULL, email TEXT UNIQUE, score REAL, avatar BLOB, meta JSON);
             CREATE TABLE orders (id INTEGER PRIMARY KEY, user_id INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE, total NUMERIC);
             CREATE INDEX orders_user ON orders(user_id);
             CREATE VIEW big_spenders AS SELECT u.name, SUM(o.total) AS spent FROM users u JOIN orders o ON o.user_id = u.id GROUP BY u.name;
             INSERT INTO users (name, email, score, avatar, meta) VALUES ('Ada', 'ada@example.com', 9.5, x'00ff', '{\"a\":1}'), ('Linus', 'linus@example.com', NULL, NULL, NULL);
             INSERT INTO orders (user_id, total) VALUES (1, 10.5), (1, 20), (2, 99);",
            ExecOptions::default(),
            None,
        )
        .await
        .unwrap();
    assert_eq!(out.results.len(), 6);
    assert!(out.results.iter().all(|r| r.error.is_none()), "{:?}", out.results);
    assert_eq!(out.results[4].rows_affected, Some(2));
    assert_eq!(out.results[5].keyword, "INSERT");
    s
}

#[tokio::test]
async fn multi_statement_results_and_types() {
    let dir = tempfile::tempdir().unwrap();
    let s = setup(dir.path()).await;
    let progress = std::sync::Mutex::new(Vec::new());
    let cb = |p: QueryProgress| progress.lock().unwrap().push(p);
    let out = s
        .query(
            "-- connection: test\nSELECT id, name, score, avatar, meta FROM users ORDER BY id; SELECT count(*) AS n FROM orders; UPDATE users SET score = 1 WHERE id = 2",
            ExecOptions::default(),
            Some(&cb),
        )
        .await;
    // Read-write session: the UPDATE runs.
    let out = out.unwrap();
    assert_eq!(out.total_statements, 3);
    assert_eq!(out.results.len(), 3);
    let users = &out.results[0];
    assert_eq!(users.columns.iter().map(|c| c.name.as_str()).collect::<Vec<_>>(), vec!["id", "name", "score", "avatar", "meta"]);
    assert_eq!(users.columns[0].type_name, "INTEGER");
    assert_eq!(users.rows[0], vec![json!(1), json!("Ada"), json!(9.5), json!("AP8="), json!("{\"a\":1}")]);
    assert_eq!(users.rows[1][2], Value::Null);
    assert_eq!(users.sql, "SELECT id, name, score, avatar, meta FROM users ORDER BY id", "leading comments are not part of the statement");
    assert_eq!(out.results[1].rows, vec![vec![json!(3)]]);
    assert_eq!(out.results[2].rows_affected, Some(1));
    {
        let p = progress.lock().unwrap();
        assert_eq!(p.len(), 6, "started + finished per statement");
        assert!(matches!(&p[0], QueryProgress::Started { index: 0, total: 3, .. }));
    }

    // A failing statement stops the script and is reported in place.
    let out = s.query("SELECT 1; SELECT * FROM nope; SELECT 3", ExecOptions::default(), None).await.unwrap();
    assert_eq!(out.results.len(), 2);
    assert!(out.results[1].error.as_deref().unwrap().contains("no such table"));
}

#[tokio::test]
async fn schema_introspection() {
    let dir = tempfile::tempdir().unwrap();
    let s = setup(dir.path()).await;
    let sql = s.sql().unwrap();
    let tree = sql.schema().await.unwrap();
    assert_eq!(tree.default_schema.as_deref(), Some("main"));
    let main = tree.schemas.iter().find(|x| x.name == "main").unwrap();
    let names: Vec<&str> = main.tables.iter().map(|t| t.name.as_str()).collect();
    assert_eq!(names, vec!["big_spenders", "orders", "users"]);
    let users = main.tables.iter().find(|t| t.name == "users").unwrap();
    assert_eq!(users.kind, TableKind::Table);
    assert!(users.columns.iter().find(|c| c.name == "id").unwrap().primary_key);
    assert!(!users.columns.iter().find(|c| c.name == "name").unwrap().nullable);
    assert_eq!(main.tables[0].kind, TableKind::View);

    let d = sql.table_details(None, "orders").await.unwrap();
    assert_eq!(d.primary_key, vec!["id"]);
    assert!(d.indexes.iter().any(|i| i.name == "orders_user" && i.columns == vec!["user_id"] && !i.unique));
    assert_eq!(d.foreign_keys.len(), 1);
    let fk = &d.foreign_keys[0];
    assert_eq!((fk.ref_table.as_str(), fk.columns.clone(), fk.ref_columns.clone()), ("users", vec!["user_id".to_string()], vec!["id".to_string()]));
    assert_eq!(fk.on_delete.as_deref(), Some("CASCADE"));
    assert!(d.ddl.unwrap().starts_with("CREATE TABLE orders"));
    let u = sql.table_details(Some("main"), "users").await.unwrap();
    assert!(u.indexes.iter().any(|i| i.unique && i.columns == vec!["email"]));
    assert!(matches!(sql.table_details(None, "missing").await, Err(DbError::NotFound(_))));
}

#[tokio::test]
async fn paging_caps_rows() {
    let dir = tempfile::tempdir().unwrap();
    let s = open(dir.path(), "paging.db", false).await;
    s.query(
        "CREATE TABLE n (v INTEGER); WITH RECURSIVE c(x) AS (SELECT 1 UNION ALL SELECT x + 1 FROM c WHERE x < 2500) INSERT INTO n SELECT x FROM c",
        ExecOptions::default(),
        None,
    )
    .await
    .unwrap();
    let q = "SELECT v FROM n ORDER BY v";
    let p1 = s.query(q, ExecOptions::new(None, None), None).await.unwrap();
    assert_eq!(p1.results[0].rows.len(), 1000, "default cap");
    assert!(p1.results[0].more);
    let p2 = s.query(q, ExecOptions::new(Some(1000), Some(1000)), None).await.unwrap();
    assert_eq!(p2.results[0].rows[0], vec![json!(1001)]);
    assert_eq!(p2.results[0].offset, 1000);
    assert!(p2.results[0].more);
    let p3 = s.query(q, ExecOptions::new(Some(1000), Some(2000)), None).await.unwrap();
    assert_eq!(p3.results[0].rows.len(), 500);
    assert!(!p3.results[0].more);
    // Paging never re-runs writes.
    let e = s.query("DELETE FROM n WHERE v > 2400", ExecOptions::new(None, Some(10)), None).await.unwrap_err();
    assert!(e.to_string().contains("only read-only statements can be paged"), "{e}");
}

#[tokio::test]
async fn read_only_is_enforced_twice() {
    let dir = tempfile::tempdir().unwrap();
    drop(setup(dir.path()).await);
    let s = open(dir.path(), "app.db", true).await;
    assert!(s.is_read_only());
    let out = s.query("SELECT count(*) FROM users", ExecOptions::default(), None).await.unwrap();
    assert_eq!(out.results[0].rows, vec![vec![json!(2)]]);

    // 1. The classifier rejects the whole script before anything runs.
    let e = s.query("SELECT 1; DELETE FROM users; SELECT 2", ExecOptions::default(), None).await.unwrap_err();
    assert!(matches!(e, DbError::ReadOnly(_)), "{e:?}");
    assert!(e.to_string().contains("`DELETE`"));
    for stmt in ["DROP TABLE users", "PRAGMA user_version = 3", "WITH x AS (SELECT 1) INSERT INTO users(name) SELECT 'x'", "VACUUM"] {
        assert!(matches!(s.query(stmt, ExecOptions::default(), None).await, Err(DbError::ReadOnly(_))), "{stmt}");
    }
    // 2. The database connection itself is read-only (SQLITE_OPEN_READ_ONLY), even when the
    // classifier is bypassed.
    let SqlConn::Lite(_) = s.sql().unwrap() else { panic!("sqlite") };
    let direct = s.sql().unwrap().run_statement("INSERT INTO users (name) VALUES ('sneaky')", ExecOptions::default(), true).await;
    assert!(direct.unwrap_err().to_string().to_lowercase().contains("readonly"), "the file is opened read-only");
    // Edits and inserts need write access.
    let req = EditRequest { schema: None, table: "users".into(), edits: vec![RowEdit::Delete { key: obj(json!({"id": 1})) }] };
    assert!(matches!(s.edit_apply(&req).await, Err(DbError::ReadOnly(_))));
    assert!(s.edit_preview(&req).is_ok(), "previews are always allowed");

    // Toggle to read-write (reopens the file read-write), then back.
    s.set_read_only(false, None).await.unwrap();
    let out = s.query("UPDATE users SET name = 'Ada L.' WHERE id = 1", ExecOptions::default(), None).await.unwrap();
    assert_eq!(out.results[0].rows_affected, Some(1));
    s.set_read_only(true, None).await.unwrap();
    assert!(s.query("UPDATE users SET name = 'x'", ExecOptions::default(), None).await.is_err());
}

#[tokio::test]
async fn protected_connections_need_typed_confirmation() {
    let dir = tempfile::tempdir().unwrap();
    drop(setup(dir.path()).await);
    let mut c = ConnectionFile::new("Prod Copy", DbKind::Sqlite);
    c.database = Some("app.db".into());
    c.protected = true;
    let mut r = Resolver::new(&NoSecrets);
    let rc = resolve_connection(&c, &mut r, dir.path()).unwrap();
    let s = Session::connect(rc, &TunnelManager::new(), ConnectOptions::default()).await.unwrap();
    assert!(s.is_read_only() && s.info().protected);
    assert!(s.set_read_only(false, None).await.is_err());
    assert!(s.set_read_only(false, Some("prod copy")).await.is_err());
    assert!(!s.set_read_only(false, Some("Prod Copy")).await.unwrap().read_only);
}

#[tokio::test]
async fn parameterized_edits() {
    let dir = tempfile::tempdir().unwrap();
    let s = setup(dir.path()).await;
    let req = EditRequest {
        schema: None,
        table: "users".into(),
        edits: vec![
            RowEdit::Update { key: obj(json!({"id": 1})), values: obj(json!({"name": "Robert'); DROP TABLE users;--", "score": 7})) },
            RowEdit::Insert { values: obj(json!({"name": "Grace", "email": "grace@example.com", "meta": {"x": [1, 2]}})) },
            RowEdit::Delete { key: obj(json!({"id": 2})) },
        ],
    };
    let preview = s.edit_preview(&req).unwrap();
    assert_eq!(preview.statements[0].sql, r#"UPDATE "users" SET "name" = ?, "score" = ? WHERE "id" = ?"#);
    assert!(preview.script.contains("'Robert''); DROP TABLE users;--'"));
    assert_eq!(s.edit_apply(&req).await.unwrap(), vec![1, 1, 1]);
    let out = s.query("SELECT id, name, score, meta FROM users ORDER BY id", ExecOptions::default(), None).await.unwrap();
    let rows = &out.results[0].rows;
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0], vec![json!(1), json!("Robert'); DROP TABLE users;--"), json!(7.0), json!("{\"a\":1}")]);
    assert_eq!(rows[1][1], json!("Grace"));
    assert_eq!(rows[1][3], json!("{\"x\":[1,2]}"), "objects are stored as JSON text");

    // A key that matches nothing rolls back the whole batch.
    let bad = EditRequest {
        schema: None,
        table: "users".into(),
        edits: vec![
            RowEdit::Update { key: obj(json!({"id": 1})), values: obj(json!({"name": "changed"})) },
            RowEdit::Delete { key: obj(json!({"id": 999})) },
        ],
    };
    let e = s.edit_apply(&bad).await.unwrap_err();
    assert!(e.to_string().contains("instead of exactly 1"), "{e}");
    let out = s.query("SELECT name FROM users WHERE id = 1", ExecOptions::default(), None).await.unwrap();
    assert_eq!(out.results[0].rows[0][0], json!("Robert'); DROP TABLE users;--"), "rolled back");
}

#[tokio::test]
async fn cancel_interrupts_a_running_query() {
    let dir = tempfile::tempdir().unwrap();
    let s = std::sync::Arc::new(open(dir.path(), "cancel.db", false).await);
    let runner = s.clone();
    let task = tokio::spawn(async move {
        runner
            .query("WITH RECURSIVE c(x) AS (SELECT 1 UNION ALL SELECT x + 1 FROM c) SELECT count(*) FROM c", ExecOptions::default(), None)
            .await
    });
    // Wait until the statement is running, then cancel it.
    for _ in 0..100 {
        if s.is_running() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    tokio::time::sleep(Duration::from_millis(150)).await;
    assert!(s.cancel().await.unwrap());
    let out = tokio::time::timeout(Duration::from_secs(10), task).await.expect("cancel stops the query").unwrap().unwrap();
    assert!(out.cancelled);
    assert_eq!(out.results[0].error.as_deref(), Some("cancelled"));
    assert!(!s.cancel().await.unwrap(), "nothing running any more");
    // The session is still usable.
    let ok = s.query("SELECT 42", ExecOptions::default(), None).await.unwrap();
    assert_eq!(ok.results[0].rows[0][0], json!(42));
}

#[tokio::test]
async fn seeding_insert_rows_and_query_to_json() {
    let dir = tempfile::tempdir().unwrap();
    let s = setup(dir.path()).await;
    // insert_rows: batched and parameterized.
    let rows: Vec<Map<String, Value>> = (0..1203).map(|i| obj(json!({"name": format!("user {i}"), "email": format!("u{i}@x.test"), "meta": {"i": i}}))).collect();
    assert_eq!(s.insert_rows(None, "users", rows).await.unwrap(), 1203);

    // seed_from_schema: generate an entity with devall-data and insert it.
    let mut schema = devall_data::Schema::new("people");
    schema.seed = Some(42);
    schema.entities.push(devall_data::Entity::new(
        "people",
        25,
        vec![devall_data::Field::new("name", devall_data::FieldKind::FullName), devall_data::Field::new("email", devall_data::FieldKind::email()).unique()],
    ));
    devall_data::store::save(dir.path(), "people.data.yaml", &schema).unwrap();
    let r = s.seed_from_schema(dir.path(), "people.data.yaml", "people", None, "users", None).await.unwrap();
    assert_eq!(r.rows, 25);
    assert!(s.seed_from_schema(dir.path(), "people.data.yaml", "nope", None, "users", None).await.is_err());

    let v = s.query_to_json("SELECT count(*) AS n FROM users", None).await.unwrap();
    assert_eq!(v, json!([{"n": 2 + 1203 + 25}]));
    let v = s.query_to_json("SELECT id, name FROM users ORDER BY id LIMIT 2", None).await.unwrap();
    assert_eq!(v[0], json!({"id": 1, "name": "Ada"}));
    assert!(s.query_to_json("DELETE FROM users", None).await.is_err(), "query → JSON is read-only");
    assert!(s.query_to_json("SELECT 1; SELECT 2", None).await.is_err());
}

#[tokio::test]
async fn explain_and_missing_files() {
    let dir = tempfile::tempdir().unwrap();
    let s = setup(dir.path()).await;
    let plan = s.sql().unwrap().explain("SELECT * FROM orders WHERE user_id = 1", false).await.unwrap();
    assert_eq!(plan.format, "sqlite");
    assert!(plan.text.unwrap().contains("orders_user"), "uses the index");

    // Read-only sessions never create files.
    let mut c = ConnectionFile::new("x", DbKind::Sqlite);
    c.database = Some("missing.db".into());
    let mut r = Resolver::new(&NoSecrets);
    let rc = resolve_connection(&c, &mut r, dir.path()).unwrap();
    let e = Session::connect(rc, &TunnelManager::new(), ConnectOptions::default()).await.err().unwrap();
    assert!(e.to_string().contains("not found"), "{e}");
    assert!(!dir.path().join("missing.db").exists());
}

#[tokio::test]
async fn fake_data_schema_from_the_database_seeds_consistently() {
    let dir = tempfile::tempdir().unwrap();
    let s = open(dir.path(), "shop.db", false).await;
    s.query(
        "PRAGMA foreign_keys = ON;
         CREATE TABLE customers (id INTEGER PRIMARY KEY, first_name TEXT NOT NULL, last_name TEXT, email TEXT NOT NULL UNIQUE, created_at TEXT, avatar BLOB);
         CREATE TABLE products (sku VARCHAR(8) PRIMARY KEY, name TEXT NOT NULL, price NUMERIC(10,2) NOT NULL);
         CREATE TABLE orders (id INTEGER PRIMARY KEY, customer_id INTEGER NOT NULL REFERENCES customers(id),
                              status TEXT NOT NULL CHECK (status IN ('pending', 'paid', 'shipped')), total NUMERIC(10,2), note TEXT);
         CREATE TABLE order_items (id INTEGER PRIMARY KEY, order_id INTEGER NOT NULL REFERENCES orders, product_sku VARCHAR(8) NOT NULL REFERENCES products(sku), quantity INTEGER NOT NULL);",
        ExecOptions::default(),
        None,
    )
    .await
    .unwrap();

    let req = devall_db::fakeschema::FakeSchemaRequest { name: "Shop".into(), options: devall_data::FromDbOptions { count: 10, ..Default::default() }, ..Default::default() };
    let r = s.fake_schema(&req).await.unwrap();
    let names: Vec<&str> = r.schema.entities.iter().map(|e| e.name.as_str()).collect();
    assert_eq!(names, ["customers", "order_items", "orders", "products"]);
    let customers = r.schema.entity("customers").unwrap();
    let kind = |e: &devall_data::Entity, f: &str| e.fields.iter().find(|x| x.name == f).map(|x| x.kind.type_name());
    assert_eq!(kind(customers, "id"), Some("sequence"));
    assert_eq!(kind(customers, "first_name"), Some("firstName"));
    assert_eq!(kind(customers, "email"), Some("email"));
    assert!(customers.fields.iter().any(|f| f.name == "email" && f.unique));
    assert_eq!(kind(customers, "created_at"), Some("datetime"));
    assert_eq!(kind(customers, "avatar"), None, "BLOBs are left to the database");
    assert!(customers.fields.iter().any(|f| f.name == "last_name" && f.null_percent > 0.0), "nullable → null rate");
    let orders = r.schema.entity("orders").unwrap();
    assert!(matches!(&orders.fields.iter().find(|f| f.name == "status").unwrap().kind, devall_data::FieldKind::Enum { values, .. } if values.len() == 3));
    assert_eq!(orders.fields.iter().find(|f| f.name == "customer_id").unwrap().kind, devall_data::FieldKind::reference("customers", "id"));
    let items = r.schema.entity("order_items").unwrap();
    assert_eq!(items.fields.iter().find(|f| f.name == "order_id").unwrap().kind, devall_data::FieldKind::reference("orders", "id"), "implicit PK reference");
    assert_eq!(items.fields.iter().find(|f| f.name == "product_sku").unwrap().kind, devall_data::FieldKind::reference("products", "sku"));
    assert_eq!(orders.count, 20, "child tables get more rows");
    assert_eq!(orders.extra.get("table").and_then(Value::as_str), Some("main.orders"));

    // Save it and seed every table parents-first; the FK-enforcing database accepts every row.
    devall_data::store::save(dir.path(), "shop.data.yaml", &r.schema).unwrap();
    let seeded = s.seed_all_from_schema(dir.path(), "shop.data.yaml", None, None).await.unwrap();
    let targets: Vec<&str> = seeded.iter().map(|x| x.target.as_str()).collect();
    assert_eq!(targets.first(), Some(&"main.customers"), "{targets:?}");
    assert_eq!(seeded.iter().map(|x| x.rows).sum::<u64>(), 10 + 10 + 20 + 20);
    let orphans = s
        .query_to_json("SELECT count(*) AS n FROM order_items i LEFT JOIN orders o ON o.id = i.order_id LEFT JOIN products p ON p.sku = i.product_sku WHERE o.id IS NULL OR p.sku IS NULL", None)
        .await
        .unwrap();
    assert_eq!(orphans, json!([{"n": 0}]));
    let bad_status = s.query_to_json("SELECT count(*) AS n FROM orders WHERE status NOT IN ('pending','paid','shipped')", None).await.unwrap();
    assert_eq!(bad_status, json!([{"n": 0}]));

    // Only a subset: references to excluded tables get plain values plus a warning.
    let req = devall_db::fakeschema::FakeSchemaRequest { tables: vec![devall_db::fakeschema::TableRef { schema: None, name: "orders".into() }], ..Default::default() };
    let r = s.fake_schema(&req).await.unwrap();
    assert_eq!(r.schema.entities.len(), 1);
    assert!(r.warnings.iter().any(|w| w.contains("customers")), "{:?}", r.warnings);
}
