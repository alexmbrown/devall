//! Saved regular expressions with their test cases: `utilities/regex/<slug>.regex.yaml`. Committed
//! with the project so a pattern's expectations (should / shouldn't match) travel with it and can
//! be reviewed in pull requests. Paths are `/`-separated, relative to `utilities/regex/`.

use std::fs;
use std::path::PathBuf;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::error::{Error, IoContext, Result};
use crate::fsutil::{normalize_rel, read_yaml, safe_join, slugify, unique_name, write_yaml};
use crate::model::{Extra, FORMAT_VERSION};
use crate::project::Project;

pub const UTILITIES_DIR: &str = "utilities";
pub const REGEX_DIR: &str = "regex";
pub const REGEX_EXT: &str = ".regex.yaml";

fn format_version() -> u32 {
    FORMAT_VERSION
}

/// `utilities/regex/<slug>.regex.yaml`
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct RegexFile {
    #[serde(default = "format_version")]
    pub version: u32,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub pattern: String,
    /// Subset of `imsxU`.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub flags: String,
    /// `rust` (linear time, the default) or `pcre-like` (look-around, backreferences).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub flavor: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub replacement: Option<String>,
    /// Strings the pattern must find a match in.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub should_match: Vec<String>,
    /// Strings it must not match anywhere.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub should_not_match: Vec<String>,
    #[serde(flatten)]
    pub extra: Extra,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct RegexEntry {
    pub path: String,
    pub name: String,
    pub pattern: String,
    pub cases: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl Project {
    pub fn regex_root(&self) -> PathBuf {
        self.root().join(UTILITIES_DIR).join(REGEX_DIR)
    }

    fn regex_path(&self, rel: &str) -> Result<PathBuf> {
        let rel = normalize_rel(rel);
        if !rel.ends_with(REGEX_EXT) {
            return Err(Error::InvalidPath(rel));
        }
        safe_join(&self.regex_root(), &rel)
    }

    /// Every saved regex (recursively), sorted by name.
    pub fn regexes(&self) -> Result<Vec<RegexEntry>> {
        fn walk(dir: &std::path::Path, rel: &str, out: &mut Vec<RegexEntry>) {
            let Ok(rd) = fs::read_dir(dir) else { return };
            for e in rd.flatten() {
                let name = e.file_name().to_string_lossy().to_string();
                if name.starts_with('.') {
                    continue;
                }
                let child = crate::fsutil::rel_join(rel, &name);
                if e.path().is_dir() {
                    walk(&e.path(), &child, out);
                } else if name.ends_with(REGEX_EXT) {
                    out.push(match read_yaml::<RegexFile>(&e.path()) {
                        Ok(f) => RegexEntry { path: child, name: f.name, pattern: f.pattern, cases: f.should_match.len() + f.should_not_match.len(), error: None },
                        Err(err) => RegexEntry { name: name.trim_end_matches(REGEX_EXT).to_string(), path: child, pattern: String::new(), cases: 0, error: Some(err.to_string()) },
                    });
                }
            }
        }
        let mut out = Vec::new();
        walk(&self.regex_root(), "", &mut out);
        out.sort_by_key(|e| e.name.to_lowercase());
        Ok(out)
    }

    pub fn read_regex(&self, rel: &str) -> Result<RegexFile> {
        let path = self.regex_path(rel)?;
        if !path.is_file() {
            return Err(Error::NotFound(format!("{UTILITIES_DIR}/{REGEX_DIR}/{}", normalize_rel(rel))));
        }
        read_yaml(&path)
    }

    /// Saves to `rel`, or (when `None`) to a new `<slug(name)>.regex.yaml`; returns the path.
    pub fn save_regex(&self, rel: Option<&str>, file: &RegexFile) -> Result<String> {
        if file.name.trim().is_empty() {
            return Err(Error::Invalid("a saved regex needs a name".into()));
        }
        let rel = match rel.map(normalize_rel).filter(|r| !r.is_empty()) {
            Some(r) => r,
            None => {
                let dir = self.regex_root();
                fs::create_dir_all(&dir).at(&dir)?;
                unique_name(&dir, &slugify(&file.name), REGEX_EXT)
            }
        };
        let mut f = file.clone();
        f.version = FORMAT_VERSION;
        f.name = f.name.trim().to_string();
        write_yaml(&self.regex_path(&rel)?, &f)?;
        Ok(rel)
    }

    pub fn delete_regex(&self, rel: &str) -> Result<()> {
        let path = self.regex_path(rel)?;
        if !path.exists() {
            return Err(Error::NotFound(normalize_rel(rel)));
        }
        trash::delete(&path).map_err(|e| Error::Io { path: path.clone(), source: std::io::Error::other(e.to_string()) })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn save_list_read() {
        let dir = tempfile::tempdir().unwrap();
        let p = Project::init(dir.path(), "R").unwrap();
        let f = RegexFile {
            version: 1,
            name: "Order id".into(),
            description: None,
            pattern: r"ORD-\d{4}".into(),
            flags: String::new(),
            flavor: None,
            replacement: None,
            should_match: vec!["ORD-1234".into()],
            should_not_match: vec!["ORD-12".into(), "ord-1234".into()],
            extra: Extra::new(),
        };
        let rel = p.save_regex(None, &f).unwrap();
        assert_eq!(rel, "order-id.regex.yaml");
        assert_eq!(p.save_regex(None, &f).unwrap(), "order-id-2.regex.yaml", "new saves never overwrite");
        let text = fs::read_to_string(p.regex_root().join(&rel)).unwrap();
        assert!(text.contains("shouldNotMatch:") && !text.contains("flags"), "{text}");
        assert_eq!(p.read_regex(&rel).unwrap(), f);
        let list = p.regexes().unwrap();
        assert_eq!(list.len(), 2);
        assert_eq!((list[0].name.as_str(), list[0].cases), ("Order id", 3));
        assert!(p.read_regex("../../devall.yaml").is_err());
        assert!(p.save_regex(Some("x.yaml"), &f).is_err(), "must keep the .regex.yaml extension");
    }
}
