//! Schema-constrained generation with validation and one repair retry.
//!
//! 1. Ask with the JSON schema (enforced by the backend when it can; always described in the
//!    prompt).
//! 2. Extract JSON from the answer (tolerating code fences / chatter / `<think>`), validate it
//!    against the schema in Rust, then run the feature's own check (regex compiles and matches the
//!    samples, JSONPath selects something…).
//! 3. On failure, send the problem back once and ask for corrected JSON.
//!
//! A *schema* failure after the retry is an error; a failed *domain* check returns the best
//! answer with `problem` set so the UI can show what didn't validate.

use serde_json::Value;

use crate::prompts::REPAIR_PROMPT;
use crate::provider::{ChatMessage, ChatRequest, Provider, strip_think};
use crate::{AiError, Result, schema};

#[derive(Debug, Clone)]
pub struct Structured {
    pub value: Value,
    pub raw: String,
    pub model: String,
    pub repaired: bool,
    pub schema_enforced: bool,
    /// Domain check that still fails after the repair attempt.
    pub problem: Option<String>,
}

/// Finds the JSON document in a model answer.
pub fn extract_json(text: &str) -> Option<Value> {
    let t = strip_think(text);
    let t = t.trim();
    if let Ok(v) = serde_json::from_str::<Value>(t)
        && (v.is_object() || v.is_array())
    {
        return Some(v);
    }
    if let Some(start) = t.find("```") {
        let after = &t[start + 3..];
        let after = after.strip_prefix("json").or_else(|| after.strip_prefix("JSON")).unwrap_or(after);
        if let Some(end) = after.find("```")
            && let Ok(v) = serde_json::from_str::<Value>(after[..end].trim())
        {
            return Some(v);
        }
    }
    let open = t.find(['{', '['])?;
    let close_char = if t.as_bytes()[open] == b'{' { '}' } else { ']' };
    let close = t.rfind(close_char)?;
    (close > open).then(|| serde_json::from_str(&t[open..=close]).ok()).flatten()
}

enum Outcome {
    Good(Value),
    Soft(Value, String),
    Bad(String),
}

pub type DomainCheck<'a> = &'a (dyn Fn(&Value) -> std::result::Result<(), String> + Send + Sync);

fn assess(text: &str, schema: &Value, domain: DomainCheck<'_>) -> Outcome {
    let Some(v) = extract_json(text) else {
        return Outcome::Bad("the answer was not valid JSON.".into());
    };
    let errs = schema::validate(&v, schema);
    if !errs.is_empty() {
        return Outcome::Bad(format!("it did not match the required JSON shape: {}.", errs.join("; ")));
    }
    match domain(&v) {
        Ok(()) => Outcome::Good(v),
        Err(e) => Outcome::Soft(v, e),
    }
}

fn clip(s: &str, n: usize) -> String {
    if s.chars().count() <= n { s.to_string() } else { s.chars().take(n).collect() }
}

/// Runs `req` (which must carry a JSON schema) and validates the answer.
pub async fn generate(provider: &Provider, req: &ChatRequest, domain: DomainCheck<'_>) -> Result<Structured> {
    let schema = req.json_schema.as_ref().map(|s| s.schema.clone()).ok_or_else(|| AiError::Invalid("structured call without a schema".into()))?;
    let first = provider.chat(req).await?;
    let problem = match assess(&first.text, &schema, domain) {
        Outcome::Good(value) => {
            return Ok(Structured { value, raw: first.text, model: first.model, repaired: false, schema_enforced: first.schema_enforced, problem: None });
        }
        Outcome::Soft(value, p) => (Some(value), p),
        Outcome::Bad(p) => (None, p),
    };
    let mut retry = req.clone();
    retry.messages.push(ChatMessage::assistant(clip(&first.text, 6000)));
    retry.messages.push(ChatMessage::user(REPAIR_PROMPT.replace("{problem}", &problem.1)));
    let second = provider.chat(&retry).await?;
    match assess(&second.text, &schema, domain) {
        Outcome::Good(value) => Ok(Structured { value, raw: second.text, model: second.model, repaired: true, schema_enforced: second.schema_enforced, problem: None }),
        Outcome::Soft(value, p) => Ok(Structured { value, raw: second.text, model: second.model, repaired: true, schema_enforced: second.schema_enforced, problem: Some(p) }),
        Outcome::Bad(p) => match problem.0 {
            // The first answer was well-formed; keep it rather than failing.
            Some(value) => Ok(Structured { value, raw: first.text, model: first.model, repaired: false, schema_enforced: first.schema_enforced, problem: Some(problem.1) }),
            None => Err(AiError::InvalidOutput { message: p, raw: clip(&second.text, 2000) }),
        },
    }
}

/// What a live check found out about an accepted candidate (a plan, a dry-run result…).
pub type CheckNote = Value;
pub type CheckFuture<'a> = std::pin::Pin<Box<dyn std::future::Future<Output = std::result::Result<CheckNote, String>> + Send + 'a>>;

/// A check that needs I/O (a database `EXPLAIN`, a Kubernetes dry run). `Ok(note)` accepts the
/// candidate (the note is returned with it); `Err` is fed back to the model once, like a failed
/// domain check.
pub trait Checker: Send + Sync {
    fn check<'a>(&'a self, candidate: &'a Value) -> CheckFuture<'a>;
}

/// Accepts everything (no live validation available).
pub struct NoCheck;

impl Checker for NoCheck {
    fn check<'a>(&'a self, _candidate: &'a Value) -> CheckFuture<'a> {
        Box::pin(async { Ok(Value::Null) })
    }
}

/// A validated answer plus what the live check said about it.
#[derive(Debug, Clone)]
pub struct Checked {
    pub out: Structured,
    /// The checker's note for the accepted answer (`None` when it still failed — see
    /// `out.problem`).
    pub note: Option<CheckNote>,
}

async fn assess_checked(text: &str, schema: &Value, domain: DomainCheck<'_>, checker: &dyn Checker) -> (Outcome, Option<CheckNote>) {
    match assess(text, schema, domain) {
        Outcome::Good(v) => match checker.check(&v).await {
            Ok(note) => (Outcome::Good(v), Some(note)),
            Err(e) => (Outcome::Soft(v, e), None),
        },
        other => (other, None),
    }
}

/// [`generate`] with a synchronous domain check *and* an asynchronous live check (run only when
/// the answer is well-formed). One repair round for either kind of failure.
pub async fn generate_checked(provider: &Provider, req: &ChatRequest, domain: DomainCheck<'_>, checker: &dyn Checker) -> Result<Checked> {
    let schema = req.json_schema.as_ref().map(|s| s.schema.clone()).ok_or_else(|| AiError::Invalid("structured call without a schema".into()))?;
    let first = provider.chat(req).await?;
    let problem = match assess_checked(&first.text, &schema, domain, checker).await {
        (Outcome::Good(value), note) => {
            return Ok(Checked { out: Structured { value, raw: first.text, model: first.model, repaired: false, schema_enforced: first.schema_enforced, problem: None }, note });
        }
        (Outcome::Soft(value, p), _) => (Some(value), p),
        (Outcome::Bad(p), _) => (None, p),
    };
    let mut retry = req.clone();
    retry.messages.push(ChatMessage::assistant(clip(&first.text, 6000)));
    retry.messages.push(ChatMessage::user(REPAIR_PROMPT.replace("{problem}", &problem.1)));
    let second = provider.chat(&retry).await?;
    let done = |value, raw, model, enforced, repaired, problem, note| Checked { out: Structured { value, raw, model, repaired, schema_enforced: enforced, problem }, note };
    match assess_checked(&second.text, &schema, domain, checker).await {
        (Outcome::Good(value), note) => Ok(done(value, second.text, second.model, second.schema_enforced, true, None, note)),
        (Outcome::Soft(value, p), _) => Ok(done(value, second.text, second.model, second.schema_enforced, true, Some(p), None)),
        (Outcome::Bad(p), _) => match problem.0 {
            Some(value) => Ok(done(value, first.text, first.model, first.schema_enforced, false, Some(problem.1), None)),
            None => Err(AiError::InvalidOutput { message: p, raw: clip(&second.text, 2000) }),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn extracts_json_from_chatty_answers() {
        assert_eq!(extract_json(r#"{"a":1}"#), Some(json!({"a":1})));
        assert_eq!(extract_json("<think>hmm</think>\n```json\n{\"a\": 2}\n```\nDone."), Some(json!({"a":2})));
        assert_eq!(extract_json("Sure! Here it is: {\"a\": [1, 2]} hope it helps"), Some(json!({"a":[1,2]})));
        assert_eq!(extract_json("[1,2,3]"), Some(json!([1, 2, 3])));
        assert_eq!(extract_json("no json here"), None);
    }
}
