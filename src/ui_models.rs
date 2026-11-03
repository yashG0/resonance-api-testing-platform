//! Slint model adapters for persistence and application data.

use crate::{
    AppWindow, Assertion, AssertionItem, Collection, CollectionItem, CollectionVariable,
    EnvironmentItem, HistoryItem, KeyValueRow, RequestHistory, RequestItem, ResponseCookieItem,
    ResponseHeaderItem, SavedRequest, VariableItem,
};
use slint::{Model, ModelRc, VecModel};

pub(crate) fn request_tab_index(app: &AppWindow, key: i32) -> Option<i32> {
    let tabs = app.get_request_tabs();
    (0..tabs.row_count()).find_map(|index| {
        tabs.row_data(index)
            .filter(|tab| tab.key == key)
            .map(|_| index as i32)
    })
}

pub(crate) fn to_request_model(requests: Vec<SavedRequest>) -> ModelRc<RequestItem> {
    let items = requests
        .into_iter()
        .map(|request| RequestItem {
            id: request.id as i32,
            name: request.name.into(),
            method: request.method.into(),
            url: request.url.into(),
        })
        .collect::<Vec<_>>();
    ModelRc::new(VecModel::from(items))
}

fn contains_ascii_case_insensitive(value: &str, query: &str) -> bool {
    let query = query.as_bytes();
    !query.is_empty()
        && value
            .as_bytes()
            .windows(query.len())
            .any(|window| window.eq_ignore_ascii_case(query))
}

pub(crate) fn filter_request_model(
    requests: &ModelRc<RequestItem>,
    query: &str,
) -> ModelRc<RequestItem> {
    let query = query.trim().to_ascii_lowercase();
    let items = (0..requests.row_count())
        .filter_map(|index| requests.row_data(index))
        .filter(|request| {
            query.is_empty()
                || contains_ascii_case_insensitive(request.name.as_str(), &query)
                || contains_ascii_case_insensitive(request.method.as_str(), &query)
                || contains_ascii_case_insensitive(request.url.as_str(), &query)
        })
        .collect::<Vec<_>>();
    ModelRc::new(VecModel::from(items))
}

pub(crate) fn replace_saved_request_models(ui: &AppWindow, requests: Vec<SavedRequest>) {
    let all_requests = to_request_model(requests);
    let filtered_requests = filter_request_model(&all_requests, ui.get_request_search().as_str());
    ui.set_all_saved_requests(all_requests);
    ui.set_saved_requests(filtered_requests);
}

pub(crate) fn to_collection_model(collections: Vec<Collection>) -> ModelRc<CollectionItem> {
    let items = collections
        .into_iter()
        .map(|collection| CollectionItem {
            id: collection.id as i32,
            name: collection.name.into(),
        })
        .collect::<Vec<_>>();
    ModelRc::new(VecModel::from(items))
}

pub(crate) fn to_variable_model(variables: Vec<CollectionVariable>) -> ModelRc<VariableItem> {
    let items = variables
        .into_iter()
        .map(|variable| VariableItem {
            key: variable.key.into(),
            value: variable.value.into(),
            is_secret: variable.is_secret,
        })
        .collect::<Vec<_>>();
    ModelRc::new(VecModel::from(items))
}

pub(crate) fn to_key_value_model(value: &str, separator: char) -> ModelRc<KeyValueRow> {
    let rows = value
        .lines()
        .filter_map(|line| {
            let line = line.trim();
            if line.is_empty() {
                return None;
            }
            let (key, value) = line.split_once(separator).unwrap_or((line, ""));
            let key = key.trim();
            if key.is_empty() {
                return None;
            }
            Some(KeyValueRow {
                key: key.into(),
                value: value.trim().into(),
                enabled: true,
            })
        })
        .collect::<Vec<_>>();
    ModelRc::new(VecModel::from(rows))
}

pub(crate) fn serialize_key_value_rows(rows: ModelRc<KeyValueRow>, separator: &str) -> String {
    rows.iter()
        .filter(|row| row.enabled && !row.key.trim().is_empty())
        .map(|row| format!("{}{}{}", row.key.trim(), separator, row.value.trim()))
        .collect::<Vec<_>>()
        .join("\n")
}

pub(crate) fn to_response_header_model(headers: &str) -> ModelRc<ResponseHeaderItem> {
    let items = headers
        .lines()
        .filter_map(|line| {
            let (name, value) = line.split_once(':')?;
            Some(ResponseHeaderItem {
                name: name.trim().into(),
                value: value.trim().into(),
            })
        })
        .collect::<Vec<_>>();
    ModelRc::new(VecModel::from(items))
}

pub(crate) fn to_response_cookie_model(cookies: &str) -> ModelRc<ResponseCookieItem> {
    let items = cookies
        .split("\n\n")
        .filter_map(|cookie| {
            let mut lines = cookie.lines();
            let heading = lines.next()?.trim();
            let (_, name) = heading.strip_prefix("Cookie ")?.split_once(" · ")?;
            let value = lines
                .next()
                .unwrap_or_default()
                .trim()
                .strip_prefix("Value:")
                .unwrap_or_default()
                .trim();
            let attributes = lines
                .map(str::trim)
                .filter(|line| !line.is_empty())
                .collect::<Vec<_>>()
                .join("  ·  ");
            Some(ResponseCookieItem {
                name: name.trim().into(),
                value: value.into(),
                attributes: attributes.into(),
            })
        })
        .collect::<Vec<_>>();
    ModelRc::new(VecModel::from(items))
}

pub(crate) fn to_environment_model(
    items: Vec<(i32, String, CollectionVariable)>,
) -> ModelRc<EnvironmentItem> {
    let items = items
        .into_iter()
        .map(
            |(collection_id, collection_name, variable)| EnvironmentItem {
                collection_id,
                collection_name: collection_name.into(),
                key: variable.key.into(),
                value: variable.value.into(),
                is_secret: variable.is_secret,
            },
        )
        .collect::<Vec<_>>();
    ModelRc::new(VecModel::from(items))
}

pub(crate) fn parse_header_rows(source: &str) -> ModelRc<KeyValueRow> {
    let rows = source
        .lines()
        .filter_map(|line| {
            let (key, value) = line.split_once(':')?;
            let key = key.trim();
            if key.is_empty() {
                return None;
            }
            Some(KeyValueRow {
                key: key.into(),
                value: value.trim().into(),
                enabled: true,
            })
        })
        .collect::<Vec<_>>();
    ModelRc::new(VecModel::from(rows))
}

pub(crate) fn to_assertion_model(source: &str) -> ModelRc<AssertionItem> {
    let items = serde_json::from_str::<Vec<Assertion>>(source)
        .unwrap_or_default()
        .into_iter()
        .enumerate()
        .map(|(index, assertion)| {
            let (title, summary, kind, first, second) = match assertion {
                Assertion::HttpStatus { expected } => (
                    "HTTP status",
                    format!("equals {expected}"),
                    "HTTP status",
                    expected.to_string(),
                    String::new(),
                ),
                Assertion::HttpStatusClass { class } => (
                    "HTTP status class",
                    format!("is {class}xx"),
                    "HTTP status class",
                    format!("{class}xx"),
                    String::new(),
                ),
                Assertion::HttpStatusAnyOf { expected } => (
                    "HTTP status any of",
                    expected
                        .iter()
                        .map(u16::to_string)
                        .collect::<Vec<_>>()
                        .join(", "),
                    "HTTP status any of",
                    expected
                        .iter()
                        .map(u16::to_string)
                        .collect::<Vec<_>>()
                        .join(", "),
                    String::new(),
                ),
                Assertion::LatencyUnder { max_ms } => (
                    "Latency",
                    format!("under {max_ms} ms"),
                    "Latency under (ms)",
                    max_ms.to_string(),
                    String::new(),
                ),
                Assertion::BodyContains { text } => (
                    "Body contains",
                    text.clone(),
                    "Body contains",
                    text,
                    String::new(),
                ),
                Assertion::JsonPointer { pointer, expected } => (
                    "JSON Pointer",
                    format!("{pointer} = {expected}"),
                    "JSON Pointer equals",
                    pointer,
                    expected.to_string(),
                ),
                Assertion::HeaderExists { name } => (
                    "Header exists",
                    name.clone(),
                    "Header exists",
                    name,
                    String::new(),
                ),
                Assertion::HeaderEquals { name, expected } => (
                    "Header equals",
                    format!("{name} = {expected}"),
                    "Header equals",
                    name,
                    expected,
                ),
                Assertion::HeaderContains { name, text } => (
                    "Header contains",
                    format!("{name} contains {text}"),
                    "Header contains",
                    name,
                    text,
                ),
                Assertion::CaptureJsonPointer { pointer, variable } => (
                    "Capture JSON Pointer",
                    format!("{pointer} → {{{variable}}}"),
                    "Capture JSON Pointer",
                    pointer,
                    variable,
                ),
            };
            AssertionItem {
                index: index as i32,
                title: title.into(),
                summary: summary.into(),
                kind: kind.into(),
                first: first.into(),
                second: second.into(),
            }
        })
        .collect::<Vec<_>>();
    ModelRc::new(VecModel::from(items))
}

pub(crate) fn to_history_model(history: Vec<RequestHistory>) -> ModelRc<HistoryItem> {
    let items = history
        .into_iter()
        .map(|item| HistoryItem {
            request_id: item.request_id as i32,
            request_name: item.request_name.into(),
            collection_name: item.collection_name.into(),
            method: item.method.into(),
            url: item.url.into(),
            status: item.status.into(),
            duration: format!("{} ms", item.elapsed_ms).into(),
            date: item.created_at.into(),
        })
        .collect::<Vec<_>>();
    ModelRc::new(VecModel::from(items))
}

pub(crate) fn filter_history_items(
    history: &ModelRc<HistoryItem>,
    filter: &str,
) -> ModelRc<HistoryItem> {
    let items = (0..history.row_count())
        .filter_map(|index| history.row_data(index))
        .filter(|item| match filter {
            "Success" => item.status.starts_with('2') || item.status.starts_with('3'),
            "Failed" => !item.status.starts_with('2') && !item.status.starts_with('3'),
            _ => true,
        })
        .collect::<Vec<_>>();
    ModelRc::new(VecModel::from(items))
}

pub(crate) fn paginate_history_items(
    history: &ModelRc<HistoryItem>,
    page: i32,
    page_size: i32,
) -> ModelRc<HistoryItem> {
    let page_size = page_size.max(1) as usize;
    let start = page.saturating_sub(1).max(0) as usize * page_size;
    let items = (start..(start + page_size).min(history.row_count()))
        .filter_map(|index| history.row_data(index))
        .collect::<Vec<_>>();
    ModelRc::new(VecModel::from(items))
}
