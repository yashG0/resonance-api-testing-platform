<div align="center">
  <img src="assets/logo/resonance-logo.svg" alt="Resonance" width="92" />
  <h1>Resonance</h1>
  <p><strong>A native, local-first API client for building and testing HTTP requests.</strong></p>
  <p>
    <img alt="Rust" src="https://img.shields.io/badge/built_with-Rust-DEA584?logo=rust&logoColor=white" />
    <img alt="Slint" src="https://img.shields.io/badge/interface-Slint-2389DA" />
    <img alt="License: MIT" src="https://img.shields.io/badge/license-MIT-2EA66B.svg" />
  </p>
  <p>
    <a href="https://github.com/yashG0/resonance-api-testing-platform/releases">Download Resonance</a>
    ·
    <a href="#getting-started">Build from source</a>
    ·
    <a href="#architecture">Architecture</a>
  </p>
</div>

---

Resonance helps you create, send, inspect, and repeat API requests from a focused desktop workspace. Requests and collections stay on your machine, while HTTP traffic goes directly to the endpoint you choose.

## At a glance

<div align="center">
  <img src="assets/images/1.png" alt="Resonance request workspace in dark theme, showing request editing and a formatted JSON response" width="100%" />
  <br />
  <sub>Request workspace · dark theme</sub>
</div>

<div align="center">
  <img src="assets/images/2.png" alt="Resonance request workspace in light theme, showing request editing and a formatted JSON response" width="100%" />
  <br />
  <sub>The same workspace · light theme</sub>
</div>

## What you can do

- **Build HTTP requests** with GET, POST, PUT, PATCH, DELETE, HEAD, and OPTIONS methods.
- **Configure each request** with query parameters, headers, JSON or text bodies, and authentication.
- **Organize work** in collections and keep multiple request tabs open with their response state preserved.
- **Reuse values** with collection variables and environment configuration.
- **Inspect responses** with formatted, raw, and JSON tree views, plus status, timing, headers, and cookies.
- **Run collection regression checks** and review per-request outcomes and response details in a dedicated view.
- **Review request history** with filtering, sorting, and pagination.
- **Work locally** with SQLite storage and operating-system credential storage for saved secrets.
- **Choose your appearance** with system, light, and dark themes.

## Downloads

Get the latest release from [GitHub Releases](https://github.com/yashG0/resonance-api-testing-platform/releases). Release artifacts are published for:

| Platform | Architecture |
| --- | --- |
| Linux | x86_64 |
| Windows | x86_64 |
| macOS | Intel (x86_64) and Apple Silicon (aarch64) |

See [release notes](RELEASE_NOTES.md) for installation requirements and platform-specific notes. Linux builds require the X11 or Wayland libraries used by Slint. macOS application bundles are currently unsigned and not notarized.

## Getting started

### Build and run

You’ll need a recent Rust toolchain with Cargo, plus the native libraries required by Slint for your platform.

```bash
git clone https://github.com/yashG0/resonance-api-testing-platform.git
cd resonance-api-testing-platform
cargo run --release
```

To build without launching the app:

```bash
cargo build --release
```

The executable is written to `target/release/`.

### Development checks

```bash
cargo fmt --check
cargo check
cargo test
```

## Architecture

Resonance is a native desktop application. The Slint interface collects requests and presents results; Rust coordinates storage and HTTP work; SQLite stores local collections, requests, variables, and recent history. The app does not route API traffic through a Resonance server.

<div align="center">
  <img src="assets/images/resonance-system-design.png" alt="Resonance system design showing the user, Slint interface, Rust and Tokio coordinator, direct HTTP request flow, and local SQLite storage" width="100%" />
  <br />
  <sub>Request flow and local storage boundaries</sub>
</div>

### Technology

| Area | Technology |
| --- | --- |
| Desktop UI | [Slint](https://slint.dev/) |
| Application logic | Rust |
| Async runtime and HTTP | Tokio and Reqwest |
| Local persistence | SQLite via rusqlite |
| Saved credentials | Operating-system credential store |

## Project status

Resonance is under active development. Check the release page and [release notes](RELEASE_NOTES.md) for the current version, supported artifacts, and known platform limitations.

## License

Resonance is distributed under the terms in [`LICENSE`](LICENSE).
