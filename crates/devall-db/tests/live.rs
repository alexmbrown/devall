//! Integration tests against real servers. Each test is skipped unless its URL is set:
//!
//! | Variable | Example |
//! |---|---|
//! | `DEVALL_TEST_PG_URL` | `postgres://postgres:pw@127.0.0.1:5432/postgres` |
//! | `DEVALL_TEST_MYSQL_URL` | `mysql://root:pw@127.0.0.1:3306/test` |
//! | `DEVALL_TEST_REDIS_URL` | `redis://127.0.0.1:6379/15` |
//! | `DEVALL_TEST_MONGO_URL` | `mongodb://127.0.0.1:27017/devall_test` |
//! | `DEVALL_TEST_MSSQL_URL` | `sqlserver://sa:Passw0rd!@127.0.0.1:1433/tempdb?trustServerCertificate=true` |
//!
//! Tests create uniquely named objects and drop them afterwards.

use std::sync::Arc;
use std::time::Duration;

use devall_core::Resolver;
use devall_core::vars::NoSecrets;
use devall_db::sql::edit::{EditRequest, RowEdit};
use devall_db::sql::ExecOptions;
use devall_db::{ConnectOptions, ConnectionFile, DbError, DbKind, Session, TunnelManager, resolve_connection};
use serde_json::{Value, json};

async fn session(var: &str, kind: DbKind) -> Option<Session> {
    let Ok(url) = std::env::var(var) else {
        eprintln!("skipped: set {var} to run this test");
        return None;
    };
    let mut c = ConnectionFile::new("live", kind);
    c.url = Some(url);
    c.read_only = false;
    let mut r = Resolver::new(&NoSecrets);
    let rc = resolve_connection(&c, &mut r, std::path::Path::new(".")).unwrap();
    Some(Session::connect(rc, &TunnelManager::new(), ConnectOptions::default()).await.expect("connect"))
}

fn suffix() -> String {
    ulid::Ulid::generate().to_string().to_lowercase()
}

async fn q(s: &Session, sql: &str) -> devall_db::QueryOutput {
    let out = s.query(sql, ExecOptions::default(), None).await.unwrap();
    for r in &out.results {
        assert!(r.error.is_none(), "{sql}: {:?}", r.error);
    }
    out
}

#[tokio::test]
async fn postgres_live() {
    let Some(s) = session("DEVALL_TEST_PG_URL", DbKind::Postgres).await else { return };
    let t = format!("devall_t_{}", suffix());
    q(&s, &format!(
        "CREATE TABLE {t}_users (id serial PRIMARY KEY, name text NOT NULL, big bigint, price numeric(10,2), ok boolean, doc jsonb, raw bytea, at timestamptz DEFAULT now());
         CREATE TABLE {t}_orders (id serial PRIMARY KEY, user_id int REFERENCES {t}_users(id) ON DELETE CASCADE);
         CREATE INDEX {t}_orders_user ON {t}_orders(user_id);
         INSERT INTO {t}_users (name, big, price, ok, doc, raw) VALUES ('Ada', 9223372036854775807, 12.50, true, '{{\"a\": [1]}}', '\\x00ff'), ('Bob', 1, NULL, false, NULL, NULL);"
    ))
    .await;
    let out = q(&s, &format!("SELECT id, name, big, price, ok, doc, raw FROM {t}_users ORDER BY id; SELECT 1 AS one")).await;
    let r = &out.results[0];
    assert_eq!(r.columns.iter().map(|c| c.type_name.as_str()).collect::<Vec<_>>(), vec!["int4", "text", "int8", "numeric", "bool", "jsonb", "bytea"]);
    assert_eq!(r.rows[0], vec![json!(1), json!("Ada"), json!("9223372036854775807"), json!("12.50"), json!(true), json!({"a": [1]}), json!("AP8=")]);
    assert_eq!(out.results[1].rows, vec![vec![json!(1)]]);

    // Paging and stopping oversized reads early (the session stays usable).
    let big = s.query("SELECT g FROM generate_series(1, 20000000) g", ExecOptions::new(Some(10), None), None).await.unwrap();
    assert_eq!(big.results[0].rows.len(), 10);
    assert!(big.results[0].more);
    let page = q(&s, "SELECT g FROM generate_series(1, 30) g").await;
    assert_eq!(page.results[0].rows.len(), 30);
    let p2 = s.query("SELECT g FROM generate_series(1, 30) g", ExecOptions::new(Some(10), Some(20)), None).await.unwrap();
    assert_eq!(p2.results[0].rows[0], vec![json!(21)]);

    // Schema.
    let sql = s.sql().unwrap();
    let tree = sql.schema().await.unwrap();
    assert!(tree.schemas.iter().flat_map(|x| &x.tables).any(|x| x.name == format!("{t}_users")));
    let d = sql.table_details(Some("public"), &format!("{t}_orders")).await.unwrap();
    assert_eq!(d.primary_key, vec!["id"]);
    assert_eq!(d.foreign_keys[0].on_delete.as_deref(), Some("CASCADE"));
    assert!(d.indexes.iter().any(|i| i.name == format!("{t}_orders_user")));
    assert!(d.ddl.unwrap().contains("CREATE TABLE"));

    // Parameterized edits with text-format params (int key, jsonb, numeric).
    let edit = EditRequest {
        schema: Some("public".into()),
        table: format!("{t}_users"),
        edits: vec![RowEdit::Update { key: json!({"id": 2}).as_object().unwrap().clone(), values: json!({"doc": {"x": 1}, "price": "3.10", "ok": true}).as_object().unwrap().clone() }],
    };
    assert_eq!(s.edit_apply(&edit).await.unwrap(), vec![1]);
    let out = q(&s, &format!("SELECT doc, price, ok FROM {t}_users WHERE id = 2")).await;
    assert_eq!(out.results[0].rows[0], vec![json!({"x": 1}), json!("3.10"), json!(true)]);

    // EXPLAIN (ANALYZE runs inside a rolled-back transaction).
    let plan = sql.explain(&format!("DELETE FROM {t}_orders"), true).await.unwrap();
    assert!(plan.rolled_back);
    assert!(plan.plan.is_array());

    // Read-only: classifier + session characteristics.
    s.set_read_only(true, None).await.unwrap();
    assert!(matches!(s.query(&format!("DELETE FROM {t}_users"), ExecOptions::default(), None).await, Err(DbError::ReadOnly(_))));
    let direct = sql.run_statement(&format!("INSERT INTO {t}_users (name) VALUES ('x')"), ExecOptions::default(), true).await.unwrap_err();
    assert!(direct.to_string().contains("25006"), "{direct}");
    let seq = s.query(&format!("SELECT nextval('{t}_users_id_seq')"), ExecOptions::default(), None).await.unwrap();
    assert!(seq.results[0].error.as_deref().unwrap_or("").contains("read-only"), "functions that write are stopped by the server");
    s.set_read_only(false, None).await.unwrap();

    // Cancel.
    let s = Arc::new(s);
    let runner = s.clone();
    let task = tokio::spawn(async move { runner.query("SELECT pg_sleep(30)", ExecOptions::default(), None).await });
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert!(s.cancel().await.unwrap());
    let out = tokio::time::timeout(Duration::from_secs(10), task).await.unwrap().unwrap().unwrap();
    assert!(out.cancelled);
    q(&s, &format!("DROP TABLE {t}_orders; DROP TABLE {t}_users")).await;
}

#[tokio::test]
async fn mysql_live() {
    let Some(s) = session("DEVALL_TEST_MYSQL_URL", DbKind::Mysql).await else { return };
    let t = format!("devall_t_{}", suffix());
    q(&s, &format!(
        "CREATE TABLE {t} (id INT AUTO_INCREMENT PRIMARY KEY, name VARCHAR(50) NOT NULL, big BIGINT UNSIGNED, price DECIMAL(10,2), doc JSON, raw VARBINARY(4), flag BIT(1), at DATETIME);
         INSERT INTO {t} (name, big, price, doc, raw, flag, at) VALUES ('Ada', 18446744073709551615, 12.50, '{{\"a\": 1}}', x'00ff', b'1', '2024-05-01 10:20:30'), ('Bob', 1, NULL, NULL, NULL, b'0', NULL);"
    ))
    .await;
    let out = q(&s, &format!("SELECT id, name, big, price, doc, raw, flag, at FROM {t} ORDER BY id")).await;
    assert_eq!(out.results[0].rows[0], vec![json!(1), json!("Ada"), json!("18446744073709551615"), json!("12.50"), json!({"a": 1}), json!("AP8="), json!(1), json!("2024-05-01 10:20:30")]);
    let d = s.sql().unwrap().table_details(None, &t).await.unwrap();
    assert_eq!(d.primary_key, vec!["id"]);
    assert!(d.ddl.unwrap().contains("CREATE TABLE"));

    let edit = EditRequest { schema: None, table: t.clone(), edits: vec![RowEdit::Delete { key: json!({"id": 2}).as_object().unwrap().clone() }] };
    assert_eq!(s.edit_apply(&edit).await.unwrap(), vec![1]);

    let plan = s.sql().unwrap().explain(&format!("SELECT * FROM {t} WHERE id = 1"), false).await.unwrap();
    assert!(plan.format == "mysql" || plan.format == "text");

    s.set_read_only(true, None).await.unwrap();
    assert!(matches!(s.query(&format!("UPDATE {t} SET name = 'x'"), ExecOptions::default(), None).await, Err(DbError::ReadOnly(_))));
    let direct = s.sql().unwrap().run_statement(&format!("UPDATE {t} SET name = 'x'"), ExecOptions::default(), true).await.unwrap_err();
    assert!(direct.to_string().contains("1792"), "{direct}");
    s.set_read_only(false, None).await.unwrap();

    let s = Arc::new(s);
    let runner = s.clone();
    let task = tokio::spawn(async move { runner.query("SELECT SLEEP(30)", ExecOptions::default(), None).await });
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert!(s.cancel().await.unwrap());
    tokio::time::timeout(Duration::from_secs(10), task).await.expect("KILL QUERY stops SLEEP").unwrap().unwrap();
    q(&s, &format!("DROP TABLE {t}")).await;
}

#[tokio::test]
async fn sqlserver_live() {
    let Some(s) = session("DEVALL_TEST_MSSQL_URL", DbKind::Sqlserver).await else { return };
    assert!(s.info().server_version.contains("SQL"), "{}", s.info().server_version);
    let t = format!("devall_t_{}", suffix());
    q(&s, &format!(
        "CREATE TABLE dbo.{t}_users (id int IDENTITY(1,1) PRIMARY KEY, name nvarchar(50) NOT NULL, big bigint, price decimal(10,2), ok bit, uid uniqueidentifier, raw varbinary(4), at datetime2(3), status nvarchar(10) NOT NULL DEFAULT N'open' CHECK (status IN (N'open', N'closed')));
         CREATE TABLE dbo.{t}_orders (id int IDENTITY PRIMARY KEY, user_id int REFERENCES dbo.{t}_users(id) ON DELETE CASCADE);
         CREATE INDEX ix_{t}_user ON dbo.{t}_orders(user_id);
         INSERT INTO dbo.{t}_users (name, big, price, ok, uid, raw, at) VALUES (N'Ada', 9223372036854775807, 12.50, 1, '6F9619FF-8B86-D011-B42D-00C04FC964FF', 0x00FF, '2024-05-01T10:20:30.123'), (N'Bob', 1, NULL, 0, NULL, NULL, NULL)"
    ))
    .await;
    // GO batches; variables need the whole batch; several result sets from one statement.
    let out = q(&s, &format!("SELECT id, name, big, price, ok, uid, raw, at FROM dbo.{t}_users ORDER BY id\nGO\nDECLARE @n int = 2;\nSELECT @n AS n;\nSELECT @n + 1 AS m")).await;
    let r = &out.results[0];
    assert_eq!(r.columns.iter().map(|c| c.type_name.as_str()).collect::<Vec<_>>(), vec!["int", "nvarchar", "bigint", "decimal", "bit", "uniqueidentifier", "varbinary", "datetime2"]);
    assert_eq!(r.rows[0], vec![json!(1), json!("Ada"), json!("9223372036854775807"), json!("12.50"), json!(true), json!("6F9619FF-8B86-D011-B42D-00C04FC964FF"), json!("AP8="), json!("2024-05-01T10:20:30.123")]);
    assert_eq!((out.results[1].rows.clone(), out.results[2].rows.clone()), (vec![vec![json!(2)]], vec![vec![json!(3)]]));

    // Row caps (SET ROWCOUNT) and paging.
    let series = "SELECT TOP (50000) ROW_NUMBER() OVER (ORDER BY (SELECT NULL)) AS g FROM sys.all_objects a CROSS JOIN sys.all_objects b ORDER BY g";
    let big = s.query(series, ExecOptions::new(Some(10), None), None).await.unwrap();
    assert_eq!(big.results[0].rows.len(), 10);
    assert!(big.results[0].more);
    let p2 = s.query(series, ExecOptions::new(Some(10), Some(20)), None).await.unwrap();
    assert_eq!(p2.results[0].rows[0], vec![json!(21)]);
    // The cap never leaks into writes.
    let upd = q(&s, &format!("UPDATE dbo.{t}_users SET big = big")).await;
    assert_eq!(upd.results[0].rows_affected, Some(2));

    // Schema, details, DDL.
    let sql = s.sql().unwrap();
    let tree = sql.schema().await.unwrap();
    assert_eq!(tree.default_schema.as_deref(), Some("dbo"));
    assert!(tree.schemas.iter().flat_map(|x| &x.tables).any(|x| x.name == format!("{t}_users")));
    let d = sql.table_details(Some("dbo"), &format!("{t}_orders")).await.unwrap();
    assert_eq!(d.primary_key, vec!["id"]);
    assert_eq!(d.foreign_keys[0].on_delete.as_deref(), Some("CASCADE"));
    assert!(d.indexes.iter().any(|i| i.name == format!("ix_{t}_user")));
    let u = sql.table_details(None, &format!("{t}_users")).await.unwrap();
    assert!(u.ddl.as_deref().unwrap().contains("IDENTITY(1,1)"));
    let spec = devall_db::fakeschema::table_spec(&u, &Default::default());
    assert_eq!(spec.columns.iter().find(|c| c.name == "status").unwrap().enum_values, vec!["closed".to_string(), "open".into()]);

    // Parameterized edits (@P1…) in a transaction.
    let edit = EditRequest {
        schema: Some("dbo".into()),
        table: format!("{t}_users"),
        edits: vec![RowEdit::Update { key: json!({"id": 2}).as_object().unwrap().clone(), values: json!({"price": "3.10", "ok": true, "at": "2024-01-02T03:04:05"}).as_object().unwrap().clone() }],
    };
    assert_eq!(s.edit_apply(&edit).await.unwrap(), vec![1]);
    let out = q(&s, &format!("SELECT price, ok, at FROM dbo.{t}_users WHERE id = 2")).await;
    assert_eq!(out.results[0].rows[0], vec![json!("3.10"), json!(true), json!("2024-01-02T03:04:05.000")]);
    let rows: Vec<serde_json::Map<String, Value>> = (0..1500).map(|i| json!({"name": format!("n{i}"), "big": i}).as_object().unwrap().clone()).collect();
    assert_eq!(s.insert_rows(Some("dbo"), &format!("{t}_users"), rows).await.unwrap(), 1500);

    // SHOWPLAN XML → plan tree; STATISTICS XML inside a rolled-back transaction.
    let plan = sql.explain(&format!("SELECT * FROM dbo.{t}_users WHERE id = 1"), false).await.unwrap();
    assert_eq!(plan.format, "sqlserver");
    assert!(plan.plan[0]["Plan"]["Node Type"].is_string(), "{}", plan.plan);
    let actual = sql.explain(&format!("DELETE FROM dbo.{t}_orders"), true).await.unwrap();
    assert!(actual.rolled_back);

    // Read-only: refused before anything is sent.
    s.set_read_only(true, None).await.unwrap();
    assert!(matches!(s.query(&format!("DELETE FROM dbo.{t}_users"), ExecOptions::default(), None).await, Err(DbError::ReadOnly(_))));
    assert!(matches!(s.query("EXEC sp_who", ExecOptions::default(), None).await, Err(DbError::ReadOnly(_))));
    q(&s, "SELECT * INTO #scratch FROM sys.objects; SELECT COUNT(*) FROM #scratch").await;
    s.set_read_only(false, None).await.unwrap();

    // Cancel (TDS attention); the session stays usable.
    let s = Arc::new(s);
    let runner = s.clone();
    let task = tokio::spawn(async move { runner.query("WAITFOR DELAY '00:00:30'", ExecOptions::default(), None).await });
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert!(s.cancel().await.unwrap());
    let out = tokio::time::timeout(Duration::from_secs(10), task).await.unwrap().unwrap().unwrap();
    assert!(out.cancelled || out.results[0].error.as_deref() == Some("cancelled"), "{out:?}");
    q(&s, &format!("DROP TABLE dbo.{t}_orders; DROP TABLE dbo.{t}_users")).await;
}

#[tokio::test]
async fn redis_live() {
    let Some(s) = session("DEVALL_TEST_REDIS_URL", DbKind::Redis).await else { return };
    let r = s.redis().unwrap();
    let p = format!("devall:{}:", suffix());
    for line in [
        format!("SET {p}str hello"),
        format!("HSET {p}hash a 1 b 2"),
        format!("RPUSH {p}list x y z"),
        format!("SADD {p}set m1 m2"),
        format!("ZADD {p}zset 1.5 one 2 two"),
        format!("XADD {p}stream * k v"),
        format!("EXPIRE {p}str 100"),
    ] {
        let reply = r.command(&line, false).await.unwrap();
        assert!(reply.error.is_none(), "{line}: {reply:?}");
    }
    let page = r.scan("0", Some(&format!("{p}*")), None, 100).await.unwrap();
    let mut all = page.keys;
    let mut cursor = page.cursor;
    while cursor != "0" {
        let more = r.scan(&cursor, Some(&format!("{p}*")), None, 100).await.unwrap();
        all.extend(more.keys);
        cursor = more.cursor;
    }
    assert_eq!(all.len(), 6);
    assert!(all.iter().any(|k| k.kind == "zset"));
    assert!(all.iter().find(|k| k.key.ends_with("str")).unwrap().ttl_ms > 0);
    use devall_db::redisdb::{RedisKey, RedisValue};
    let v = r.get(&RedisKey::Text(format!("{p}hash")), None, 0, 100).await.unwrap();
    assert!(matches!(v.value, RedisValue::Hash { ref fields, len: 2, .. } if fields.len() == 2));
    let v = r.get(&RedisKey::Text(format!("{p}zset")), None, 0, 100).await.unwrap();
    assert!(matches!(v.value, RedisValue::Zset { ref members, .. } if members[0].score == json!(1.5)));
    assert!(r.command(&format!("DEL {p}str"), true).await.is_err(), "writes are blocked read-only");
    assert!(r.memory_usage(&RedisKey::Text(format!("{p}str"))).await.unwrap().is_some());

    // Pub/sub round trip.
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<Value>();
    let handle = r
        .subscribe(&[format!("{p}chan")], &[], Arc::new(move |m| {
            let _ = tx.send(json!(m.payload));
        }))
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(r.publish(&format!("{p}chan"), "ping").await.unwrap(), 1);
    assert_eq!(tokio::time::timeout(Duration::from_secs(5), rx.recv()).await.unwrap(), Some(json!("ping")));
    handle.abort();
    let keys: Vec<RedisKey> = all.iter().map(|k| RedisKey::Text(k.key.clone())).collect();
    assert_eq!(r.delete(&keys).await.unwrap(), 6);
}

#[tokio::test]
async fn mongo_live() {
    let Some(s) = session("DEVALL_TEST_MONGO_URL", DbKind::Mongodb).await else { return };
    use devall_db::mongo::{AggregateRequest, FindRequest, MongoWrite};
    let m = s.mongo().unwrap();
    let db = s.info().database.unwrap_or_else(|| "devall_test".into());
    let coll = format!("c_{}", suffix());
    m.write(&MongoWrite::Insert { database: db.clone(), collection: coll.clone(), documents: "[{name: 'Ada', age: 36, at: ISODate('2024-05-01')}, {name: 'Bob', age: 20}]".into() }).await.unwrap();
    let found = m
        .find(&FindRequest { database: db.clone(), collection: coll.clone(), filter: Some("{ age: { $gt: 30 } }".into()), sort: Some("{name: 1}".into()), ..Default::default() })
        .await
        .unwrap();
    assert_eq!(found.docs.len(), 1);
    assert_eq!(found.docs[0]["name"], json!("Ada"));
    assert!(found.docs[0]["_id"]["$oid"].is_string());
    assert!(found.docs[0]["at"]["$date"].is_string());
    let agg = m.aggregate(&AggregateRequest { database: db.clone(), collection: coll.clone(), pipeline: "[{$group: {_id: null, n: {$sum: 1}}}]".into(), limit: None }, true).await.unwrap();
    assert_eq!(agg.docs[0]["n"], json!(2));
    assert!(matches!(
        m.aggregate(&AggregateRequest { database: db.clone(), collection: coll.clone(), pipeline: "[{$out: 'copy'}]".into(), limit: None }, true).await,
        Err(DbError::ReadOnly(_))
    ));
    assert_eq!(m.count(&db, &coll, Some("{name: 'Bob'}")).await.unwrap(), 1);
    assert!(!m.indexes(&db, &coll).await.unwrap().is_empty());
    assert!(m.explain_find(&FindRequest { database: db.clone(), collection: coll.clone(), ..Default::default() }, false).await.unwrap().is_object());
    assert!(m.command(&db, "{dropDatabase: 1}", true).await.is_err());
    m.command(&db, &format!("{{drop: '{coll}'}}"), false).await.unwrap();
}
