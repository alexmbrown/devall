//! Example JSON for a message type, used to pre-fill the request editor.

use prost_reflect::{Kind, MessageDescriptor};
use serde_json::{Map, Value, json};

const MAX_DEPTH: usize = 6;

/// A JSON object with every field set to its zero value (proto3 JSON mapping, `json_name`
/// keys). Only the first field of each oneof is included; recursive types stop at `{}`.
pub fn message_template(desc: &MessageDescriptor) -> Value {
    let mut stack = Vec::new();
    template(desc, &mut stack)
}

/// [`message_template`] pretty-printed.
pub fn message_template_text(desc: &MessageDescriptor) -> String {
    serde_json::to_string_pretty(&message_template(desc)).unwrap_or_else(|_| "{}".into())
}

fn template(desc: &MessageDescriptor, stack: &mut Vec<String>) -> Value {
    if let Some(v) = well_known(desc.full_name()) {
        return v;
    }
    if stack.len() >= MAX_DEPTH || stack.iter().any(|n| n == desc.full_name()) {
        return json!({});
    }
    stack.push(desc.full_name().to_string());
    let mut obj = Map::new();
    let mut oneofs_seen: Vec<String> = Vec::new();
    for f in desc.fields() {
        if let Some(o) = f.containing_oneof()
            && !o.is_synthetic()
        {
            if oneofs_seen.iter().any(|n| n == o.full_name()) {
                continue;
            }
            oneofs_seen.push(o.full_name().to_string());
        }
        let v = if f.is_map() {
            json!({})
        } else if f.is_list() {
            match f.kind() {
                Kind::Message(m) => Value::Array(vec![template(&m, stack)]),
                _ => json!([]),
            }
        } else {
            value_for(f.kind(), stack)
        };
        obj.insert(f.json_name().to_string(), v);
    }
    stack.pop();
    Value::Object(obj)
}

fn value_for(kind: Kind, stack: &mut Vec<String>) -> Value {
    match kind {
        Kind::Double | Kind::Float => json!(0.0),
        Kind::Int32 | Kind::Sint32 | Kind::Sfixed32 | Kind::Uint32 | Kind::Fixed32 => json!(0),
        // 64-bit integers are strings in the canonical JSON mapping.
        Kind::Int64 | Kind::Sint64 | Kind::Sfixed64 | Kind::Uint64 | Kind::Fixed64 => json!("0"),
        Kind::Bool => json!(false),
        Kind::String | Kind::Bytes => json!(""),
        Kind::Enum(e) => json!(e.values().next().map(|v| v.name().to_string()).unwrap_or_default()),
        Kind::Message(m) => template(&m, stack),
    }
}

fn well_known(name: &str) -> Option<Value> {
    Some(match name {
        "google.protobuf.Timestamp" => json!("1970-01-01T00:00:00Z"),
        "google.protobuf.Duration" => json!("0s"),
        "google.protobuf.Struct" | "google.protobuf.Empty" => json!({}),
        "google.protobuf.Value" => Value::Null,
        "google.protobuf.ListValue" => json!([]),
        "google.protobuf.Any" => json!({ "@type": "type.googleapis.com/google.protobuf.Empty" }),
        "google.protobuf.FieldMask" | "google.protobuf.StringValue" | "google.protobuf.BytesValue" => json!(""),
        "google.protobuf.DoubleValue" | "google.protobuf.FloatValue" => json!(0.0),
        "google.protobuf.Int64Value" | "google.protobuf.UInt64Value" => json!("0"),
        "google.protobuf.Int32Value" | "google.protobuf.UInt32Value" => json!(0),
        "google.protobuf.BoolValue" => json!(false),
        _ => return None,
    })
}
