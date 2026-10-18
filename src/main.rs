mod http;

use std::sync::Arc;

use slint::ComponentHandle;

use crate::http::HttpClient;

slint::include_modules!();

#[tokio::main]
async fn main() -> Result<(), slint::PlatformError> {
    let app = AppWindow::new()?;
    let app_weak = app.as_weak();

    let client = Arc::new(HttpClient::new().expect("Failed to initialize Resonance HTTP client"));

    app.on_send_request(move |method, url, body, headers, params| {
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
            let body = result.body;
            let elapsed = format!("{:.2} ms", result.elapsed.as_secs_f64() * 1000.0);

            let _ = slint::invoke_from_event_loop(move || {
                if let Some(ui) = app_handle.upgrade() {
                    ui.set_response_status(status.into());
                    ui.set_response_body(body.into());
                    ui.set_response_time(elapsed.into());
                    ui.set_is_loading(false);
                }
            });
        });
    });

    app.run()
}
