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
    pub headers: String,
    pub cookies: String,
    pub format: String,
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
        body_format: &str,
        auth_type: &str,
        auth_value: &str,
        auth_secret: &str,
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
        let auth_value = match interpolate_variables(auth_value, variables) {
            Ok(value) => value,
            Err(error) => return failure(&error, start),
        };
        let auth_secret = match interpolate_variables(auth_secret, variables) {
            Ok(value) => value,
            Err(error) => return failure(&error, start),
        };

        let method = match method {
            "GET" => Method::GET,
            "POST" => Method::POST,
            "PUT" => Method::PUT,
            "PATCH" => Method::PATCH,
            "DELETE" => Method::DELETE,
            "HEAD" => Method::HEAD,
            "OPTIONS" => Method::OPTIONS,
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

        let has_body = !body.is_empty();

        if has_body {
            if body_format == "JSON"
                && let Err(error) = serde_json::from_str::<serde_json::Value>(&body)
            {
                return failure(&format!("Invalid JSON request body: {error}"), start);
            }

            if !headers.contains_key(CONTENT_TYPE) {
                let content_type = match body_format {
                    "Text" => "text/plain; charset=utf-8",
                    "HTML" => "text/html; charset=utf-8",
                    "XML" => "application/xml; charset=utf-8",
                    _ => "application/json",
                };
                headers.insert(CONTENT_TYPE, HeaderValue::from_static(content_type));
            }
        }

        let mut request = self.client.request(method, parsed_url).headers(headers);
        match auth_type {
            "Bearer" if !auth_value.is_empty() => request = request.bearer_auth(auth_value),
            "Basic" if !auth_value.is_empty() => {
                request = request.basic_auth(auth_value, Some(auth_secret));
            }
            _ => {}
        }

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
        let response_headers = format_response_headers(response.headers());
        let response_cookies = format_response_cookies(
            response
                .headers()
                .get_all(reqwest::header::SET_COOKIE)
                .iter()
                .map(|value| value.to_str().unwrap_or("<binary value>")),
        );
        let content_type = response
            .headers()
            .get(CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .unwrap_or("")
            .split(';')
            .next()
            .unwrap_or("")
            .trim()
            .to_ascii_lowercase();
        let mut response = response;
        let mut bytes = Vec::new();

        loop {
            match response.chunk().await {
                Ok(Some(chunk)) => {
                    if chunk.len() > MAX_RESPONSE_BYTES - bytes.len() {
                        return HttpResult {
                            status,
                            body: "Response exceeds the 5 MiB display limit.".into(),
                            headers: response_headers,
                            cookies: response_cookies,
                            format: "Text".into(),
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
                        headers: response_headers,
                        cookies: response_cookies,
                        format: "Text".into(),
                        elapsed: start.elapsed(),
                    };
                }
            }
        }

        let raw = String::from_utf8_lossy(&bytes);
        let (format, body) = format_response_body(&raw, &content_type);

        HttpResult {
            status,
            body,
            headers: response_headers,
            cookies: response_cookies,
            format,
            elapsed: start.elapsed(),
        }
    }
}

fn format_response_headers(headers: &HeaderMap) -> String {
    let mut entries = headers
        .iter()
        .map(|(name, value)| {
            let display_name = name
                .as_str()
                .split('-')
                .map(|part| {
                    let mut characters = part.chars();
                    match characters.next() {
                        Some(first) => first.to_ascii_uppercase().to_string() + characters.as_str(),
                        None => String::new(),
                    }
                })
                .collect::<Vec<_>>()
                .join("-");
            format!(
                "{display_name}: {}",
                value.to_str().unwrap_or("<binary value>")
            )
        })
        .collect::<Vec<_>>();
    entries.sort_unstable_by_key(|entry| entry.to_ascii_lowercase());
    entries.join("\n")
}

fn format_response_cookies<'a>(cookies: impl Iterator<Item = &'a str>) -> String {
    cookies
        .enumerate()
        .map(|(index, cookie)| {
            let mut parts = cookie.split(';').map(str::trim);
            let first = parts.next().unwrap_or_default();
            let (name, value) = first.split_once('=').unwrap_or((first, ""));
            let mut formatted = format!(
                "Cookie {} · {}\n  Value: {}",
                index + 1,
                name.trim(),
                value.trim()
            );

            for attribute in parts {
                if attribute.is_empty() {
                    continue;
                }
                if let Some((name, value)) = attribute.split_once('=') {
                    formatted.push_str(&format!("\n  {}: {}", name.trim(), value.trim()));
                } else {
                    formatted.push_str(&format!("\n  {}", attribute));
                }
            }

            formatted
        })
        .collect::<Vec<_>>()
        .join("\n\n")
}

fn format_response_body(raw: &str, content_type: &str) -> (String, String) {
    let media_type = content_type
        .split(';')
        .next()
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase();
    let is_json = media_type == "application/json"
        || media_type.ends_with("+json")
        || serde_json::from_str::<serde_json::Value>(raw).is_ok();

    if is_json {
        let body = serde_json::from_str::<serde_json::Value>(raw)
            .and_then(|json| serde_json::to_string_pretty(&json))
            .unwrap_or_else(|_| raw.to_owned());
        return ("JSON".into(), body);
    }

    let is_html = media_type == "text/html"
        || media_type == "application/xhtml+xml"
        || raw
            .trim_start()
            .to_ascii_lowercase()
            .starts_with("<!doctype html")
        || raw.trim_start().to_ascii_lowercase().starts_with("<html");
    if is_html {
        return ("HTML".into(), format_html(raw));
    }

    if media_type == "application/xml" || media_type == "text/xml" || media_type.ends_with("+xml") {
        return ("XML".into(), format_html(raw));
    }

    let format = if media_type.starts_with("text/") || media_type.is_empty() {
        "Text"
    } else {
        media_type.as_str()
    };
    (format.into(), raw.to_owned())
}

/// Adds indentation and line breaks to HTML while preserving tag contents.
/// Script and style contents are kept as one raw-text block so their `<` and `>`
/// characters aren't mistaken for HTML tags.
fn format_html(input: &str) -> String {
    const VOID_ELEMENTS: &[&str] = &[
        "area", "base", "br", "col", "embed", "hr", "img", "input", "link", "meta", "param",
        "source", "track", "wbr",
    ];

    let mut output = String::with_capacity(input.len().saturating_add(input.len() / 8));
    let mut cursor = 0;
    let mut depth = 0usize;

    while cursor < input.len() {
        let Some(relative_start) = input[cursor..].find('<') else {
            append_html_text(&mut output, &input[cursor..], depth);
            break;
        };
        let start = cursor + relative_start;
        if start > cursor {
            append_html_text(&mut output, &input[cursor..start], depth);
        }

        if input[start..].starts_with("<!--") {
            if let Some(end) = input[start + 4..].find("-->") {
                append_html_line(&mut output, &input[start..start + 4 + end + 3], depth);
                cursor = start + 4 + end + 3;
                continue;
            }
        }

        let Some(end) = find_html_tag_end(input, start + 1) else {
            append_html_text(&mut output, &input[start..], depth);
            break;
        };
        let tag = input[start + 1..end].trim();
        if tag.is_empty() {
            append_html_line(&mut output, "<>", depth);
            cursor = end + 1;
            continue;
        }

        let is_closing = tag.starts_with('/');
        let tag_name = tag
            .trim_start_matches('/')
            .split(|character: char| character.is_ascii_whitespace() || character == '/')
            .next()
            .unwrap_or("")
            .to_ascii_lowercase();
        if is_closing {
            depth = depth.saturating_sub(1);
            append_html_line(&mut output, &format!("<{tag}>"), depth);
        } else {
            append_html_line(&mut output, &format!("<{tag}>"), depth);
            let self_closing = tag.starts_with('!')
                || tag.starts_with('?')
                || tag.ends_with('/')
                || VOID_ELEMENTS.contains(&tag_name.as_str());
            if !self_closing {
                depth = depth.saturating_add(1);
            }
        }
        cursor = end + 1;

        if !is_closing && matches!(tag_name.as_str(), "script" | "style") {
            let closing_tag = format!("</{tag_name}");
            let remainder = input[cursor..].to_ascii_lowercase();
            if let Some(close_start) = remainder.find(&closing_tag) {
                let raw = input[cursor..cursor + close_start].trim();
                if !raw.is_empty() {
                    append_html_line(&mut output, raw, depth);
                }
                cursor += close_start;
            }
        }
    }

    output.trim_end().to_owned()
}

fn find_html_tag_end(input: &str, start: usize) -> Option<usize> {
    let mut quote = None;
    for (offset, character) in input[start..].char_indices() {
        match (quote, character) {
            (Some(current), value) if current == value => quote = None,
            (None, '\'' | '"') => quote = Some(character),
            (None, '>') => return Some(start + offset),
            _ => {}
        }
    }
    None
}

fn append_html_text(output: &mut String, text: &str, depth: usize) {
    let text = text.trim();
    if !text.is_empty() {
        append_html_line(output, text, depth);
    }
}

fn append_html_line(output: &mut String, line: &str, depth: usize) {
    if !output.is_empty() {
        output.push('\n');
    }
    for _ in 0..depth.min(64) {
        output.push_str("  ");
    }
    output.push_str(line.trim());
}

fn interpolate_variables(
    input: &str,
    variables: &HashMap<String, String>,
) -> Result<String, String> {
    let mut output = String::with_capacity(input.len());
    let mut cursor = 0;

    while cursor < input.len() {
        let Some(relative_start) = input[cursor..].find('{') else {
            output.push_str(&input[cursor..]);
            break;
        };
        let start = cursor + relative_start;
        output.push_str(&input[cursor..start]);

        let double_braced = input[start..].starts_with("{{");
        let key_start = start + if double_braced { 2 } else { 1 };
        let closing = if double_braced { "}}" } else { "}" };
        let Some(end) = input[key_start..].find(closing) else {
            if double_braced {
                return Err("Unclosed variable placeholder. Use {{variable_name}}.".into());
            }
            output.push('{');
            cursor = start + 1;
            continue;
        };

        let key = input[key_start..key_start + end].trim();
        if !valid_placeholder_key(key) {
            if double_braced {
                return Err("Variable placeholders must contain a valid variable name.".into());
            }
            output.push('{');
            cursor = start + 1;
            continue;
        }

        let Some(value) = variables.get(key) else {
            return Err(format!(
                "Collection variable '{{{{{key}}}}}' is not defined."
            ));
        };
        output.push_str(value);
        cursor = key_start + end + closing.len();
    }

    Ok(output)
}

fn valid_placeholder_key(key: &str) -> bool {
    let mut chars = key.chars();
    matches!(chars.next(), Some(character) if character == '_' || character.is_ascii_alphabetic())
        && chars.all(|character| character == '_' || character.is_ascii_alphanumeric())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_json_html_and_plain_text_responses_by_content_type() {
        let (format, json) = format_response_body(r#"{"ok":true,"count":2}"#, "application/json");
        assert_eq!(format, "JSON");
        assert!(json.contains("\n  \"ok\": true"));

        let (format, html) = format_response_body(
            "<!doctype html><html><body><h1>Hello</h1><script>if (a < b) { run(); }</script></body></html>",
            "text/html; charset=utf-8",
        );
        assert_eq!(format, "HTML");
        assert!(html.contains("\n<html>"));
        assert!(html.contains("if (a < b) { run(); }"));

        let raw_text = "first line\nsecond line";
        let (format, text) = format_response_body(raw_text, "text/plain; charset=utf-8");
        assert_eq!(format, "Text");
        assert_eq!(text, raw_text);
    }

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
            interpolate_variables("{base_url}/users?key={api_key}", &variables).unwrap(),
            "https://api.example.com/users?key=secret"
        );
        let host_variables = HashMap::from([("api".into(), "api.example.com".into())]);
        assert_eq!(
            interpolate_variables("https://{api}/health", &host_variables).unwrap(),
            "https://api.example.com/health"
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
        assert!(
            interpolate_variables("https://{missing}/", &variables)
                .unwrap_err()
                .contains("missing")
        );
    }
}

fn failure(message: &str, start: Instant) -> HttpResult {
    HttpResult {
        status: "Error".into(),
        body: message.into(),
        headers: String::new(),
        cookies: String::new(),
        format: "Text".into(),
        elapsed: start.elapsed(),
    }
}
