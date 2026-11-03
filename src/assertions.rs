//! Minimal, deterministic assertions for collection responses.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::time::Duration;

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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CollectionAssertionOutcome {
    pub passed: bool,
    pub state: String,
    pub checks: Vec<AssertionResult>,
}

pub fn evaluate_collection_assertions(
    assertions: &[Assertion],
    status: &str,
    http_state: &str,
    elapsed: Duration,
    body: &str,
) -> CollectionAssertionOutcome {
    let status_code = status
        .split_whitespace()
        .next()
        .and_then(|value| value.parse::<u16>().ok());
    let response_received = matches!(http_state, "success" | "http-error");
    let checks = assertions
        .iter()
        .map(|assertion| evaluate(assertion, status_code, elapsed, body))
        .collect::<Vec<_>>();
    let passed = if checks.is_empty() {
        http_state == "success"
    } else {
        response_received && checks.iter().all(|check| check.passed)
    };
    let state = if !response_received {
        http_state.to_owned()
    } else if passed {
        "success".to_owned()
    } else if checks.is_empty() {
        "http-error".to_owned()
    } else {
        "assertion-failed".to_owned()
    };
    CollectionAssertionOutcome {
        passed,
        state,
        checks,
    }
}

pub fn append_assertion_json(
    current: &str,
    kind: &str,
    first: &str,
    second: &str,
) -> Result<String, String> {
    let mut assertions = serde_json::from_str::<Vec<Assertion>>(current)
        .map_err(|error| format!("Current assertions are invalid JSON: {error}"))?;
    let assertion = match kind {
        "HTTP status" => {
            let expected = first
                .trim()
                .parse::<u16>()
                .ok()
                .filter(|status| (100..=599).contains(status))
                .ok_or_else(|| "Enter an HTTP status from 100 to 599".to_owned())?;
            Assertion::HttpStatus { expected }
        }
        "Latency under (ms)" => {
            let max_ms = first
                .trim()
                .parse::<u64>()
                .ok()
                .filter(|value| *value > 0)
                .ok_or_else(|| "Enter a positive latency in milliseconds".to_owned())?;
            Assertion::LatencyUnder { max_ms }
        }
        "Body contains" if !first.is_empty() => Assertion::BodyContains {
            text: first.to_owned(),
        },
        "JSON Pointer equals" => {
            if !first.is_empty() && !first.starts_with('/') {
                return Err("JSON Pointer must be empty or start with /".into());
            }
            let expected = serde_json::from_str(second)
                .map_err(|error| format!("Expected value must be valid JSON: {error}"))?;
            Assertion::JsonPointer {
                pointer: first.to_owned(),
                expected,
            }
        }
        "Body contains" => return Err("Enter text to search for in the response".into()),
        _ => return Err("Choose an assertion and enter its value".into()),
    };
    assertions.push(assertion);
    serde_json::to_string_pretty(&assertions).map_err(|error| error.to_string())
}

pub fn evaluate(
    assertion: &Assertion,
    status: Option<u16>,
    elapsed: Duration,
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
        Assertion::LatencyUnder { max_ms } => {
            let threshold = Duration::from_millis(*max_ms);
            (
                elapsed < threshold,
                format!(
                    "Latency: expected under {max_ms} ms, got {:.2} ms",
                    elapsed.as_secs_f64() * 1000.0
                ),
            )
        }
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
        assert!(
            evaluate(
                &Assertion::HttpStatus { expected: 200 },
                Some(200),
                Duration::from_millis(1),
                ""
            )
            .passed
        );
        assert!(
            !evaluate(
                &Assertion::HttpStatus { expected: 200 },
                Some(404),
                Duration::from_millis(1),
                ""
            )
            .passed
        );
    }

    #[test]
    fn evaluates_latency_under() {
        assert!(
            evaluate(
                &Assertion::LatencyUnder { max_ms: 100 },
                Some(200),
                Duration::from_micros(99_999),
                ""
            )
            .passed
        );
        assert!(
            !evaluate(
                &Assertion::LatencyUnder { max_ms: 100 },
                Some(200),
                Duration::from_millis(100),
                ""
            )
            .passed
        );
        assert!(
            !evaluate(
                &Assertion::LatencyUnder { max_ms: 100 },
                Some(200),
                Duration::from_micros(100_001),
                ""
            )
            .passed
        );
    }

    #[test]
    fn evaluates_body_contains() {
        assert!(
            evaluate(
                &Assertion::BodyContains {
                    text: "hello".into()
                },
                Some(200),
                Duration::from_millis(1),
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
                Duration::from_millis(1),
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
                Duration::from_millis(1),
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
                Duration::from_millis(1),
                r#"{"data":{"id":7}}"#
            )
            .passed
        );
    }

    #[test]
    fn collection_outcome_keeps_legacy_http_failures_without_assertions() {
        let outcome = evaluate_collection_assertions(
            &[],
            "404 Not Found",
            "http-error",
            Duration::from_millis(5),
            "missing",
        );
        assert!(!outcome.passed);
        assert_eq!(outcome.state, "http-error");
    }

    #[test]
    fn matching_expected_http_error_can_pass_assertions() {
        let outcome = evaluate_collection_assertions(
            &[Assertion::HttpStatus { expected: 404 }],
            "404 Not Found",
            "http-error",
            Duration::from_millis(5),
            "missing",
        );
        assert!(outcome.passed);
        assert_eq!(outcome.state, "success");
    }

    #[test]
    fn failed_assertion_and_transport_error_fail_collection_result() {
        let assertion = Assertion::BodyContains {
            text: "ready".into(),
        };
        let mismatch = evaluate_collection_assertions(
            std::slice::from_ref(&assertion),
            "200 OK",
            "success",
            Duration::from_millis(5),
            "still pending",
        );
        assert!(!mismatch.passed);
        assert_eq!(mismatch.state, "assertion-failed");

        let transport_error = evaluate_collection_assertions(
            &[assertion],
            "Connection failed",
            "connection-error",
            Duration::from_millis(5),
            "",
        );
        assert!(!transport_error.passed);
        assert_eq!(transport_error.state, "connection-error");
    }

    #[test]
    fn appends_validated_assertions_to_saved_json() {
        let updated = append_assertion_json("[]", "HTTP status", "201", "").unwrap();
        let assertions: Vec<Assertion> = serde_json::from_str(&updated).unwrap();
        assert_eq!(assertions, vec![Assertion::HttpStatus { expected: 201 }]);
        assert!(append_assertion_json("[]", "HTTP status", "99", "").is_err());
        assert!(append_assertion_json("[]", "JSON Pointer equals", "/id", "not-json").is_err());
    }
}
