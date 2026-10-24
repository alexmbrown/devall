//! Tests against a local tonic server built from `tests/fixtures/greeter.proto` (compiled with
//! protox at test time) with a hand-written dynamic service and the tonic reflection service.

use std::convert::Infallible;
use std::task::{Context, Poll};

use futures_util::StreamExt;
use prost_reflect::Value;
use tonic::body::Body;
use tonic::server::NamedService;
use tonic::{Code, Streaming};

use super::*;

fn fixtures() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests").join("fixtures")
}

fn fixture_pool() -> DescriptorPool {
    compile_protos(&[fixtures().join("greeter.proto")], &[]).unwrap()
}

type BoxStream = Pin<Box<dyn futures_util::Stream<Item = std::result::Result<DynamicMessage, Status>> + Send>>;

#[derive(Clone)]
struct Greeter {
    pool: DescriptorPool,
}

impl NamedService for Greeter {
    const NAME: &'static str = "test.v1.Greeter";
}

fn name_of(m: &DynamicMessage) -> String {
    m.get_field_by_name("name").and_then(|v| v.as_str().map(str::to_string)).unwrap_or_default()
}

impl tower::Service<http::Request<Body>> for Greeter {
    type Response = http::Response<Body>;
    type Error = Infallible;
    type Future = Pin<Box<dyn std::future::Future<Output = std::result::Result<Self::Response, Infallible>> + Send>>;

    fn poll_ready(&mut self, _: &mut Context<'_>) -> Poll<std::result::Result<(), Infallible>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, req: http::Request<Body>) -> Self::Future {
        let pool = self.pool.clone();
        Box::pin(async move {
            let req_desc = pool.get_message_by_name("test.v1.HelloRequest").unwrap();
            let reply_desc = pool.get_message_by_name("test.v1.HelloReply").unwrap();
            let countdown_desc = pool.get_message_by_name("test.v1.CountdownRequest").unwrap();
            let reply = move |text: String, n: i32| {
                let mut m = DynamicMessage::new(reply_desc.clone());
                m.set_field_by_name("message", Value::String(text));
                m.set_field_by_name("n", Value::I32(n));
                m
            };
            let resp = match req.uri().path() {
                "/test.v1.Greeter/SayHello" => {
                    let reply = reply.clone();
                    let svc = tower::service_fn(move |r: tonic::Request<DynamicMessage>| {
                        let reply = reply.clone();
                        async move {
                            let name = name_of(r.get_ref());
                            if name == "fail" {
                                let details = RpcStatus {
                                    code: 3,
                                    message: "bad name".into(),
                                    details: vec![prost_types::Any {
                                        type_url: "type.googleapis.com/test.v1.HelloReply".into(),
                                        value: reply("why".into(), 7).encode_to_vec(),
                                    }],
                                };
                                return Err(Status::with_details(Code::InvalidArgument, "bad name", details.encode_to_vec().into()));
                            }
                            let mut resp = tonic::Response::new(reply(format!("Hello {name}"), 1));
                            if let Some(v) = r.metadata().get("x-echo") {
                                resp.metadata_mut().insert("x-echo", v.clone());
                            }
                            Ok(resp)
                        }
                    });
                    tonic::server::Grpc::new(DynamicCodec::new(req_desc)).unary(svc, req).await
                }
                "/test.v1.Greeter/Countdown" => {
                    let reply = reply.clone();
                    let svc = tower::service_fn(move |r: tonic::Request<DynamicMessage>| {
                        let reply = reply.clone();
                        async move {
                            let from = r.get_ref().get_field_by_name("from").and_then(|v| v.as_i32()).unwrap_or(0);
                            let items: Vec<std::result::Result<DynamicMessage, Status>> = (1..=from).rev().map(|i| Ok(reply(format!("t-{i}"), i))).collect();
                            Ok::<_, Status>(tonic::Response::new(Box::pin(futures_util::stream::iter(items)) as BoxStream))
                        }
                    });
                    tonic::server::Grpc::new(DynamicCodec::new(countdown_desc)).server_streaming(svc, req).await
                }
                "/test.v1.Greeter/Collect" => {
                    let reply = reply.clone();
                    let svc = tower::service_fn(move |r: tonic::Request<Streaming<DynamicMessage>>| {
                        let reply = reply.clone();
                        async move {
                            let mut s = r.into_inner();
                            let mut names = Vec::new();
                            while let Some(m) = s.message().await? {
                                names.push(name_of(&m));
                            }
                            Ok(tonic::Response::new(reply(names.join(","), names.len() as i32)))
                        }
                    });
                    tonic::server::Grpc::new(DynamicCodec::new(req_desc)).client_streaming(svc, req).await
                }
                "/test.v1.Greeter/Chat" => {
                    let reply = reply.clone();
                    let svc = tower::service_fn(move |r: tonic::Request<Streaming<DynamicMessage>>| {
                        let reply = reply.clone();
                        async move {
                            let out = r.into_inner().map(move |m| m.map(|m| reply(format!("echo {}", name_of(&m)), 0)));
                            Ok::<_, Status>(tonic::Response::new(Box::pin(out) as BoxStream))
                        }
                    });
                    tonic::server::Grpc::new(DynamicCodec::new(req_desc)).streaming(svc, req).await
                }
                _ => Status::unimplemented("no such method").into_http(),
            };
            Ok(resp)
        })
    }
}

#[derive(Clone, Copy, PartialEq)]
enum Reflect {
    None,
    Both,
    AlphaOnly,
}

async fn start_server(reflect: Reflect) -> String {
    let pool = fixture_pool();
    let fds = prost_types::FileDescriptorSet { file: pool.file_descriptor_protos().cloned().collect() };
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let incoming = tonic::transport::server::TcpIncoming::from(listener);
    let mut router = tonic::transport::Server::builder().add_service(Greeter { pool });
    if reflect == Reflect::Both {
        router = router.add_service(tonic_reflection::server::Builder::configure().register_file_descriptor_set(fds.clone()).build_v1().unwrap());
    }
    if reflect != Reflect::None {
        router = router.add_service(tonic_reflection::server::Builder::configure().register_file_descriptor_set(fds).build_v1alpha().unwrap());
    }
    tokio::spawn(router.serve_with_incoming(incoming));
    format!("grpc://{addr}")
}

fn opts(url: &str, method: &str, message: &str) -> GrpcOptions {
    GrpcOptions {
        target: GrpcTarget::parse(url, true, Duration::from_secs(5)).unwrap(),
        service: "test.v1.Greeter".into(),
        method: method.into(),
        message: message.into(),
        metadata: Vec::new(),
        deadline: None,
        source: SchemaSource::Reflection,
    }
}

fn incoming(events: &[ProtoEvent]) -> Vec<serde_json::Value> {
    events
        .iter()
        .filter_map(|e| match e {
            ProtoEvent::Message { direction: Direction::In, body, .. } => Some(serde_json::from_str(body).unwrap()),
            _ => None,
        })
        .collect()
}

async fn run(o: &GrpcOptions) -> (GrpcStatus, Vec<ProtoEvent>) {
    let (sink, mut rx) = EventSink::channel();
    let (_h, fut) = start_call(o, sink, false).await.unwrap();
    let status = tokio::time::timeout(Duration::from_secs(10), fut).await.unwrap();
    let mut events = Vec::new();
    while let Ok(e) = rx.try_recv() {
        events.push(e);
    }
    (status, events)
}

#[test]
fn targets() {
    let t = GrpcTarget::parse("grpcs://api.example.com", true, Duration::from_secs(1)).unwrap();
    assert_eq!((t.host.as_str(), t.port, t.tls), ("api.example.com", 443, true));
    assert_eq!(t.origin(), "https://api.example.com:443");
    let t = GrpcTarget::parse("localhost:50051", true, Duration::from_secs(1)).unwrap();
    assert_eq!((t.port, t.tls), (50051, false));
    let t = GrpcTarget::parse("http://[::1]:9000/", true, Duration::from_secs(1)).unwrap();
    assert_eq!(t.origin(), "http://[::1]:9000");
    assert!(GrpcTarget::parse("ws://x", true, Duration::from_secs(1)).is_err());
    assert!(GrpcTarget::parse("  ", true, Duration::from_secs(1)).is_err());
}

#[test]
fn templates_cover_every_field_kind_and_parse_back() {
    let pool = fixture_pool();
    let desc = pool.get_message_by_name("test.v1.HelloRequest").unwrap();
    let t = message_template(&desc);
    assert_eq!(t["name"], "");
    assert_eq!(t["mood"], "MOOD_UNSPECIFIED");
    assert_eq!(t["tags"], serde_json::json!([]));
    assert_eq!(t["at"], "1970-01-01T00:00:00Z");
    assert!(t.get("email").is_some() && t.get("phone").is_none(), "only the first oneof field: {t}");
    assert_eq!(t["counts"], serde_json::json!({}));
    assert_eq!(t["big"], "0");
    assert_eq!(t["inner"]["again"], serde_json::json!({}), "recursion stops: {t}");
    assert_eq!(t["inners"][0]["ok"], false);
    // The template is a valid request.
    message_from_json(&desc, &message_template_text(&desc)).unwrap();
    assert!(message_from_json(&desc, "{\"nope\": 1}").is_err());
    assert!(message_from_json(&desc, "").is_ok());
}

#[test]
fn proto_compile_errors_are_readable() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("bad.proto"), "syntax = \"proto3\";\nmessage X { strin a = 1; }\n").unwrap();
    let e = compile_protos(&[dir.path().join("bad.proto")], &[]).unwrap_err();
    assert!(e.to_string().contains("strin"), "{e}");
    assert!(compile_protos(&[], &[]).is_err());
    assert!(compile_protos(&[dir.path().join("missing.proto")], &[]).is_err());
}

#[test]
fn metadata_conversion() {
    let m = metadata_map(&[("X-Token".into(), "abc".into()), ("trace-bin".into(), "AAEC".into())]).unwrap();
    assert_eq!(m.get("x-token").unwrap(), "abc");
    assert_eq!(m.get_bin("trace-bin").unwrap().to_bytes().unwrap().as_ref(), &[0u8, 1, 2]);
    assert!(metadata_map(&[("bad key".into(), "x".into())]).is_err());
    assert!(metadata_map(&[("k".into(), "a\nb".into())]).is_err());
}

#[tokio::test]
async fn describe_via_reflection_v1_and_v1alpha() {
    for mode in [Reflect::Both, Reflect::AlphaOnly] {
        let url = start_server(mode).await;
        let d = describe(&opts(&url, "", "")).await.unwrap();
        assert_eq!(d.source, "reflection");
        let names: Vec<&str> = d.services.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, vec!["test.v1.Greeter"], "reflection services are hidden");
        let methods = &d.services[0].methods;
        let find = |n: &str| methods.iter().find(|m| m.name == n).unwrap();
        assert!(!find("SayHello").client_streaming && !find("SayHello").server_streaming);
        assert!(find("Countdown").server_streaming);
        assert!(find("Collect").client_streaming);
        assert!(find("Chat").client_streaming && find("Chat").server_streaming);
        assert_eq!(find("SayHello").path, "/test.v1.Greeter/SayHello");
        assert!(find("SayHello").template.contains("\"mood\": \"MOOD_UNSPECIFIED\""));
    }
}

#[tokio::test]
async fn describe_without_reflection_explains() {
    let url = start_server(Reflect::None).await;
    let e = describe(&opts(&url, "", "")).await.unwrap_err();
    assert!(e.to_string().contains("does not support gRPC reflection"), "{e}");
}

#[tokio::test]
async fn unary_with_metadata_and_proto_files() {
    let url = start_server(Reflect::None).await;
    let mut o = opts(&url, "SayHello", "{\"name\":\"Ada\",\"mood\":\"HAPPY\"}");
    o.source = SchemaSource::ProtoFiles { files: vec![fixtures().join("greeter.proto")], includes: vec![] };
    o.metadata = vec![("x-echo".into(), "42".into())];
    let (status, events) = run(&o).await;
    assert!(status.is_ok(), "{status:?}");
    assert_eq!((status.messages_sent, status.messages_received), (1, 1));
    assert!(status.headers.iter().any(|(k, v)| k == "x-echo" && v == "42"), "{:?}", status.headers);
    let got = incoming(&events);
    assert_eq!(got, vec![serde_json::json!({ "message": "Hello Ada", "n": 1 })]);
    assert!(events.iter().any(|e| matches!(e, ProtoEvent::Message { direction: Direction::Out, label: Some(l), .. } if l == "Greeter/SayHello")));
    assert!(matches!(events.last(), Some(ProtoEvent::Closed { code: Some(0), .. })));
}

#[tokio::test]
async fn server_streaming_and_client_streaming() {
    let url = start_server(Reflect::Both).await;
    let (status, events) = run(&opts(&url, "Countdown", "{\"from\": 3}")).await;
    assert!(status.is_ok());
    let got: Vec<String> = incoming(&events).iter().map(|v| v["message"].as_str().unwrap().to_string()).collect();
    assert_eq!(got, vec!["t-3", "t-2", "t-1"]);

    let (status, events) = run(&opts(&url, "Collect", "[{\"name\":\"a\"},{\"name\":\"b\"}]")).await;
    assert!(status.is_ok());
    assert_eq!(status.messages_sent, 2);
    assert_eq!(incoming(&events), vec![serde_json::json!({ "message": "a,b", "n": 2 })]);
}

#[tokio::test]
async fn bidi_interactive_send_end_and_cancel() {
    let url = start_server(Reflect::Both).await;
    let (sink, mut rx) = EventSink::channel();
    let (h, fut) = start_call(&opts(&url, "Chat", "{\"name\":\"first\"}"), sink, true).await.unwrap();
    let task = tokio::spawn(fut);
    assert!(h.is_open());
    h.send_json("{\"name\":\"second\"}").unwrap();
    assert!(h.send_json("{\"bogus\":1}").is_err());
    // Wait for both echoes before closing.
    let mut echoes = Vec::new();
    while echoes.len() < 2 {
        if let ProtoEvent::Message { direction: Direction::In, body, .. } = tokio::time::timeout(Duration::from_secs(5), rx.recv()).await.unwrap().unwrap() {
            echoes.push(body);
        }
    }
    assert!(echoes[0].contains("echo first") && echoes[1].contains("echo second"), "{echoes:?}");
    h.end();
    assert!(!h.is_open());
    assert!(h.send_json("{}").is_err());
    let status = tokio::time::timeout(Duration::from_secs(5), task).await.unwrap().unwrap();
    assert!(status.is_ok(), "{status:?}");
    assert_eq!((status.messages_sent, status.messages_received), (2, 2));

    // Cancel an open call.
    let (h, fut) = start_call(&opts(&url, "Chat", ""), EventSink::noop(), true).await.unwrap();
    let task = tokio::spawn(fut);
    h.cancel();
    let status = tokio::time::timeout(Duration::from_secs(5), task).await.unwrap().unwrap();
    assert_eq!(status.code_name, "CANCELLED");

    // A client-side deadline on a call that never finishes.
    let mut o = opts(&url, "Chat", "");
    o.deadline = Some(Duration::from_millis(200));
    let (_h, fut) = start_call(&o, EventSink::noop(), true).await.unwrap();
    let status = tokio::time::timeout(Duration::from_secs(5), fut).await.unwrap();
    assert_eq!(status.code, 4);
}

#[tokio::test]
async fn errors_carry_status_and_decoded_details() {
    let url = start_server(Reflect::Both).await;
    let (status, events) = run(&opts(&url, "SayHello", "{\"name\":\"fail\"}")).await;
    assert_eq!((status.code, status.code_name.as_str(), status.message.as_str()), (3, "INVALID_ARGUMENT", "bad name"));
    let details = status.details.unwrap();
    assert_eq!(details["details"][0]["message"], "why");
    assert_eq!(details["details"][0]["@type"], "type.googleapis.com/test.v1.HelloReply");
    assert!(matches!(events.last(), Some(ProtoEvent::Closed { code: Some(3), reason: Some(r), .. }) if r.contains("bad name")));

    // Bad input is rejected before anything is sent.
    let e = start_call(&opts(&url, "SayHello", "{\"name\": 5}"), EventSink::noop(), false).await.err().unwrap();
    assert!(e.to_string().contains("HelloRequest"), "{e}");
    let e = start_call(&opts(&url, "Nope", "{}"), EventSink::noop(), false).await.err().unwrap();
    assert!(e.to_string().contains("no method Nope"), "{e}");
}

#[tokio::test]
async fn resolve_from_file() {
    let dir = tempfile::tempdir().unwrap();
    let mut f = GrpcFile::new("g");
    f.url = "{{host}}".into();
    f.use_reflection = false;
    f.proto_files = vec!["protos/a.proto".into()];
    f.import_paths = vec!["vendor".into()];
    f.metadata = vec![devall_core::KeyValue::new("authorization", "Bearer {{tok}}")];
    f.deadline_ms = 1500;
    let mut r = Resolver::new(&devall_core::vars::NoSecrets);
    r.push_layer([("host".to_string(), "grpcs://h:1".to_string()), ("tok".to_string(), "t".to_string())].into_iter().collect());
    let o = GrpcOptions::resolve(&f, &mut r, dir.path()).unwrap();
    assert!(o.target.tls && o.target.verify_tls);
    assert_eq!(o.deadline, Some(Duration::from_millis(1500)));
    assert_eq!(o.metadata, vec![("authorization".to_string(), "Bearer t".to_string())]);
    match o.source {
        SchemaSource::ProtoFiles { files, includes } => {
            assert_eq!(files, vec![dir.path().join("protos/a.proto")]);
            assert_eq!(includes, vec![dir.path().join("vendor"), dir.path().to_path_buf()]);
        }
        other => panic!("{other:?}"),
    }
}
