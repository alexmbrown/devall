//! Pub/Sub over REST (`v1`), against the emulator (which has no UI at all) or a real project.

use std::collections::{BTreeMap, HashSet, VecDeque};
use std::time::Duration;

use base64::Engine;
use reqwest::Method;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::{Call, GcpClient, GcpTarget, q, seg, short_name};
use crate::error::{Error, Result};
use crate::stream::Sink;

pub const API: &str = "https://pubsub.googleapis.com";
const B64: base64::engine::GeneralPurpose = base64::engine::general_purpose::STANDARD;

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Topic {
    pub name: String,
    pub full_name: String,
    pub labels: BTreeMap<String, String>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Subscription {
    pub name: String,
    pub full_name: String,
    /// Short topic name (`_deleted-topic_` when the topic is gone).
    pub topic: String,
    pub ack_deadline_seconds: i64,
    pub push_endpoint: Option<String>,
    pub filter: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct PublishMessage {
    /// Payload text, or base64 when `base64` is set.
    pub data: String,
    pub base64: bool,
    pub attributes: BTreeMap<String, String>,
    pub ordering_key: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReceivedMessage {
    pub ack_id: String,
    pub message_id: String,
    pub publish_time: Option<String>,
    /// The payload as text when it is valid UTF-8.
    pub text: Option<String>,
    pub data_base64: String,
    pub size: usize,
    pub attributes: BTreeMap<String, String>,
    pub ordering_key: Option<String>,
    pub delivery_attempt: Option<i64>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum PubSubTailMsg {
    Messages { messages: Vec<ReceivedMessage> },
    Error { message: String },
}

fn str_map(v: Option<&Value>) -> BTreeMap<String, String> {
    v.and_then(Value::as_object).map(|m| m.iter().filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_string()))).collect()).unwrap_or_default()
}

/// The `topics/{t}:publish` body. Text payloads are base64-encoded as the API requires.
pub fn publish_body(messages: &[PublishMessage]) -> Result<Value> {
    if messages.is_empty() {
        return Err(Error::Invalid("nothing to publish".into()));
    }
    let mut out = Vec::new();
    for m in messages {
        let data = if m.base64 {
            let raw = m.data.trim();
            B64.decode(raw).map_err(|e| Error::Invalid(format!("payload is not valid base64: {e}")))?;
            raw.to_string()
        } else {
            B64.encode(m.data.as_bytes())
        };
        if data.is_empty() && m.attributes.is_empty() {
            return Err(Error::Invalid("a message needs data or at least one attribute".into()));
        }
        let mut msg = json!({ "data": data });
        if !m.attributes.is_empty() {
            msg["attributes"] = json!(m.attributes);
        }
        if let Some(k) = m.ordering_key.as_deref().filter(|k| !k.is_empty()) {
            msg["orderingKey"] = json!(k);
        }
        out.push(msg);
    }
    Ok(json!({ "messages": out }))
}

/// `receivedMessages` of a pull response.
pub fn parse_pull(v: &Value) -> Vec<ReceivedMessage> {
    let Some(list) = v.get("receivedMessages").and_then(Value::as_array) else { return Vec::new() };
    list.iter()
        .map(|r| {
            let m = r.get("message").cloned().unwrap_or(Value::Null);
            let data_base64 = m.get("data").and_then(Value::as_str).unwrap_or("").to_string();
            let bytes = B64.decode(&data_base64).unwrap_or_default();
            ReceivedMessage {
                ack_id: r.get("ackId").and_then(Value::as_str).unwrap_or("").into(),
                message_id: m.get("messageId").and_then(Value::as_str).unwrap_or("").into(),
                publish_time: m.get("publishTime").and_then(Value::as_str).map(str::to_string),
                size: bytes.len(),
                text: String::from_utf8(bytes).ok(),
                data_base64,
                attributes: str_map(m.get("attributes")),
                ordering_key: m.get("orderingKey").and_then(Value::as_str).filter(|s| !s.is_empty()).map(str::to_string),
                delivery_attempt: r.get("deliveryAttempt").and_then(Value::as_i64),
            }
        })
        .collect()
}

fn parse_topic(v: &Value) -> Topic {
    let full = v.get("name").and_then(Value::as_str).unwrap_or("").to_string();
    Topic { name: short_name(&full), full_name: full, labels: str_map(v.get("labels")) }
}

fn parse_subscription(v: &Value) -> Subscription {
    let full = v.get("name").and_then(Value::as_str).unwrap_or("").to_string();
    Subscription {
        name: short_name(&full),
        full_name: full,
        topic: short_name(v.get("topic").and_then(Value::as_str).unwrap_or("")),
        ack_deadline_seconds: v.get("ackDeadlineSeconds").and_then(Value::as_i64).unwrap_or(10),
        push_endpoint: v.pointer("/pushConfig/pushEndpoint").and_then(Value::as_str).filter(|s| !s.is_empty()).map(str::to_string),
        filter: v.get("filter").and_then(Value::as_str).filter(|s| !s.is_empty()).map(str::to_string),
    }
}

fn check_name(kind: &str, name: &str) -> Result<()> {
    let ok = !name.is_empty() && name.len() <= 255 && name.chars().all(|c| c.is_ascii_alphanumeric() || "-_.~+%".contains(c)) && !name.starts_with("goog");
    if ok { Ok(()) } else { Err(Error::Invalid(format!("invalid {kind} name {name:?} (letters, digits, - _ . ~ + %; not starting with \"goog\")"))) }
}

impl GcpClient {
    fn pubsub_url(&self, t: &GcpTarget, rest: &str) -> Result<String> {
        Ok(format!("{}/v1/projects/{}/{rest}", t.base(API), seg(t.check_project()?)))
    }

    async fn paged(&self, t: &GcpTarget, rest: &str, key: &str) -> Result<Vec<Value>> {
        let mut out = Vec::new();
        let mut token: Option<String> = None;
        for _ in 0..50 {
            let mut url = format!("{}?pageSize=500", self.pubsub_url(t, rest)?);
            if let Some(tok) = &token {
                url.push_str(&format!("&pageToken={}", q(tok)));
            }
            let v = self.call(Call { method: Method::GET, url, body: None, target: t, emulator_bearer: None }).await?;
            out.extend(v.get(key).and_then(Value::as_array).cloned().unwrap_or_default());
            token = v.get("nextPageToken").and_then(Value::as_str).filter(|s| !s.is_empty()).map(str::to_string);
            if token.is_none() {
                break;
            }
        }
        Ok(out)
    }

    pub async fn pubsub_topics(&self, t: &GcpTarget) -> Result<Vec<Topic>> {
        let mut v: Vec<Topic> = self.paged(t, "topics", "topics").await?.iter().map(parse_topic).collect();
        v.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(v)
    }

    pub async fn pubsub_subscriptions(&self, t: &GcpTarget) -> Result<Vec<Subscription>> {
        let mut v: Vec<Subscription> = self.paged(t, "subscriptions", "subscriptions").await?.iter().map(parse_subscription).collect();
        v.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(v)
    }

    pub async fn pubsub_create_topic(&self, t: &GcpTarget, name: &str) -> Result<Topic> {
        t.check_write("Creating a topic")?;
        check_name("topic", name)?;
        let url = self.pubsub_url(t, &format!("topics/{}", seg(name)))?;
        let v = self.call(Call { method: Method::PUT, url, body: Some(json!({})), target: t, emulator_bearer: None }).await?;
        Ok(parse_topic(&v))
    }

    pub async fn pubsub_delete_topic(&self, t: &GcpTarget, name: &str) -> Result<()> {
        t.check_write("Deleting a topic")?;
        let url = self.pubsub_url(t, &format!("topics/{}", seg(name)))?;
        self.call(Call { method: Method::DELETE, url, body: None, target: t, emulator_bearer: None }).await?;
        Ok(())
    }

    pub async fn pubsub_create_subscription(&self, t: &GcpTarget, name: &str, topic: &str, ack_deadline_seconds: Option<i64>) -> Result<Subscription> {
        t.check_write("Creating a subscription")?;
        check_name("subscription", name)?;
        let project = t.check_project()?;
        let mut body = json!({ "topic": format!("projects/{project}/topics/{topic}") });
        if let Some(s) = ack_deadline_seconds {
            body["ackDeadlineSeconds"] = json!(s.clamp(10, 600));
        }
        let url = self.pubsub_url(t, &format!("subscriptions/{}", seg(name)))?;
        let v = self.call(Call { method: Method::PUT, url, body: Some(body), target: t, emulator_bearer: None }).await?;
        Ok(parse_subscription(&v))
    }

    pub async fn pubsub_delete_subscription(&self, t: &GcpTarget, name: &str) -> Result<()> {
        t.check_write("Deleting a subscription")?;
        let url = self.pubsub_url(t, &format!("subscriptions/{}", seg(name)))?;
        self.call(Call { method: Method::DELETE, url, body: None, target: t, emulator_bearer: None }).await?;
        Ok(())
    }

    /// Publishes and returns the message ids.
    pub async fn pubsub_publish(&self, t: &GcpTarget, topic: &str, messages: &[PublishMessage]) -> Result<Vec<String>> {
        t.check_write("Publishing")?;
        let url = self.pubsub_url(t, &format!("topics/{}:publish", seg(topic)))?;
        let v = self.call(Call { method: Method::POST, url, body: Some(publish_body(messages)?), target: t, emulator_bearer: None }).await?;
        Ok(v.get("messageIds").and_then(Value::as_array).map(|a| a.iter().filter_map(|x| x.as_str().map(str::to_string)).collect()).unwrap_or_default())
    }

    /// Pulls up to `max` messages; acknowledges them when `ack` (otherwise they are redelivered
    /// after the ack deadline). Pulling from a real project counts as a write.
    pub async fn pubsub_pull(&self, t: &GcpTarget, subscription: &str, max: u32, ack: bool) -> Result<Vec<ReceivedMessage>> {
        t.check_write("Pulling messages")?;
        let url = self.pubsub_url(t, &format!("subscriptions/{}:pull", seg(subscription)))?;
        let body = json!({ "maxMessages": max.clamp(1, 1000), "returnImmediately": true });
        let v = self.call(Call { method: Method::POST, url, body: Some(body), target: t, emulator_bearer: None }).await?;
        let msgs = parse_pull(&v);
        if ack && !msgs.is_empty() {
            self.pubsub_ack(t, subscription, msgs.iter().map(|m| m.ack_id.clone()).collect()).await?;
        }
        Ok(msgs)
    }

    pub async fn pubsub_ack(&self, t: &GcpTarget, subscription: &str, ack_ids: Vec<String>) -> Result<()> {
        t.check_write("Acknowledging")?;
        if ack_ids.is_empty() {
            return Ok(());
        }
        let url = self.pubsub_url(t, &format!("subscriptions/{}:acknowledge", seg(subscription)))?;
        self.call(Call { method: Method::POST, url, body: Some(json!({ "ackIds": ack_ids })), target: t, emulator_bearer: None }).await?;
        Ok(())
    }

    /// Live tail: pulls continuously and streams new messages. With `ack` off, messages are left
    /// for other consumers and duplicates from redelivery are filtered by message id.
    pub async fn pubsub_tail(&self, t: &GcpTarget, subscription: &str, ack: bool, sink: Sink<PubSubTailMsg>) {
        if let Err(e) = t.check_write("Tailing (pulling) a subscription") {
            sink(PubSubTailMsg::Error { message: e.to_string() });
            return;
        }
        let mut seen: HashSet<String> = HashSet::new();
        let mut order: VecDeque<String> = VecDeque::new();
        let mut idle = Duration::from_millis(500);
        loop {
            match self.pubsub_pull(t, subscription, 100, ack).await {
                Ok(msgs) => {
                    let fresh: Vec<ReceivedMessage> = msgs.into_iter().filter(|m| ack || seen.insert(m.message_id.clone())).collect();
                    if !ack {
                        for m in &fresh {
                            order.push_back(m.message_id.clone());
                        }
                        while order.len() > 10_000 {
                            if let Some(old) = order.pop_front() {
                                seen.remove(&old);
                            }
                        }
                    }
                    if fresh.is_empty() {
                        tokio::time::sleep(idle).await;
                        idle = (idle * 2).min(Duration::from_secs(2));
                        if !sink(PubSubTailMsg::Messages { messages: Vec::new() }) {
                            return;
                        }
                    } else {
                        idle = Duration::from_millis(500);
                        if !sink(PubSubTailMsg::Messages { messages: fresh }) {
                            return;
                        }
                    }
                }
                Err(e) => {
                    if !sink(PubSubTailMsg::Error { message: e.to_string() }) {
                        return;
                    }
                    tokio::time::sleep(Duration::from_secs(3)).await;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use super::*;
    use crate::gcp::testserver;

    #[test]
    fn publish_payload() {
        let mut attrs = BTreeMap::new();
        attrs.insert("type".to_string(), "order.created".to_string());
        let body = publish_body(&[
            PublishMessage { data: "{\"id\":1}".into(), base64: false, attributes: attrs, ordering_key: Some("customer-7".into()) },
            PublishMessage { data: "aGk=".into(), base64: true, ..Default::default() },
        ])
        .unwrap();
        let msgs = body["messages"].as_array().unwrap();
        assert_eq!(msgs[0]["data"], B64.encode("{\"id\":1}"));
        assert_eq!(msgs[0]["attributes"]["type"], "order.created");
        assert_eq!(msgs[0]["orderingKey"], "customer-7");
        assert_eq!(msgs[1]["data"], "aGk=");
        assert!(msgs[1].get("attributes").is_none());
        assert!(publish_body(&[]).is_err());
        assert!(publish_body(&[PublishMessage { data: "%%%".into(), base64: true, ..Default::default() }]).is_err());
        assert!(publish_body(&[PublishMessage::default()]).is_err());
    }

    #[test]
    fn pull_parsing() {
        let v = json!({ "receivedMessages": [
            { "ackId": "a1", "deliveryAttempt": 2, "message": { "data": B64.encode("hello"), "messageId": "1", "publishTime": "2026-10-21T10:00:00Z", "attributes": { "k": "v" }, "orderingKey": "" } },
            { "ackId": "a2", "message": { "data": B64.encode([0xff, 0xfe]), "messageId": "2" } }
        ] });
        let m = parse_pull(&v);
        assert_eq!(m.len(), 2);
        assert_eq!(m[0].text.as_deref(), Some("hello"));
        assert_eq!(m[0].attributes.get("k").map(String::as_str), Some("v"));
        assert_eq!(m[0].delivery_attempt, Some(2));
        assert_eq!(m[0].ordering_key, None);
        assert_eq!(m[1].text, None);
        assert_eq!(m[1].size, 2);
        assert!(parse_pull(&json!({})).is_empty());
    }

    #[tokio::test]
    async fn emulator_round_trip_against_fake_server() {
        let router: testserver::Router = Arc::new(|r: &testserver::Recorded| {
            let p = r.path.as_str();
            match (r.method.as_str(), p) {
                ("GET", p) if p.starts_with("/v1/projects/demo/topics?") => (200, json!({ "topics": [ { "name": "projects/demo/topics/orders" }, { "name": "projects/demo/topics/audit" } ] }).to_string()),
                ("GET", p) if p.starts_with("/v1/projects/demo/subscriptions?") => (200, json!({ "subscriptions": [ { "name": "projects/demo/subscriptions/orders-tail", "topic": "projects/demo/topics/orders", "ackDeadlineSeconds": 10 } ] }).to_string()),
                ("PUT", "/v1/projects/demo/topics/new-topic") => (200, json!({ "name": "projects/demo/topics/new-topic" }).to_string()),
                ("PUT", "/v1/projects/demo/subscriptions/new-sub") => (200, json!({ "name": "projects/demo/subscriptions/new-sub", "topic": "projects/demo/topics/orders", "ackDeadlineSeconds": 30 }).to_string()),
                ("POST", "/v1/projects/demo/topics/orders:publish") => (200, json!({ "messageIds": ["101"] }).to_string()),
                ("POST", "/v1/projects/demo/subscriptions/orders-tail:pull") => (200, json!({ "receivedMessages": [ { "ackId": "ack-1", "message": { "data": B64.encode("hi"), "messageId": "101" } } ] }).to_string()),
                ("POST", "/v1/projects/demo/subscriptions/orders-tail:acknowledge") => (200, "{}".into()),
                ("DELETE", _) => (200, "{}".into()),
                _ => (404, json!({ "error": { "code": 404, "message": format!("no route {p}"), "status": "NOT_FOUND" } }).to_string()),
            }
        });
        let (host, log) = testserver::start(router).await;
        let client = GcpClient::new().unwrap();
        let t = GcpTarget::emulator("demo", &host);

        let topics = client.pubsub_topics(&t).await.unwrap();
        assert_eq!(topics.iter().map(|t| t.name.as_str()).collect::<Vec<_>>(), vec!["audit", "orders"]);
        let subs = client.pubsub_subscriptions(&t).await.unwrap();
        assert_eq!(subs[0].topic, "orders");
        assert_eq!(client.pubsub_create_topic(&t, "new-topic").await.unwrap().name, "new-topic");
        let sub = client.pubsub_create_subscription(&t, "new-sub", "orders", Some(30)).await.unwrap();
        assert_eq!(sub.ack_deadline_seconds, 30);
        let ids = client.pubsub_publish(&t, "orders", &[PublishMessage { data: "hello".into(), ..Default::default() }]).await.unwrap();
        assert_eq!(ids, vec!["101"]);
        let pulled = client.pubsub_pull(&t, "orders-tail", 10, true).await.unwrap();
        assert_eq!(pulled[0].text.as_deref(), Some("hi"));
        client.pubsub_delete_topic(&t, "new-topic").await.unwrap();
        let err = client.pubsub_publish(&t, "missing", &[PublishMessage { data: "x".into(), ..Default::default() }]).await.unwrap_err().to_string();
        assert!(err.contains("404") && err.contains("NOT_FOUND"), "{err}");
        assert!(client.pubsub_create_topic(&t, "goog-bad").await.is_err());

        let log = log.lock().unwrap();
        let publish = log.iter().find(|r| r.path.ends_with(":publish")).unwrap();
        assert_eq!(publish.json()["messages"][0]["data"], B64.encode("hello"));
        assert_eq!(publish.header("content-type"), Some("application/json"));
        assert!(publish.header("authorization").is_none(), "emulator calls carry no credentials");
        let create_sub = log.iter().find(|r| r.path == "/v1/projects/demo/subscriptions/new-sub").unwrap();
        assert_eq!(create_sub.json()["topic"], "projects/demo/topics/orders");
        let ack = log.iter().find(|r| r.path.ends_with(":acknowledge")).unwrap();
        assert_eq!(ack.json()["ackIds"][0], "ack-1");
        let pull = log.iter().find(|r| r.path.ends_with(":pull")).unwrap();
        assert_eq!(pull.json()["maxMessages"], 10);
    }

    #[tokio::test]
    async fn tail_streams_and_dedupes_without_ack() {
        let router: testserver::Router = Arc::new(|r: &testserver::Recorded| {
            if r.path.ends_with(":pull") {
                (200, json!({ "receivedMessages": [ { "ackId": "a", "message": { "data": B64.encode("same"), "messageId": "7" } } ] }).to_string())
            } else {
                (200, "{}".into())
            }
        });
        let (host, log) = testserver::start(router).await;
        let client = GcpClient::new().unwrap();
        let t = GcpTarget::emulator("demo", &host);
        let got: Arc<Mutex<Vec<String>>> = Arc::default();
        let g = got.clone();
        let calls = Arc::new(Mutex::new(0usize));
        let c2 = calls.clone();
        let sink: Sink<PubSubTailMsg> = Arc::new(move |m: PubSubTailMsg| {
            *c2.lock().unwrap() += 1;
            if let PubSubTailMsg::Messages { messages } = m {
                g.lock().unwrap().extend(messages.into_iter().map(|m| m.message_id));
            }
            *c2.lock().unwrap() < 3
        });
        tokio::time::timeout(Duration::from_secs(10), client.pubsub_tail(&t, "s", false, sink)).await.unwrap();
        assert_eq!(*got.lock().unwrap(), vec!["7"], "redelivered message shown once");
        assert!(!log.lock().unwrap().iter().any(|r| r.path.ends_with(":acknowledge")));
    }

    #[test]
    fn real_cloud_is_read_only_by_default() {
        let t = GcpTarget { project: "prod".into(), emulator: None, allow_writes: false };
        let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        let client = GcpClient::new().unwrap();
        let err = rt.block_on(client.pubsub_publish(&t, "x", &[PublishMessage { data: "x".into(), ..Default::default() }])).unwrap_err();
        assert!(matches!(err, Error::ReadOnly(_)), "{err}");
        let err = rt.block_on(client.pubsub_pull(&t, "s", 1, false)).unwrap_err();
        assert!(matches!(err, Error::ReadOnly(_)));
    }
}
