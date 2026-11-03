# Design notes

Cosmic API is a local desktop client for composing and running HTTP requests. Rust owns request execution, persistence, credential handling, and response processing. Slint provides the desktop interface.

## Request flow

1. The UI holds the active request draft and sends a typed `RequestSpec` to the Rust HTTP client.
2. Collection variables are interpolated before the request is sent. Secret variables are retrieved from the operating-system credential store; their values are not stored in SQLite.
3. A shared `reqwest::Client` sends traffic directly to the selected endpoint. The client applies timeouts, limits the response body, and prevents custom credentials from crossing origins on redirects.
4. The result retains the numeric HTTP status, response headers, body, and transport outcome. The UI presents the response and can render JSON as formatted text or a tree.
5. Collection runs evaluate saved assertions against each response. Captured JSON Pointer values are available to later requests in that run. Retry and stop-on-failure options control desktop execution; the headless runner uses the same evaluator, can emit JUnit XML, and can repeat the collection for each row in a bounded JSON data file.

## Local data

SQLite stores collections, requests, non-secret variables, assertion definitions, and capped request history. Request authentication material and variables explicitly marked secret are stored through the operating-system credential vault. History redacts common sensitive URL query parameters before persistence. The database and other request fields remain local plaintext, so the user should protect the local account and backups.

## Trade-offs

- **Slint:** a compact native UI framework that lets the project share a Rust-first implementation across desktop platforms. The current UI is still concentrated in a large Slint file; view extraction remains future work.
- **Window decorations:** GNOME sessions use the app's custom client-side frame; other desktops use their native window frame so operating-system controls and window-management behavior remain available.
- **Fonts:** the UI and code views request the platform's generic system UI and monospace families, avoiding a runtime dependency on Adwaita Sans or JetBrains Mono being installed.
- **SQLite:** simple, transactional local persistence with schema migrations. It avoids a service dependency, but collections are not yet represented as Git-friendly files.
- **Direct HTTP:** requests go from the app to the configured endpoint without a Cosmic API server. The app currently focuses on HTTP and does not yet provide OAuth, GraphQL-specific editing, WebSocket, or SSE workflows.
- **Operating-system credential storage:** secrets are kept out of the database. Saving a secret depends on an available, functioning platform credential service; the app does not silently fall back to plaintext.
- **reqwest and rustls:** a shared async HTTP client provides connection reuse and a portable TLS implementation. Detailed DNS/connect/TLS timing breakdowns are not currently exposed.

## Current architecture limits

`src/main.rs` and `ui/app-window.slint` still contain substantial application wiring and view logic. The request runner is separated, and request/assertion/storage/HTTP logic has unit coverage. The headless runner is currently part of the desktop binary and uses the app's local database; `--env` overlays variables from a selected collection because named environments are not yet a separate stored model. See the roadmap in the project issue tracker before treating this design as a finished architecture.
