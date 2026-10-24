//! Log viewer engine and project event store for /dev/all.
//!
//! * [`file::LogFile`] indexes multi-GB files without loading them (compact line index built in the
//!   background, random access to line ranges, gzip and UTF-16 support, follow/tail with
//!   truncation and rotation detection).
//! * [`parse`] detects and parses JSON lines (incl. Docker/CRI wrappers), logfmt, Apache/Nginx
//!   access logs, syslog (RFC 3164/5424), klog and `[timestamp] LEVEL message`-style lines, and
//!   groups multi-line stack traces into the previous entry.
//! * [`query`] is a small query language (free text, regex, `field>=value`, AND/OR/NOT).
//! * [`analyze`] computes histograms, top field values and Drain-like message patterns.
//! * [`sql`] runs SQL over parsed entries (DuckDB when built with the `duckdb` feature, else SQLite).
//! * [`view::LogSet`] is one file or several merged by timestamp; it caches filter results.
//! * [`events::EventStore`] is the project event timeline (`<project data>/events.sqlite`, outside the repository) every
//!   module records into; [`correlation`] extracts correlation ids from headers and bodies.

pub mod analyze;
pub mod correlation;
pub mod events;
pub mod file;
pub mod index;
pub mod parse;
pub mod query;
pub mod sql;
pub mod time;
pub mod view;

mod par;

pub use analyze::{FieldInfo, Histogram, Pattern, TopValues};
pub use events::{Event, EventFilter, EventStore, Retention};
pub use file::{LogFile, OpenOptions, Progress, Refresh};
pub use parse::{Format, Level, LogEntry};
pub use query::{Filter, Query, QueryError};
pub use sql::{SqlEngine, SqlResult};
pub use view::{LogSet, Page};

#[derive(Debug, thiserror::Error)]
pub enum LogError {
    #[error("{0}")]
    Io(#[from] std::io::Error),
    #[error("{0}")]
    Query(#[from] QueryError),
    #[error("SQL: {0}")]
    Sql(String),
    #[error("event store: {0}")]
    Db(#[from] rusqlite::Error),
    #[error("{0}")]
    Invalid(String),
    #[error("cancelled")]
    Cancelled,
}

pub type Result<T> = std::result::Result<T, LogError>;

/// Current time in Unix milliseconds.
pub fn now_ms() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0)
}
