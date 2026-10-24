//! Load testing for /dev/all.
//!
//! A test (`<project>/load/<slug>.load.yaml`) load-tests saved requests — a weighted mix or an
//! ordered flow — with k6-style executors:
//!
//! | Executor | Model | Latency measured from |
//! |---|---|---|
//! | `constantVus`, `rampingVus` | closed (VUs loop) | the actual send |
//! | `constantArrivalRate`, `rampingArrivalRate` | open (iterations on a schedule) | the **intended** start — no coordinated omission |
//!
//! Typical use:
//!
//! ```no_run
//! # async fn demo(store: &dyn devall_core::SecretStore) -> devall_load::Result<()> {
//! use devall_load::*;
//! let project = devall_core::Project::open("/path/to/project")?;
//! let test = store::read_test(project.root(), "smoke.load.yaml")?;
//! let env = effective_environment(&test, None, Some("local"));
//! let plan = build_plan(&project, "smoke.load.yaml", &test, env.as_deref(), store)?;
//! let preflight = plan.preflight(); // warnings + `requiresConfirmation`
//! let secrets = plan.secret_values.clone();
//! let summary = run(plan, RunOptions::default(), None, RunControl::new()).await?;
//! let file = store::write_run(project.root(), &summary, &secrets)?;
//! # Ok(()) }
//! ```
//!
//! [`distributed`] splits a run across `devall load agent` workers ([`agent`]) on the user's own
//! machines and merges their HDR histograms exactly; [`cloud`] runs the open-model profile from
//! the relay's cloud regions (verified targets only) and reads the merged result back.

pub mod agent;
pub mod cli;
pub mod cloud;
pub mod compare;
pub mod distributed;
pub mod duration;
pub mod engine;
pub mod error;
pub mod metrics;
pub mod model;
pub mod plan;
pub mod schedule;
pub mod schema;
pub mod store;
pub mod summary;
pub mod threshold;
pub mod validate;

pub use compare::{BaselineBreakdown, CompareOptions, Comparison, MetricDelta, Verdict, baseline_breakdown, compare};
pub use duration::{DurationSpec, format_duration, parse_duration};
pub use agent::{AgentOptions, AgentServer, start_agent};
pub use distributed::{AgentTarget, DistributedOptions, K8sOptions, k8s_manifests, parse_agents, run_distributed};
pub use engine::{ProgressFn, RunControl, RunOptions, run, run_detailed};
pub use metrics::RecorderExport;
pub use error::{LoadError, Result};
pub use model::*;
pub use plan::{Confirmation, Plan, PortBudget, Preflight, build_plan, effective_environment, is_local_host, rebase_url};
pub use store::{LoadTestEntry, RunEntry};
pub use summary::{AgentRun, ErrorKinds, LoadSnapshot, LoadSummary, Machine, MockBaseline, Percentiles, RequestStats, RunPhase, TimelinePoint, Totals};
pub use threshold::{ThresholdResult, parse_threshold};
pub use validate::{Issue, MAX_VUS, validate};

#[cfg(test)]
mod tests;
