mod http;
mod storage;

#[cfg(target_os = "linux")]
use std::process::Command;
use std::sync::Arc;

use crate::http::HttpClient;
use crate::storage::{NewRequest, RequestStore, SavedRequest};
use slint::{ComponentHandle, ModelRc, VecModel};

slint::include_modules!();

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

#[tokio::main]
async fn main() -> Result<(), slint::PlatformError> {
    let store = Arc::new(
        RequestStore::open_default().expect("Failed to initialize Resonance request database"),
    );

    let client = Arc::new(HttpClient::new().expect("Failed to initialize Resonance HTTP client"));

    let app = AppWindow::new()?;
    let system_dark_mode = detect_system_dark_mode(app.get_system_dark_mode());
    app.set_system_dark_mode(system_dark_mode);
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

    // Send HTTP requests without blocking the UI.
    app.on_send_request({
        let app_weak = app_weak.clone();
        let client = Arc::clone(&client);

        move |method, url, body, headers, params| {
            let app_handle = app_weak.clone();
            let client = Arc::clone(&client);

            let method = method.to_string();
            let url = url.to_string();
            let body = body.to_string();
            let headers = headers.to_string();
            let params = params.to_string();

            tokio::spawn(async move {
                let result = client.send(&method, &url, &body, &headers, &params).await;

                let status = result.status;
                let response_body = result.body;
                let elapsed = format!("{:.2} ms", result.elapsed.as_secs_f64() * 1000.0);

                let _ = slint::invoke_from_event_loop(move || {
                    if let Some(ui) = app_handle.upgrade() {
                        ui.set_response_status(status.into());
                        ui.set_response_body(response_body.into());
                        ui.set_response_time(elapsed.into());
                        ui.set_is_loading(false);
                    }
                });
            });
        }
    });

    // Save new requests or update the currently selected request.
    app.on_save_request({
        let app_weak = app_weak.clone();
        let store = Arc::clone(&store);

        move |id, name, method, url, headers, params, body| {
            let app_handle = app_weak.clone();
            let store = Arc::clone(&store);

            let request = NewRequest {
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

                let list_result = store.list();

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

    // Load saved requests into the sidebar.
    app.on_load_requests({
        let app_weak = app_weak.clone();
        let store = Arc::clone(&store);

        move || {
            let app_handle = app_weak.clone();
            let store = Arc::clone(&store);

            tokio::task::spawn_blocking(move || {
                let result = store.list();

                let _ = slint::invoke_from_event_loop(move || {
                    if let Some(ui) = app_handle.upgrade() {
                        match result {
                            Ok(requests) => {
                                ui.set_saved_requests(to_request_model(requests));
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

    // Restore a saved request into the editor.
    app.on_select_request({
        let app_weak = app_weak.clone();
        let store = Arc::clone(&store);

        move |id| {
            if id <= 0 {
                return;
            }

            let app_handle = app_weak.clone();
            let store = Arc::clone(&store);

            tokio::task::spawn_blocking(move || {
                let result = store.get(id as i64);

                let _ = slint::invoke_from_event_loop(move || {
                    if let Some(ui) = app_handle.upgrade() {
                        match result {
                            Ok(Some(request)) => {
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
                            }
                            Ok(None) => {
                                ui.set_response_status("Request not found; refreshing list".into());
                                ui.invoke_load_requests();
                            }
                            Err(error) => ui.set_response_status(
                                format!("Could not load request: {error}").into(),
                            ),
                        }
                    }
                });
            });
        }
    });

    // Delete a saved request, then refresh the sidebar.
    app.on_delete_request({
        let app_weak = app_weak.clone();
        let store = Arc::clone(&store);

        move |id| {
            if id <= 0 {
                return;
            }

            let app_handle = app_weak.clone();
            let store = Arc::clone(&store);

            tokio::task::spawn_blocking(move || {
                let result = store.delete(id as i64);
                let list_result = store.list();

                let _ = slint::invoke_from_event_loop(move || {
                    if let Some(ui) = app_handle.upgrade() {
                        match result {
                            Ok(true) => {
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
                            }
                            Ok(false) => {
                                ui.set_response_status("Request was already deleted".into())
                            }
                            Err(error) => {
                                ui.set_response_status(format!("Delete failed: {error}").into())
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

    app.invoke_load_requests();
    app.run()
}
