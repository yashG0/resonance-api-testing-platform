mod assertions;
mod cli;
mod collection_runner;
mod credentials;
mod curl;
mod http;
mod platform;
mod response_ui;
mod storage;
mod ui_models;

use std::{
    collections::{HashMap, HashSet},
    future::Future,
    hash::{Hash, Hasher},
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};

use crate::assertions::{
    Assertion, append_assertion_json, remove_assertion_json, update_assertion_json,
};
use crate::http::HttpClient;
use crate::http::client::RequestSpec;
use crate::storage::{
    Collection, CollectionVariable, HistoryRecord, NewRequest, RequestHistory, RequestStore,
    SavedRequest,
};
use platform::{
    copy_text_to_clipboard, detect_system_accent_color, detect_system_dark_mode,
    use_gnome_custom_frame,
};
use slint::{ComponentHandle, Model, ModelRc, VecModel};

slint::include_modules!();

pub(crate) use response_ui::{
    auto_pair_request_body, build_json_tree, format_response_body, tokenize_response,
};

const MAX_STRUCTURED_VIEW_BYTES: usize = 256 * 1024;

fn parse_request_timeout_ms(value: &str) -> Result<i64, String> {
    let timeout_ms = value.trim().parse::<i64>().map_err(|_| {
        "Request timeout must be a number between 100 and 600000 milliseconds.".to_owned()
    })?;
    if !(crate::storage::MIN_REQUEST_TIMEOUT_MS..=crate::storage::MAX_REQUEST_TIMEOUT_MS)
        .contains(&timeout_ms)
    {
        return Err("Request timeout must be between 100 and 600000 milliseconds.".to_owned());
    }
    Ok(timeout_ms)
}

#[cfg(test)]
mod request_timeout_tests {
    use super::parse_request_timeout_ms;

    #[test]
    fn accepts_only_supported_timeout_values() {
        assert_eq!(parse_request_timeout_ms("30000"), Ok(30_000));
        assert_eq!(parse_request_timeout_ms(" 100 "), Ok(100));
        assert!(parse_request_timeout_ms("99").is_err());
        assert!(parse_request_timeout_ms("600001").is_err());
        assert!(parse_request_timeout_ms("later").is_err());
    }
}

trait JoinHandleThen<T>: Sized {
    fn then<F, Fut>(self, continuation: F)
    where
        T: Send + 'static,
        F: FnOnce(Result<T, tokio::task::JoinError>) -> Fut + Send + 'static,
        Fut: Future<Output = ()> + Send + 'static;
}

impl<T: Send + 'static> JoinHandleThen<T> for tokio::task::JoinHandle<T> {
    fn then<F, Fut>(self, continuation: F)
    where
        F: FnOnce(Result<T, tokio::task::JoinError>) -> Fut + Send + 'static,
        Fut: Future<Output = ()> + Send + 'static,
    {
        tokio::spawn(async move {
            continuation(self.await).await;
        });
    }
}

pub(crate) use ui_models::*;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let arguments = std::env::args().skip(1).collect::<Vec<_>>();
    if let Some(command) = arguments.first() {
        if matches!(command.as_str(), "--help" | "-h") {
            println!("Cosmic API\n\n{}", cli::help());
            return Ok(());
        }
        if command == "run" {
            match cli::run(&arguments[1..]).await {
                Ok(true) => return Ok(()),
                Ok(false) => std::process::exit(1),
                Err(error) => {
                    eprintln!("{error}");
                    std::process::exit(2);
                }
            }
        }
        eprintln!("Unknown command '{command}'.\n\n{}", cli::help());
        std::process::exit(2);
    }

    let store = Arc::new(RequestStore::open_default().map_err(|error| {
        slint::PlatformError::from(format!(
            "Could not initialize Cosmic API local storage: {error}"
        ))
    })?);
    let client = Arc::new(HttpClient::new().map_err(|error| {
        slint::PlatformError::from(format!("Could not initialize the HTTP client: {error}"))
    })?);
    let app = AppWindow::new()?;
    app.set_custom_frame(use_gnome_custom_frame());
    app.set_shortcut_modifier(if cfg!(target_os = "macos") {
        "⌘".into()
    } else {
        "Ctrl".into()
    });
    app.set_shortcut_alt_modifier(if cfg!(target_os = "macos") {
        "⌥".into()
    } else {
        "Alt".into()
    });
    slint::set_xdg_app_id("io.github.yashG0.resonance-api-testing-platform")?;
    app.set_system_dark_mode(detect_system_dark_mode(app.get_system_dark_mode()));
    app.set_system_accent_color(detect_system_accent_color(app.get_system_accent_color()));
    let app_weak = app.as_weak();
    let tree_expansion_state =
        Arc::new(Mutex::new(HashMap::<String, (u64, HashSet<String>)>::new()));

    app.on_refresh_system_theme({
        let app_weak = app_weak.clone();
        move || {
            if let Some(ui) = app_weak.upgrade() {
                let fallback = ui.get_system_dark_mode();
                ui.set_system_dark_mode(detect_system_dark_mode(fallback));
                let accent_fallback = ui.get_system_accent_color();
                ui.set_system_accent_color(detect_system_accent_color(accent_fallback));
            }
        }
    });

    app.on_serialize_param_rows(|rows| serialize_key_value_rows(rows, "=").into());
    app.on_serialize_header_rows(|rows| serialize_key_value_rows(rows, ": ").into());
    app.on_append_assertion(|current, kind, first, second| {
        append_assertion_json(
            current.as_str(),
            kind.as_str(),
            first.as_str(),
            second.as_str(),
        )
        .unwrap_or_default()
        .into()
    });
    app.on_remove_assertion(|current, index| {
        remove_assertion_json(current.as_str(), index.max(0) as usize)
            .unwrap_or_default()
            .into()
    });
    app.on_update_assertion(|current, index, kind, first, second| {
        update_assertion_json(
            current.as_str(),
            index.max(0) as usize,
            kind.as_str(),
            first.as_str(),
            second.as_str(),
        )
        .unwrap_or_default()
        .into()
    });
    app.on_parse_assertions(|source| to_assertion_model(source.as_str()));
    app.on_parse_curl(|command| match crate::curl::parse_curl(command.as_str()) {
        Ok(request) => CurlDraft {
            error: "".into(),
            method: request.method.into(),
            url: request.url.into(),
            headers: request.headers.into(),
            body: request.body.into(),
            body_format: request.body_format.into(),
            auth_type: request.auth_type.into(),
            auth_value: request.auth_value.into(),
            auth_secret: request.auth_secret.into(),
        },
        Err(error) => CurlDraft {
            error: error.into(),
            method: "".into(),
            url: "".into(),
            headers: "".into(),
            body: "".into(),
            body_format: "".into(),
            auth_type: "".into(),
            auth_value: "".into(),
            auth_secret: "".into(),
        },
    });
    app.on_build_curl(
        |method, url, body, headers, params, body_format, auth_type, auth_value, auth_secret| {
            crate::curl::build_curl(&RequestSpec {
                method: method.as_str(),
                url: url.as_str(),
                body: body.as_str(),
                headers: headers.as_str(),
                query_params: params.as_str(),
                body_format: body_format.as_str(),
                auth_type: auth_type.as_str(),
                auth_value: auth_value.as_str(),
                auth_secret: auth_secret.as_str(),
                timeout_ms: crate::storage::DEFAULT_REQUEST_TIMEOUT_MS as u64,
                follow_redirects: true,
            })
            .into()
        },
    );
    app.on_parse_header_rows(|source| parse_header_rows(source.as_str()));

    app.on_auto_pair_request_body(|text, cursor, anchor, key, format| {
        auto_pair_request_body(text.as_str(), cursor, anchor, key.as_str(), format.as_str())
    });

    app.on_tokenize_response(|source, format, dark, max_columns| {
        tokenize_response(source.as_str(), format.as_str(), dark, max_columns)
    });

    app.on_build_json_tree({
        let tree_expansion_state = Arc::clone(&tree_expansion_state);
        move |scope, source, dark| {
            let source_text = source.to_string();
            let scope = scope.to_string();
            let mut hasher = std::collections::hash_map::DefaultHasher::new();
            source_text.hash(&mut hasher);
            let source_hash = hasher.finish();
            let expanded_paths = if let Ok(mut state) = tree_expansion_state.lock() {
                let entry = state
                    .entry(scope)
                    .or_insert_with(|| (source_hash, HashSet::new()));
                if entry.0 != source_hash {
                    entry.0 = source_hash;
                    entry.1.clear();
                }
                entry
                    .1
                    .iter()
                    .map(slint::SharedString::from)
                    .collect::<Vec<_>>()
            } else {
                Vec::new()
            };
            build_json_tree(source_text.as_str(), &expanded_paths, dark)
        }
    });

    app.on_parse_response_headers(|headers| to_response_header_model(headers.as_str()));

    app.on_toggle_json_tree_node({
        let tree_expansion_state = Arc::clone(&tree_expansion_state);
        move |scope, path| {
            if let Ok(mut state) = tree_expansion_state.lock() {
                let expanded = state
                    .entry(scope.to_string())
                    .or_insert_with(|| (0, HashSet::new()));
                let path = path.to_string();
                if !expanded.1.remove(&path) {
                    expanded.1.insert(path);
                }
            }
        }
    });

    // Keep one clipboard owner alive for the application lifetime. This is
    // required by X11/Wayland selection ownership and also avoids concurrent
    // platform clipboard access from the response controls.
    let clipboard = Arc::new(Mutex::new(None::<arboard::Clipboard>));
    app.on_copy_text({
        let clipboard = Arc::clone(&clipboard);
        move |text| {
            if let Err(error) = copy_text_to_clipboard(&clipboard, text.as_str()) {
                eprintln!("Cosmic API: could not copy response: {error}");
            }
        }
    });

    app.on_suggest_request_name({
        let app_weak = app_weak.clone();
        let store = Arc::clone(&store);
        move |collection_id| {
            let mut names = HashSet::new();
            if let Ok(requests) = store.list_for_collection(collection_id as i64) {
                names.extend(requests.into_iter().map(|request| request.name));
            }
            if let Some(ui) = app_weak.upgrade() {
                let tabs = ui.get_request_tabs();
                for index in 0..tabs.row_count() {
                    if let Some(tab) = tabs.row_data(index)
                        && tab.collection_id == collection_id
                    {
                        names.insert(tab.name.to_string());
                    }
                }
            }
            let mut suffix = 1_u64;
            loop {
                let candidate = format!("New Request {suffix}");
                if !names.contains(&candidate) {
                    return candidate.into();
                }
                suffix += 1;
            }
        }
    });

    // Load the selected collection's variables off the UI thread, then send the
    // fully resolved request without blocking Slint's event loop.
    app.on_send_request({
        let app_weak = app_weak.clone();
        let client = Arc::clone(&client);
        let store = Arc::clone(&store);

        move |draft| {
            let app_handle = app_weak.clone();
            let client = Arc::clone(&client);
            let store = Arc::clone(&store);
            let method = draft.method.to_string();
            let url = draft.url.to_string();
            let body = draft.body.to_string();
            let headers = draft.headers.to_string();
            let params = draft.params.to_string();
            let body_format = draft.body_format.to_string();
            let auth_type = draft.auth_type.to_string();
            let auth_value = draft.auth_value.to_string();
            let auth_secret = draft.auth_secret.to_string();
            let request_id = draft.request_id;
            let collection_id = draft.collection_id as i64;
            let tab_key = draft.tab_key;
            let follow_redirects = draft.follow_redirects;
            let timeout_ms = match parse_request_timeout_ms(draft.timeout_ms.as_str()) {
                Ok(timeout_ms) => timeout_ms,
                Err(message) => {
                    if let Some(ui) = app_weak.upgrade() {
                        ui.set_response_status(message.into());
                        ui.set_response_state("error".into());
                    }
                    return;
                }
            };
            let assertions =
                match serde_json::from_str::<Vec<Assertion>>(draft.assertions_json.as_str()) {
                    Ok(assertions) => assertions,
                    Err(error) => {
                        let message = format!("Invalid assertions JSON: {error}");
                        let validation_ui = app_weak.clone();
                        let _ = slint::invoke_from_event_loop(move || {
                            if let Some(ui) = validation_ui.upgrade() {
                                ui.set_response_status(message.into());
                                ui.set_response_state("error".into());
                            }
                        });
                        return;
                    }
                };
            let name = draft.name.to_string();
            tokio::spawn(async move {
                // Save first so each request is available in its collection as soon
                // as the response arrives. Existing requests are updated in place.
                let mut request = NewRequest {
                    collection_id,
                    name: name.clone(),
                    method: method.clone(),
                    url: url.clone(),
                    headers: headers.clone(),
                    query_params: params.clone(),
                    body: body.clone(),
                    body_format: body_format.clone(),
                    auth_type: auth_type.clone(),
                    auth_value: auth_value.clone(),
                    auth_secret: auth_secret.clone(),
                    assertions: assertions.clone(),
                    timeout_ms,
                    follow_redirects,
                };
                let save_store = Arc::clone(&store);
                let save_result = tokio::task::spawn_blocking(move || {
                    // New drafts start with the same friendly title. Give each
                    // one a collection-local name before inserting it.
                    if request_id <= 0 {
                        let existing = save_store.list_for_collection(collection_id)?;
                        if request.name.trim().starts_with("New Request ") {
                            let mut suffix =
                                request.name.trim()[12..].parse::<u64>().unwrap_or(1).max(1);
                            while existing.iter().any(|item| item.name == request.name) {
                                suffix += 1;
                                request.name = format!("New Request {suffix}");
                            }
                        }
                    }
                    let saved_name = request.name.clone();
                    let saved_id =
                        if request_id > 0 && save_store.update(request_id as i64, &request)? {
                            request_id as i64
                        } else {
                            save_store.create(&request)?.id
                        };
                    let requests = save_store.list_for_collection(collection_id)?;
                    let variables = save_store.variables_map(collection_id)?;
                    Ok::<_, crate::storage::StoreError>((saved_id, saved_name, requests, variables))
                })
                .await;

                let (saved_id, saved_name, requests, variables) = match save_result {
                    Ok(Ok(saved)) => saved,
                    Ok(Err(error)) => {
                        let message = format!("Auto-save failed; request was not sent: {error}");
                        let _ = slint::invoke_from_event_loop(move || {
                            if let Some(ui) = app_handle.upgrade() {
                                let Some(tab_index) = request_tab_index(&ui, tab_key) else {
                                    return;
                                };
                                ui.invoke_set_request_tab_response(
                                    tab_index,
                                    tab_key,
                                    "Auto-save failed".into(),
                                    message.into(),
                                    "".into(),
                                    "---".into(),
                                    "error".into(),
                                    "Text".into(),
                                    "".into(),
                                    "".into(),
                                    false,
                                    ModelRc::default(),
                                    ModelRc::default(),
                                );
                            }
                        });
                        return;
                    }
                    Err(error) => {
                        let message = format!("Auto-save failed; request was not sent: {error}");
                        let _ = slint::invoke_from_event_loop(move || {
                            if let Some(ui) = app_handle.upgrade() {
                                let Some(tab_index) = request_tab_index(&ui, tab_key) else {
                                    return;
                                };
                                ui.invoke_set_request_tab_response(
                                    tab_index,
                                    tab_key,
                                    "Auto-save failed".into(),
                                    message.into(),
                                    "".into(),
                                    "---".into(),
                                    "error".into(),
                                    "Text".into(),
                                    "".into(),
                                    "".into(),
                                    false,
                                    ModelRc::default(),
                                    ModelRc::default(),
                                );
                            }
                        });
                        return;
                    }
                };

                let list_handle = app_handle.clone();
                let persisted_name = saved_name.clone();
                let _ = slint::invoke_from_event_loop(move || {
                    if let Some(ui) = list_handle.upgrade() {
                        if ui.get_expanded_collection_id() as i64 == collection_id {
                            replace_saved_request_models(&ui, requests);
                        }
                        if let Some(tab_index) = request_tab_index(&ui, tab_key) {
                            ui.invoke_set_request_tab_identity(
                                tab_index,
                                tab_key,
                                saved_id as i32,
                                persisted_name.into(),
                            );
                        }
                    }
                });

                let result = client
                    .send(
                        &RequestSpec {
                            method: &method,
                            url: &url,
                            body: &body,
                            headers: &headers,
                            query_params: &params,
                            body_format: &body_format,
                            auth_type: &auth_type,
                            auth_value: &auth_value,
                            auth_secret: &auth_secret,
                            timeout_ms: timeout_ms as u64,
                            follow_redirects,
                        },
                        &variables,
                    )
                    .await;
                let history_store = Arc::clone(&store);
                let history_status = result.status.clone();
                let history_method = method.clone();
                let history_url = url.clone();
                let history_name = saved_name.clone();
                let elapsed_ms = (result.elapsed.as_secs_f64() * 1000.0) as i64;
                let history_handle = app_handle.clone();
                tokio::task::spawn_blocking(move || {
                    history_store.record_history(&HistoryRecord {
                        request_id: saved_id,
                        collection_id,
                        request_name: history_name,
                        method: history_method,
                        url: history_url,
                        status: history_status,
                        elapsed_ms,
                    })?;
                    history_store.list_history()
                })
                .then(move |history_result| async move {
                    if let Ok(Ok(history)) = history_result {
                        let _ = slint::invoke_from_event_loop(move || {
                            if let Some(ui) = history_handle.upgrade()
                                && ui.get_active_view() == "History"
                            {
                                ui.set_history_items(to_history_model(history));
                            }
                        });
                    }
                });
                let response_state = result.state.as_label().to_owned();
                let elapsed = format!("{:.2} ms", result.elapsed.as_secs_f64() * 1000.0);
                let response_status = result.status.clone();
                let response_body = result.body;
                let response_format = result.format.clone();
                let response_headers = result.headers.clone();
                let response_cookies = result.cookies.clone();
                let response_large = response_body.len() > MAX_STRUCTURED_VIEW_BYTES;
                let response_body_formatted = if response_large
                    || !matches!(response_format.as_str(), "JSON" | "HTML" | "XML")
                {
                    String::new()
                } else {
                    let formatted_body_source = response_body.clone();
                    let formatted_body_format = response_format.clone();
                    tokio::task::spawn_blocking(move || {
                        format_response_body(&formatted_body_source, &formatted_body_format)
                    })
                    .await
                    .unwrap_or_default()
                };

                let _ = slint::invoke_from_event_loop(move || {
                    if let Some(ui) = app_handle.upgrade() {
                        let Some(tab_index) = request_tab_index(&ui, tab_key) else {
                            return;
                        };
                        let header_items = to_response_header_model(&response_headers);
                        let cookie_items = to_response_cookie_model(&response_cookies);
                        ui.invoke_set_request_tab_response(
                            tab_index,
                            tab_key,
                            response_status.into(),
                            response_body.into(),
                            response_body_formatted.into(),
                            elapsed.into(),
                            response_state.into(),
                            response_format.into(),
                            response_headers.into(),
                            response_cookies.into(),
                            response_large,
                            header_items,
                            cookie_items,
                        );
                    }
                });
            });
        }
    });

    crate::collection_runner::register(
        &app,
        app_weak.clone(),
        Arc::clone(&client),
        Arc::clone(&store),
    );

    app.on_save_request({
        let app_weak = app_weak.clone();
        let store = Arc::clone(&store);

        move |draft| {
            let app_handle = app_weak.clone();
            let store = Arc::clone(&store);
            let id = draft.request_id;
            let collection_id = draft.collection_id;
            let tab_key = draft.tab_key;
            let assertions =
                match serde_json::from_str::<Vec<Assertion>>(draft.assertions_json.as_str()) {
                    Ok(assertions) => assertions,
                    Err(error) => {
                        if let Some(ui) = app_weak.upgrade() {
                            ui.set_response_status(
                                format!("Invalid assertions JSON: {error}").into(),
                            );
                        }
                        return;
                    }
                };
            let mut request = NewRequest {
                collection_id: collection_id as i64,
                name: draft.name.to_string(),
                method: draft.method.to_string(),
                url: draft.url.to_string(),
                headers: draft.headers.to_string(),
                query_params: draft.params.to_string(),
                body: draft.body.to_string(),
                body_format: draft.body_format.to_string(),
                auth_type: draft.auth_type.to_string(),
                auth_value: draft.auth_value.to_string(),
                auth_secret: draft.auth_secret.to_string(),
                assertions,
                timeout_ms: match parse_request_timeout_ms(draft.timeout_ms.as_str()) {
                    Ok(timeout_ms) => timeout_ms,
                    Err(message) => {
                        if let Some(ui) = app_weak.upgrade() {
                            ui.set_response_status(message.into());
                            ui.set_response_state("error".into());
                        }
                        return;
                    }
                },
                follow_redirects: draft.follow_redirects,
            };
            tokio::task::spawn_blocking(move || {
                if id <= 0
                    && request.name.trim().starts_with("New Request ")
                    && let Ok(existing) = store.list_for_collection(collection_id as i64)
                {
                    let mut suffix = request.name.trim()[12..].parse::<u64>().unwrap_or(1).max(1);
                    while existing.iter().any(|item| item.name == request.name) {
                        suffix += 1;
                        request.name = format!("New Request {suffix}");
                    }
                }
                let saved_name = request.name.clone();
                let result = if id > 0 {
                    store
                        .update(id as i64, &request)
                        .map(|updated| if updated { id as i64 } else { 0 })
                } else {
                    store.create(&request).map(|saved| saved.id)
                };
                let list_result = store.list_for_collection(collection_id as i64);

                let _ = slint::invoke_from_event_loop(move || {
                    if let Some(ui) = app_handle.upgrade() {
                        match result {
                            Ok(saved_id) if saved_id > 0 => {
                                if ui.get_active_request_tab_key() == tab_key {
                                    ui.invoke_store_active_request_tab();
                                }
                                if let Some(tab_index) = request_tab_index(&ui, tab_key) {
                                    ui.invoke_set_request_tab_identity(
                                        tab_index,
                                        tab_key,
                                        saved_id as i32,
                                        saved_name.clone().into(),
                                    );
                                    if ui.get_active_request_tab_key() == tab_key {
                                        ui.set_response_status(
                                            format!("Saved: {saved_name}").into(),
                                        );
                                    }
                                }
                            }
                            Ok(_) => ui.set_response_status(
                                "Save failed: request no longer exists".into(),
                            ),
                            Err(error) => {
                                ui.set_response_status(format!("Save failed: {error}").into())
                            }
                        }
                        if ui.get_expanded_collection_id() as i64 == collection_id as i64
                            && let Ok(requests) = list_result
                        {
                            replace_saved_request_models(&ui, requests);
                        }
                    }
                });
            });
        }
    });

    app.on_load_collections({
        let app_weak = app_weak.clone();
        let store = Arc::clone(&store);
        move || {
            let app_handle = app_weak.clone();
            let store = Arc::clone(&store);
            tokio::task::spawn_blocking(move || store.list_collections()).then(
                move |result| async move {
                    let _ = slint::invoke_from_event_loop(move || {
                        if let Some(ui) = app_handle.upgrade() {
                            match result {
                                Ok(Ok(collections)) => {
                                    let selected = collections
                                        .iter()
                                        .find(|item| {
                                            item.id == ui.get_selected_collection_id() as i64
                                        })
                                        .or_else(|| {
                                            collections.iter().find(|item| {
                                                item.name.eq_ignore_ascii_case("Public API")
                                            })
                                        })
                                        .or_else(|| collections.first())
                                        .cloned();
                                    ui.set_collections(to_collection_model(collections));
                                    if let Some(selected) = selected {
                                        let id = selected.id as i32;
                                        ui.set_selected_collection_id(id);
                                        ui.set_expanded_collection_id(id);
                                        ui.set_selected_collection_name(
                                            selected.name.clone().into(),
                                        );
                                        ui.invoke_load_requests(id);
                                        ui.invoke_load_collection_variables(id);
                                    }
                                }
                                Ok(Err(error)) => ui.set_variables_message(
                                    format!("Could not load collections: {error}").into(),
                                ),
                                Err(error) => ui.set_variables_message(
                                    format!("Could not load collections: {error}").into(),
                                ),
                            }
                        }
                    });
                },
            );
        }
    });

    app.on_load_requests({
        let app_weak = app_weak.clone();
        let store = Arc::clone(&store);
        move |collection_id| {
            let app_handle = app_weak.clone();
            let store = Arc::clone(&store);
            tokio::task::spawn_blocking(move || store.list_for_collection(collection_id as i64))
                .then(move |result| async move {
                    let _ = slint::invoke_from_event_loop(move || {
                        if let Some(ui) = app_handle.upgrade() {
                            match result {
                                Ok(Ok(requests)) => {
                                    if ui.get_expanded_collection_id() == collection_id {
                                        replace_saved_request_models(&ui, requests);
                                    }
                                }
                                Ok(Err(error))
                                    if ui.get_expanded_collection_id() == collection_id =>
                                {
                                    ui.set_response_status(format!("Load failed: {error}").into())
                                }
                                Err(error) if ui.get_expanded_collection_id() == collection_id => {
                                    ui.set_response_status(format!("Load failed: {error}").into())
                                }
                                _ => {}
                            }
                        }
                    });
                });
        }
    });

    app.on_search_requests({
        let app_weak = app_weak.clone();
        move |query| {
            if let Some(ui) = app_weak.upgrade() {
                ui.set_saved_requests(filter_request_model(
                    &ui.get_all_saved_requests(),
                    query.as_str(),
                ));
            }
        }
    });

    app.on_filter_history({
        let app_weak = app_weak.clone();
        move |filter| {
            app_weak
                .upgrade()
                .map(|ui| filter_history_items(&ui.get_history_items(), filter.as_str()))
                .unwrap_or_else(|| ModelRc::new(VecModel::from(Vec::<HistoryItem>::new())))
        }
    });

    app.on_paginate_history({
        let app_weak = app_weak.clone();
        move |page, page_size| {
            app_weak
                .upgrade()
                .map(|ui| paginate_history_items(&ui.get_history_visible_items(), page, page_size))
                .unwrap_or_else(|| ModelRc::new(VecModel::from(Vec::<HistoryItem>::new())))
        }
    });

    app.on_load_history({
        let app_weak = app_weak.clone();
        let store = Arc::clone(&store);
        move |sort_order| {
            let load_started = tokio::time::Instant::now();
            let generation = app_weak
                .upgrade()
                .map(|ui| ui.get_history_load_generation())
                .unwrap_or_default();
            let app_handle = app_weak.clone();
            let store = Arc::clone(&store);
            tokio::task::spawn_blocking(move || {
                let mut history = store.list_history()?;
                if sort_order == "Oldest first" {
                    history.reverse();
                }
                Ok::<_, crate::storage::StoreError>(history)
            })
            .then(move |result| async move {
                // Keep the same visible spinner as the response view long enough
                // for its rotation to register on fast local history reads.
                let spinner_minimum = Duration::from_millis(350);
                if let Some(remaining) = spinner_minimum.checked_sub(load_started.elapsed()) {
                    tokio::time::sleep(remaining).await;
                }
                let _ = slint::invoke_from_event_loop(move || {
                    if let Some(ui) = app_handle.upgrade() {
                        if ui.get_history_load_generation() != generation {
                            return;
                        }
                        match result {
                            Ok(Ok(history)) => {
                                let history = to_history_model(history);
                                let visible = filter_history_items(
                                    &history,
                                    ui.get_history_filter().as_str(),
                                );
                                let page = paginate_history_items(
                                    &visible,
                                    ui.get_history_page(),
                                    ui.get_history_page_size(),
                                );
                                ui.set_history_items(history);
                                ui.set_history_visible_items(visible);
                                ui.set_history_page_items(page);
                                ui.set_history_loading(false);
                                ui.set_history_loader_visible(false);
                                ui.set_history_load_error("".into());
                            }
                            Ok(Err(error)) => {
                                ui.set_history_loading(false);
                                ui.set_history_loader_visible(false);
                                ui.set_history_load_error(
                                    format!("Could not load request history: {error}").into(),
                                );
                            }
                            Err(error) => {
                                ui.set_history_loading(false);
                                ui.set_history_loader_visible(false);
                                ui.set_history_load_error(
                                    format!("History loading task failed: {error}").into(),
                                );
                            }
                        }
                    }
                });
            });
        }
    });

    let environment_load_generation = Arc::new(AtomicU64::new(0));
    app.on_load_environments({
        let app_weak = app_weak.clone();
        let store = Arc::clone(&store);
        let environment_load_generation = Arc::clone(&environment_load_generation);
        move || {
            let generation = environment_load_generation
                .fetch_add(1, Ordering::Relaxed)
                .wrapping_add(1);
            let app_handle = app_weak.clone();
            let environment_load_generation = Arc::clone(&environment_load_generation);
            let store = Arc::clone(&store);
            tokio::task::spawn_blocking(move || {
                store.list_all_variables().map(|items| {
                    items
                        .into_iter()
                        .map(|(collection_id, name, variable)| {
                            (collection_id as i32, name, variable)
                        })
                        .collect::<Vec<_>>()
                })
            })
            .then(move |result| async move {
                let _ = slint::invoke_from_event_loop(move || {
                    if let Some(ui) = app_handle.upgrade() {
                        if environment_load_generation.load(Ordering::Relaxed) != generation {
                            return;
                        }
                        match result {
                            Ok(Ok(items)) => ui.set_environment_items(to_environment_model(items)),
                            Ok(Err(error)) => ui.set_variables_message(
                                format!("Could not load environments: {error}").into(),
                            ),
                            Err(error) => ui.set_variables_message(
                                format!("Could not load environments: {error}").into(),
                            ),
                        }
                    }
                });
            });
        }
    });

    app.on_load_collection_variables({
        let app_weak = app_weak.clone();
        let store = Arc::clone(&store);
        move |collection_id| {
            let app_handle = app_weak.clone();
            let store = Arc::clone(&store);
            tokio::task::spawn_blocking(move || store.list_variables(collection_id as i64)).then(
                move |result| async move {
                    let _ = slint::invoke_from_event_loop(move || {
                        if let Some(ui) = app_handle.upgrade() {
                            if ui.get_selected_collection_id() != collection_id {
                                return;
                            }
                            match result {
                                Ok(Ok(variables)) => {
                                    ui.set_collection_variables(to_variable_model(variables));
                                    ui.set_variables_message("".into());
                                }
                                Ok(Err(error)) => ui.set_variables_message(
                                    format!("Could not load variables: {error}").into(),
                                ),
                                Err(error) => ui.set_variables_message(
                                    format!("Could not load variables: {error}").into(),
                                ),
                            }
                        }
                    });
                },
            );
        }
    });

    app.on_create_collection({
        let app_weak = app_weak.clone();
        let store = Arc::clone(&store);
        move |name| {
            let app_handle = app_weak.clone();
            let store = Arc::clone(&store);
            let name = name.to_string();
            tokio::task::spawn_blocking(move || {
                let created = store.create_collection(&name)?;
                let collections = store.list_collections()?;
                Ok::<_, crate::storage::StoreError>((created, collections))
            })
            .then(move |result| async move {
                let _ = slint::invoke_from_event_loop(move || {
                    if let Some(ui) = app_handle.upgrade() {
                        match result {
                            Ok(Ok((created, collections))) => {
                                let id = created.id as i32;
                                ui.invoke_store_active_request_tab();
                                ui.set_collections(to_collection_model(collections));
                                ui.set_selected_collection_id(id);
                                ui.set_expanded_collection_id(id);
                                ui.set_selected_collection_name(created.name.into());
                                ui.set_active_view("Request".into());
                                ui.set_active_request_id(-1);
                                ui.set_active_request("".into());
                                ui.set_request_name_draft("".into());
                                ui.set_editing_request_name(false);
                                ui.set_is_loading(false);
                                ui.set_active_method("GET".into());
                                ui.set_request_url("".into());
                                ui.set_request_headers("Accept: application/json".into());
                                ui.set_request_header_rows(to_key_value_model(
                                    "Accept: application/json",
                                    ':',
                                ));
                                ui.set_request_params("".into());
                                ui.set_request_param_rows(to_key_value_model("", '='));
                                ui.set_request_body("".into());
                                ui.set_body_format("JSON".into());
                                ui.set_request_timeout_ms("30000".into());
                                ui.set_follow_redirects(true);
                                ui.set_auth_type("None".into());
                                ui.set_auth_value("".into());
                                ui.set_auth_secret("".into());
                                ui.set_active_tab("Body".into());
                                ui.set_response_status("No response yet".into());
                                ui.set_response_body(
                                    "Send a request to inspect its response.".into(),
                                );
                                ui.set_response_time("---".into());
                                ui.set_response_state("idle".into());
                                ui.set_response_format("Text".into());
                                ui.invoke_register_active_request_tab();
                                ui.invoke_load_requests(id);
                                ui.invoke_load_collection_variables(id);
                                ui.set_variables_message("Collection created.".into());
                                ui.set_new_collection_name("".into());
                                ui.set_show_new_collection(false);
                                ui.invoke_focus_request_url();
                            }
                            Ok(Err(error)) => ui.set_variables_message(
                                format!("Could not create collection: {error}").into(),
                            ),
                            Err(error) => ui.set_variables_message(
                                format!("Could not create collection: {error}").into(),
                            ),
                        }
                    }
                });
            });
        }
    });

    app.on_rename_collection({
        let app_weak = app_weak.clone();
        let store = Arc::clone(&store);
        move |collection_id, name| {
            let app_handle = app_weak.clone();
            let store = Arc::clone(&store);
            let name = name.to_string();
            tokio::task::spawn_blocking(move || {
                let renamed = store.rename_collection(collection_id as i64, &name)?;
                let collections = store.list_collections()?;
                Ok::<_, crate::storage::StoreError>((renamed, collections))
            })
            .then(move |result| async move {
                let _ = slint::invoke_from_event_loop(move || {
                    if let Some(ui) = app_handle.upgrade() {
                        match result {
                            Ok(Ok((renamed, collections))) => {
                                ui.set_collections(to_collection_model(collections));
                                if ui.get_selected_collection_id() == renamed.id as i32 {
                                    ui.set_selected_collection_name(renamed.name.into());
                                }
                                ui.set_editing_collection_id(-1);
                                ui.set_variables_message("Collection renamed.".into());
                                if ui.get_active_view() == "Environments" {
                                    ui.invoke_load_environments();
                                }
                            }
                            Ok(Err(error)) => ui.set_variables_message(
                                format!("Could not rename collection: {error}").into(),
                            ),
                            Err(error) => ui.set_variables_message(
                                format!("Could not rename collection: {error}").into(),
                            ),
                        }
                    }
                });
            });
        }
    });

    app.on_delete_collection({
        let app_weak = app_weak.clone();
        let store = Arc::clone(&store);
        move |collection_id| {
            let app_handle = app_weak.clone();
            let store = Arc::clone(&store);
            tokio::task::spawn_blocking(move || {
                store.delete_collection(collection_id as i64)?;
                store.list_collections()
            })
            .then(move |result| async move {
                let _ = slint::invoke_from_event_loop(move || {
                    if let Some(ui) = app_handle.upgrade() {
                        match result {
                            Ok(Ok(collections)) => {
                                let current_selected = ui.get_selected_collection_id();
                                let selected_id = collections
                                    .iter()
                                    .find(|collection| {
                                        collection.id as i32 == current_selected
                                            && current_selected != collection_id
                                    })
                                    .map(|collection| collection.id as i32)
                                    .or_else(|| {
                                        collections.first().map(|collection| collection.id as i32)
                                    });
                                let Some(selected_id) = selected_id else {
                                    ui.set_variables_message(
                                        "Could not delete collection: no collections remain."
                                            .into(),
                                    );
                                    return;
                                };
                                let selection_changed = current_selected != selected_id;
                                let tabs = ui.get_request_tabs();
                                let tabs_to_close = (0..tabs.row_count())
                                    .filter(|index| {
                                        tabs.row_data(*index)
                                            .is_some_and(|tab| tab.collection_id == collection_id)
                                    })
                                    .collect::<Vec<_>>();
                                for index in tabs_to_close.into_iter().rev() {
                                    ui.invoke_close_request_tab(index as i32);
                                }
                                let selection_changed = selection_changed
                                    || ui.get_selected_collection_id() != selected_id;
                                let selected_name = collections
                                    .iter()
                                    .find(|collection| collection.id as i32 == selected_id)
                                    .map(|collection| collection.name.clone());
                                ui.set_collections(to_collection_model(collections));
                                ui.set_confirm_delete_collection_id(-1);
                                if let Some(selected_name) = selected_name {
                                    ui.set_selected_collection_id(selected_id);
                                    ui.set_expanded_collection_id(selected_id);
                                    ui.set_selected_collection_name(selected_name.into());
                                }
                                if selection_changed {
                                    ui.set_active_request_id(-1);
                                    ui.set_active_request("".into());
                                    ui.set_editing_request_name(false);
                                    ui.set_active_method("GET".into());
                                    ui.set_request_url("".into());
                                    ui.set_request_params("".into());
                                    ui.set_request_param_rows(to_key_value_model("", '='));
                                    ui.set_request_body("".into());
                                    ui.set_body_format("JSON".into());
                                    ui.set_request_timeout_ms("30000".into());
                                    ui.set_follow_redirects(true);
                                    ui.set_auth_type("None".into());
                                    ui.set_auth_value("".into());
                                    ui.set_auth_secret("".into());
                                    ui.set_request_headers("Accept: application/json".into());
                                    ui.set_request_header_rows(to_key_value_model(
                                        "Accept: application/json",
                                        ':',
                                    ));
                                    ui.set_response_status("No response yet".into());
                                    ui.set_response_body(
                                        "Send a request to inspect its response.".into(),
                                    );
                                    ui.set_response_time("---".into());
                                    ui.set_response_state("idle".into());
                                    ui.set_response_format("Text".into());
                                    ui.set_response_headers("".into());
                                    ui.set_response_cookies("".into());
                                    ui.set_response_header_items(to_response_header_model(""));
                                    ui.set_response_cookie_items(to_response_cookie_model(""));
                                    ui.set_response_tab("Body".into());
                                    ui.invoke_load_requests(selected_id);
                                    ui.invoke_load_collection_variables(selected_id);
                                }
                                ui.set_variables_message(
                                    "Collection and its saved data deleted.".into(),
                                );
                                if ui.get_active_view() == "Environments" {
                                    ui.invoke_load_environments();
                                }
                            }
                            Ok(Err(error)) => ui.set_variables_message(
                                format!("Could not delete collection: {error}").into(),
                            ),
                            Err(error) => ui.set_variables_message(
                                format!("Could not delete collection: {error}").into(),
                            ),
                        }
                    }
                });
            });
        }
    });

    app.on_create_collection_variable({
        let app_weak = app_weak.clone();
        let store = Arc::clone(&store);
        move |collection_id, key, value, is_secret| {
            let app_handle = app_weak.clone();
            let store = Arc::clone(&store);
            let key = key.to_string();
            let value = value.to_string();
            let submitted_key = key.clone();
            let submitted_value = value.clone();
            tokio::task::spawn_blocking(move || {
                store.create_variable(collection_id as i64, &key, &value, is_secret)?;
                let variables = store.list_variables(collection_id as i64)?;
                Ok::<_, crate::storage::StoreError>(variables)
            })
            .then(move |result| async move {
                let _ = slint::invoke_from_event_loop(move || {
                    if let Some(ui) = app_handle.upgrade() {
                        if ui.get_selected_collection_id() != collection_id {
                            return;
                        }
                        match result {
                            Ok(Ok(variables)) => {
                                ui.set_collection_variables(to_variable_model(variables));
                                if ui.get_variable_key() == submitted_key
                                    && ui.get_variable_value() == submitted_value
                                {
                                    ui.set_variable_key("".into());
                                    ui.set_variable_value("".into());
                                    ui.set_variable_is_secret(false);
                                }
                                ui.set_variables_message("Variable added.".into());
                                if ui.get_active_view() == "Environments" {
                                    ui.invoke_load_environments();
                                }
                            }
                            Ok(Err(error)) => ui.set_variables_message(
                                format!("Could not add variable: {error}").into(),
                            ),
                            Err(error) => ui.set_variables_message(
                                format!("Could not add variable: {error}").into(),
                            ),
                        }
                    }
                });
            });
        }
    });

    app.on_update_collection_variable({
        let app_weak = app_weak.clone();
        let store = Arc::clone(&store);
        move |collection_id, old_key, new_key, value, is_secret| {
            let app_handle = app_weak.clone();
            let store = Arc::clone(&store);
            let old_key = old_key.to_string();
            let new_key = new_key.to_string();
            let value = value.to_string();
            let submitted_key = old_key.clone();
            tokio::task::spawn_blocking(move || {
                store.update_variable(
                    collection_id as i64,
                    &old_key,
                    &new_key,
                    &value,
                    is_secret,
                )?;
                store.list_variables(collection_id as i64)
            })
            .then(move |result| async move {
                let _ = slint::invoke_from_event_loop(move || {
                    if let Some(ui) = app_handle.upgrade() {
                        if ui.get_selected_collection_id() != collection_id {
                            return;
                        }
                        let is_same_edit = ui.get_editing_variable_collection_id() == collection_id
                            && ui.get_editing_variable_key() == submitted_key;
                        match result {
                            Ok(Ok(variables)) => {
                                ui.set_collection_variables(to_variable_model(variables));
                                if ui.get_active_view() == "Environments" {
                                    ui.invoke_load_environments();
                                }
                                if is_same_edit {
                                    ui.set_editing_variable_key("".into());
                                    ui.set_editing_variable_collection_id(-1);
                                    ui.set_editing_variable_draft_key("".into());
                                    ui.set_editing_variable_draft_value("".into());
                                    ui.set_editing_variable_message("".into());
                                }
                            }
                            Ok(Err(error)) if is_same_edit => ui.set_editing_variable_message(
                                format!("Could not update variable: {error}").into(),
                            ),
                            Err(error) if is_same_edit => ui.set_editing_variable_message(
                                format!("Could not update variable: {error}").into(),
                            ),
                            _ => {}
                        }
                    }
                });
            });
        }
    });

    app.on_delete_collection_variable({
        let app_weak = app_weak.clone();
        let store = Arc::clone(&store);
        move |collection_id, key| {
            let app_handle = app_weak.clone();
            let store = Arc::clone(&store);
            let key = key.to_string();
            tokio::task::spawn_blocking(move || {
                store.delete_variable(collection_id as i64, &key)?;
                store.list_variables(collection_id as i64)
            })
            .then(move |result| async move {
                let _ = slint::invoke_from_event_loop(move || {
                    if let Some(ui) = app_handle.upgrade() {
                        if ui.get_selected_collection_id() != collection_id {
                            return;
                        }
                        match result {
                            Ok(Ok(variables)) => {
                                ui.set_collection_variables(to_variable_model(variables));
                                ui.set_variables_message("Variable deleted.".into());
                                if ui.get_active_view() == "Environments" {
                                    ui.invoke_load_environments();
                                }
                            }
                            Ok(Err(error)) => ui.set_variables_message(
                                format!("Could not delete variable: {error}").into(),
                            ),
                            Err(error) => ui.set_variables_message(
                                format!("Could not delete variable: {error}").into(),
                            ),
                        }
                    }
                });
            });
        }
    });

    let request_selection_generation = Arc::new(AtomicU64::new(0));
    app.on_select_request({
        let app_weak = app_weak.clone();
        let store = Arc::clone(&store);
        let request_selection_generation = Arc::clone(&request_selection_generation);
        move |id| {
            if id <= 0 {
                return;
            }
            let generation = request_selection_generation
                .fetch_add(1, Ordering::Relaxed)
                .wrapping_add(1);
            if let Some(ui) = app_weak.upgrade() {
                let tabs = ui.get_request_tabs();
                for index in 0..tabs.row_count() {
                    if tabs.row_data(index).is_some_and(|tab| tab.request_id == id) {
                        ui.invoke_activate_request_tab(index as i32);
                        return;
                    }
                }
                ui.invoke_store_active_request_tab();
            }
            let app_handle = app_weak.clone();
            let request_selection_generation = Arc::clone(&request_selection_generation);
            let store = Arc::clone(&store);
            tokio::task::spawn_blocking(move || {
                store.get(id as i64).and_then(|request| {
                    let Some(request) = request else {
                        return Ok(None);
                    };
                    let collections = store.list_collections()?;
                    let collection_name = collections
                        .into_iter()
                        .find(|collection| collection.id == request.collection_id)
                        .map(|collection| collection.name)
                        .unwrap_or_default();
                    let requests = store.list_for_collection(request.collection_id)?;
                    Ok(Some((request, collection_name, requests)))
                })
            })
            .then(move |result| async move {
                let _ = slint::invoke_from_event_loop(move || {
                    if let Some(ui) = app_handle.upgrade() {
                        if request_selection_generation.load(Ordering::Relaxed) != generation {
                            return;
                        }
                        match result {
                            Ok(Ok(Some((request, collection_name, requests)))) => {
                                let collection_id = request.collection_id as i32;
                                ui.set_selected_collection_id(collection_id);
                                ui.set_expanded_collection_id(collection_id);
                                ui.set_selected_collection_name(collection_name.into());
                                replace_saved_request_models(&ui, requests);
                                ui.invoke_load_collection_variables(collection_id);
                                ui.set_active_view("Request".into());
                                ui.set_is_loading(false);
                                ui.set_active_request_id(request.id as i32);
                                ui.set_active_request(request.name.into());
                                ui.set_editing_request_name(false);
                                ui.set_active_method(request.method.into());
                                ui.set_request_url(request.url.into());
                                ui.set_request_header_rows(to_key_value_model(
                                    &request.headers,
                                    ':',
                                ));
                                ui.set_request_param_rows(to_key_value_model(
                                    &request.query_params,
                                    '=',
                                ));
                                ui.set_request_headers(request.headers.into());
                                ui.set_request_params(request.query_params.into());
                                ui.set_request_body(request.body.into());
                                ui.set_body_format(request.body_format.into());
                                ui.set_request_timeout_ms(request.timeout_ms.to_string().into());
                                ui.set_follow_redirects(request.follow_redirects);
                                ui.set_auth_type(request.auth_type.into());
                                ui.set_auth_value(request.auth_value.into());
                                ui.set_auth_secret(request.auth_secret.into());
                                ui.set_request_assertions(
                                    serde_json::to_string(&request.assertions)
                                        .unwrap_or_else(|_| "[]".to_owned())
                                        .into(),
                                );
                                ui.set_response_status("No response yet".into());
                                ui.set_response_body(
                                    "Send a request to inspect its response.".into(),
                                );
                                ui.set_response_time("---".into());
                                ui.set_response_state("idle".into());
                                ui.set_response_format("Text".into());
                                ui.invoke_register_active_request_tab();
                            }
                            Ok(Ok(None)) => {
                                ui.set_response_status("Request not found; refreshing list".into());
                                ui.invoke_load_requests(ui.get_expanded_collection_id());
                            }
                            Ok(Err(error)) => ui.set_response_status(
                                format!("Could not load request: {error}").into(),
                            ),
                            Err(error) => ui.set_response_status(
                                format!("Could not load request: {error}").into(),
                            ),
                        }
                    }
                });
            });
        }
    });

    app.on_delete_request({
        let app_weak = app_weak.clone();
        let store = Arc::clone(&store);
        move |id, collection_id| {
            if id <= 0 {
                return;
            }
            let app_handle = app_weak.clone();
            let store = Arc::clone(&store);
            tokio::task::spawn_blocking(move || {
                let result = store.delete(id as i64);
                let list_result = store.list_for_collection(collection_id as i64);
                (result, list_result)
            })
            .then(move |result| async move {
                let _ = slint::invoke_from_event_loop(move || {
                    if let Some(ui) = app_handle.upgrade() {
                        match result {
                            Ok((Ok(true), requests)) => {
                                let selected_collection =
                                    ui.get_selected_collection_id() == collection_id;
                                let selected_collection_name = ui.get_selected_collection_name();
                                let tabs = ui.get_request_tabs();
                                let deleted_tab_index = (0..tabs.row_count()).find(|index| {
                                    tabs.row_data(*index)
                                        .is_some_and(|tab| tab.request_id == id)
                                });
                                if let Some(index) = deleted_tab_index {
                                    ui.invoke_close_request_tab(index as i32);
                                }
                                if ui.get_expanded_collection_id() == collection_id
                                    && let Ok(requests) = requests
                                {
                                    replace_saved_request_models(&ui, requests);
                                    if selected_collection && ui.get_active_request_tab_index() < 0
                                    {
                                        ui.set_selected_collection_id(collection_id);
                                        ui.set_expanded_collection_id(collection_id);
                                        ui.set_selected_collection_name(selected_collection_name);
                                        ui.invoke_show_empty_collection();
                                    }
                                } else {
                                    ui.set_response_status(
                                        "Request deleted, but the collection could not be refreshed"
                                            .into(),
                                    );
                                }
                            }
                            Ok((Ok(false), requests)) => {
                                ui.set_response_status("Request was already deleted".into());
                                if ui.get_expanded_collection_id() == collection_id
                                    && let Ok(requests) = requests
                                {
                                    replace_saved_request_models(&ui, requests);
                                }
                            }
                            Ok((Err(error), _)) => {
                                ui.set_response_status(format!("Delete failed: {error}").into())
                            }
                            Err(error) => {
                                ui.set_response_status(format!("Delete failed: {error}").into())
                            }
                        }
                    }
                });
            });
        }
    });

    app.on_open_latest_request({
        let app_weak = app_weak.clone();
        let store = Arc::clone(&store);
        move |collection_id| {
            let app_handle = app_weak.clone();
            let store = Arc::clone(&store);
            tokio::task::spawn_blocking(move || {
                store
                    .list_for_collection(collection_id as i64)
                    .map(|requests| requests.into_iter().max_by_key(|request| request.id))
            })
            .then(move |result| async move {
                let _ = slint::invoke_from_event_loop(move || {
                    if let Some(ui) = app_handle.upgrade() {
                        if ui.get_selected_collection_id() != collection_id {
                            return;
                        }
                        match result {
                            Ok(Ok(Some(request))) => {
                                ui.invoke_select_request(request.id as i32);
                            }
                            Ok(Ok(None)) => ui.invoke_show_empty_collection(),
                            Ok(Err(error)) => {
                                ui.invoke_show_empty_collection();
                                ui.set_variables_message(
                                    format!("Could not open the latest request: {error}").into(),
                                );
                            }
                            Err(error) => {
                                ui.invoke_show_empty_collection();
                                ui.set_variables_message(
                                    format!("Could not open the latest request: {error}").into(),
                                );
                            }
                        }
                    }
                });
            });
        }
    });

    let theme_weak = app.as_weak();
    let theme_fallback = app.get_system_dark_mode();
    let accent_fallback = app.get_system_accent_color();
    std::thread::spawn(move || {
        loop {
            std::thread::sleep(Duration::from_secs(2));
            let system_dark = detect_system_dark_mode(theme_fallback);
            let system_accent = detect_system_accent_color(accent_fallback);
            let app_handle = theme_weak.clone();
            if slint::invoke_from_event_loop(move || {
                if let Some(ui) = app_handle.upgrade() {
                    if ui.get_theme_mode() == "System" && ui.get_system_dark_mode() != system_dark {
                        ui.set_system_dark_mode(system_dark);
                    }
                    if ui.get_system_accent_color() != system_accent {
                        ui.set_system_accent_color(system_accent);
                    }
                }
            })
            .is_err()
            {
                break;
            }
        }
    });

    app.invoke_load_collections();
    Ok(app.run()?)
}
