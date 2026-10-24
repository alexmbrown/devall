//! Project-wide chaos (a /dev/all extension, `GET/PUT /__admin/devall/chaos`): added latency with
//! jitter, random error statuses and random connection faults for every mock request.

use serde::{Deserialize, Serialize};

use crate::request::MockRequest;
use crate::stub::Fault;
use crate::util::{java_regex, regex_matches};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ChaosConfig {
    pub enabled: bool,
    /// Latency added to every response, in milliseconds.
    pub latency_ms: u64,
    /// Extra random latency in `0..=jitter_ms`.
    pub jitter_ms: u64,
    /// Probability (0..=1) of answering with one of `error_statuses` instead of the stub.
    pub error_rate: f64,
    pub error_statuses: Vec<u16>,
    /// Probability (0..=1) of a connection fault instead of a response.
    pub fault_rate: f64,
    pub faults: Vec<Fault>,
    /// Only requests whose URL (path + query) fully matches this regex are affected.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url_pattern: Option<String>,
}

impl Default for ChaosConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            latency_ms: 0,
            jitter_ms: 0,
            error_rate: 0.0,
            error_statuses: vec![500, 502, 503, 504],
            fault_rate: 0.0,
            faults: Fault::ALL.to_vec(),
            url_pattern: None,
        }
    }
}

/// Named presets (the same ones as the app's Chaos dialog), for `devall mock serve --chaos`.
pub const CHAOS_PRESETS: &[(&str, &str)] = &[
    ("slow", "Slow network: +800 ms latency, up to 600 ms jitter"),
    ("flaky", "Flaky backend: 10% 5xx responses, 50–200 ms latency"),
    ("broken", "Broken connections: 5% connection faults"),
    ("all", "Everything at once: latency, 10% 5xx and 5% faults"),
];

impl ChaosConfig {
    /// A preset by name (`slow`, `flaky`, `broken`, `all`; a few longer aliases work too).
    pub fn preset(name: &str) -> Option<Self> {
        let base = Self { enabled: true, ..Self::default() };
        Some(match name.trim().to_ascii_lowercase().as_str() {
            "slow" | "slow-network" => Self { latency_ms: 800, jitter_ms: 600, ..base },
            "flaky" | "flaky-backend" => Self { latency_ms: 50, jitter_ms: 150, error_rate: 0.1, ..base },
            "broken" | "broken-connections" => Self { fault_rate: 0.05, ..base },
            "all" | "everything" => Self { latency_ms: 300, jitter_ms: 700, error_rate: 0.1, fault_rate: 0.05, ..base },
            "off" | "none" => Self::default(),
            _ => return None,
        })
    }
}

#[derive(Debug, Clone, Default)]
pub(crate) struct Chaos {
    pub config: ChaosConfig,
    url_re: Option<fancy_regex::Regex>,
}

/// What chaos does to one request.
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct ChaosDecision {
    pub latency_ms: u64,
    pub error_status: Option<u16>,
    pub fault: Option<Fault>,
}

impl ChaosDecision {
    pub fn to_json(&self) -> serde_json::Value {
        let mut m = serde_json::Map::new();
        m.insert("latencyMs".into(), self.latency_ms.into());
        if let Some(s) = self.error_status {
            m.insert("errorStatus".into(), s.into());
        }
        if let Some(f) = self.fault {
            m.insert("fault".into(), f.name().into());
        }
        serde_json::Value::Object(m)
    }
}

impl Chaos {
    pub fn new(config: ChaosConfig) -> Result<Self, String> {
        for (name, rate) in [("errorRate", config.error_rate), ("faultRate", config.fault_rate)] {
            if !(0.0..=1.0).contains(&rate) {
                return Err(format!("{name} must be between 0 and 1"));
            }
        }
        if config.error_rate > 0.0 && config.error_statuses.is_empty() {
            return Err("errorStatuses must not be empty when errorRate > 0".into());
        }
        if let Some(s) = config.error_statuses.iter().find(|s| !(100..=999).contains(*s)) {
            return Err(format!("invalid status {s}"));
        }
        if config.fault_rate > 0.0 && config.faults.is_empty() {
            return Err("faults must not be empty when faultRate > 0".into());
        }
        let url_re = match config.url_pattern.as_deref().filter(|p| !p.is_empty()) {
            Some(p) => Some(java_regex(p)?),
            None => None,
        };
        Ok(Self { config, url_re })
    }

    pub fn decide(&self, req: &MockRequest) -> Option<ChaosDecision> {
        let c = &self.config;
        if !c.enabled {
            return None;
        }
        if let Some(re) = &self.url_re
            && !regex_matches(re, &req.url)
        {
            return None;
        }
        let latency_ms = c.latency_ms + if c.jitter_ms > 0 { fastrand::u64(0..=c.jitter_ms) } else { 0 };
        let mut d = ChaosDecision { latency_ms, ..Default::default() };
        if c.fault_rate > 0.0 && fastrand::f64() < c.fault_rate {
            d.fault = Some(c.faults[fastrand::usize(..c.faults.len())]);
        } else if c.error_rate > 0.0 && fastrand::f64() < c.error_rate {
            d.error_status = Some(c.error_statuses[fastrand::usize(..c.error_statuses.len())]);
        }
        Some(d)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decisions() {
        let req = MockRequest::new("GET", "/api/x", vec![], Vec::new());
        assert!(Chaos::default().decide(&req).is_none());
        let always_error = Chaos::new(ChaosConfig { enabled: true, latency_ms: 5, error_rate: 1.0, error_statuses: vec![503], ..Default::default() }).unwrap();
        let d = always_error.decide(&req).unwrap();
        assert_eq!(d.error_status, Some(503));
        assert_eq!(d.latency_ms, 5);
        let faults = Chaos::new(ChaosConfig { enabled: true, fault_rate: 1.0, faults: vec![Fault::EmptyResponse], ..Default::default() }).unwrap();
        assert_eq!(faults.decide(&req).unwrap().fault, Some(Fault::EmptyResponse));
        let filtered = Chaos::new(ChaosConfig { enabled: true, error_rate: 1.0, url_pattern: Some("/other.*".into()), ..Default::default() }).unwrap();
        assert!(filtered.decide(&req).is_none());
        let jitter = Chaos::new(ChaosConfig { enabled: true, latency_ms: 10, jitter_ms: 5, ..Default::default() }).unwrap();
        for _ in 0..20 {
            let l = jitter.decide(&req).unwrap().latency_ms;
            assert!((10..=15).contains(&l));
        }
        assert!(Chaos::new(ChaosConfig { error_rate: 1.5, ..Default::default() }).is_err());
        assert!(Chaos::new(ChaosConfig { error_rate: 0.5, error_statuses: vec![], ..Default::default() }).is_err());
        assert!(Chaos::new(ChaosConfig { url_pattern: Some("(".into()), ..Default::default() }).is_err());
        let json = serde_json::to_value(ChaosConfig::default()).unwrap();
        assert_eq!(json["errorStatuses"], serde_json::json!([500, 502, 503, 504]));
        assert_eq!(json["faults"][0], "CONNECTION_RESET_BY_PEER");
        for (name, _) in CHAOS_PRESETS {
            let p = ChaosConfig::preset(name).unwrap();
            assert!(p.enabled && Chaos::new(p).is_ok(), "{name}");
        }
        assert_eq!(ChaosConfig::preset("Flaky-Backend").unwrap().error_rate, 0.1);
        assert!(!ChaosConfig::preset("off").unwrap().enabled);
        assert!(ChaosConfig::preset("nope").is_none());
    }
}
