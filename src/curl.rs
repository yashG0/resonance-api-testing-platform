use crate::http::client::RequestSpec;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CurlRequest {
    pub method: String,
    pub url: String,
    pub headers: String,
    pub body: String,
    pub body_format: String,
    pub auth_type: String,
    pub auth_value: String,
    pub auth_secret: String,
}

pub fn parse_curl(input: &str) -> Result<CurlRequest, String> {
    let tokens = shell_words(input)?;
    let mut tokens = tokens.iter().peekable();
    if tokens.peek().is_some_and(|token| {
        token.eq_ignore_ascii_case("curl") || token.eq_ignore_ascii_case("curl.exe")
    }) {
        tokens.next();
    }

    let mut method = None;
    let mut url = None;
    let mut headers = Vec::new();
    let mut body = None;
    let mut auth_type = "None".to_owned();
    let mut auth_value = String::new();
    let mut auth_secret = String::new();

    while let Some(token) = tokens.next() {
        let token = token.as_str();
        let value_option = match token {
            "-X" | "--request" => Some("method"),
            "-H" | "--header" => Some("header"),
            "-d" | "--data" | "--data-raw" | "--data-binary" => Some("body"),
            "-u" | "--user" => Some("basic auth"),
            "--url" => Some("url"),
            "-A" | "--user-agent" => Some("user agent"),
            "-b" | "--cookie" => Some("cookie"),
            "--oauth2-bearer" => Some("bearer token"),
            "--compressed" | "-s" | "--silent" | "-S" | "--show-error" | "-v" | "--verbose"
            | "--globoff" => None,
            "-k" | "--insecure" => {
                return Err("The imported command disables TLS certificate checks. This client does not import that unsafe option.".into());
            }
            other if other.starts_with("-") => {
                return Err(format!("Unsupported cURL option: {other}"));
            }
            other => {
                if url.replace(other.to_owned()).is_some() {
                    return Err("The cURL command contains more than one URL".into());
                }
                continue;
            }
        };

        if let Some(option) = value_option {
            let value = tokens
                .next()
                .ok_or_else(|| format!("Missing value after {token}"))?;
            match option {
                "method" => method = Some(value.to_ascii_uppercase()),
                "header" => {
                    if !value.contains(':') {
                        return Err("Each cURL header must use the form Name: value".into());
                    }
                    headers.push(value.to_owned());
                }
                "body" => {
                    if body.is_some() {
                        return Err("Multiple cURL data options are not supported".into());
                    }
                    body = Some(value.to_owned());
                }
                "basic auth" => {
                    let (username, password) = value.split_once(':').unwrap_or((value, ""));
                    auth_type = "Basic".to_owned();
                    auth_value = username.to_owned();
                    auth_secret = password.to_owned();
                }
                "url" => url = Some(value.to_owned()),
                "user agent" => headers.push(format!("User-Agent: {value}")),
                "cookie" => headers.push(format!("Cookie: {value}")),
                "bearer token" => {
                    auth_type = "Bearer".to_owned();
                    auth_value = value.to_owned();
                }
                _ => unreachable!("all cURL values are handled"),
            }
        }
    }

    let url = url.ok_or_else(|| "The cURL command does not include a URL".to_owned())?;
    let method = method.unwrap_or_else(|| {
        if body.is_some() {
            "POST".to_owned()
        } else {
            "GET".to_owned()
        }
    });
    if !matches!(
        method.as_str(),
        "GET" | "POST" | "PUT" | "PATCH" | "DELETE" | "HEAD" | "OPTIONS"
    ) {
        return Err(format!("Unsupported HTTP method in cURL command: {method}"));
    }
    let body = body.unwrap_or_default();
    let body_format = if serde_json::from_str::<serde_json::Value>(&body).is_ok() {
        "JSON"
    } else {
        "Text"
    };

    Ok(CurlRequest {
        method,
        url,
        headers: headers.join("\n"),
        body,
        body_format: body_format.to_owned(),
        auth_type,
        auth_value,
        auth_secret,
    })
}

pub fn build_curl(spec: &RequestSpec<'_>) -> String {
    let mut command = format!("curl --request {}", shell_quote(spec.method));
    let url = append_query_params(spec.url, spec.query_params);
    command.push_str(&format!(" {}", shell_quote(&url)));
    let generated_auth_header = match spec.auth_type {
        "Bearer" if !spec.auth_value.is_empty() => Some("authorization".to_owned()),
        "API Key" => Some(spec.auth_value.trim().to_ascii_lowercase()),
        _ => None,
    };
    let has_content_type = spec
        .headers
        .lines()
        .filter_map(|line| line.split_once(':'))
        .any(|(name, _)| name.trim().eq_ignore_ascii_case("content-type"));
    for header in spec
        .headers
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
    {
        if let Some((name, _)) = header.split_once(':')
            && generated_auth_header
                .as_deref()
                .is_some_and(|auth_name| name.trim().eq_ignore_ascii_case(auth_name))
        {
            continue;
        }
        command.push_str(&format!(" \\\n  --header {}", shell_quote(header)));
    }
    if !spec.body.is_empty() && !has_content_type {
        let content_type = match spec.body_format {
            "Text" => "text/plain; charset=utf-8",
            "HTML" => "text/html; charset=utf-8",
            "XML" => "application/xml; charset=utf-8",
            _ => "application/json",
        };
        command.push_str(&format!(
            " \\\n  --header {}",
            shell_quote(&format!("Content-Type: {content_type}"))
        ));
    }
    match spec.auth_type {
        "Bearer" if !spec.auth_value.is_empty() => command.push_str(&format!(
            " \\\n  --header {}",
            shell_quote(&format!("Authorization: Bearer {}", spec.auth_value))
        )),
        "Basic" if !spec.auth_value.is_empty() => command.push_str(&format!(
            " \\\n  --user {}",
            shell_quote(&format!("{}:{}", spec.auth_value, spec.auth_secret))
        )),
        "API Key" => command.push_str(&format!(
            " \\\n  --header {}",
            shell_quote(&format!("{}: {}", spec.auth_value, spec.auth_secret))
        )),
        _ => {}
    }
    if !spec.body.is_empty() {
        command.push_str(&format!(" \\\n  --data-raw {}", shell_quote(spec.body)));
    }
    command
}

fn append_query_params(url: &str, params: &str) -> String {
    let Ok(mut parsed) = reqwest::Url::parse(url) else {
        return url.to_owned();
    };
    for line in params
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
    {
        if let Some((key, value)) = line.split_once('=') {
            parsed
                .query_pairs_mut()
                .append_pair(key.trim(), value.trim());
        }
    }
    parsed.into()
}

fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\"'\"'"))
}

fn shell_words(input: &str) -> Result<Vec<String>, String> {
    let mut words = Vec::new();
    let mut word = String::new();
    let mut quote = None;
    let mut escaping = false;
    let mut started = false;

    for character in input.chars() {
        if escaping {
            word.push(character);
            escaping = false;
            started = true;
            continue;
        }
        match (quote, character) {
            (Some('\''), '\'') | (Some('"'), '"') => quote = None,
            (Some('"'), '\\') => escaping = true,
            (Some(_), character) => word.push(character),
            (None, '\'') | (None, '"') => {
                quote = Some(character);
                started = true;
            }
            (None, '\\') => escaping = true,
            (None, character) if character.is_whitespace() => {
                if started {
                    words.push(std::mem::take(&mut word));
                    started = false;
                }
            }
            (None, character) => {
                word.push(character);
                started = true;
            }
        }
    }
    if escaping || quote.is_some() {
        return Err("The cURL command has an unfinished quote or escape".into());
    }
    if started {
        words.push(word);
    }
    Ok(words)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn imports_common_curl_requests_with_quoted_values() {
        let request = parse_curl(
            r#"curl -X PATCH 'https://example.com/items/7?mode=fast' -H 'Content-Type: application/json' --data-raw '{"name":"a b"}'"#,
        )
        .unwrap();

        assert_eq!(request.method, "PATCH");
        assert_eq!(request.url, "https://example.com/items/7?mode=fast");
        assert_eq!(request.headers, "Content-Type: application/json");
        assert_eq!(request.body, r#"{"name":"a b"}"#);
        assert_eq!(request.body_format, "JSON");
    }

    #[test]
    fn imports_basic_auth_and_rejects_unsafe_tls_disabling() {
        let request = parse_curl("curl --user 'alice:secret' https://example.com").unwrap();
        assert_eq!(request.auth_type, "Basic");
        assert_eq!(request.auth_value, "alice");
        assert_eq!(request.auth_secret, "secret");
        assert!(parse_curl("curl -k https://example.com").is_err());
    }

    #[test]
    fn curl_export_quotes_shell_metacharacters_and_keeps_auth() {
        let spec = RequestSpec {
            method: "POST",
            url: "https://example.com/",
            body: "it's data",
            headers: "X-Label: a b",
            query_params: "q=a b",
            body_format: "Text",
            auth_type: "Bearer",
            auth_value: "secret token",
            auth_secret: "",
            timeout_ms: 30_000,
            follow_redirects: true,
        };
        let command = build_curl(&spec);
        assert!(command.contains("'X-Label: a b'"));
        assert!(command.contains("'Content-Type: text/plain; charset=utf-8'"));
        assert!(command.contains("'it'\"'\"'s data'"));
        assert!(command.contains("Authorization: Bearer secret token"));
        assert!(command.contains("q=a+b"));
    }
}
