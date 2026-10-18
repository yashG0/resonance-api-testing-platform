use std::{
    collections::HashMap,
    time::{Duration, Instant},
};

use reqwest::{
    Client, Method, Url,
    header::{CONTENT_TYPE, HeaderMap, HeaderName, HeaderValue},
};

const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
const MAX_RESPONSE_BYTES: usize = 5 * 1024 * 1024;

#[derive(Debug)]
pub struct HttpResult {
    pub status: String,
    pub body: String,
    pub elapsed: Duration,
}

pub struct HttpClient {
    client: Client,
}

impl HttpClient {
    pub fn new() -> Result<Self, reqwest::Error> {
        let client = Client::builder()
            .connect_timeout(CONNECT_TIMEOUT)
            .timeout(REQUEST_TIMEOUT)
            .user_agent(concat!("Resonance/", env!("CARGO_PKG_VERSION")))
            .build()?;

        Ok(Self { client })
    }

    pub async fn send(
        &self,
        method: &str,
        url: &str,
        body: &str,
        headers_text: &str,
        params_text: &str,
        variables: &HashMap<String, String>,
    ) -> HttpResult {
        let start = Instant::now();

        let url = match interpolate_variables(url, variables) {
            Ok(value) => value,
            Err(error) => return failure(&error, start),
        };
        let body = match interpolate_variables(body, variables) {
            Ok(value) => value,
            Err(error) => return failure(&error, start),
        };
        let headers_text = match interpolate_variables(headers_text, variables) {
            Ok(value) => value,
            Err(error) => return failure(&error, start),
        };
        let params_text = match interpolate_variables(params_text, variables) {
            Ok(value) => value,
            Err(error) => return failure(&error, start),
        };

        let method = match method {
            "GET" => Method::GET,
            "POST" => Method::POST,
            "PUT" => Method::PUT,
            "DELETE" => Method::DELETE,
            _ => return failure("Unsupported HTTP method.", start),
        };

        // Parse the URL and append encoded query parameters.
        let mut parsed_url = match Url::parse(&url) {
            Ok(url) if matches!(url.scheme(), "http" | "https") => url,
            Ok(_) => return failure("Only HTTP and HTTPS URLs are supported.", start),
            Err(error) => return failure(&format!("Invalid URL: {error}"), start),
        };

        for (index, line) in params_text.lines().enumerate() {
            let line = line.trim();

            if line.is_empty() {
                continue;
            }

            let Some((key, value)) = line.split_once('=') else {
                return failure(
                    &format!(
                        "Invalid query parameter on line {}. Expected key=value.",
                        index + 1
                    ),
                    start,
                );
            };

            let key = key.trim();
            if key.is_empty() {
                return failure(
                    &format!("Query parameter name is empty on line {}.", index + 1),
                    start,
                );
            }

            parsed_url.query_pairs_mut().append_pair(key, value.trim());
        }

        // Parse custom headers.
        let mut headers = HeaderMap::new();

        for (index, line) in headers_text.lines().enumerate() {
            let line = line.trim();

            if line.is_empty() {
                continue;
            }

            let Some((name, value)) = line.split_once(':') else {
                return failure(
                    &format!(
                        "Invalid header on line {}. Expected Name: Value.",
                        index + 1
                    ),
                    start,
                );
            };

            let name = match HeaderName::from_bytes(name.trim().as_bytes()) {
                Ok(name) => name,
                Err(_) => {
                    return failure(
                        &format!("Invalid header name on line {}.", index + 1),
                        start,
                    );
                }
            };

            let value = match HeaderValue::from_str(value.trim()) {
                Ok(value) => value,
                Err(_) => {
                    return failure(
                        &format!("Invalid header value on line {}.", index + 1),
                        start,
                    );
                }
            };

            headers.append(name, value);
        }

        let has_body = matches!(method, Method::POST | Method::PUT) && !body.is_empty();

        if has_body {
            if let Err(error) = serde_json::from_str::<serde_json::Value>(&body) {
                return failure(&format!("Invalid JSON request body: {error}"), start);
            }

            if !headers.contains_key(CONTENT_TYPE) {
                headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
            }
        }

        let mut request = self.client.request(method, parsed_url).headers(headers);

        if has_body {
            request = request.body(body);
        }

        let response = match request.send().await {
            Ok(response) => response,
            Err(error) => {
                let message = if error.is_timeout() {
                    "Request timed out.".to_owned()
                } else {
                    format!("Request failed: {error}")
                };

                return failure(&message, start);
            }
        };

        let status = response.status().to_string();
        let mut response = response;
        let mut bytes = Vec::new();

        loop {
            match response.chunk().await {
                Ok(Some(chunk)) => {
                    if chunk.len() > MAX_RESPONSE_BYTES - bytes.len() {
                        return HttpResult {
                            status,
                            body: "Response exceeds the 5 MiB display limit.".into(),
                            elapsed: start.elapsed(),
                        };
                    }

                    bytes.extend_from_slice(&chunk);
                }
                Ok(None) => break,
                Err(error) => {
                    return HttpResult {
                        status,
                        body: format!("Failed to read response body: {error}"),
                        elapsed: start.elapsed(),
                    };
                }
            }
        }

        let raw = String::from_utf8_lossy(&bytes);

        let body = match serde_json::from_str::<serde_json::Value>(&raw) {
            Ok(json) => serde_json::to_string_pretty(&json).unwrap_or_else(|_| raw.into_owned()),
            Err(_) => raw.into_owned(),
        };

        HttpResult {
            status,
            body,
            elapsed: start.elapsed(),
        }
    }
}

fn interpolate_variables(
    input: &str,
    variables: &HashMap<String, String>,
) -> Result<String, String> {
    let mut output = String::with_capacity(input.len());
    let mut remaining = input;

    while let Some(start) = remaining.find("{{") {
        output.push_str(&remaining[..start]);
        let after_open = &remaining[start + 2..];
        let Some(end) = after_open.find("}}") else {
            return Err("Unclosed variable placeholder. Use {{variable_name}}.".into());
        };

        let key = after_open[..end].trim();
        if key.is_empty() {
            return Err("Variable placeholders cannot be empty.".into());
        }
        let Some(value) = variables.get(key) else {
            return Err(format!(
                "Collection variable '{{{{{key}}}}}' is not defined."
            ));
        };
        output.push_str(value);
        remaining = &after_open[end + 2..];
    }

    output.push_str(remaining);
    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn substitutes_collection_variables() {
        let variables = HashMap::from([
            ("base_url".into(), "https://api.example.com".into()),
            ("api_key".into(), "secret".into()),
        ]);
        assert_eq!(
            interpolate_variables("{{base_url}}/users?key={{api_key}}", &variables).unwrap(),
            "https://api.example.com/users?key=secret"
        );
        assert_eq!(
            interpolate_variables("plain text", &variables).unwrap(),
            "plain text"
        );
    }

    #[test]
    fn reports_missing_and_malformed_variables() {
        let variables = HashMap::new();
        assert!(
            interpolate_variables("{{base_url}}", &variables)
                .unwrap_err()
                .contains("base_url")
        );
        assert!(
            interpolate_variables("{{base_url", &variables)
                .unwrap_err()
                .contains("Unclosed")
        );
    }
}

fn failure(message: &str, start: Instant) -> HttpResult {
    HttpResult {
        status: "Error".into(),
        body: message.into(),
        elapsed: start.elapsed(),
    }
}
