//! GitLab REST API (gitlab.com and self-managed): `POST /projects/:id/merge_requests` with
//! reviewers looked up by user name (`GET /users?username=`) and labels. Drafts get GitLab's
//! `Draft:` title prefix.

use percent_encoding::{NON_ALPHANUMERIC, utf8_percent_encode};
use serde_json::json;

use crate::{Created, ForgeError, NewRequest, Remote, Result, api_error, clean, json_body, send};

fn request(client: &reqwest::Client, method: reqwest::Method, url: &str, token: &str) -> reqwest::RequestBuilder {
    client.request(method, url).header("PRIVATE-TOKEN", token.trim())
}

/// `Draft: ` in front unless it's there already.
pub fn draft_title(title: &str, draft: bool) -> String {
    let t = title.trim();
    let lower = t.to_lowercase();
    if draft && !lower.starts_with("draft:") && !lower.starts_with("[draft]") { format!("Draft: {t}") } else { t.to_string() }
}

pub(crate) async fn create(client: &reqwest::Client, remote: &Remote, token: &str, req: &NewRequest) -> Result<Created> {
    let api = remote.api_url.trim_end_matches('/');
    let project = utf8_percent_encode(&remote.path, NON_ALPHANUMERIC).to_string();
    let mut warnings = Vec::new();
    let mut reviewer_ids: Vec<u64> = Vec::new();
    for name in req.reviewers.iter().map(|r| r.trim().trim_start_matches('@')).filter(|r| !r.is_empty()) {
        let url = format!("{api}/users?username={}", utf8_percent_encode(name, NON_ALPHANUMERIC));
        match send(request(client, reqwest::Method::GET, &url, token), token).await {
            Ok(r) if r.status == 200 => match r.json.as_array().and_then(|a| a.first()).and_then(|u| u["id"].as_u64()) {
                Some(id) => reviewer_ids.push(id),
                None => warnings.push(format!("no GitLab user “{name}” — not added as a reviewer")),
            },
            Ok(r) => warnings.push(api_error(&format!("Looking up reviewer {name}"), &r, token).to_string()),
            Err(e) => warnings.push(clean(&e.to_string(), token)),
        }
    }
    let labels: Vec<&str> = req.labels.iter().map(|l| l.trim()).filter(|l| !l.is_empty()).collect();
    let mut body = json!({
        "source_branch": req.head,
        "target_branch": req.base,
        "title": draft_title(&req.title, req.draft),
        "description": req.body,
    });
    if !labels.is_empty() {
        body["labels"] = json!(labels.join(","));
    }
    if !reviewer_ids.is_empty() {
        body["reviewer_ids"] = json!(reviewer_ids);
    }
    let r = send(request(client, reqwest::Method::POST, &format!("{api}/projects/{project}/merge_requests"), token).header("Content-Type", "application/json").body(json_body(&body)), token).await?;
    if r.status != 201 {
        return Err(api_error("Creating the merge request", &r, token));
    }
    let number = r.json["iid"].as_u64().ok_or_else(|| ForgeError::Api("GitLab didn't return the merge request number".into()))?;
    let url = r.json["web_url"].as_str().map(str::to_string).unwrap_or_else(|| format!("{}/-/merge_requests/{number}", remote.web_url));
    Ok(Created { number, url, warnings, via_cli: false })
}

pub(crate) async fn whoami(client: &reqwest::Client, api_url: &str, token: &str) -> Result<String> {
    let r = send(request(client, reqwest::Method::GET, &format!("{}/user", api_url.trim_end_matches('/')), token), token).await?;
    if r.status != 200 {
        return Err(api_error("Checking the token", &r, token));
    }
    r.json["username"].as_str().map(str::to_string).ok_or_else(|| ForgeError::Api("GitLab didn't say who the token belongs to".into()))
}
