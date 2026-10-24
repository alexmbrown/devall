//! A cookie jar the UI can inspect and edit (reqwest's built-in `Jar` can't be enumerated).

use std::sync::RwLock;

use reqwest::header::HeaderValue;
use serde::Serialize;

#[derive(Default)]
pub struct CookieJar(RwLock<cookie_store::CookieStore>);

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct CookieInfo {
    pub name: String,
    pub value: String,
    pub domain: String,
    pub path: String,
    pub secure: bool,
    pub http_only: bool,
    /// RFC 3339, or `None` for session cookies.
    pub expires: Option<String>,
}

impl CookieJar {
    pub fn list(&self) -> Vec<CookieInfo> {
        let store = self.0.read().unwrap();
        let mut out: Vec<CookieInfo> = store
            .iter_unexpired()
            .map(|c| CookieInfo {
                name: c.name().to_string(),
                value: c.value().to_string(),
                domain: c.domain.as_cow().map(|d| d.into_owned()).unwrap_or_default(),
                path: c.path.to_string(),
                secure: c.secure().unwrap_or(false),
                http_only: c.http_only().unwrap_or(false),
                expires: c.expires_datetime().map(|t| {
                    let secs = t.unix_timestamp();
                    chrono::DateTime::from_timestamp(secs, 0).map(|d| d.to_rfc3339()).unwrap_or_default()
                }),
            })
            .collect();
        out.sort_by(|a, b| (&a.domain, &a.path, &a.name).cmp(&(&b.domain, &b.path, &b.name)));
        out
    }

    pub fn remove(&self, domain: &str, path: &str, name: &str) -> bool {
        self.0.write().unwrap().remove(domain, path, name).is_some()
    }

    pub fn clear(&self) {
        self.0.write().unwrap().clear();
    }

    /// Adds a cookie as if `url` had set it (`name=value; Path=/…`).
    pub fn add(&self, set_cookie: &str, url: &url::Url) -> Result<(), String> {
        let c = cookie::Cookie::parse(set_cookie.to_string()).map_err(|e| e.to_string())?;
        self.0.write().unwrap().store_response_cookies(std::iter::once(c.into_owned()), url);
        Ok(())
    }
}

impl reqwest::cookie::CookieStore for CookieJar {
    fn set_cookies(&self, cookie_headers: &mut dyn Iterator<Item = &HeaderValue>, url: &url::Url) {
        let parsed = cookie_headers.filter_map(|v| v.to_str().ok()).filter_map(|s| cookie::Cookie::parse(s.to_string()).ok()).map(|c| c.into_owned());
        self.0.write().unwrap().store_response_cookies(parsed, url);
    }

    fn cookies(&self, url: &url::Url) -> Option<HeaderValue> {
        let s = self.0.read().unwrap().get_request_values(url).map(|(n, v)| format!("{n}={v}")).collect::<Vec<_>>().join("; ");
        if s.is_empty() { None } else { HeaderValue::from_str(&s).ok() }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use reqwest::cookie::CookieStore;

    #[test]
    fn store_list_remove() {
        let jar = CookieJar::default();
        let url = url::Url::parse("https://api.example.com/v1/login").unwrap();
        let h = [HeaderValue::from_static("sid=abc; Path=/; HttpOnly; Secure"), HeaderValue::from_static("theme=dark")];
        jar.set_cookies(&mut h.iter(), &url);
        let list = jar.list();
        assert_eq!(list.len(), 2);
        let sid = list.iter().find(|c| c.name == "sid").unwrap();
        assert!(sid.http_only && sid.secure);
        assert!(jar.cookies(&url::Url::parse("https://api.example.com/v1/x").unwrap()).unwrap().to_str().unwrap().contains("sid=abc"));
        assert!(jar.remove(&sid.domain, &sid.path, "sid"));
        jar.add("k=v; Path=/", &url).unwrap();
        assert_eq!(jar.list().len(), 2);
        jar.clear();
        assert!(jar.list().is_empty());
    }
}
