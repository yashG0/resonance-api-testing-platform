use std::time::{Duration, Instant};

use reqwest::{Client, Method};

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

    pub async fn send(&self, method: &str, url: &str) -> HttpResult {
        let start = Instant::now();

        let method = match method {
            "GET" => Method::GET,
            "POST" => Method::POST,
            "PUT" => Method::PUT,
            "DELETE" => Method::DELETE,
            _ => {
                return HttpResult {
                    status: "Error".into(),
                    body: "Unsupported HTTP method.".into(),
                    elapsed: start.elapsed(),
                };
            }
        };

        let response = match self.client.request(method, url).send().await {
            Ok(response) => response,
            Err(error) => {
                let message = if error.is_timeout() {
                    "Request timed out.".to_owned()
                } else if error.is_connect() {
                    format!("Connection failed: {error}")
                } else {
                    format!("Request failed: {error}")
                };

                return HttpResult {
                    status: "Error".into(),
                    body: message,
                    elapsed: start.elapsed(),
                };
            }
        };

        // HTTP 4xx and 5xx responses are still valid responses.
        let status = response.status().to_string();
        let mut response = response;
        let mut body = Vec::new();

        loop {
            match response.chunk().await {
                Ok(Some(chunk)) => {
                    if chunk.len() > MAX_RESPONSE_BYTES - body.len() {
                        body.clear();
                        body.extend_from_slice(b"Response body exceeds the 5 MiB display limit.");
                        break;
                    }

                    body.extend_from_slice(&chunk);
                }
                Ok(None) => break,
                Err(error) => {
                    body = format!("Failed to read response body: {error}").into_bytes();
                    break;
                }
            }
        }

        let raw_body = String::from_utf8_lossy(&body);

        let formatted_body = match serde_json::from_str::<serde_json::Value>(&raw_body) {
            Ok(json) => {
                serde_json::to_string_pretty(&json).unwrap_or_else(|_| raw_body.into_owned())
            }
            Err(_) => raw_body.into_owned(),
        };

        HttpResult {
            status,
            body: formatted_body,
            elapsed: start.elapsed(),
        }
    }
}
