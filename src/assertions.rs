//! Minimal, deterministic assertions for collection responses.

use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Assertion {
    HttpStatus { expected: u16 },
    LatencyUnder { max_ms: u64 },
    BodyContains { text: String },
    JsonPointer { pointer: String, expected: Value },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AssertionResult {
    pub passed: bool,
    pub message: String,
}

pub fn evaluate(
    assertion: &Assertion,
    status: Option<u16>,
    elapsed_ms: u64,
    body: &str,
) -> AssertionResult {
    let (passed, message) = match assertion {
        Assertion::HttpStatus { expected } => {
            let actual = status
                .map(|s| s.to_string())
                .unwrap_or_else(|| "no response".into());
            (
                status == Some(*expected),
                format!("HTTP status: expected {expected}, got {actual}"),
            )
        }
        Assertion::LatencyUnder { max_ms } => (
            elapsed_ms < *max_ms,
            format!("Latency: expected under {max_ms} ms, got {elapsed_ms} ms"),
        ),
        Assertion::BodyContains { text } => (body.contains(text), format!("Body contains: {text}")),
        Assertion::JsonPointer { pointer, expected } => {
            let parsed = serde_json::from_str::<Value>(body);
            let actual = parsed
                .ok()
                .and_then(|value| value.pointer(pointer).cloned());
            (
                actual.as_ref() == Some(expected),
                format!(
                    "JSON Pointer {pointer}: expected {expected}, got {}",
                    actual
                        .map(|v| v.to_string())
                        .unwrap_or_else(|| "missing or invalid JSON".into())
                ),
            )
        }
    };
    AssertionResult { passed, message }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn evaluates_http_status() {
        assert!(evaluate(&Assertion::HttpStatus { expected: 200 }, Some(200), 1, "").passed);
        assert!(!evaluate(&Assertion::HttpStatus { expected: 200 }, Some(404), 1, "").passed);
    }

    #[test]
    fn evaluates_latency_under() {
        assert!(evaluate(&Assertion::LatencyUnder { max_ms: 100 }, Some(200), 99, "").passed);
        assert!(!evaluate(&Assertion::LatencyUnder { max_ms: 100 }, Some(200), 100, "").passed);
    }

    #[test]
    fn evaluates_body_contains() {
        assert!(
            evaluate(
                &Assertion::BodyContains {
                    text: "hello".into()
                },
                Some(200),
                1,
                "hello world"
            )
            .passed
        );
        assert!(
            !evaluate(
                &Assertion::BodyContains {
                    text: "missing".into()
                },
                Some(200),
                1,
                "hello"
            )
            .passed
        );
    }

    #[test]
    fn evaluates_json_pointer() {
        assert!(
            evaluate(
                &Assertion::JsonPointer {
                    pointer: "/data/id".into(),
                    expected: serde_json::json!(7)
                },
                Some(200),
                1,
                r#"{"data":{"id":7}}"#
            )
            .passed
        );
        assert!(
            !evaluate(
                &Assertion::JsonPointer {
                    pointer: "/data/id".into(),
                    expected: serde_json::json!(8)
                },
                Some(200),
                1,
                r#"{"data":{"id":7}}"#
            )
            .passed
        );
    }
}
