//! End-to-end tests: a real project on disk, a local server with controllable latency, and the
//! engine on its own runtime. Engine tests run one at a time (timing-sensitive).

mod server;

use std::sync::atomic::Ordering;
use std::time::Duration;

use devall_core::{HttpRequestFile, KeyValue, MemorySecretStore, Project, SecretScope, SecretStore};

use crate::cli;
use crate::compare::{CompareOptions, Verdict, compare};
use crate::engine::{RunControl, RunOptions, run};
use crate::error::LoadError;
use crate::model::*;
use crate::plan::build_plan;
use crate::store;
use crate::summary::{LoadSnapshot, LoadSummary, Percentiles, RunPhase};
use std::sync::Arc;

static ENGINE: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

struct Fixture {
    _dir: tempfile::TempDir,
    project: Project,
    secrets: MemorySecretStore,
}

/// A project whose `local` environment points `baseUrl` at `base`, with requests
/// `(path, method, url)`.
fn fixture(base: &str, requests: &[(&str, &str, &str)]) -> Fixture {
    devall_core::local::isolate_for_tests();
    let dir = tempfile::tempdir().unwrap();
    let project = Project::init(dir.path(), "Load demo").unwrap();
    let mut env = project.read_environment("local").unwrap();
    env.variables.insert("baseUrl".into(), base.into());
    project.save_environment("local", &env).unwrap();
    for (path, method, url) in requests {
        let mut r = HttpRequestFile::new(path.trim_end_matches(".req.yaml"));
        r.method = method.to_string();
        r.url = url.to_string();
        project.save_request(path, &r).unwrap();
    }
    Fixture { _dir: dir, project, secrets: MemorySecretStore::default() }
}

fn test_file(requests: &[(&str, u32)], executor: Executor) -> LoadTestFile {
    let mut t = LoadTestFile::template("T", None);
    t.scenario.requests = requests.iter().map(|(p, w)| WeightedRequest { path: p.to_string(), weight: *w }).collect();
    t.executor = executor;
    t.thresholds.clear();
    t
}

fn rate(rate: f64, duration: &str, pre: u32, max: Option<u32>) -> Executor {
    Executor::ConstantArrivalRate { rate, time_unit: "1s".into(), duration: duration.into(), pre_allocated_vus: pre, max_vus: max }
}

fn opts() -> RunOptions {
    RunOptions {
        worker_threads: Some(2),
        snapshot_interval: Duration::from_millis(100),
        graceful_stop: Duration::from_secs(5),
        abort_grace: Duration::from_secs(1),
        ..Default::default()
    }
}

async fn run_test(f: &Fixture, test: &LoadTestFile) -> LoadSummary {
    let plan = build_plan(&f.project, "t.load.yaml", test, Some("local"), &f.secrets).unwrap();
    run(plan, opts(), None, RunControl::new()).await.unwrap()
}

// ---------- file format ----------

const EXAMPLE: &str = "\
version: 1
name: Users API
environment: local
scenario:
  requests:
  - path: users/list.req.yaml
    weight: 3
  - path: users/get.req.yaml
executor:
  type: rampingArrivalRate
  startRate: 5
  timeUnit: 1m
  stages:
  - duration: 30s
    target: 50
  - duration: 2m
    target: 50.5
  preAllocatedVus: 10
  maxVus: 100
thinkTime:
  min: 100ms
  max: 1s
thresholds:
- p95 < 300ms
- errorRate < 1%
checks:
- type: status
  expect:
  - 2xx
- type: bodyContains
  text: id
http:
  timeoutMs: 5000
  http2: true
futureField:
  keep: me
";

#[test]
fn yaml_round_trip_is_stable() {
    let t: LoadTestFile = serde_yaml_ng::from_str(EXAMPLE).unwrap();
    assert!(crate::validate::validate(&t).is_empty(), "{:?}", crate::validate::validate(&t));
    assert_eq!(t.scenario.requests[1].weight, 1);
    assert!(matches!(t.executor, Executor::RampingArrivalRate { start_rate, .. } if start_rate == 5.0));
    assert_eq!(t.executor.planned_duration(), Duration::from_secs(150));
    assert!(matches!(t.think_time, Some(ThinkTime::Range { .. })));
    assert!(t.extra.contains_key("futureField"));
    assert_eq!(serde_yaml_ng::to_string(&t).unwrap(), EXAMPLE);

    // Short forms.
    let t: LoadTestFile = serde_yaml_ng::from_str(
        "name: Flow\nscenario:\n  flow: [auth/login.req.yaml, users/list.req.yaml]\nexecutor: { type: constantVus, vus: 5, duration: 30 }\nthinkTime: 1s\n",
    )
    .unwrap();
    assert!(t.scenario.is_flow());
    assert_eq!(t.executor.planned_duration(), Duration::from_secs(30));
    assert_eq!(t.think_time, Some(ThinkTime::Fixed("1s".into())));
    let text = serde_yaml_ng::to_string(&t).unwrap();
    assert!(text.starts_with("version: 1\nname: Flow\n"), "{text}");
    assert!(!text.contains("http:") && !text.contains("checks"), "{text}");
}

#[test]
fn store_crud_and_listing() {
    let f = fixture("http://127.0.0.1:1", &[("a.req.yaml", "GET", "{{baseUrl}}/")]);
    let root = f.project.root();
    let path = store::create_test(root, "Checkout spike", Some(LoadTestFile::template("x", Some("a.req.yaml".into())))).unwrap();
    assert_eq!(path, "checkout-spike.load.yaml");
    assert_eq!(store::create_test(root, "Checkout spike", None).unwrap(), "checkout-spike-2.load.yaml");
    let mut t = store::read_test(root, &path).unwrap();
    assert_eq!(t.name, "Checkout spike");
    t.thresholds = vec!["p99 < 1s".into()];
    store::save_test(root, &path, &t).unwrap();
    assert_eq!(store::read_test(root, &path).unwrap(), t);
    std::fs::create_dir_all(root.join("load/nested")).unwrap();
    std::fs::write(root.join("load/nested/broken.load.yaml"), "name: [unclosed").unwrap();
    let list = store::list_tests(root).unwrap();
    assert_eq!(list.iter().map(|e| e.path.as_str()).collect::<Vec<_>>(), vec!["checkout-spike-2.load.yaml", "checkout-spike.load.yaml", "nested/broken.load.yaml"]);
    assert!(list[2].error.is_some());
    assert_eq!(list[1].executor.as_deref(), Some("constantArrivalRate"));
    assert!(store::read_test(root, "../devall.yaml").is_err());
    assert!(store::read_test(root, "runs/x.load.yaml").is_err());
    assert_eq!(store::test_slug("nested/a.load.yaml"), "nested/a");
}

#[test]
fn plan_reports_missing_requests_and_validation() {
    let f = fixture("http://127.0.0.1:1", &[("a.req.yaml", "GET", "{{baseUrl}}/{{missing}}")]);
    let t = test_file(&[("nope.req.yaml", 1)], rate(1.0, "1s", 1, None));
    match build_plan(&f.project, "t.load.yaml", &t, Some("local"), &f.secrets) {
        Err(LoadError::Request { path, .. }) => assert_eq!(path, "nope.req.yaml"),
        other => panic!("{other:?}"),
    }
    let bad = test_file(&[], rate(0.0, "1s", 1, None));
    assert!(matches!(build_plan(&f.project, "t.load.yaml", &bad, None, &f.secrets), Err(LoadError::Validation(_))));
    let ok = test_file(&[("a.req.yaml", 1)], rate(1.0, "1s", 1, None));
    let plan = build_plan(&f.project, "t.load.yaml", &ok, Some("local"), &f.secrets).unwrap();
    assert_eq!(plan.unresolved, vec!["{{missing}} in a.req.yaml"]);
    assert!(plan.preflight().warnings.iter().any(|w| w.contains("{{missing}}")));
}

#[test]
fn preflight_guard_rails() {
    let f = fixture(
        "http://127.0.0.1:9",
        &[("remote.req.yaml", "GET", "http://example.com/x"), ("local.req.yaml", "GET", "{{baseUrl}}/"), ("post.req.yaml", "POST", "{{baseUrl}}/")],
    );
    let plan = |t: &LoadTestFile, env: Option<&str>| build_plan(&f.project, "t.load.yaml", t, env, &f.secrets).unwrap().preflight();

    // High rate at a host that isn't this machine → confirm.
    let p = plan(&test_file(&[("remote.req.yaml", 1)], rate(200.0, "10s", 10, Some(100))), None);
    let c = p.requires_confirmation.expect("needs confirmation");
    assert_eq!(c.hosts, vec!["example.com:80"]);
    assert!(c.reason.contains("example.com"));
    assert_eq!(p.expected_iterations, Some(2000));
    // Low rate, or local target → no confirmation.
    assert!(plan(&test_file(&[("remote.req.yaml", 1)], rate(10.0, "10s", 10, None)), None).requires_confirmation.is_none());
    assert!(plan(&test_file(&[("local.req.yaml", 1)], rate(5000.0, "10s", 10, Some(500))), Some("local")).requires_confirmation.is_none());
    // Many VUs at a remote host (closed model) → confirm.
    let vus = Executor::ConstantVus { vus: 60, duration: "10s".into() };
    assert!(plan(&test_file(&[("remote.req.yaml", 1)], vus), None).requires_confirmation.is_some());

    // Protected environment + writes → confirm, even locally.
    let mut env = f.project.read_environment("local").unwrap();
    env.protected = true;
    f.project.save_environment("local", &env).unwrap();
    let p = plan(&test_file(&[("post.req.yaml", 1)], rate(1.0, "1s", 1, None)), Some("local"));
    assert!(p.requires_confirmation.unwrap().reason.contains("protected"));
    assert!(plan(&test_file(&[("local.req.yaml", 1)], rate(1.0, "1s", 1, None)), Some("local")).requires_confirmation.is_none());

    // `Connection: close` at 100/s for 5 minutes would exhaust Windows' ephemeral ports.
    let mut req = f.project.read_request("local.req.yaml").unwrap();
    req.headers.push(KeyValue::new("Connection", "close"));
    f.project.save_request("closing.req.yaml", &req).unwrap();
    let p = plan(&test_file(&[("closing.req.yaml", 1)], rate(100.0, "5m", 10, None)), Some("local"));
    assert!(p.warnings.iter().any(|w| w.contains("ephemeral ports")), "{:?}", p.warnings);
    let p = plan(&test_file(&[("closing.req.yaml", 1)], rate(5.0, "10s", 10, None)), Some("local"));
    assert!(!p.warnings.iter().any(|w| w.contains("ephemeral ports")), "{:?}", p.warnings);
}

#[tokio::test]
async fn cli_refuses_unconfirmed_remote_load() {
    let f = fixture("http://127.0.0.1:9", &[("remote.req.yaml", "GET", "http://example.com/x")]);
    let t = test_file(&[("remote.req.yaml", 1)], rate(500.0, "10s", 10, Some(100)));
    store::save_test(f.project.root(), "remote.load.yaml", &t).unwrap();
    let err = cli::run_test_file(f.project.root(), "remote.load.yaml", &f.secrets, cli::CliRunOptions::default()).await.err().unwrap();
    assert!(matches!(err, LoadError::ConfirmationRequired(_)), "{err}");
    assert_eq!(cli::error_exit_code(&err), cli::EXIT_CONFIRMATION_REQUIRED);
}

// ---------- engine ----------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn constant_vus_runs_closed_loop() {
    let _g = ENGINE.lock().await;
    let srv = server::start().await;
    let f = fixture(&srv.base, &[("slow.req.yaml", "GET", "{{baseUrl}}/delay/20")]);
    let t = test_file(&[("slow.req.yaml", 1)], Executor::ConstantVus { vus: 4, duration: "1s".into() });
    let s = run_test(&f, &t).await;
    assert!(!s.aborted && !s.open_model && s.service_time.is_none());
    assert_eq!(s.totals.max_vus, 4);
    assert_eq!(s.totals.failed, 0);
    // Each VU loops back-to-back: iterations ≈ VUs × duration / mean latency.
    let expected = 4.0 * 1000.0 / s.latency.mean;
    let got = s.totals.iterations as f64;
    assert!((got - expected).abs() <= expected * 0.25 + 4.0, "iterations {got}, expected ≈{expected:.0} (mean {} ms)", s.latency.mean);
    assert!(s.latency.p50 >= 19.0, "{:?}", s.latency);
    // Keep-alive: about one connection per VU (a pool return can race the next send).
    assert!(srv.state.connections.load(Ordering::SeqCst) <= 8, "connections {}", srv.state.connections.load(Ordering::SeqCst));
    assert_eq!(s.totals.requests, srv.state.requests.load(Ordering::SeqCst));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ramping_vus_reaches_target() {
    let _g = ENGINE.lock().await;
    let srv = server::start().await;
    let f = fixture(&srv.base, &[("r.req.yaml", "GET", "{{baseUrl}}/delay/5")]);
    let ex = Executor::RampingVus {
        start_vus: 0,
        stages: vec![Stage { duration: "600ms".into(), target: 6.0 }, Stage { duration: "600ms".into(), target: 6.0 }, Stage { duration: "300ms".into(), target: 0.0 }],
    };
    let s = run_test(&f, &test_file(&[("r.req.yaml", 1)], ex)).await;
    assert_eq!(s.totals.max_vus, 6);
    assert!(s.totals.iterations > 50, "{}", s.totals.iterations);
    assert!((1400..3500).contains(&s.duration_ms), "{}", s.duration_ms);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn constant_arrival_rate_is_exact() {
    let _g = ENGINE.lock().await;
    let srv = server::start().await;
    let f = fixture(&srv.base, &[("ok.req.yaml", "GET", "{{baseUrl}}/")]);
    let s = run_test(&f, &test_file(&[("ok.req.yaml", 1)], rate(50.0, "2s", 5, Some(20)))).await;
    assert!(s.open_model);
    assert_eq!(s.totals.iterations, 100);
    assert_eq!(s.totals.dropped_iterations, 0);
    assert!((45.0..=52.0).contains(&s.totals.rps), "rps {}", s.totals.rps);
    assert!(s.service_time.is_some());
    // The timeline has one bucket per second, each ≈ 50 requests.
    assert_eq!(s.bucket_seconds, 1);
    assert!(s.timeline.len() >= 2, "{:?}", s.timeline);
    assert!(s.timeline.iter().take(2).all(|p| (45..=55).contains(&p.requests)), "{:?}", s.timeline);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ramping_arrival_rate_follows_profile() {
    let _g = ENGINE.lock().await;
    let srv = server::start().await;
    let f = fixture(&srv.base, &[("ok.req.yaml", "GET", "{{baseUrl}}/")]);
    // 0 → 60/s over 1s (30 iterations), then 60/s for 1s (60).
    let ex = Executor::RampingArrivalRate {
        start_rate: 0.0,
        time_unit: "1s".into(),
        stages: vec![Stage { duration: "1s".into(), target: 60.0 }, Stage { duration: "1s".into(), target: 60.0 }],
        pre_allocated_vus: 5,
        max_vus: Some(20),
    };
    let s = run_test(&f, &test_file(&[("ok.req.yaml", 1)], ex)).await;
    assert_eq!(s.totals.iterations, 90);
    assert!(s.timeline[0].requests < s.timeline[1].requests, "{:?}", s.timeline);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn exhausted_vus_drop_iterations() {
    let _g = ENGINE.lock().await;
    let srv = server::start().await;
    let f = fixture(&srv.base, &[("slow.req.yaml", "GET", "{{baseUrl}}/delay/300")]);
    let s = run_test(&f, &test_file(&[("slow.req.yaml", 1)], rate(40.0, "1s", 2, Some(4)))).await;
    assert_eq!(s.totals.iterations + s.totals.dropped_iterations, 40);
    assert!(s.totals.dropped_iterations >= 20, "dropped {}", s.totals.dropped_iterations);
    assert_eq!(s.totals.max_vus, 4);
    assert!(srv.state.max_in_flight.load(Ordering::SeqCst) <= 4);
    assert!(s.warnings.iter().any(|w| w.contains("dropped")), "{:?}", s.warnings);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn open_model_corrects_coordinated_omission() {
    let _g = ENGINE.lock().await;
    let stall = Duration::from_millis(600);

    // Open model: iterations keep arriving during the stall and wait for it → large p99.
    let srv = server::start().await;
    let f = fixture(&srv.base, &[("ok.req.yaml", "GET", "{{baseUrl}}/")]);
    let st = srv.state.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(500)).await;
        st.stall_for(stall);
    });
    let open = run_test(&f, &test_file(&[("ok.req.yaml", 1)], rate(50.0, "2s", 50, Some(100)))).await;

    // Closed model, same stall: one VU waits once, everything else is fast → small p99.
    let srv2 = server::start().await;
    let f2 = fixture(&srv2.base, &[("ok.req.yaml", "GET", "{{baseUrl}}/")]);
    let st2 = srv2.state.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(500)).await;
        st2.stall_for(stall);
    });
    let closed = run_test(&f2, &test_file(&[("ok.req.yaml", 1)], Executor::ConstantVus { vus: 1, duration: "2s".into() })).await;

    assert!(open.latency.p99 >= 400.0, "open p99 {:?}", open.latency);
    assert!(open.latency.p90 >= 150.0, "open p90 {:?}", open.latency);
    assert!(closed.latency.max >= 400.0, "closed max {:?}", closed.latency);
    assert!(closed.latency.p99 * 5.0 < open.latency.p99, "closed {:?} vs open {:?}", closed.latency, open.latency);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn connection_queueing_counts_toward_corrected_latency() {
    let _g = ENGINE.lock().await;
    let srv = server::start().await;
    let f = fixture(&srv.base, &[("ok.req.yaml", "GET", "{{baseUrl}}/")]);
    let st = srv.state.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(300)).await;
        st.stall_for(Duration::from_millis(400));
    });
    let mut t = test_file(&[("ok.req.yaml", 1)], rate(50.0, "1s", 50, Some(50)));
    t.http.max_connections = Some(1);
    let s = run_test(&f, &t).await;
    assert_eq!(srv.state.max_in_flight.load(Ordering::SeqCst), 1);
    let service = s.service_time.unwrap();
    assert!(s.latency.p95 >= 150.0, "corrected {:?}", s.latency);
    assert!(service.p95 < 100.0, "service {service:?}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn stop_produces_aborted_summary() {
    let _g = ENGINE.lock().await;
    let srv = server::start().await;
    let f = fixture(&srv.base, &[("slow.req.yaml", "GET", "{{baseUrl}}/delay/10")]);
    let t = test_file(&[("slow.req.yaml", 1)], Executor::ConstantVus { vus: 5, duration: "30s".into() });
    let plan = build_plan(&f.project, "t.load.yaml", &t, Some("local"), &f.secrets).unwrap();
    let control = RunControl::new();
    let c = control.clone();
    let snaps = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let sink = snaps.clone();
    let progress: crate::engine::ProgressFn = std::sync::Arc::new(move |s| sink.lock().unwrap().push(s));
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(400)).await;
        c.stop();
    });
    let started = std::time::Instant::now();
    let s = run(plan, opts(), Some(progress), control).await.unwrap();
    assert!(started.elapsed() < Duration::from_secs(5));
    assert!(s.aborted);
    assert_eq!(s.abort_reason.as_deref(), Some("stopped by user"));
    assert!(s.totals.requests > 10);
    assert_eq!(cli::exit_code(&s), cli::EXIT_ABORTED);
    let snaps = snaps.lock().unwrap();
    assert!(snaps.len() >= 3, "{}", snaps.len());
    assert_eq!(snaps.last().unwrap().phase, crate::summary::RunPhase::Finished);
    assert!(snaps.iter().any(|s| s.vus == 5));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn flow_keeps_order_cookies_and_fresh_dynamic_values() {
    let _g = ENGINE.lock().await;
    let srv = server::start().await;
    let f = fixture(&srv.base, &[("auth/login.req.yaml", "POST", "{{baseUrl}}/login"), ("me.req.yaml", "GET", "{{baseUrl}}/me?id={{$uuid}}")]);
    let mut t = test_file(&[], Executor::ConstantVus { vus: 2, duration: "500ms".into() });
    t.scenario.flow = vec!["auth/login.req.yaml".into(), "me.req.yaml".into()];
    t.thresholds = vec!["errorRate == 0".into(), "p95{me.req.yaml} < 5s".into()];
    let s = run_test(&f, &t).await;
    assert!(s.thresholds_passed, "{:?}", s.thresholds);
    let paths = srv.state.paths();
    // Steps run in order per VU, and each `/me` carried the cookie from its `/login`.
    assert_eq!(paths.first().map(|p| p.as_str()), Some("/login"));
    let me: Vec<&String> = paths.iter().filter(|p| p.starts_with("/me")).collect();
    let logins = paths.iter().filter(|p| *p == "/login").count();
    assert!(!me.is_empty() && logins >= me.len(), "{paths:?}");
    let distinct: std::collections::HashSet<&&String> = me.iter().collect();
    assert_eq!(distinct.len(), me.len(), "{{{{$uuid}}}} must change per iteration");
    assert_eq!(s.requests.len(), 2);
    assert_eq!(s.requests[1].status_codes.get("200").copied(), Some(me.len() as u64));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn thresholds_errors_and_checks() {
    let _g = ENGINE.lock().await;
    let srv = server::start().await;
    let f = fixture(&srv.base, &[("ok.req.yaml", "GET", "{{baseUrl}}/"), ("boom.req.yaml", "GET", "{{baseUrl}}/status/500")]);
    let mut t = test_file(&[("ok.req.yaml", 3), ("boom.req.yaml", 1)], rate(80.0, "1s", 10, Some(20)));
    t.thresholds = vec!["errorRate < 1%".into(), "p95 < 10s".into(), "requests > 0".into(), "p99{ok.req.yaml} < 10s".into(), "checks > 99%".into(), "dropped == 0".into()];
    t.checks = vec![Check::Status { expect: vec!["2xx".into()] }, Check::BodyContains { text: "ok".into() }];
    let s = run_test(&f, &t).await;
    let ok: Vec<bool> = s.thresholds.iter().map(|r| r.ok).collect();
    assert_eq!(ok, vec![false, true, true, true, false, true], "{:#?}", s.thresholds);
    assert!(!s.thresholds_passed);
    assert_eq!(cli::exit_code(&s), cli::EXIT_THRESHOLDS_FAILED);
    assert!((0.1..0.45).contains(&s.totals.error_rate), "{}", s.totals.error_rate);
    assert!(s.status_codes.contains_key("200") && s.status_codes.contains_key("500"));
    let boom = s.requests.iter().find(|r| r.path == "boom.req.yaml").unwrap();
    assert_eq!(boom.error_rate, 1.0);
    // Two checks per response.
    assert_eq!(s.totals.checks_passed + s.totals.checks_failed, s.totals.requests * 2);
    assert!(s.thresholds[0].message.starts_with("errorRate = "), "{}", s.thresholds[0].message);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn secrets_never_reach_the_summary() {
    let _g = ENGINE.lock().await;
    let port = server::closed_port();
    let f = fixture("http://127.0.0.1:1", &[("s.req.yaml", "GET", &format!("http://127.0.0.1:{port}/x?token={{{{secret.TOKEN}}}}"))]);
    let scope = SecretScope::new(f.project.id(), "local");
    f.secrets.set(&scope, "TOKEN", "supersecretvalue").unwrap();
    // Windows takes ~2 s to refuse a loopback connection, so give every iteration its own VU.
    let t = test_file(&[("s.req.yaml", 1)], rate(20.0, "500ms", 10, Some(20)));
    let plan = build_plan(&f.project, "t.load.yaml", &t, Some("local"), &f.secrets).unwrap();
    assert!(plan.secret_values.contains("supersecretvalue"));
    let secrets = plan.secret_values.clone();
    let s = run(plan, opts(), None, RunControl::new()).await.unwrap();
    assert_eq!(s.error_kinds.connect, s.totals.requests);
    assert!(s.totals.requests >= 9);
    assert_eq!(s.latency.p50, 0.0, "refused connections are not latency samples");
    let json = serde_json::to_string(&s).unwrap();
    assert!(!json.contains("supersecretvalue"), "{json}");
    let file = store::write_run(f.project.root(), &s, &secrets).unwrap();
    let text = std::fs::read_to_string(store::run_path(f.project.root(), &file).unwrap()).unwrap();
    assert!(!text.contains("supersecretvalue"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn runs_are_written_listed_and_compared() {
    let _g = ENGINE.lock().await;
    let srv = server::start().await;
    let f = fixture(&srv.base, &[("ok.req.yaml", "GET", "{{baseUrl}}/delay/2")]);
    let mut t = test_file(&[("ok.req.yaml", 1)], rate(20.0, "1s", 2, Some(10)));
    t.thresholds = vec!["p95 < 5s".into()];
    store::save_test(f.project.root(), "t.load.yaml", &t).unwrap();
    let base = run_test(&f, &t).await;
    let root = f.project.root();
    let none = Default::default();
    let first = store::write_run(root, &base, &none).unwrap();
    assert!(first.starts_with("load-runs/t/") && first.ends_with("Z.json"), "{first}");
    assert!(!root.join("load/runs").exists(), "summaries stay out of the repository");

    // A slower "later" run.
    let mut slower = base.clone();
    slower.run_id = "later".into();
    slower.started_at = "2999-01-01T00:00:00Z".into();
    for v in [&mut slower.latency.p50, &mut slower.latency.p95, &mut slower.latency.p99] {
        *v = *v * 3.0 + 50.0;
    }
    slower.thresholds[0].ok = false;
    let second = store::write_run(root, &slower, &none).unwrap();
    assert_ne!(first, second);

    // A summary an older version wrote into the project is still listed and readable.
    let legacy_dir = root.join("load/runs/t");
    std::fs::create_dir_all(&legacy_dir).unwrap();
    let mut old = base.clone();
    old.run_id = "legacy".into();
    old.started_at = "2001-01-01T00:00:00Z".into();
    std::fs::write(legacy_dir.join("20010101T000000Z.json"), store::summary_json(&old, &none)).unwrap();

    let runs = store::list_runs(root, "t.load.yaml").unwrap();
    assert_eq!(runs.len(), 3);
    assert_eq!(runs[0].run_id, "later");
    assert_eq!(runs[2].file, "load/runs/t/20010101T000000Z.json");
    assert_eq!(store::read_run(root, &runs[2].file).unwrap().run_id, "legacy");
    assert_eq!(store::read_run(root, &first).unwrap(), base);
    assert_eq!(store::list_tests(root).unwrap()[0].runs, 3);
    assert!(store::read_run(root, "devall.yaml").is_err());
    assert!(store::read_run(root, "load-runs/../../x.json").is_err());

    // Readable but compact: one line per timeline point.
    let text = std::fs::read_to_string(store::run_path(root, &first).unwrap()).unwrap();
    assert!(text.contains("\n    {\"t\":0,\"requests\":"), "{text}");
    assert!(text.lines().count() < 120, "{} lines", text.lines().count());

    let cmp = compare(&base, &slower, CompareOptions::default());
    let p95 = cmp.metrics.iter().find(|m| m.key == "p95").unwrap();
    assert_eq!(p95.verdict, Verdict::Regression);
    assert!(cmp.regressions >= 3, "{}", cmp.regressions);
    assert_eq!(cmp.thresholds[0].target_ok, Some(false));
    let same = compare(&base, &base, CompareOptions::default());
    assert_eq!((same.regressions, same.improvements), (0, 0));
    assert!(cli::format_summary(&base).contains("p95"));
}

// ---------- mock baseline ----------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn mock_baseline_rewrites_origins_and_tags_the_run() {
    let _g = ENGINE.lock().await;
    let srv = server::start().await;
    // The "real" host is unreachable; the run must go to the stand-in mock instead.
    let f = fixture("https://api.example.invalid:8443", &[("a.req.yaml", "GET", "{{baseUrl}}/delay/1?x=1"), ("b.req.yaml", "GET", "{{baseUrl}}/v2/{{$uuid}}")]);
    let t = test_file(&[("a.req.yaml", 1), ("b.req.yaml", 1)], rate(40.0, "1s", 4, Some(8)));
    let mut plan = build_plan(&f.project, "t.load.yaml", &t, Some("local"), &f.secrets).unwrap();
    plan.target_mock(&srv.base).unwrap();
    let pf = plan.preflight();
    assert_eq!(pf.hosts, vec![srv.base.trim_start_matches("http://").to_string()]);
    assert!(pf.requires_confirmation.is_none());
    let s = run(plan, opts(), None, RunControl::new()).await.unwrap();
    assert_eq!(s.totals.failed, 0, "{:?}", s.error_samples);
    assert!(s.totals.requests >= 38);
    let mb = s.mock_baseline.clone().unwrap();
    assert_eq!(mb.replaced_hosts, vec!["api.example.invalid:8443".to_string()]);
    assert!(srv.state.paths().iter().any(|p| p.starts_with("/delay/1?x=1")), "{:?}", srv.state.paths());
    assert!(srv.state.paths().iter().any(|p| p.starts_with("/v2/")), "dynamic requests are rebased too");
    let file = store::write_run(f.project.root(), &s, &Default::default()).unwrap();
    let runs = store::list_runs(f.project.root(), "t.load.yaml").unwrap();
    assert!(runs[0].mock_baseline);
    assert_eq!(store::read_run(f.project.root(), &file).unwrap().mock_baseline, Some(mb));

    // target: mock without a mock URL is refused by the CLI helper.
    let mut tm = t.clone();
    tm.target = LoadTarget::Mock;
    store::save_test(f.project.root(), "m.load.yaml", &tm).unwrap();
    assert!(cli::wants_mock(f.project.root(), "m.load.yaml").unwrap());
    let err = cli::run_test_file(f.project.root(), "m.load.yaml", &f.secrets, cli::CliRunOptions::default()).await.err().unwrap();
    assert!(err.to_string().contains("--against-mock"), "{err}");
    assert!(std::fs::read_to_string(f.project.root().join("load/m.load.yaml")).unwrap().contains("target: mock"));
}

// ---------- distributed (coordinator + agents on loopback) ----------

const AGENT_TOKEN: &str = "test-agent-token-0123456789";

async fn agent() -> crate::agent::AgentServer {
    crate::agent::start_agent(crate::agent::AgentOptions { listen: "127.0.0.1:0".parse().unwrap(), token: AGENT_TOKEN.into(), tls: None, version: "test".into() }).await.unwrap()
}

fn dist_opts(agents: &[&crate::agent::AgentServer], token: &str, allow_secrets: bool) -> crate::distributed::DistributedOptions {
    crate::distributed::DistributedOptions {
        agents: agents.iter().map(|a| crate::distributed::AgentTarget { address: a.addr.to_string(), weight: 1 }).collect(),
        token: token.into(),
        allow_secrets,
        start_delay: Duration::from_millis(100),
        snapshot_interval: Duration::from_millis(200),
        ..Default::default()
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn distributed_run_merges_agents_like_one_process() {
    let _g = ENGINE.lock().await;
    let srv = server::start().await;
    let f = fixture(&srv.base, &[("slow.req.yaml", "GET", "{{baseUrl}}/delay/20")]);
    let mut t = test_file(&[("slow.req.yaml", 1)], rate(40.0, "3s", 8, Some(40)));
    t.thresholds = vec!["p95 < 2s".into(), "errorRate < 1%".into(), "requests > 100".into()];
    let (a1, a2) = (agent().await, agent().await);

    let snaps = Arc::new(std::sync::Mutex::new(Vec::<LoadSnapshot>::new()));
    let sink = snaps.clone();
    let progress: crate::engine::ProgressFn = Arc::new(move |s| sink.lock().unwrap().push(s));
    let plan = build_plan(&f.project, "t.load.yaml", &t, Some("local"), &f.secrets).unwrap();
    let dist = crate::distributed::run_distributed(plan, dist_opts(&[&a1, &a2], AGENT_TOKEN, false), Some(progress), RunControl::new()).await.unwrap();
    assert_eq!(srv.state.requests.load(Ordering::SeqCst), dist.totals.requests);
    assert!((110..=125).contains(&dist.totals.requests), "{}", dist.totals.requests);
    let agents = dist.agents.as_ref().unwrap();
    assert_eq!(agents.len(), 2);
    assert!(agents.iter().all(|a| a.ok && a.share == 0.5 && (50..=65).contains(&a.requests)), "{agents:?}");
    assert_eq!(agents.iter().map(|a| a.requests).sum::<u64>(), dist.totals.requests);
    assert_eq!(dist.thresholds.len(), 3, "thresholds are evaluated on the merged result");
    assert!(dist.thresholds_passed, "{:?}", dist.thresholds);
    assert!(!dist.aborted);
    assert_eq!(dist.requests[0].requests, dist.totals.requests);
    assert_eq!(dist.timeline.iter().map(|p| p.requests).sum::<u64>(), dist.totals.requests);
    {
        let live = snaps.lock().unwrap();
        assert!(live.len() >= 2 && live.last().unwrap().phase == RunPhase::Finished, "{}", live.len());
    }

    // The same test in one process: the merged distribution matches.
    let single = run_test(&f, &t).await;
    for (a, b) in [(dist.latency.p50, single.latency.p50), (dist.latency.p90, single.latency.p90)] {
        assert!(a >= 20.0 && b >= 20.0, "{a} {b}");
        assert!((a - b).abs() < 15.0, "distributed {a} ms vs single {b} ms");
    }
    a1.closed().await;
    a2.closed().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn agents_refuse_bad_tokens_and_unconsented_secrets() {
    let srv = server::start().await;
    let f = fixture(&srv.base, &[("s.req.yaml", "GET", "{{baseUrl}}/x?token={{secret.TOKEN}}"), ("o.req.yaml", "GET", "{{baseUrl}}/")]);
    f.secrets.set(&SecretScope::new(f.project.id(), "local"), "TOKEN", "supersecretvalue").unwrap();
    let a = agent().await;

    // Auth: no token / a wrong token never reaches a run.
    let client = reqwest::Client::new();
    let r = client.get(format!("http://{}/v1/info", a.addr)).send().await.unwrap();
    assert_eq!(r.status().as_u16(), 401);
    assert_eq!(client.get(format!("http://{}/healthz", a.addr)).send().await.unwrap().status().as_u16(), 200);
    let t = test_file(&[("o.req.yaml", 1)], rate(5.0, "1s", 1, Some(2)));
    let plan = build_plan(&f.project, "t.load.yaml", &t, Some("local"), &f.secrets).unwrap();
    let err = crate::distributed::run_distributed(plan, dist_opts(&[&a], "wrong-token-0123456789", false), None, RunControl::new()).await.unwrap_err();
    assert!(matches!(&err, LoadError::Agent { message, .. } if message.contains("token")), "{err}");
    assert_eq!(srv.state.requests.load(Ordering::SeqCst), 0);

    // Secrets: refused unless explicitly allowed; then only the needed values travel.
    let t = test_file(&[("s.req.yaml", 1)], rate(5.0, "1s", 1, Some(2)));
    let plan = build_plan(&f.project, "t.load.yaml", &t, Some("local"), &f.secrets).unwrap();
    let err = plan.agent_plans(&[1], false, "r", Duration::ZERO, Duration::from_millis(500)).unwrap_err();
    assert!(matches!(&err, LoadError::SecretsToAgents(m) if m.contains("s.req.yaml") && m.contains("--allow-secrets-to-agents")), "{err}");
    let err = crate::distributed::run_distributed(plan, dist_opts(&[&a], AGENT_TOKEN, false), None, RunControl::new()).await.unwrap_err();
    assert!(matches!(err, LoadError::SecretsToAgents(_)));
    assert_eq!(srv.state.requests.load(Ordering::SeqCst), 0, "nothing was sent");
    let plan = build_plan(&f.project, "t.load.yaml", &t, Some("local"), &f.secrets).unwrap();
    let plans = plan.agent_plans(&[1], true, "r", Duration::ZERO, Duration::from_millis(500)).unwrap();
    assert_eq!(plans[0].secrets.get("TOKEN").map(String::as_str), Some("supersecretvalue"));
    let ok_plan = build_plan(&f.project, "t.load.yaml", &test_file(&[("o.req.yaml", 1)], rate(5.0, "1s", 1, Some(2))), Some("local"), &f.secrets).unwrap();
    assert!(ok_plan.agent_plans(&[1], false, "r", Duration::ZERO, Duration::from_millis(500)).unwrap()[0].secrets.is_empty(), "no secrets when none are used");

    let _g = ENGINE.lock().await;
    let s = crate::distributed::run_distributed(plan, dist_opts(&[&a], AGENT_TOKEN, true), None, RunControl::new()).await.unwrap();
    assert!(s.totals.requests >= 4 && s.totals.failed == 0, "{:?}", s.totals);
    assert!(!serde_json::to_string(&s).unwrap().contains("supersecretvalue"));
    a.closed().await;
}

// ---------- cloud regions ----------

#[test]
fn cloud_plans_resolve_requests_and_refuse_unconsented_secrets() {
    let f = fixture(
        "http://127.0.0.1:3000",
        &[("s.req.yaml", "GET", "{{baseUrl}}/x?token={{secret.TOKEN}}"), ("o.req.yaml", "POST", "{{baseUrl}}/orders"), ("u.req.yaml", "GET", "https://api.example.com/u/{{$uuid}}")],
    );
    f.secrets.set(&SecretScope::new(f.project.id(), "local"), "TOKEN", "supersecretvalue").unwrap();
    let opts = |allow: bool| crate::cloud::CloudOptions { regions: vec!["weur".into(), "enam".into()], allow_secrets: allow, run_id: "run-0001".into() };

    // Secrets: refused without consent, then the resolved value travels.
    let t = test_file(&[("s.req.yaml", 1)], rate(4.0, "5s", 2, Some(8)));
    let plan = build_plan(&f.project, "t.load.yaml", &t, Some("local"), &f.secrets).unwrap();
    let err = plan.cloud_plan(&opts(false)).unwrap_err();
    assert!(matches!(&err, LoadError::SecretsToAgents(m) if m.contains("s.req.yaml") && m.contains("--allow-secrets-to-cloud")), "{err}");
    let p = plan.cloud_plan(&opts(true)).unwrap();
    assert!(p.plan.requests[0].url.contains("supersecretvalue"));
    assert!(!format!("{:?}", p.plan).contains("supersecretvalue"), "Debug never shows values");

    // A weighted mix of a fixed request and a dynamic one; origins listed for verification.
    let t = test_file(&[("o.req.yaml", 3), ("u.req.yaml", 1)], rate(4.0, "5s", 2, Some(8)));
    let plan = build_plan(&f.project, "t.load.yaml", &t, Some("local"), &f.secrets).unwrap();
    let p = plan.cloud_plan(&opts(false)).unwrap();
    assert_eq!(p.plan.weights.as_deref(), Some(&[3, 1][..]));
    assert_eq!(p.plan.profile.start_rate, 4.0);
    assert_eq!(p.plan.profile.stages[0].duration_ms, 5000);
    assert_eq!(p.plan.max_in_flight, 8);
    assert_eq!(p.plan.run_id, "run-0001");
    assert_eq!(p.origins, vec!["http://127.0.0.1:3000", "https://api.example.com"]);
    assert!(p.warnings.iter().any(|w| w.contains("u.req.yaml") && w.contains("dynamic")), "{:?}", p.warnings);
    let json = serde_json::to_value(&p.plan).unwrap();
    assert_eq!(json["regions"], serde_json::json!(["weur", "enam"]));
    assert_eq!(json["requests"][0]["method"], "POST");
    assert!(json.get("flow").is_none());

    // Closed-model executors and mock baselines stay local.
    let vus = test_file(&[("o.req.yaml", 1)], Executor::ConstantVus { vus: 2, duration: "5s".into() });
    let plan = build_plan(&f.project, "t.load.yaml", &vus, Some("local"), &f.secrets).unwrap();
    assert!(plan.cloud_plan(&opts(false)).unwrap_err().to_string().contains("open-model"));

    // The relay's answer becomes a normal summary: thresholds evaluated here, regions as agents.
    let mut t = test_file(&[("o.req.yaml", 1)], rate(4.0, "5s", 2, Some(8)));
    t.thresholds = vec!["p95 < 100ms".into(), "errorRate < 1%".into(), "rps > 1000".into()];
    let plan = build_plan(&f.project, "t.load.yaml", &t, Some("local"), &f.secrets).unwrap();
    let pct = Percentiles { p50: 5.0, p75: 6.0, p90: 8.0, p95: 9.0, p99: 12.0, p999: 12.0, min: 1.0, max: 12.0, mean: 5.5 };
    let result = crate::cloud::CloudResult {
        run_id: "run-0001".into(),
        started_at: "2026-10-22T10:00:00Z".into(),
        duration_ms: 5000,
        planned_duration_ms: 5000,
        aborted: false,
        abort_reason: None,
        totals: crate::summary::Totals { requests: 20, iterations: 20, rps: 4.0, checks_passed: 20, checks_rate: 1.0, ..Default::default() },
        latency: pct,
        service_time: Some(pct),
        status_codes: [("200".to_string(), 20u64)].into_iter().collect(),
        error_kinds: Default::default(),
        error_samples: vec!["boom supersecretvalue".into()],
        requests: Vec::new(),
        bucket_seconds: 1,
        timeline: Vec::new(),
        warnings: vec!["edge warning".into()],
        regions: vec![crate::cloud::CloudRegionResult { region: "weur".into(), name: "Western Europe".into(), colo: Some("AMS".into()), share: 0.5, ok: true, error: None, requests: 10, failed: 0, dropped_iterations: 0, p50: 5.0, p95: 9.0 }],
    };
    let mut plan = plan;
    plan.secret_values.insert("supersecretvalue".into());
    let s = plan.cloud_summary(result, &["client warning".into()]);
    assert_eq!(s.totals.requests, 20);
    assert!(s.open_model);
    assert_eq!(s.thresholds.len(), 3);
    assert!(s.thresholds[0].ok && s.thresholds[1].ok && !s.thresholds[2].ok);
    assert!(!s.thresholds_passed);
    assert_eq!(s.agents.as_ref().unwrap()[0].address, "cloud:weur (AMS)");
    assert!(s.warnings.iter().any(|w| w == "client warning") && s.warnings.iter().any(|w| w == "edge warning"));
    assert!(!serde_json::to_string(&s).unwrap().contains("supersecretvalue"));
    assert_eq!(crate::cli::exit_code(&s), crate::cli::EXIT_THRESHOLDS_FAILED);
}
