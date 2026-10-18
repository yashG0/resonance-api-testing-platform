mod http;
mod storage;

use std::sync::Arc;

use slint::ComponentHandle;

use crate::http::HttpClient;
use crate::storage::{NewRequest, RequestStore};

slint::include_modules!();

#[tokio::main]
async fn main() -> Result<(), slint::PlatformError> {
    let store = Arc::new(
        RequestStore::open_default().expect("Failed to initialize Resonance request database"),
    );

    let app = AppWindow::new()?;
    let app_weak = app.as_weak();

    let client = Arc::new(HttpClient::new().expect("Failed to initialize Resonance HTTP client"));

    app.on_send_request({
        let app_weak = app_weak.clone();

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

    app.on_save_request({
        let app_weak = app_weak.clone();
        let store = Arc::clone(&store);

        move |name, method, url, headers, params, body| {
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
                let result = store.create(&request);

                let message = match result {
                    Ok(saved) => format!("Saved: {}", saved.name),
                    Err(error) => format!("Save failed: {error}"),
                };

                let _ = slint::invoke_from_event_loop(move || {
                    if let Some(ui) = app_handle.upgrade() {
                        ui.set_response_status(message.into());
                    }
                });
            });
        }
    });

    app.run()
}
