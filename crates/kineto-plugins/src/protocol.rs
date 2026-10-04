//! `kineto-plugin/1`: one JSON request on a plugin's stdin, one JSON answer
//! on its stdout.
//!
//! Every request has `protocol` and `operation` (`describe`, `estimate`,
//! `invoke`, `reconcile`, `voices`) and, but for `describe` and `voices`,
//! the `capability` it is for (`image.generate`, `tts.speak`); `invoke`
//! and `reconcile` carry the call's `idempotency_key`, and `invoke` and
//! `estimate` its `request`. Files a request needs are listed in `inputs`
//! by path relative to the plugin's working directory, with their SHA-256.
//!
//! Every answer has `protocol` and `status`:
//!
//! - `ok`, with a `body`: what `describe`, `estimate` or `voices` asked for;
//! - `completed`, with `outputs` (files it wrote under `outputs/`, each with
//!   `path`, `size` and `sha256`) and optional `metadata`;
//! - `pending`, with the provider's `job`;
//! - `failed`, with a `class` (`retryable` or `terminal`), `accepted`
//!   (`false` only when the provider certainly did not take the request)
//!   and a `message`;
//! - `unknown`: a reconciled call the provider has no record of.
//!
//! An answer of another protocol version is refused; fields it adds are
//! ignored.

use serde::Deserialize;
use serde_json::{Value, json};

/// The protocol this build speaks.
pub const PROTOCOL: &str = "kineto-plugin/1";

/// How a failure may be treated.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FailureClass {
    /// Asking again later may succeed.
    Retryable,
    /// Asking again unchanged will not succeed.
    Terminal,
}

/// A file a plugin answered with, as it declared it.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeclaredFile {
    pub path: String,
    pub size: u64,
    pub sha256: String,
}

/// A plugin's answer.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum Answer {
    Ok {
        #[serde(default)]
        body: Value,
    },
    Completed {
        outputs: Vec<DeclaredFile>,
        #[serde(default)]
        metadata: Value,
    },
    Pending {
        job: String,
    },
    Failed {
        class: FailureClass,
        /// `Some(false)` only when the provider certainly did not take it.
        #[serde(default)]
        accepted: Option<bool>,
        #[serde(default)]
        message: String,
    },
    Unknown,
}

/// The request line for `operation`, with `fields` beside the protocol.
#[must_use]
pub fn request(operation: &str, fields: Value) -> Value {
    let mut request = json!({ "protocol": PROTOCOL, "operation": operation });
    if let (Some(request), Value::Object(fields)) = (request.as_object_mut(), fields) {
        for (name, value) in fields {
            request.entry(name).or_insert(value);
        }
    }
    request
}

/// Read one answer line: refused unless it is a JSON object of this
/// protocol version and one of the known statuses with its fields.
pub fn answer(line: &str) -> Result<Answer, String> {
    let value: Value = serde_json::from_str(line.trim())
        .map_err(|error| format!("the answer is not JSON: {error}"))?;
    match value.get("protocol").and_then(Value::as_str) {
        Some(PROTOCOL) => {}
        Some(other) => return Err(format!("the answer speaks {other}, not {PROTOCOL}")),
        None => return Err("the answer names no protocol".to_owned()),
    }
    serde_json::from_value(value)
        .map_err(|error| format!("the answer is not one Kineto knows: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn answers_are_read_only_in_this_protocol() {
        assert_eq!(
            answer(r#"{"protocol":"kineto-plugin/1","status":"pending","job":"j1","extra":true}"#),
            Ok(Answer::Pending {
                job: "j1".to_owned()
            })
        );
        assert_eq!(
            answer(
                r#"{"protocol":"kineto-plugin/1","status":"failed","class":"terminal","accepted":false,"message":"no"}"#
            ),
            Ok(Answer::Failed {
                class: FailureClass::Terminal,
                accepted: Some(false),
                message: "no".to_owned()
            })
        );
        assert!(
            answer(r#"{"protocol":"kineto-plugin/2","status":"unknown"}"#)
                .unwrap_err()
                .contains("kineto-plugin/2")
        );
        assert!(
            answer(r#"{"status":"unknown"}"#)
                .unwrap_err()
                .contains("no protocol")
        );
        assert!(answer("not json").unwrap_err().contains("not JSON"));
        assert!(
            answer(r#"{"protocol":"kineto-plugin/1","status":"completed"}"#).is_err(),
            "outputs are required"
        );
        assert!(answer(r#"{"protocol":"kineto-plugin/1","status":"exploded"}"#).is_err());
        let built = request(
            "invoke",
            json!({"capability": "image.generate", "protocol": "other"}),
        );
        assert_eq!(
            built["protocol"], PROTOCOL,
            "the protocol is not overwritten"
        );
        assert_eq!(built["operation"], "invoke");
        assert_eq!(built["capability"], "image.generate");
    }
}
