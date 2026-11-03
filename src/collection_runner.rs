use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
};

use crate::assertions::{
    AssertionResult, CollectionAssertionOutcome, captured_variable_values,
    evaluate_collection_assertions,
};
use crate::{
    AppWindow, CollectionRunResult, MAX_STRUCTURED_VIEW_BYTES, format_response_body,
    to_response_header_model,
};
use crate::{
    http::HttpClient,
    http::HttpState,
    http::client::{HttpResult, RequestSpec},
    storage::{HistoryRecord, RequestStore},
};
use slint::{Model, ModelRc, VecModel};

pub fn register(
    app: &AppWindow,
    app_weak: slint::Weak<AppWindow>,
    client: Arc<HttpClient>,
    store: Arc<RequestStore>,
) {
    let run_generation = Arc::new(AtomicU64::new(0));
    app.on_run_collection({
        let app_weak = app_weak.clone();
        let run_generation = Arc::clone(&run_generation);
        let client = Arc::clone(&client);
        let store = Arc::clone(&store);
        move |collection_id, stop_on_failure, retries_text, delay_text| {
            let generation = run_generation
                .fetch_add(1, Ordering::Relaxed)
                .wrapping_add(1);
            let collection_name = app_weak
                .upgrade()
                .and_then(|ui| {
                    ui.get_collections()
                        .iter()
                        .find(|c| c.id == collection_id)
                        .map(|c| c.name.to_string())
                })
                .unwrap_or_else(|| "Collection".to_owned());
            let max_retries = retries_text
                .as_str()
                .parse::<u8>()
                .ok()
                .filter(|retries| *retries <= 5);
            let delay_ms = delay_text
                .as_str()
                .parse::<u64>()
                .ok()
                .filter(|delay| *delay <= 60_000);
            let (Some(max_retries), Some(delay_ms)) = (max_retries, delay_ms) else {
                if let Some(ui) = app_weak.upgrade() {
                    ui.set_collection_run_name(collection_name.into());
                    ui.set_collection_run_state("error".into());
                    ui.set_collection_run_error(
                        "Retries must be 0–5 and delay must be 0–60000 milliseconds.".into(),
                    );
                    ui.set_active_view("Regression".into());
                }
                return;
            };
            if let Some(ui) = app_weak.upgrade() {
                ui.set_collection_run_name(collection_name.clone().into());
                ui.set_collection_run_state("running".into());
                ui.set_collection_run_duration("".into());
                ui.set_collection_run_error("".into());
                ui.set_collection_run_total(0);
                ui.set_collection_run_completed(0);
                ui.set_collection_run_passed(0);
                ui.set_collection_run_failed(0);
                ui.set_collection_run_skipped(0);
                ui.set_collection_run_selected(-1);
                ui.set_collection_run_response_tab("Body".into());
                ui.set_collection_run_header_items(to_response_header_model(""));
                ui.set_collection_run_results(ModelRc::new(VecModel::from(Vec::<
                    CollectionRunResult,
                >::new(
                ))));
                ui.set_active_view("Regression".into());
            }
            let app_handle = app_weak.clone();
            let run_generation = Arc::clone(&run_generation);
            let client = Arc::clone(&client);
            let store = Arc::clone(&store);
            tokio::spawn(async move {
                let snapshot_store = Arc::clone(&store);
                let snapshot = tokio::task::spawn_blocking(move || {
                    Ok::<_, crate::storage::StoreError>((
                        snapshot_store.list_for_collection(collection_id as i64)?,
                        snapshot_store.variables_map(collection_id as i64)?,
                    ))
                })
                .await;
                let (requests, mut variables) = match snapshot {
                    Ok(Ok(snapshot)) => snapshot,
                    Ok(Err(error)) => {
                        let message = format!("Could not start collection run: {error}");
                        let _ = slint::invoke_from_event_loop(move || {
                            if let Some(ui) = app_handle.upgrade() {
                                if run_generation.load(Ordering::Relaxed) != generation {
                                    return;
                                }
                                ui.set_collection_run_state("error".into());
                                ui.set_collection_run_error(message.into());
                            }
                        });
                        return;
                    }
                    Err(error) => {
                        let message = format!("Could not start collection run: {error}");
                        let _ = slint::invoke_from_event_loop(move || {
                            if let Some(ui) = app_handle.upgrade() {
                                if run_generation.load(Ordering::Relaxed) != generation {
                                    return;
                                }
                                ui.set_collection_run_state("error".into());
                                ui.set_collection_run_error(message.into());
                            }
                        });
                        return;
                    }
                };

                let total = requests.len();
                let started = std::time::Instant::now();
                let _ = slint::invoke_from_event_loop({
                    let handle = app_handle.clone();
                    let generation_guard = Arc::clone(&run_generation);
                    move || {
                        if let Some(ui) = handle.upgrade() {
                            if generation_guard.load(Ordering::Relaxed) != generation {
                                return;
                            }
                            ui.set_collection_run_total(total as i32);
                        }
                    }
                });

                let mut passed = 0usize;
                let mut failed = 0usize;
                let mut skipped = 0usize;
                let mut history_entries: Vec<HistoryRecord> = Vec::new();
                let mut has_sent_request = false;

                for request in requests {
                    if run_generation.load(Ordering::Relaxed) != generation {
                        break;
                    }
                    if stop_on_failure && failed > 0 {
                        skipped += 1;
                        let result_item = CollectionRunResult {
                            method: request.method.clone().into(),
                            name: request.name.clone().into(),
                            url: request.url.clone().into(),
                            status: "Skipped".into(),
                            elapsed: "—".into(),
                            state: "skipped".into(),
                            assertion_summary: "Skipped after the first failed request".into(),
                            body: "This request was not sent because stop-on-failure is enabled."
                                .into(),
                            pretty_body: String::new().into(),
                            format: "Text".into(),
                            large: false,
                            headers: String::new().into(),
                        };
                        let progress_handle = app_handle.clone();
                        let generation_guard = Arc::clone(&run_generation);
                        let completed = passed + failed + skipped;
                        let _ = slint::invoke_from_event_loop(move || {
                            if let Some(ui) = progress_handle.upgrade()
                                && generation_guard.load(Ordering::Relaxed) == generation
                            {
                                ui.set_collection_run_completed(completed as i32);
                                ui.set_collection_run_passed(passed as i32);
                                ui.set_collection_run_failed(failed as i32);
                                ui.set_collection_run_skipped(skipped as i32);
                                let results = ui.get_collection_run_results();
                                if let Some(results) = results
                                    .as_any()
                                    .downcast_ref::<VecModel<CollectionRunResult>>()
                                {
                                    results.push(result_item);
                                }
                                if ui.get_collection_run_selected() < 0 {
                                    ui.set_collection_run_selected(0);
                                }
                            }
                        });
                        history_entries.push(HistoryRecord {
                            request_id: request.id,
                            collection_id: request.collection_id,
                            request_name: request.name,
                            method: request.method,
                            url: request.url,
                            status: "Skipped after first failure".to_owned(),
                            elapsed_ms: 0,
                        });
                        continue;
                    }
                    if has_sent_request && delay_ms > 0 {
                        tokio::time::sleep(std::time::Duration::from_millis(delay_ms)).await;
                    }
                    has_sent_request = true;
                    let invalid_assertions = request.assertions_error.clone();
                    let request_started = std::time::Instant::now();
                    let (mut result, outcome) = if invalid_assertions.is_some() {
                        let result = HttpResult {
                            status: "Invalid assertions".to_owned(),
                            status_code: None,
                            state: HttpState::Error,
                            body: String::new(),
                            headers: String::new(),
                            header_values: Default::default(),
                            cookies: String::new(),
                            format: "Text".to_owned(),
                            elapsed: std::time::Duration::ZERO,
                        };
                        let error = invalid_assertions.clone().unwrap_or_default();
                        (
                            result,
                            CollectionAssertionOutcome {
                                passed: false,
                                state: "assertion-failed".to_owned(),
                                checks: vec![AssertionResult {
                                    passed: false,
                                    message: error,
                                }],
                            },
                        )
                    } else {
                        let mut attempt = 0;
                        loop {
                            let result = client
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
                                result.status_code,
                                &result.header_values,
                                result.state.as_label(),
                                result.elapsed,
                                &result.body,
                            );
                            if outcome.passed || attempt >= max_retries {
                                break (result, outcome);
                            }
                            attempt += 1;
                        }
                    };
                    result.elapsed = request_started.elapsed();
                    if outcome.passed {
                        for (key, value) in
                            captured_variable_values(&request.assertions, &result.body)
                        {
                            variables.insert(key, value);
                        }
                    }
                    let success = outcome.passed;
                    let assertion_summary = if outcome.checks.is_empty() {
                        if result.state == HttpState::Success {
                            "No assertions configured".to_owned()
                        } else {
                            format!("No assertions configured · {}", result.state.as_label())
                        }
                    } else {
                        outcome
                            .checks
                            .iter()
                            .map(|item| {
                                format!("{} {}", if item.passed { "✓" } else { "✗" }, item.message)
                            })
                            .collect::<Vec<_>>()
                            .join("\n")
                    };
                    if success {
                        passed += 1;
                    } else {
                        failed += 1;
                    }
                    let formatted_body = if result.body.len() <= MAX_STRUCTURED_VIEW_BYTES
                        && matches!(result.format.as_str(), "JSON" | "HTML" | "XML")
                    {
                        let body_for_formatting = result.body.clone();
                        let format_for_formatting = result.format.clone();
                        tokio::task::spawn_blocking(move || {
                            format_response_body(&body_for_formatting, &format_for_formatting)
                        })
                        .await
                        .unwrap_or_default()
                    } else {
                        String::new()
                    };
                    let result_item = CollectionRunResult {
                        method: request.method.clone().into(),
                        name: request.name.clone().into(),
                        url: request.url.clone().into(),
                        status: result.status.clone().into(),
                        elapsed: format!("{:.2} ms", result.elapsed.as_secs_f64() * 1000.0).into(),
                        state: outcome.state.clone().into(),
                        assertion_summary: assertion_summary.into(),
                        body: result.body.clone().into(),
                        pretty_body: formatted_body.into(),
                        format: result.format.clone().into(),
                        large: result.body.len() > MAX_STRUCTURED_VIEW_BYTES,
                        headers: result.headers.clone().into(),
                    };
                    let progress_handle = app_handle.clone();
                    let generation_guard = Arc::clone(&run_generation);
                    let completed = passed + failed + skipped;
                    let _ = slint::invoke_from_event_loop(move || {
                        if let Some(ui) = progress_handle.upgrade() {
                            if generation_guard.load(Ordering::Relaxed) != generation {
                                return;
                            }
                            ui.set_collection_run_completed(completed as i32);
                            ui.set_collection_run_passed(passed as i32);
                            ui.set_collection_run_failed(failed as i32);
                            ui.set_collection_run_skipped(skipped as i32);
                            let results = ui.get_collection_run_results();
                            if let Some(results) = results
                                .as_any()
                                .downcast_ref::<VecModel<CollectionRunResult>>()
                            {
                                results.push(result_item);
                            }
                            if ui.get_collection_run_selected() < 0 {
                                ui.set_collection_run_selected(0);
                            }
                            let selected = ui.get_collection_run_selected();
                            if selected >= 0
                                && let Some(result) =
                                    ui.get_collection_run_results().row_data(selected as usize)
                            {
                                ui.set_collection_run_header_items(to_response_header_model(
                                    result.headers.as_str(),
                                ));
                            }
                        }
                    });
                    history_entries.push(HistoryRecord {
                        request_id: request.id,
                        collection_id: request.collection_id,
                        request_name: request.name.clone(),
                        method: request.method.clone(),
                        url: request.url.clone(),
                        status: if outcome.state == "assertion-failed" {
                            format!("{} · assertion failed", result.status)
                        } else {
                            result.status.clone()
                        },
                        elapsed_ms: (result.elapsed.as_secs_f64() * 1000.0) as i64,
                    });
                }
                let history_store = Arc::clone(&store);
                let _ = tokio::task::spawn_blocking(move || {
                    history_store.record_history_batch(&history_entries)
                })
                .await;

                let duration = format!("{:.2} s", started.elapsed().as_secs_f64());
                let _ = slint::invoke_from_event_loop(move || {
                    if let Some(ui) = app_handle.upgrade() {
                        if run_generation.load(Ordering::Relaxed) != generation {
                            return;
                        }
                        ui.set_collection_run_duration(duration.into());
                        ui.set_collection_run_state("completed".into());
                        if ui.get_active_view() == "History" {
                            ui.invoke_load_history(ui.get_history_sort());
                        }
                    }
                });
            });
        }
    });
}
