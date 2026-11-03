use std::sync::Arc;

use crate::assertions::evaluate_collection_assertions;
use crate::{
    AppWindow, CollectionRunResult, MAX_STRUCTURED_VIEW_BYTES, format_response_body,
    to_response_header_model,
};
use crate::{http::HttpClient, storage::RequestStore};
use slint::{Model, ModelRc, VecModel};

pub fn register(
    app: &AppWindow,
    app_weak: slint::Weak<AppWindow>,
    client: Arc<HttpClient>,
    store: Arc<RequestStore>,
) {
    app.on_run_collection({
        let app_weak = app_weak.clone();
        let client = Arc::clone(&client);
        let store = Arc::clone(&store);
        move |collection_id| {
            let collection_name = app_weak
                .upgrade()
                .and_then(|ui| {
                    ui.get_collections()
                        .iter()
                        .find(|c| c.id == collection_id)
                        .map(|c| c.name.to_string())
                })
                .unwrap_or_else(|| "Collection".to_owned());
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
                let (requests, variables) = match snapshot {
                    Ok(Ok(snapshot)) => snapshot,
                    Ok(Err(error)) => {
                        let message = format!("Could not start collection run: {error}");
                        let _ = slint::invoke_from_event_loop(move || {
                            if let Some(ui) = app_handle.upgrade() {
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
                    move || {
                        if let Some(ui) = handle.upgrade() {
                            ui.set_collection_run_total(total as i32);
                        }
                    }
                });

                let mut passed = 0usize;
                let mut failed = 0usize;
                let mut history_entries: Vec<(i64, i64, String, String, String, String, i64)> =
                    Vec::new();

                for request in requests {
                    let result = client
                        .send(
                            &request.method,
                            &request.url,
                            &request.body,
                            &request.headers,
                            &request.query_params,
                            &request.body_format,
                            &request.auth_type,
                            &request.auth_value,
                            &request.auth_secret,
                            &variables,
                        )
                        .await;
                    let outcome = evaluate_collection_assertions(
                        &request.assertions,
                        &result.status,
                        &result.state,
                        result.elapsed,
                        &result.body,
                    );
                    let success = outcome.passed;
                    let assertion_summary = if outcome.checks.is_empty() {
                        if result.state == "success" {
                            "No assertions configured".to_owned()
                        } else {
                            format!("No assertions configured · {}", result.state)
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
                        state: outcome.state.into(),
                        assertion_summary: assertion_summary.into(),
                        body: result.body.clone().into(),
                        pretty_body: formatted_body.into(),
                        format: result.format.clone().into(),
                        large: result.body.len() > MAX_STRUCTURED_VIEW_BYTES,
                        headers: result.headers.clone().into(),
                    };
                    let progress_handle = app_handle.clone();
                    let completed = passed + failed;
                    let _ = slint::invoke_from_event_loop(move || {
                        if let Some(ui) = progress_handle.upgrade() {
                            ui.set_collection_run_completed(completed as i32);
                            ui.set_collection_run_passed(passed as i32);
                            ui.set_collection_run_failed(failed as i32);
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
                    history_entries.push((
                        request.id,
                        request.collection_id,
                        request.name.clone(),
                        request.method.clone(),
                        request.url.clone(),
                        result.status.clone(),
                        (result.elapsed.as_secs_f64() * 1000.0) as i64,
                    ));
                }
                let history_store = Arc::clone(&store);
                let _ = tokio::task::spawn_blocking(move || {
                    history_store.record_history_batch(&history_entries)
                })
                .await;

                let duration = format!("{:.2} s", started.elapsed().as_secs_f64());
                let _ = slint::invoke_from_event_loop(move || {
                    if let Some(ui) = app_handle.upgrade() {
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
