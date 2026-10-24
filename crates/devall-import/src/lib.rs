//! Import, export and code generation for /dev/all.
//!
//! * [`parse_text`] / [`parse_path`] read a competitor export — auto-detected, see [`Format`] —
//!   into an [`ImportPlan`]: a format-neutral tree of folders and requests plus environments,
//!   project variables and warnings. [`ImportPlan::summary`] is the UI preview.
//! * [`apply`] writes a plan into a project through `devall_core::Project`. It never overwrites
//!   unless asked, and turns credentials into `{{secret.NAME}}` references; the values are
//!   returned in [`ApplyReport::secrets`] for the keychain and never written to files.
//! * [`export`] renders a project or folder as OpenAPI 3.1 (YAML), Postman Collection v2.1 (+
//!   environments), a JetBrains-compatible `.http` file (+ `http-client.env.json`) or Hurl.
//! * [`sync`] links a folder to the OpenAPI spec it came from and re-imports it in place.
//! * [`codegen`] turns a resolved `devall_http::PreparedRequest` into a snippet in 16 formats;
//!   [`codegen::redact`] masks secret values (raw, URL-/JSON-encoded and inside Basic auth) first.

use std::path::Path;

mod apply;
pub mod codegen;
mod detect;
mod error;
pub mod export;
mod formats;
mod plan;
mod secrets;
pub mod sync;
mod util;

pub use apply::{ApplyOptions, ApplyReport, apply};
pub use detect::{Format, detect, detect_path};
pub use error::{Error, Result};
pub use plan::{EnvSummary, ImportPlan, PlanFolder, PlanItem, PlanSummary, SecretCandidate, SummaryNode};
pub use secrets::{DetectedSecret, is_secret_name, is_secret_value, looks_like_secret};

/// Files larger than this are refused (a HAR of a long browsing session can be huge).
pub const MAX_INPUT_BYTES: u64 = 128 * 1024 * 1024;

/// Options that change how some formats are read.
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ParseOptions {
    /// HAR: keep `Cookie` headers (session cookies are then extracted as secrets).
    pub include_cookies: bool,
    /// HAR: keep requests for scripts, styles, images, fonts and other static assets.
    pub include_assets: bool,
}

/// Detects the format of `text` and parses it.
pub fn parse_text(text: &str, opts: &ParseOptions) -> Result<ImportPlan> {
    let format = detect(text).ok_or(Error::UnknownFormat)?;
    parse_text_as(text, format, opts)
}

/// Parses `text` as `format`.
pub fn parse_text_as(text: &str, format: Format, opts: &ParseOptions) -> Result<ImportPlan> {
    parse_named(text, format, opts, None)
}

fn parse_named(text: &str, format: Format, opts: &ParseOptions, name: Option<&str>) -> Result<ImportPlan> {
    let text = text.trim_start_matches('\u{feff}');
    let structured = || util::parse_structured(text);
    match format {
        Format::Postman => formats::postman::parse_collection(&structured()?),
        Format::PostmanEnvironment => formats::postman::parse_environment(&structured()?),
        Format::Insomnia => formats::insomnia::parse(&structured()?),
        Format::OpenApi | Format::Swagger => formats::openapi::parse(&structured()?),
        Format::Har => formats::har::parse(&structured()?, opts),
        Format::OpenCollection => formats::opencollection::parse_bundle(&structured()?),
        Format::Bruno => {
            if text.trim_start().starts_with('{') && detect::detect_value(&structured().unwrap_or_default()) == Some(Format::Bruno) {
                return Err(Error::Parse("bruno.json describes a collection folder; import the folder instead".into()));
            }
            formats::bruno::parse_single(text, name.unwrap_or("Bruno request"))
        }
        Format::HttpFile => Ok(formats::http_file::parse(text, name.unwrap_or("HTTP requests"))),
        Format::Hurl => Ok(formats::hurl::parse(text, name.unwrap_or("Hurl requests"))),
        Format::Curl => formats::curl::parse(text),
        Format::Plugin => Err(Error::Parse("plugin import formats are parsed by their plugin (in the app or the devall CLI)".into())),
    }
}

/// Parses a file or folder (Bruno / OpenCollection collections are folders).
pub fn parse_path(path: &Path, opts: &ParseOptions) -> Result<ImportPlan> {
    if path.is_dir() {
        return match detect_path(path) {
            Some(Format::Bruno) => formats::bruno::parse_dir(path),
            Some(Format::OpenCollection) => formats::opencollection::parse_dir(path),
            _ => Err(Error::UnknownFormat),
        };
    }
    let meta = std::fs::metadata(path).map_err(error::io_err(path))?;
    if meta.len() > MAX_INPUT_BYTES {
        return Err(Error::Parse(format!("{} is too large ({} MB)", path.display(), meta.len() / 1024 / 1024)));
    }
    let file = path.file_name().map(|n| n.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
    let parent = path.parent().unwrap_or(Path::new("."));
    if file == "bruno.json" || file == "collection.bru" {
        return formats::bruno::parse_dir(parent);
    }
    let text = std::fs::read_to_string(path).map_err(error::io_err(path))?;
    let format = detect_path(path).or_else(|| detect(&text)).ok_or(Error::UnknownFormat)?;
    if format == Format::OpenCollection && (file == "opencollection.yml" || file == "opencollection.yaml") {
        return formats::opencollection::parse_dir(parent);
    }
    let stem = path.file_stem().map(|s| s.to_string_lossy().to_string());
    match format {
        Format::HttpFile => Ok(formats::http_file::parse_file(&text, path)),
        _ => parse_named(&text, format, opts, stem.as_deref()),
    }
}

/// Parses either inline text or a path — the shape the UI sends.
pub fn parse_source(text: Option<&str>, path: Option<&Path>, opts: &ParseOptions) -> Result<ImportPlan> {
    match (path, text) {
        (Some(p), _) => parse_path(p, opts),
        (None, Some(t)) if !t.trim().is_empty() => parse_text(t, opts),
        _ => Err(Error::Parse("nothing to import".into())),
    }
}
