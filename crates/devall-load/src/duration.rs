//! Human durations as written in test files: `30s`, `2m`, `1m30s`, `500ms`, `1.5s`, `1h`.
//!
//! [`DurationSpec`] keeps the text exactly as the user wrote it (so saving a file never rewrites
//! `90s` to `1m30s`) and is parsed on demand. Invalid text still deserializes — validation reports
//! it with a field path instead of making the whole file unreadable.

use std::borrow::Cow;
use std::fmt;
use std::time::Duration;

use schemars::{JsonSchema, Schema, SchemaGenerator, json_schema};
use serde::de::{self, Visitor};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

#[derive(Debug, Clone, PartialEq, Eq, Hash, Default)]
pub struct DurationSpec(pub String);

impl DurationSpec {
    pub fn new(text: impl Into<String>) -> Self {
        Self(text.into())
    }

    pub fn from_duration(d: Duration) -> Self {
        Self(format_duration(d))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn parse(&self) -> Result<Duration, String> {
        parse_duration(&self.0)
    }

    /// Parsed value, or zero when invalid (only call after validation).
    pub fn get(&self) -> Duration {
        self.parse().unwrap_or_default()
    }
}

impl fmt::Display for DurationSpec {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl From<&str> for DurationSpec {
    fn from(s: &str) -> Self {
        Self(s.to_string())
    }
}

/// Parses `1h30m`, `2m`, `30s`, `1.5s`, `500ms`, `250us`. A bare number means seconds.
pub fn parse_duration(text: &str) -> Result<Duration, String> {
    let s = text.trim();
    let bad = || format!("invalid duration '{text}' (use e.g. 30s, 2m, 1m30s, 500ms)");
    if s.is_empty() {
        return Err(bad());
    }
    if let Ok(secs) = s.parse::<f64>() {
        return secs_to_duration(secs).ok_or_else(bad);
    }
    let mut total = 0f64; // seconds
    let mut rest = s;
    while !rest.is_empty() {
        let num_len = rest.find(|c: char| !(c.is_ascii_digit() || c == '.')).unwrap_or(rest.len());
        if num_len == 0 {
            return Err(bad());
        }
        let n: f64 = rest[..num_len].parse().map_err(|_| bad())?;
        rest = &rest[num_len..];
        let unit_len = rest.find(|c: char| c.is_ascii_digit() || c == '.').unwrap_or(rest.len());
        let unit = rest[..unit_len].trim();
        rest = rest[unit_len..].trim_start();
        let factor = match unit {
            "h" => 3600.0,
            "m" | "min" => 60.0,
            "s" | "sec" => 1.0,
            "ms" => 1e-3,
            "us" | "µs" => 1e-6,
            _ => return Err(bad()),
        };
        total += n * factor;
    }
    secs_to_duration(total).ok_or_else(bad)
}

fn secs_to_duration(secs: f64) -> Option<Duration> {
    (secs.is_finite() && (0.0..1e9).contains(&secs)).then(|| Duration::from_secs_f64(secs))
}

/// Compact canonical form: `1h2m3s`, `1m30s`, `30s`, `1.5s`, `500ms`, `0s`.
pub fn format_duration(d: Duration) -> String {
    let ms = d.as_millis();
    if ms == 0 {
        return if d.is_zero() { "0s".into() } else { format!("{}us", d.as_micros()) };
    }
    if ms < 1000 {
        return format!("{ms}ms");
    }
    if !ms.is_multiple_of(1000) {
        let s = d.as_secs_f64();
        if s < 60.0 {
            return format!("{}s", trim_float(s));
        }
        return format!("{ms}ms");
    }
    let mut secs = d.as_secs();
    let mut out = String::new();
    let h = secs / 3600;
    secs %= 3600;
    let m = secs / 60;
    secs %= 60;
    if h > 0 {
        out.push_str(&format!("{h}h"));
    }
    if m > 0 {
        out.push_str(&format!("{m}m"));
    }
    if secs > 0 || out.is_empty() {
        out.push_str(&format!("{secs}s"));
    }
    out
}

fn trim_float(v: f64) -> String {
    let s = format!("{v:.3}");
    s.trim_end_matches('0').trim_end_matches('.').to_string()
}

impl Serialize for DurationSpec {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for DurationSpec {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct V;
        impl Visitor<'_> for V {
            type Value = DurationSpec;
            fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
                f.write_str("a duration such as 30s, 2m or 500ms")
            }
            fn visit_str<E: de::Error>(self, v: &str) -> Result<Self::Value, E> {
                Ok(DurationSpec(v.to_string()))
            }
            fn visit_u64<E: de::Error>(self, v: u64) -> Result<Self::Value, E> {
                Ok(DurationSpec(format!("{v}s")))
            }
            fn visit_i64<E: de::Error>(self, v: i64) -> Result<Self::Value, E> {
                Ok(DurationSpec(format!("{v}s")))
            }
            fn visit_f64<E: de::Error>(self, v: f64) -> Result<Self::Value, E> {
                Ok(DurationSpec(format!("{}s", trim_float(v))))
            }
        }
        d.deserialize_any(V)
    }
}

impl JsonSchema for DurationSpec {
    fn schema_name() -> Cow<'static, str> {
        "Duration".into()
    }
    fn json_schema(_: &mut SchemaGenerator) -> Schema {
        json_schema!({
            "type": "string",
            "description": "A duration: 30s, 2m, 1m30s, 500ms, 1.5s, 1h",
            "pattern": "^\\s*(\\d+(\\.\\d+)?\\s*(h|m|min|s|sec|ms|us|µs)\\s*)+$|^\\d+(\\.\\d+)?$",
            "examples": ["30s", "2m", "1m30s", "500ms"]
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_common_forms() {
        assert_eq!(parse_duration("30s").unwrap(), Duration::from_secs(30));
        assert_eq!(parse_duration("2m").unwrap(), Duration::from_secs(120));
        assert_eq!(parse_duration("1m30s").unwrap(), Duration::from_secs(90));
        assert_eq!(parse_duration("1h 2m").unwrap(), Duration::from_secs(3720));
        assert_eq!(parse_duration("500ms").unwrap(), Duration::from_millis(500));
        assert_eq!(parse_duration("1.5s").unwrap(), Duration::from_millis(1500));
        assert_eq!(parse_duration("250us").unwrap(), Duration::from_micros(250));
        assert_eq!(parse_duration("10").unwrap(), Duration::from_secs(10));
        for bad in ["", "s", "10x", "-1s", "1.2.3s", "ms5"] {
            assert!(parse_duration(bad).is_err(), "{bad} should fail");
        }
    }

    #[test]
    fn formats_compactly() {
        assert_eq!(format_duration(Duration::from_secs(90)), "1m30s");
        assert_eq!(format_duration(Duration::from_secs(3600)), "1h");
        assert_eq!(format_duration(Duration::from_millis(500)), "500ms");
        assert_eq!(format_duration(Duration::from_millis(1500)), "1.5s");
        assert_eq!(format_duration(Duration::ZERO), "0s");
        for s in ["1h2m3s", "45s", "2m", "750ms"] {
            assert_eq!(format_duration(parse_duration(s).unwrap()), s);
        }
    }

    #[test]
    fn serde_keeps_text_and_accepts_numbers() {
        let d: DurationSpec = serde_yaml_ng::from_str("90s").unwrap();
        assert_eq!(d.as_str(), "90s");
        assert_eq!(serde_yaml_ng::to_string(&d).unwrap().trim(), "90s");
        let d: DurationSpec = serde_yaml_ng::from_str("45").unwrap();
        assert_eq!(d.get(), Duration::from_secs(45));
        // Invalid text survives deserialization; validation reports it.
        let d: DurationSpec = serde_yaml_ng::from_str("soon").unwrap();
        assert!(d.parse().is_err());
    }
}
