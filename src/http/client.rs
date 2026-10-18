use std::time::{Duration, Instant};

use reqwest::{
    Client, Method,
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
    ) -> HttpResult {
        let start = Instant::now();

        let method = match method {
            "GET" => Method::GET,
            "POST" => Method::POST,
            "PUT" => Method::PUT,
            "DELETE" => Method::DELETE,
            _ => {
                return failure("Unsupported HTTP method.", start);
            }
        };

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

        if has_body && !headers.contains_key(CONTENT_TYPE) {
            headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
        }

        let mut request = self.client.request(method, url).headers(headers);

        if has_body {
            // Validate JSON before sending because the default
            // content type is application/json.
            if let Err(error) = serde_json::from_str::<serde_json::Value>(body) {
                return failure(&format!("Invalid JSON request body: {error}"), start);
            }

            request = request.body(body.to_owned());
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

fn failure(message: &str, start: Instant) -> HttpResult {
    HttpResult {
        status: "Error".into(),
        body: message.into(),
        elapsed: start.elapsed(),
    }
}
