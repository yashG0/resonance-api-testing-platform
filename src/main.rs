mod http;
mod storage;

#[cfg(target_os = "linux")]
use std::process::Command;
use std::{future::Future, sync::Arc};

use crate::http::HttpClient;
use crate::storage::{Collection, CollectionVariable, NewRequest, RequestStore, SavedRequest};
use slint::{ComponentHandle, ModelRc, VecModel};

slint::include_modules!();

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

fn detect_system_dark_mode(fallback: bool) -> bool {
    #[cfg(target_os = "linux")]
    {
        if let Ok(output) = Command::new("gsettings")
            .args(["get", "org.gnome.desktop.interface", "color-scheme"])
            .output()
        {
            if output.status.success() {
                let preference = String::from_utf8_lossy(&output.stdout);
                if preference.contains("prefer-dark") {
                    return true;
                }
                if preference.contains("prefer-light") {
                    return false;
                }
            }
        }
    }

    fallback
}

fn to_request_model(requests: Vec<SavedRequest>) -> ModelRc<RequestItem> {
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

fn to_collection_model(collections: Vec<Collection>) -> ModelRc<CollectionItem> {
    let items = collections
        .into_iter()
        .map(|collection| CollectionItem {
            id: collection.id as i32,
            name: collection.name.into(),
        })
        .collect::<Vec<_>>();
    ModelRc::new(VecModel::from(items))
}

fn to_variable_model(variables: Vec<CollectionVariable>) -> ModelRc<VariableItem> {
    let items = variables
        .into_iter()
        .map(|variable| VariableItem {
            key: variable.key.into(),
            value: variable.value.into(),
        })
        .collect::<Vec<_>>();
    ModelRc::new(VecModel::from(items))
}

#[tokio::main]
async fn main() -> Result<(), slint::PlatformError> {
    let store = Arc::new(
        RequestStore::open_default().expect("Failed to initialize Resonance request database"),
    );
    let client = Arc::new(HttpClient::new().expect("Failed to initialize Resonance HTTP client"));

    let app = AppWindow::new()?;
    app.set_system_dark_mode(detect_system_dark_mode(app.get_system_dark_mode()));
    let app_weak = app.as_weak();

    app.on_refresh_system_theme({
        let app_weak = app_weak.clone();
        move || {
            if let Some(ui) = app_weak.upgrade() {
                let fallback = ui.get_system_dark_mode();
                ui.set_system_dark_mode(detect_system_dark_mode(fallback));
            }
        }
    });

    // Load the selected collection's variables off the UI thread, then send the
    // fully resolved request without blocking Slint's event loop.
    app.on_send_request({
        let app_weak = app_weak.clone();
        let client = Arc::clone(&client);
        let store = Arc::clone(&store);

        move |method, url, body, headers, params, collection_id, request_id, name| {
            let app_handle = app_weak.clone();
            let client = Arc::clone(&client);
            let store = Arc::clone(&store);
            let method = method.to_string();
            let url = url.to_string();
            let body = body.to_string();
            let headers = headers.to_string();
            let params = params.to_string();
            let name = name.to_string();
            let collection_id = collection_id as i64;

            tokio::spawn(async move {
                // Save first so each request is available in its collection as soon
                // as the response arrives. Existing requests are updated in place.
                let request = NewRequest {
                    collection_id,
                    name: name.clone(),
                    method: method.clone(),
                    url: url.clone(),
                    headers: headers.clone(),
                    query_params: params.clone(),
                    body: body.clone(),
                };
                let save_store = Arc::clone(&store);
                let save_result = tokio::task::spawn_blocking(move || {
                    let saved_id =
                        if request_id > 0 && save_store.update(request_id as i64, &request)? {
                            request_id as i64
                        } else {
                            save_store.create(&request)?.id
                        };
                    let requests = save_store.list_for_collection(collection_id)?;
                    let variables = save_store.variables_map(collection_id)?;
                    Ok::<_, crate::storage::StoreError>((saved_id, requests, variables))
                })
                .await;

                let (saved_id, requests, variables) = match save_result {
                    Ok(Ok(saved)) => saved,
                    Ok(Err(error)) => {
                        let message = format!("Auto-save failed; request was not sent: {error}");
                        let _ = slint::invoke_from_event_loop(move || {
                            if let Some(ui) = app_handle.upgrade() {
                                ui.set_response_status("Auto-save failed".into());
                                ui.set_response_state("error".into());
                                ui.set_response_format("Text".into());
                                ui.set_response_body(message.into());
                                ui.set_response_time("---".into());
                                ui.set_is_loading(false);
                            }
                        });
                        return;
                    }
                    Err(error) => {
                        let message = format!("Auto-save failed; request was not sent: {error}");
                        let _ = slint::invoke_from_event_loop(move || {
                            if let Some(ui) = app_handle.upgrade() {
                                ui.set_response_status("Auto-save failed".into());
                                ui.set_response_state("error".into());
                                ui.set_response_format("Text".into());
                                ui.set_response_body(message.into());
                                ui.set_response_time("---".into());
                                ui.set_is_loading(false);
                            }
                        });
                        return;
                    }
                };

                let list_handle = app_handle.clone();
                let visible_url = url.clone();
                let visible_name = name.clone();
                let _ = slint::invoke_from_event_loop(move || {
                    if let Some(ui) = list_handle.upgrade() {
                        if ui.get_selected_collection_id() as i64 == collection_id {
                            ui.set_saved_requests(to_request_model(requests));
                            if ui.get_request_url().as_str() == visible_url
                                && ui.get_active_request().as_str() == visible_name
                            {
                                ui.set_active_request_id(saved_id as i32);
                            }
                        }
                    }
                });

                let result = client
                    .send(&method, &url, &body, &headers, &params, &variables)
                    .await;
                let response_state = result
                    .status
                    .split_whitespace()
                    .next()
                    .and_then(|code| code.parse::<u16>().ok())
                    .map(|code| {
                        if (200..400).contains(&code) {
                            "success"
                        } else {
                            "http-error"
                        }
                    })
                    .unwrap_or("error");

                let _ = slint::invoke_from_event_loop(move || {
                    if let Some(ui) = app_handle.upgrade() {
                        if ui.get_selected_collection_id() as i64 == collection_id
                            && ui.get_active_request_id() as i64 == saved_id
                        {
                            ui.set_response_status(result.status.into());
                            ui.set_response_state(response_state.into());
                            ui.set_response_format(result.format.into());
                            ui.set_response_body(result.body.into());
                            ui.set_response_time(
                                format!("{:.2} ms", result.elapsed.as_secs_f64() * 1000.0).into(),
                            );
                            ui.set_is_loading(false);
                        }
                    }
                });
            });
        }
    });

    app.on_save_request({
        let app_weak = app_weak.clone();
        let store = Arc::clone(&store);

        move |id, collection_id, name, method, url, headers, params, body| {
            let app_handle = app_weak.clone();
            let store = Arc::clone(&store);
            let request = NewRequest {
                collection_id: collection_id as i64,
                name: name.to_string(),
                method: method.to_string(),
                url: url.to_string(),
                headers: headers.to_string(),
                query_params: params.to_string(),
                body: body.to_string(),
            };

            tokio::task::spawn_blocking(move || {
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
                                ui.set_active_request_id(saved_id as i32);
                                ui.set_response_status(format!("Saved: {saved_name}").into());
                            }
                            Ok(_) => ui.set_response_status(
                                "Save failed: request no longer exists".into(),
                            ),
                            Err(error) => {
                                ui.set_response_status(format!("Save failed: {error}").into())
                            }
                        }
                        if let Ok(requests) = list_result {
                            ui.set_saved_requests(to_request_model(requests));
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
                                        .or_else(|| collections.first())
                                        .cloned();
                                    ui.set_collections(to_collection_model(collections));
                                    if let Some(selected) = selected {
                                        let id = selected.id as i32;
                                        ui.set_selected_collection_id(id);
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
                                    ui.set_saved_requests(to_request_model(requests))
                                }
                                Ok(Err(error)) => {
                                    ui.set_response_status(format!("Load failed: {error}").into())
                                }
                                Err(error) => {
                                    ui.set_response_status(format!("Load failed: {error}").into())
                                }
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
                                ui.set_collections(to_collection_model(collections));
                                ui.set_selected_collection_id(id);
                                ui.set_selected_collection_name(created.name.into());
                                ui.set_active_view("Request".into());
                                ui.set_active_request_id(-1);
                                ui.set_active_request("New Request".into());
                                ui.set_active_method("GET".into());
                                ui.set_request_url("".into());
                                ui.set_request_headers("Accept: application/json".into());
                                ui.set_request_params("".into());
                                ui.set_request_body("".into());
                                ui.invoke_load_requests(id);
                                ui.invoke_load_collection_variables(id);
                                ui.set_variables_message("Collection created.".into());
                                ui.set_new_collection_name("".into());
                                ui.set_show_new_collection(false);
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
                                ui.set_selected_collection_name(renamed.name.into());
                                ui.set_rename_collection_name(ui.get_selected_collection_name());
                                ui.set_show_rename_collection(false);
                                ui.set_variables_message("Collection renamed.".into());
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
                                let Some(selected) = collections.first().cloned() else {
                                    ui.set_variables_message(
                                        "Could not delete collection: no collections remain."
                                            .into(),
                                    );
                                    return;
                                };
                                let id = selected.id as i32;
                                ui.set_collections(to_collection_model(collections));
                                ui.set_selected_collection_id(id);
                                ui.set_selected_collection_name(selected.name.into());
                                ui.set_confirm_delete_collection(false);
                                ui.set_active_request_id(-1);
                                ui.set_active_request("New Request".into());
                                ui.set_active_method("GET".into());
                                ui.set_request_url("".into());
                                ui.set_request_params("".into());
                                ui.set_request_body("".into());
                                ui.set_request_headers("Accept: application/json".into());
                                ui.set_response_status("No response yet".into());
                                ui.set_response_body(
                                    "Send a request to inspect its response.".into(),
                                );
                                ui.set_response_time("---".into());
                                ui.set_response_state("idle".into());
                                ui.set_response_format("Text".into());
                                ui.invoke_load_requests(id);
                                ui.invoke_load_collection_variables(id);
                                ui.set_variables_message(
                                    "Collection and its saved data deleted.".into(),
                                );
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

    app.on_save_collection_variable({
        let app_weak = app_weak.clone();
        let store = Arc::clone(&store);
        move |collection_id, key, value| {
            let app_handle = app_weak.clone();
            let store = Arc::clone(&store);
            let key = key.to_string();
            let value = value.to_string();
            tokio::task::spawn_blocking(move || {
                store.upsert_variable(collection_id as i64, &key, &value)?;
                let variables = store.list_variables(collection_id as i64)?;
                Ok::<_, crate::storage::StoreError>(variables)
            })
            .then(move |result| async move {
                let _ = slint::invoke_from_event_loop(move || {
                    if let Some(ui) = app_handle.upgrade() {
                        match result {
                            Ok(Ok(variables)) => {
                                ui.set_collection_variables(to_variable_model(variables));
                                ui.set_variables_message("Variable saved.".into());
                            }
                            Ok(Err(error)) => ui.set_variables_message(
                                format!("Could not save variable: {error}").into(),
                            ),
                            Err(error) => ui.set_variables_message(
                                format!("Could not save variable: {error}").into(),
                            ),
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
                        match result {
                            Ok(Ok(variables)) => {
                                ui.set_collection_variables(to_variable_model(variables));
                                ui.set_variables_message("Variable deleted.".into());
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

    app.on_select_request({
        let app_weak = app_weak.clone();
        let store = Arc::clone(&store);
        move |id| {
            if id <= 0 {
                return;
            }
            let app_handle = app_weak.clone();
            let store = Arc::clone(&store);
            tokio::task::spawn_blocking(move || store.get(id as i64)).then(
                move |result| async move {
                    let _ = slint::invoke_from_event_loop(move || {
                        if let Some(ui) = app_handle.upgrade() {
                            match result {
                                Ok(Ok(Some(request))) => {
                                    ui.set_active_view("Request".into());
                                    ui.set_is_loading(false);
                                    ui.set_active_request_id(request.id as i32);
                                    ui.set_active_request(request.name.into());
                                    ui.set_active_method(request.method.into());
                                    ui.set_request_url(request.url.into());
                                    ui.set_request_headers(request.headers.into());
                                    ui.set_request_params(request.query_params.into());
                                    ui.set_request_body(request.body.into());
                                    ui.set_response_status("No response yet".into());
                                    ui.set_response_body(
                                        "Send a request to inspect its response.".into(),
                                    );
                                    ui.set_response_time("---".into());
                                    ui.set_response_state("idle".into());
                                    ui.set_response_format("Text".into());
                                }
                                Ok(Ok(None)) => {
                                    ui.set_response_status(
                                        "Request not found; refreshing list".into(),
                                    );
                                    ui.invoke_load_requests(ui.get_selected_collection_id());
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
                },
            );
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
                                if ui.get_active_request_id() == id {
                                    ui.set_active_request_id(-1);
                                    ui.set_active_request("New Request".into());
                                    ui.set_active_method("GET".into());
                                    ui.set_request_url("".into());
                                    ui.set_request_headers("Accept: application/json".into());
                                    ui.set_request_params("".into());
                                    ui.set_request_body("".into());
                                }
                                ui.set_response_status("Request deleted".into());
                                if let Ok(requests) = requests {
                                    ui.set_saved_requests(to_request_model(requests));
                                }
                            }
                            Ok((Ok(false), requests)) => {
                                ui.set_response_status("Request was already deleted".into());
                                if let Ok(requests) = requests {
                                    ui.set_saved_requests(to_request_model(requests));
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

    app.invoke_load_collections();
    app.run()
}
