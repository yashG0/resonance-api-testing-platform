//! Headless collection runner used by local scripts and CI jobs.

use std::{collections::HashMap, fs, io::Read, path::PathBuf, time::Instant};

use crate::{
    assertions::{
        AssertionResult, CollectionAssertionOutcome, captured_variable_values,
        evaluate_collection_assertions,
    },
    http::{HttpClient, client::RequestSpec},
    storage::{HistoryRecord, RequestStore, SavedRequest},
};

const MAX_DATA_FILE_BYTES: usize = 4 * 1024 * 1024;
const MAX_DATA_ROWS: usize = 1_000;
const MAX_VARIABLES_PER_ROW: usize = 256;
const MAX_DATA_VALUE_BYTES: usize = 1024 * 1024;
const MAX_RUN_CASES: usize = 10_000;

#[derive(Debug, Default, PartialEq, Eq)]
struct RunOptions {
    collection: String,
    environment: Option<String>,
    database_path: Option<PathBuf>,
    data_path: Option<PathBuf>,
    junit_path: Option<PathBuf>,
}

#[derive(Debug)]
struct CaseResult {
    request: SavedRequest,
    status: String,
    elapsed_ms: i64,
    iteration: Option<usize>,
    outcome: CollectionAssertionOutcome,
}

pub async fn run(arguments: &[String]) -> Result<bool, String> {
    let options = parse_options(arguments)?;
    let store = match options.database_path.as_deref() {
        Some(path) => RequestStore::open(path),
        None => RequestStore::open_default(),
    }
    .map_err(|error| error.to_string())?;
    let collections = store
        .list_collections()
        .map_err(|error| error.to_string())?;
    let collection = collections
        .iter()
        .find(|collection| collection.name == options.collection)
        .ok_or_else(|| format!("Collection '{}' was not found.", options.collection))?;
    let requests = store
        .list_for_collection(collection.id)
        .map_err(|error| error.to_string())?;
    if requests.is_empty() {
        return Err(format!(
            "Collection '{}' has no saved requests.",
            collection.name
        ));
    }
    let data_rows = match options.data_path.as_deref() {
        Some(path) => load_data_rows(path)?,
        None => vec![HashMap::new()],
    };
    let case_count = requests
        .len()
        .checked_mul(data_rows.len())
        .filter(|count| *count <= MAX_RUN_CASES)
        .ok_or_else(|| {
            format!("This run would exceed the {MAX_RUN_CASES} request-iteration limit.")
        })?;

    let mut base_variables = store
        .list_variables(collection.id)
        .map_err(|error| error.to_string())?
        .into_iter()
        .map(|variable| (variable.key, variable.value))
        .collect::<HashMap<_, _>>();
    if let Some(environment_name) = options.environment.as_deref() {
        let environment = collections
            .iter()
            .find(|collection| collection.name == environment_name)
            .ok_or_else(|| format!("Environment '{environment_name}' was not found."))?;
        for variable in store
            .list_variables(environment.id)
            .map_err(|error| error.to_string())?
        {
            base_variables.insert(variable.key, variable.value);
        }
    }

    let client = HttpClient::new().map_err(|error| error.to_string())?;
    let run_started = Instant::now();
    let mut cases = Vec::with_capacity(case_count);
    let mut history = Vec::with_capacity(case_count);

    for (row_index, data_row) in data_rows.iter().enumerate() {
        let mut variables = base_variables.clone();
        variables.extend(data_row.clone());
        for request in requests.iter().cloned() {
            let request_started = Instant::now();
            let (status, elapsed_ms, outcome) =
                if let Some(error) = request.assertions_error.as_ref() {
                    (
                        "Invalid assertions".to_owned(),
                        0,
                        CollectionAssertionOutcome {
                            passed: false,
                            state: "assertion-failed".to_owned(),
                            checks: vec![AssertionResult {
                                passed: false,
                                message: error.clone(),
                            }],
                        },
                    )
                } else {
                    let response = client
                        .send(
                            &RequestSpec {
                                method: &request.method,
                                url: &request.url,
                                body: &request.body,
                                headers: &request.headers,
                                query_params: &request.query_params,
                                body_format: &request.body_format,
                                auth_type: &request.auth_type,
                                auth_value: &request.auth_value,
                                auth_secret: &request.auth_secret,
                                timeout_ms: request.timeout_ms as u64,
                                follow_redirects: request.follow_redirects,
                            },
                            &variables,
                        )
                        .await;
                    let outcome = evaluate_collection_assertions(
                        &request.assertions,
                        response.status_code,
                        &response.header_values,
                        response.state.as_label(),
                        response.elapsed,
                        &response.body,
                    );
                    if outcome.passed {
                        for (key, value) in
                            captured_variable_values(&request.assertions, &response.body)
                        {
                            variables.insert(key, value);
                        }
                    }
                    (
                        response.status,
                        request_started.elapsed().as_millis().min(i64::MAX as u128) as i64,
                        outcome,
                    )
                };

            let history_status = if outcome.passed {
                status.clone()
            } else {
                format!("{} · assertions failed", status)
            };
            history.push(HistoryRecord {
                request_id: request.id,
                collection_id: request.collection_id,
                request_name: request.name.clone(),
                method: request.method.clone(),
                url: request.url.clone(),
                status: history_status,
                elapsed_ms,
            });
            cases.push(CaseResult {
                request,
                status,
                elapsed_ms,
                iteration: options.data_path.as_ref().map(|_| row_index + 1),
                outcome,
            });
        }
    }

    if let Err(error) = store.record_history_batch(&history) {
        eprintln!("Warning: could not save request history: {error}");
    }

    let failures = cases.iter().filter(|case| !case.outcome.passed).count();
    for case in &cases {
        println!(
            "{}{} {} — {} ({} ms)",
            if case.outcome.passed { "PASS" } else { "FAIL" },
            case.iteration
                .map_or_else(String::new, |iteration| format!(" [{iteration}]")),
            case.request.name,
            case.status,
            case.elapsed_ms
        );
        for check in &case.outcome.checks {
            println!(
                "  {} {}",
                if check.passed { "✓" } else { "✗" },
                check.message
            );
        }
    }
    println!(
        "{} tests, {} passed, {} failed in {:.2}s",
        cases.len(),
        cases.len() - failures,
        failures,
        run_started.elapsed().as_secs_f64()
    );

    if let Some(path) = options.junit_path {
        let xml = junit_xml(&cases);
        fs::write(&path, xml).map_err(|error| {
            format!("Could not write JUnit report '{}': {error}", path.display())
        })?;
        println!("JUnit report written to {}", path.display());
    }

    Ok(failures == 0)
}

pub fn help() -> &'static str {
    "Commands:\n  cosmic-api             Open the desktop app\n  cosmic-api run <collection> [--database <file>] [--env <collection-name>] [--data <rows.json>] [--junit <report.xml>]\n  cosmic-api --help\n\n--database points to a prepared SQLite database (useful in CI); without it the app's normal database is used.\n--env selects a collection's variables as an overlay; those values override variables from the target collection.\n--data runs the collection once per object in a JSON array; each object's scalar values override variables."
}

fn parse_options(arguments: &[String]) -> Result<RunOptions, String> {
    let mut options = RunOptions::default();
    let mut index = 0;
    while index < arguments.len() {
        match arguments[index].as_str() {
            "--env" | "--database" | "--data" | "--junit" => {
                let flag = arguments[index].as_str();
                index += 1;
                let value = arguments
                    .get(index)
                    .filter(|value| !value.starts_with("--"))
                    .ok_or_else(|| format!("{flag} requires a value."))?;
                if flag == "--env" {
                    if options.environment.replace(value.clone()).is_some() {
                        return Err("--env may only be provided once.".to_owned());
                    }
                } else if flag == "--database" {
                    if options
                        .database_path
                        .replace(PathBuf::from(value))
                        .is_some()
                    {
                        return Err("--database may only be provided once.".to_owned());
                    }
                } else if flag == "--data" {
                    if options.data_path.replace(PathBuf::from(value)).is_some() {
                        return Err("--data may only be provided once.".to_owned());
                    }
                } else if options.junit_path.replace(PathBuf::from(value)).is_some() {
                    return Err("--junit may only be provided once.".to_owned());
                }
            }
            value if value.starts_with('-') => {
                return Err(format!("Unknown run option '{value}'."));
            }
            value if options.collection.is_empty() => options.collection = value.to_owned(),
            value => return Err(format!("Unexpected argument '{value}'.")),
        }
        index += 1;
    }
    if options.collection.is_empty() {
        return Err(usage().to_owned());
    }
    Ok(options)
}

fn usage() -> &'static str {
    "Usage: cosmic-api run <collection> [--database <file>] [--env <collection-name>] [--data <rows.json|rows.csv>] [--junit <report.xml>]"
}

fn load_data_rows(path: &std::path::Path) -> Result<Vec<HashMap<String, String>>, String> {
    let file = fs::File::open(path)
        .map_err(|error| format!("Could not read data file '{}': {error}", path.display()))?;
    let mut data = Vec::new();
    file.take((MAX_DATA_FILE_BYTES + 1) as u64)
        .read_to_end(&mut data)
        .map_err(|error| format!("Could not read data file '{}': {error}", path.display()))?;
    if data.len() > MAX_DATA_FILE_BYTES {
        return Err(format!(
            "Data file is larger than the {MAX_DATA_FILE_BYTES}-byte limit."
        ));
    }
    let rows = if path
        .extension()
        .and_then(std::ffi::OsStr::to_str)
        .is_some_and(|extension| extension.eq_ignore_ascii_case("csv"))
    {
        csv_rows_as_json(&data)?
    } else {
        serde_json::from_slice::<serde_json::Value>(&data)
            .map_err(|error| format!("Data file must contain a JSON array of objects: {error}"))?
    };
    let rows = rows
        .as_array()
        .ok_or_else(|| "Data file must contain a JSON array of objects.".to_owned())?;
    if rows.is_empty() || rows.len() > MAX_DATA_ROWS {
        return Err(format!(
            "Data file must contain between 1 and {MAX_DATA_ROWS} rows."
        ));
    }

    rows.iter()
        .enumerate()
        .map(|(row_index, row)| {
            let object = row.as_object().ok_or_else(|| {
                format!("Data row {} must be a JSON object.", row_index + 1)
            })?;
            if object.len() > MAX_VARIABLES_PER_ROW {
                return Err(format!(
                    "Data row {} has more than {MAX_VARIABLES_PER_ROW} variables.",
                    row_index + 1
                ));
            }
            object
                .iter()
                .map(|(key, value)| {
                    if !valid_data_variable_name(key) {
                        return Err(format!(
                            "Invalid variable name '{key}' in data row {}.",
                            row_index + 1
                        ));
                    }
                    let value = match value {
                        serde_json::Value::String(value) => value.clone(),
                        serde_json::Value::Number(value) => value.to_string(),
                        serde_json::Value::Bool(value) => value.to_string(),
                        _ => {
                            return Err(format!(
                                "Variable '{key}' in data row {} must be a string, number, or boolean.",
                                row_index + 1
                            ));
                        }
                    };
                    if value.len() > MAX_DATA_VALUE_BYTES {
                        return Err(format!(
                            "Variable '{key}' in data row {} is larger than the {MAX_DATA_VALUE_BYTES}-byte limit.",
                            row_index + 1
                        ));
                    }
                    Ok((key.clone(), value))
                })
                .collect::<Result<HashMap<_, _>, String>>()
        })
        .collect()
}

fn csv_rows_as_json(data: &[u8]) -> Result<serde_json::Value, String> {
    let text =
        std::str::from_utf8(data).map_err(|error| format!("CSV data must be UTF-8: {error}"))?;
    let mut records = parse_csv_records(text)?;
    if records.is_empty() {
        return Err("CSV data must include a header and at least one row.".to_owned());
    }
    let headers = records.remove(0);
    if headers.is_empty() || headers.len() > MAX_VARIABLES_PER_ROW {
        return Err(format!(
            "CSV header must contain between 1 and {MAX_VARIABLES_PER_ROW} columns."
        ));
    }
    for (index, header) in headers.iter().enumerate() {
        if header.is_empty() {
            return Err(format!("CSV header {} cannot be empty.", index + 1));
        }
        if headers[..index].contains(header) {
            return Err(format!("CSV header '{header}' is duplicated."));
        }
    }

    let rows = records
        .into_iter()
        .enumerate()
        .map(|(index, record)| {
            if record.len() != headers.len() {
                return Err(format!(
                    "CSV row {} has {} values; expected {}.",
                    index + 2,
                    record.len(),
                    headers.len()
                ));
            }
            Ok(serde_json::Value::Object(
                headers
                    .iter()
                    .cloned()
                    .zip(record.into_iter().map(serde_json::Value::String))
                    .collect(),
            ))
        })
        .collect::<Result<Vec<_>, String>>()?;
    Ok(serde_json::Value::Array(rows))
}

fn parse_csv_records(text: &str) -> Result<Vec<Vec<String>>, String> {
    let mut records = Vec::new();
    let mut record = Vec::new();
    let mut field = String::new();
    let mut chars = text.chars().peekable();
    let mut quoted = false;
    let mut quote_closed = false;

    while let Some(character) = chars.next() {
        if quoted {
            if character == '"' {
                if chars.peek() == Some(&'"') {
                    chars.next();
                    field.push('"');
                } else {
                    quoted = false;
                    quote_closed = true;
                }
            } else {
                field.push(character);
            }
            continue;
        }

        match character {
            ',' => {
                record.push(std::mem::take(&mut field));
                quote_closed = false;
            }
            '\n' | '\r' => {
                if character == '\r' && chars.peek() == Some(&'\n') {
                    chars.next();
                }
                record.push(std::mem::take(&mut field));
                records.push(std::mem::take(&mut record));
                quote_closed = false;
            }
            '"' if field.is_empty() && !quote_closed => quoted = true,
            '"' => return Err("CSV contains a quote inside an unquoted field.".to_owned()),
            _ if quote_closed => {
                return Err("CSV contains characters after a closing quote.".to_owned());
            }
            _ => field.push(character),
        }
    }

    if quoted {
        return Err("CSV contains an unterminated quoted field.".to_owned());
    }
    if !record.is_empty() || !field.is_empty() || quote_closed {
        record.push(field);
        records.push(record);
    }
    Ok(records)
}

fn valid_data_variable_name(name: &str) -> bool {
    let mut characters = name.chars();
    matches!(characters.next(), Some(first) if first == '_' || first.is_ascii_alphabetic())
        && name.len() <= 128
        && characters.all(|character| character == '_' || character.is_ascii_alphanumeric())
}

fn junit_xml(cases: &[CaseResult]) -> String {
    let failures = cases.iter().filter(|case| !case.outcome.passed).count();
    let mut xml = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<testsuite name=\"Cosmic API collection\" tests=\"{}\" failures=\"{}\" errors=\"0\" skipped=\"0\">\n",
        cases.len(),
        failures
    );
    for case in cases {
        let test_name = case.iteration.map_or_else(
            || case.request.name.clone(),
            |iteration| format!("{} [iteration {iteration}]", case.request.name),
        );
        xml.push_str(&format!(
            "  <testcase classname=\"{}\" name=\"{}\" time=\"{:.3}\">\n",
            xml_escape(&case.request.name),
            xml_escape(&test_name),
            case.elapsed_ms as f64 / 1000.0
        ));
        if !case.outcome.passed {
            let message = case
                .outcome
                .checks
                .iter()
                .filter(|check| !check.passed)
                .map(|check| check.message.as_str())
                .collect::<Vec<_>>()
                .join("; ");
            let message = if message.is_empty() {
                format!("Request ended with status {}", case.status)
            } else {
                message
            };
            xml.push_str(&format!(
                "    <failure message=\"{}\">{}</failure>\n",
                xml_escape(&message),
                xml_escape(&format!("{}: {message}", case.status))
            ));
        }
        xml.push_str("  </testcase>\n");
    }
    xml.push_str("</testsuite>\n");
    xml
}

fn xml_escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

#[cfg(test)]
mod tests {
    use super::{
        CaseResult, RunOptions, junit_xml, load_data_rows, parse_options, run, xml_escape,
    };
    use crate::{
        assertions::{Assertion, AssertionResult, CollectionAssertionOutcome},
        storage::{NewRequest, RequestStore, SavedRequest},
    };
    use std::{
        io::{Read, Write},
        net::TcpListener,
        path::PathBuf,
        thread,
    };

    fn args(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| (*value).to_owned()).collect()
    }

    #[test]
    fn parses_collection_environment_and_junit_options() {
        assert_eq!(
            parse_options(&args(&[
                "Staging API",
                "--database",
                "test-data.sqlite3",
                "--data",
                "rows.json",
                "--env",
                "Staging",
                "--junit",
                "artifacts/result.xml"
            ])),
            Ok(RunOptions {
                collection: "Staging API".into(),
                environment: Some("Staging".into()),
                database_path: Some(PathBuf::from("test-data.sqlite3")),
                data_path: Some(PathBuf::from("rows.json")),
                junit_path: Some(PathBuf::from("artifacts/result.xml")),
            })
        );
    }

    #[test]
    fn rejects_missing_duplicate_and_unknown_options() {
        assert!(parse_options(&[]).is_err());
        assert!(parse_options(&args(&["API", "--env"])).is_err());
        assert!(parse_options(&args(&["API", "--env", "one", "--env", "two"])).is_err());
        assert!(parse_options(&args(&["API", "--unknown"])).is_err());
        assert!(parse_options(&args(&["API", "--database"])).is_err());
        assert!(parse_options(&args(&["API", "--database", "one", "--database", "two"])).is_err());
        assert!(parse_options(&args(&["API", "--data"])).is_err());
        assert!(parse_options(&args(&["API", "--data", "one", "--data", "two"])).is_err());
    }

    #[test]
    fn escapes_junit_xml_values() {
        assert_eq!(xml_escape("a<&\"'"), "a&lt;&amp;&quot;&apos;");
    }

    #[test]
    fn parses_scalar_json_rows_and_rejects_nested_values() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let path = directory.path().join("rows.json");
        std::fs::write(
            &path,
            r#"[{"base_url":"https://one.test","enabled":true,"count":3}]"#,
        )
        .expect("write data file");
        let rows = load_data_rows(&path).expect("parse rows");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0]["base_url"], "https://one.test");
        assert_eq!(rows[0]["enabled"], "true");
        assert_eq!(rows[0]["count"], "3");

        std::fs::write(&path, r#"[{"nested":{"key":"value"}}]"#).expect("write nested data");
        assert!(
            load_data_rows(&path)
                .unwrap_err()
                .contains("must be a string, number, or boolean")
        );
        std::fs::write(&path, r#"[{"invalid-key":"value"}]"#).expect("write invalid variable name");
        assert!(
            load_data_rows(&path)
                .unwrap_err()
                .contains("Invalid variable name")
        );
    }

    #[test]
    fn parses_csv_rows_with_quoted_commas_quotes_and_newlines() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let path = directory.path().join("rows.CSV");
        std::fs::write(
            &path,
            "item,note\r\nfirst,\"hello, \"\"world\"\"\"\r\nsecond,\"line one\nline two\"\r\n",
        )
        .expect("write CSV rows");

        let rows = load_data_rows(&path).expect("parse CSV rows");
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0]["item"], "first");
        assert_eq!(rows[0]["note"], "hello, \"world\"");
        assert_eq!(rows[1]["note"], "line one\nline two");

        std::fs::write(&path, "item,item\nfirst,second\n").expect("write duplicate headers");
        assert!(load_data_rows(&path).unwrap_err().contains("duplicated"));
        std::fs::write(&path, "item,note\nfirst\n").expect("write short row");
        assert!(load_data_rows(&path).unwrap_err().contains("expected 2"));
        std::fs::write(&path, "item\n\"unterminated").expect("write malformed CSV");
        assert!(load_data_rows(&path).unwrap_err().contains("unterminated"));
    }

    #[test]
    fn emits_junit_failure_for_assertions_and_transport_errors() {
        let request = SavedRequest {
            id: 1,
            collection_id: 2,
            name: "lookup & verify".into(),
            method: "GET".into(),
            url: "https://example.test".into(),
            headers: String::new(),
            query_params: String::new(),
            body: String::new(),
            body_format: "None".into(),
            auth_type: "None".into(),
            auth_value: String::new(),
            auth_secret: String::new(),
            assertions: vec![],
            timeout_ms: crate::storage::DEFAULT_REQUEST_TIMEOUT_MS,
            assertions_error: None,
            follow_redirects: true,
        };
        let cases = [
            CaseResult {
                request: request.clone(),
                status: "200 OK".into(),
                elapsed_ms: 12,
                iteration: None,
                outcome: CollectionAssertionOutcome {
                    passed: false,
                    state: "assertion-failed".into(),
                    checks: vec![AssertionResult {
                        passed: false,
                        message: "body contains <ready>".into(),
                    }],
                },
            },
            CaseResult {
                request,
                status: "Connection failed".into(),
                elapsed_ms: 0,
                iteration: None,
                outcome: CollectionAssertionOutcome {
                    passed: false,
                    state: "connection-error".into(),
                    checks: vec![],
                },
            },
        ];
        let xml = junit_xml(&cases);
        assert!(xml.contains("tests=\"2\" failures=\"2\""));
        assert!(xml.contains("lookup &amp; verify"));
        assert!(xml.contains("body contains &lt;ready&gt;"));
        assert!(xml.contains("Request ended with status Connection failed"));
    }

    #[tokio::test]
    async fn runs_a_saved_collection_from_a_ci_database_and_writes_junit() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let database = directory.path().join("ci.sqlite3");
        let data = directory.path().join("rows.json");
        let junit = directory.path().join("junit.xml");
        std::fs::write(&data, r#"[{"item":"first"},{"item":"second"}]"#).expect("write data rows");
        let store = RequestStore::open(&database).expect("open test database");
        let collection = store
            .create_collection("CI checks")
            .expect("create collection");

        let server = TcpListener::bind("127.0.0.1:0").expect("bind mock server");
        let url = format!(
            "http://{}/{{{{item}}}}",
            server.local_addr().expect("server address")
        );
        let (path_sender, path_receiver) = std::sync::mpsc::channel();
        let server_thread = thread::spawn(move || {
            for _ in 0..2 {
                let (mut stream, _) = server.accept().expect("accept request");
                let mut request = [0_u8; 2048];
                let bytes = stream.read(&mut request).expect("read request");
                let request_line = String::from_utf8_lossy(&request[..bytes]);
                let path = request_line
                    .lines()
                    .next()
                    .and_then(|line| line.split_whitespace().nth(1))
                    .unwrap_or_default()
                    .to_owned();
                path_sender.send(path).expect("send captured path");
                let body = r#"{"status":"ready"}"#;
                write!(
                    stream,
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    body.len(),
                    body
                )
                .expect("write response");
            }
        });

        store
            .create(&NewRequest {
                collection_id: collection.id,
                name: "Ready check".into(),
                method: "GET".into(),
                url,
                headers: String::new(),
                query_params: String::new(),
                body: String::new(),
                body_format: "None".into(),
                auth_type: "None".into(),
                auth_value: String::new(),
                auth_secret: String::new(),
                assertions: vec![Assertion::BodyContains {
                    text: "not-ready".into(),
                }],
                timeout_ms: crate::storage::DEFAULT_REQUEST_TIMEOUT_MS,
                follow_redirects: true,
            })
            .expect("save request");

        let arguments = args(&[
            "CI checks",
            "--database",
            database.to_str().expect("database path"),
            "--data",
            data.to_str().expect("data path"),
            "--junit",
            junit.to_str().expect("JUnit path"),
        ]);
        assert!(!run(&arguments).await.expect("run collection"));
        server_thread.join().expect("join mock server");

        let report = std::fs::read_to_string(&junit).expect("read JUnit report");
        assert!(report.contains("tests=\"2\" failures=\"2\""));
        assert!(report.contains("Ready check [iteration 1]"));
        assert!(report.contains("Ready check [iteration 2]"));
        assert!(report.contains("Body contains: not-ready"));
        let paths = [
            path_receiver.recv().expect("first captured path"),
            path_receiver.recv().expect("second captured path"),
        ];
        assert_eq!(paths, ["/first", "/second"]);
        assert!(store.list_history().expect("read history").len() == 2);
    }
}
