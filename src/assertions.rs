//! Minimal, deterministic assertions for collection responses.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{collections::HashMap, time::Duration};

pub type ResponseHeaders = HashMap<String, Vec<String>>;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Assertion {
    HttpStatus { expected: u16 },
    HttpStatusClass { class: u16 },
    HttpStatusAnyOf { expected: Vec<u16> },
    LatencyUnder { max_ms: u64 },
    BodyContains { text: String },
    JsonPointer { pointer: String, expected: Value },
    HeaderExists { name: String },
    HeaderEquals { name: String, expected: String },
    HeaderContains { name: String, text: String },
    CaptureJsonPointer { pointer: String, variable: String },
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
    status_code: Option<u16>,
    headers: &ResponseHeaders,
    http_state: &str,
    elapsed: Duration,
    body: &str,
) -> CollectionAssertionOutcome {
    let response_received = matches!(http_state, "success" | "http-error");
    let parsed_json = assertions
        .iter()
        .any(|assertion| {
            matches!(
                assertion,
                Assertion::JsonPointer { .. } | Assertion::CaptureJsonPointer { .. }
            )
        })
        .then(|| serde_json::from_str::<Value>(body).ok())
        .flatten();
    let checks = assertions
        .iter()
        .map(|assertion| {
            evaluate_with_json(
                assertion,
                status_code,
                headers,
                elapsed,
                body,
                parsed_json.as_ref(),
            )
        })
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
        "HTTP status class" => {
            let class = first
                .trim()
                .strip_suffix("xx")
                .and_then(|value| value.parse::<u16>().ok())
                .filter(|value| (1..=5).contains(value))
                .ok_or_else(|| "Choose a status class from 1xx to 5xx".to_owned())?;
            Assertion::HttpStatusClass { class }
        }
        "HTTP status any of" => {
            let expected = first
                .split(',')
                .map(str::trim)
                .map(str::parse::<u16>)
                .collect::<Result<Vec<_>, _>>()
                .map_err(|_| "Enter comma-separated HTTP status codes".to_owned())?;
            if expected.is_empty() || expected.iter().any(|code| !(100..=599).contains(code)) {
                return Err("Enter one or more HTTP statuses from 100 to 599".into());
            }
            Assertion::HttpStatusAnyOf { expected }
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
        "Header exists" => Assertion::HeaderExists {
            name: validate_header_name(first)?,
        },
        "Header equals" => Assertion::HeaderEquals {
            name: validate_header_name(first)?,
            expected: second.to_owned(),
        },
        "Header contains" if !second.is_empty() => Assertion::HeaderContains {
            name: validate_header_name(first)?,
            text: second.to_owned(),
        },
        "Header contains" => return Err("Enter text to match in the response header".into()),
        "Capture JSON Pointer" => {
            if !first.is_empty() && !first.starts_with('/') {
                return Err("JSON Pointer must be empty or start with /".into());
            }
            let variable = second.trim();
            if !valid_variable_key(variable) {
                return Err("Variable names must start with a letter or underscore and contain only letters, numbers, or underscores".into());
            }
            Assertion::CaptureJsonPointer {
                pointer: first.to_owned(),
                variable: variable.to_owned(),
            }
        }
        "Body contains" => return Err("Enter text to search for in the response".into()),
        _ => return Err("Choose an assertion and enter its value".into()),
    };
    assertions.push(assertion);
    serde_json::to_string_pretty(&assertions).map_err(|error| error.to_string())
}

pub fn remove_assertion_json(current: &str, index: usize) -> Result<String, String> {
    let mut assertions = serde_json::from_str::<Vec<Assertion>>(current)
        .map_err(|error| format!("Current assertions are invalid JSON: {error}"))?;
    if index >= assertions.len() {
        return Err("Assertion no longer exists".to_owned());
    }
    assertions.remove(index);
    serde_json::to_string_pretty(&assertions).map_err(|error| error.to_string())
}

pub fn update_assertion_json(
    current: &str,
    index: usize,
    kind: &str,
    first: &str,
    second: &str,
) -> Result<String, String> {
    let mut assertions = serde_json::from_str::<Vec<Assertion>>(current)
        .map_err(|error| format!("Current assertions are invalid JSON: {error}"))?;
    if index >= assertions.len() {
        return Err("Assertion no longer exists".to_owned());
    }
    let replacement_json = append_assertion_json("[]", kind, first, second)?;
    let mut replacement = serde_json::from_str::<Vec<Assertion>>(&replacement_json)
        .map_err(|error| format!("Could not parse updated assertion: {error}"))?;
    assertions[index] = replacement.remove(0);
    serde_json::to_string_pretty(&assertions).map_err(|error| error.to_string())
}

#[cfg(test)]
fn evaluate(
    assertion: &Assertion,
    status: Option<u16>,
    elapsed: Duration,
    body: &str,
) -> AssertionResult {
    let parsed_json = serde_json::from_str::<Value>(body).ok();
    evaluate_with_json(
        assertion,
        status,
        &ResponseHeaders::new(),
        elapsed,
        body,
        parsed_json.as_ref(),
    )
}

fn evaluate_with_json(
    assertion: &Assertion,
    status: Option<u16>,
    headers: &ResponseHeaders,
    elapsed: Duration,
    body: &str,
    parsed_json: Option<&Value>,
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
        Assertion::HttpStatusClass { class } => {
            let passed = status.is_some_and(|status| status / 100 == *class);
            (
                passed,
                format!(
                    "HTTP status class: expected {class}xx, got {}",
                    status.map_or_else(|| "no response".into(), |code| format!("{}xx", code / 100))
                ),
            )
        }
        Assertion::HttpStatusAnyOf { expected } => {
            let actual = status
                .map(|code| code.to_string())
                .unwrap_or_else(|| "no response".into());
            (
                status.is_some_and(|code| expected.contains(&code)),
                format!(
                    "HTTP status: expected one of {}, got {actual}",
                    expected
                        .iter()
                        .map(u16::to_string)
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
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
            let actual = parsed_json.and_then(|value| value.pointer(pointer));
            (
                actual.is_some_and(|actual| json_values_equal(actual, expected)),
                format!(
                    "JSON Pointer {pointer}: expected {expected}, got {}",
                    actual
                        .map(ToString::to_string)
                        .unwrap_or_else(|| "missing or invalid JSON".into())
                ),
            )
        }
        Assertion::HeaderExists { name } => {
            let values = header_values(headers, name);
            (
                values.is_some(),
                format!(
                    "Header {name} {}",
                    if values.is_some() {
                        "exists"
                    } else {
                        "is missing"
                    }
                ),
            )
        }
        Assertion::HeaderEquals { name, expected } => {
            let values = header_values(headers, name);
            let passed = values.is_some_and(|values| values.iter().any(|value| value == expected));
            (
                passed,
                format!(
                    "Header {name}: expected {expected}, got {}",
                    display_header_values(values)
                ),
            )
        }
        Assertion::HeaderContains { name, text } => {
            let values = header_values(headers, name);
            let passed =
                values.is_some_and(|values| values.iter().any(|value| value.contains(text)));
            (
                passed,
                format!(
                    "Header {name} contains {text}: got {}",
                    display_header_values(values)
                ),
            )
        }
        Assertion::CaptureJsonPointer { pointer, variable } => {
            let found = parsed_json.and_then(|value| value.pointer(pointer));
            let passed = found.is_some() && valid_variable_key(variable);
            (
                passed,
                if passed {
                    format!("JSON Pointer {pointer} will be captured into {{{variable}}}")
                } else {
                    format!("Could not capture JSON Pointer {pointer} into {{{variable}}}")
                },
            )
        }
    };
    AssertionResult { passed, message }
}

pub fn captured_variable_values(assertions: &[Assertion], body: &str) -> Vec<(String, String)> {
    let Some(value) = assertions
        .iter()
        .any(|assertion| matches!(assertion, Assertion::CaptureJsonPointer { .. }))
        .then(|| serde_json::from_str::<Value>(body).ok())
        .flatten()
    else {
        return Vec::new();
    };
    assertions
        .iter()
        .filter_map(|assertion| {
            let Assertion::CaptureJsonPointer { pointer, variable } = assertion else {
                return None;
            };
            let captured = value.pointer(pointer)?;
            let captured = match captured {
                Value::String(value) => value.clone(),
                _ => captured.to_string(),
            };
            Some((variable.clone(), captured))
        })
        .collect()
}

pub fn valid_variable_key(key: &str) -> bool {
    let mut chars = key.chars();
    matches!(chars.next(), Some(character) if character == '_' || character.is_ascii_alphabetic())
        && chars.all(|character| character == '_' || character.is_ascii_alphanumeric())
}

fn validate_header_name(name: &str) -> Result<String, String> {
    let name = name.trim();
    reqwest::header::HeaderName::from_bytes(name.as_bytes())
        .map(|header| header.as_str().to_owned())
        .map_err(|_| "Enter a valid response header name".to_owned())
}

fn header_values<'a>(headers: &'a ResponseHeaders, name: &str) -> Option<&'a [String]> {
    headers
        .iter()
        .find(|(header_name, _)| header_name.eq_ignore_ascii_case(name))
        .map(|(_, values)| values.as_slice())
}

fn display_header_values(values: Option<&[String]>) -> String {
    values
        .map(|values| values.join(", "))
        .unwrap_or_else(|| "missing".to_owned())
}

fn json_values_equal(left: &Value, right: &Value) -> bool {
    match (left, right) {
        (Value::Number(left), Value::Number(right)) if !left.is_f64() && !right.is_f64() => {
            left == right
        }
        (Value::Number(left), Value::Number(right)) => left
            .as_f64()
            .zip(right.as_f64())
            .is_some_and(|(l, r)| l == r),
        (Value::Array(left), Value::Array(right)) => {
            left.len() == right.len()
                && left
                    .iter()
                    .zip(right)
                    .all(|(left, right)| json_values_equal(left, right))
        }
        (Value::Object(left), Value::Object(right)) => {
            left.len() == right.len()
                && left.iter().all(|(key, left)| {
                    right
                        .get(key)
                        .is_some_and(|right| json_values_equal(left, right))
                })
        }
        _ => left == right,
    }
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
    fn json_pointer_compares_equivalent_numbers_and_nested_values() {
        let assertions = [
            Assertion::JsonPointer {
                pointer: "/count".into(),
                expected: serde_json::json!(7.0),
            },
            Assertion::JsonPointer {
                pointer: "/nested".into(),
                expected: serde_json::json!({"ratio": 2.0}),
            },
        ];
        let outcome = evaluate_collection_assertions(
            &assertions,
            Some(200),
            &ResponseHeaders::new(),
            "success",
            Duration::from_millis(1),
            r#"{"count":7,"nested":{"ratio":2}}"#,
        );

        assert!(outcome.passed);
    }

    #[test]
    fn json_pointer_does_not_round_distinct_large_integers_together() {
        let assertion = Assertion::JsonPointer {
            pointer: "/id".into(),
            expected: serde_json::json!(9_007_199_254_740_993_u64),
        };
        let result = evaluate(
            &assertion,
            Some(200),
            Duration::from_millis(1),
            r#"{"id":9007199254740992}"#,
        );

        assert!(!result.passed);
    }

    #[test]
    fn evaluates_status_class_and_any_of_checks() {
        let assertions = [
            Assertion::HttpStatusClass { class: 2 },
            Assertion::HttpStatusAnyOf {
                expected: vec![200, 201],
            },
        ];
        let outcome = evaluate_collection_assertions(
            &assertions,
            Some(201),
            &ResponseHeaders::new(),
            "success",
            Duration::from_millis(1),
            "",
        );

        assert!(outcome.passed);
    }

    #[test]
    fn evaluates_response_header_checks_case_insensitively() {
        let assertions = [
            Assertion::HeaderExists {
                name: "content-type".into(),
            },
            Assertion::HeaderEquals {
                name: "CONTENT-TYPE".into(),
                expected: "application/json".into(),
            },
            Assertion::HeaderContains {
                name: "X-Request-Id".into(),
                text: "abc".into(),
            },
        ];
        let headers = HashMap::from([
            ("Content-Type".into(), vec!["application/json".into()]),
            ("x-request-id".into(), vec!["abc-123".into()]),
        ]);
        let outcome = evaluate_collection_assertions(
            &assertions,
            Some(200),
            &headers,
            "success",
            Duration::from_millis(1),
            "{}",
        );

        assert!(outcome.passed);
    }

    #[test]
    fn captures_json_pointer_values_for_later_requests() {
        let assertions = [Assertion::CaptureJsonPointer {
            pointer: "/data/token".into(),
            variable: "session_token".into(),
        }];
        let outcome = evaluate_collection_assertions(
            &assertions,
            Some(200),
            &ResponseHeaders::new(),
            "success",
            Duration::from_millis(1),
            r#"{"data":{"token":"abc123"}}"#,
        );

        assert!(outcome.passed);
        assert_eq!(
            captured_variable_values(&assertions, r#"{"data":{"token":"abc123"}}"#),
            vec![("session_token".into(), "abc123".into())]
        );
    }

    #[test]
    fn collection_outcome_keeps_legacy_http_failures_without_assertions() {
        let outcome = evaluate_collection_assertions(
            &[],
            Some(404),
            &ResponseHeaders::new(),
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
            Some(404),
            &ResponseHeaders::new(),
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
            Some(200),
            &ResponseHeaders::new(),
            "success",
            Duration::from_millis(5),
            "still pending",
        );
        assert!(!mismatch.passed);
        assert_eq!(mismatch.state, "assertion-failed");

        let transport_error = evaluate_collection_assertions(
            &[assertion],
            None,
            &ResponseHeaders::new(),
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
        assert!(append_assertion_json("[]", "HTTP status class", "6xx", "").is_err());
        assert!(append_assertion_json("[]", "HTTP status any of", "200, 700", "").is_err());
        assert!(append_assertion_json("[]", "JSON Pointer equals", "/id", "not-json").is_err());
        assert!(append_assertion_json("[]", "Header equals", "bad header", "x").is_err());
    }

    #[test]
    fn removes_one_assertion_without_rewriting_others() {
        let source = serde_json::to_string(&[
            Assertion::HttpStatus { expected: 200 },
            Assertion::HeaderExists {
                name: "etag".into(),
            },
        ])
        .unwrap();
        let updated = remove_assertion_json(&source, 0).unwrap();
        let assertions: Vec<Assertion> = serde_json::from_str(&updated).unwrap();

        assert_eq!(
            assertions,
            vec![Assertion::HeaderExists {
                name: "etag".into()
            }]
        );
        assert!(remove_assertion_json(&source, 2).is_err());
    }

    #[test]
    fn updates_an_assertion_in_place_and_preserves_its_neighbors() {
        let source = serde_json::to_string(&[
            Assertion::HttpStatus { expected: 200 },
            Assertion::HeaderExists {
                name: "etag".into(),
            },
        ])
        .unwrap();
        let updated = update_assertion_json(&source, 0, "HTTP status", "201", "").unwrap();
        let assertions: Vec<Assertion> = serde_json::from_str(&updated).unwrap();

        assert_eq!(assertions[0], Assertion::HttpStatus { expected: 201 });
        assert_eq!(
            assertions[1],
            Assertion::HeaderExists {
                name: "etag".into()
            }
        );
    }
}
