//! GitHub / GitHub Enterprise REST API: `POST /repos/{owner}/{repo}/pulls`, then reviewers and
//! labels (failures there are warnings: the pull request exists).

use serde_json::json;

use crate::{Created, ForgeError, NewRequest, Remote, Result, api_error, clean, json_body, send};

fn request(client: &reqwest::Client, method: reqwest::Method, url: &str, token: &str) -> reqwest::RequestBuilder {
    client
        .request(method, url)
        .header("Authorization", format!("Bearer {}", token.trim()))
        .header("Accept", "application/vnd.github+json")
        .header("X-GitHub-Api-Version", "2022-11-28")
}

pub(crate) async fn create(client: &reqwest::Client, remote: &Remote, token: &str, req: &NewRequest) -> Result<Created> {
    let base = format!("{}/repos/{}", remote.api_url.trim_end_matches('/'), remote.path);
    let body = json!({ "title": req.title.trim(), "head": req.head, "base": req.base, "body": req.body, "draft": req.draft });
    let r = send(request(client, reqwest::Method::POST, &format!("{base}/pulls"), token).header("Content-Type", "application/json").body(json_body(&body)), token).await?;
    if r.status != 201 {
        return Err(api_error("Creating the pull request", &r, token));
    }
    let number = r.json["number"].as_u64().ok_or_else(|| ForgeError::Api("GitHub didn't return the pull request number".into()))?;
    let url = r.json["html_url"].as_str().map(str::to_string).unwrap_or_else(|| format!("{}/pull/{number}", remote.web_url));
    let mut warnings = Vec::new();
    let (teams, users): (Vec<&String>, Vec<&String>) = req.reviewers.iter().filter(|r| !r.trim().is_empty()).partition(|r| r.contains('/'));
    if !users.is_empty() || !teams.is_empty() {
        let team_slugs: Vec<&str> = teams.iter().filter_map(|t| t.split('/').nth(1)).collect();
        let body = json!({ "reviewers": users, "team_reviewers": team_slugs });
        match send(request(client, reqwest::Method::POST, &format!("{base}/pulls/{number}/requested_reviewers"), token).header("Content-Type", "application/json").body(json_body(&body)), token).await {
            Ok(r) if r.status == 201 || r.status == 200 => {}
            Ok(r) => warnings.push(api_error("Requesting reviewers", &r, token).to_string()),
            Err(e) => warnings.push(clean(&e.to_string(), token)),
        }
    }
    let labels: Vec<&String> = req.labels.iter().filter(|l| !l.trim().is_empty()).collect();
    if !labels.is_empty() {
        let body = json!({ "labels": labels });
        match send(request(client, reqwest::Method::POST, &format!("{base}/issues/{number}/labels"), token).header("Content-Type", "application/json").body(json_body(&body)), token).await {
            Ok(r) if r.status == 200 => {}
            Ok(r) => warnings.push(api_error("Adding labels", &r, token).to_string()),
            Err(e) => warnings.push(clean(&e.to_string(), token)),
        }
    }
    Ok(Created { number, url, warnings, via_cli: false })
}

pub(crate) async fn whoami(client: &reqwest::Client, api_url: &str, token: &str) -> Result<String> {
    let r = send(request(client, reqwest::Method::GET, &format!("{}/user", api_url.trim_end_matches('/')), token), token).await?;
    if r.status != 200 {
        return Err(api_error("Checking the token", &r, token));
    }
    r.json["login"].as_str().map(str::to_string).ok_or_else(|| ForgeError::Api("GitHub didn't say who the token belongs to".into()))
}
