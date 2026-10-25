# Resonance

**A native desktop API testing client built with Rust and Slint.**

Resonance is a desktop application for creating, sending, and inspecting HTTP requests. It combines a modern interface with local data storage and an offline-friendly workflow.

## Features

- **HTTP requests** — Send requests using supported HTTP methods.
- **Request configuration** — Work with URLs, query parameters, headers, request bodies, and authentication.
- **Response inspection** — Inspect response bodies, headers, status codes, and timing.
- **Collections** — Organize and manage saved requests.
- **Environment variables** — Reuse variables across requests and collections.
- **Request history** — Review previous requests.
- **Local storage** — Store application data locally using SQLite.
- **Credential protection** — Use the operating system's credential store.
- **Native desktop UI** — Built with Rust and Slint, with light and dark themes.

## Download

Resonance v1.1.0 binaries are available on the GitHub Releases page for supported platforms.

**Platforms**
- Linux x86_64
- Windows x86_64
- macOS x86_64 (Intel) and aarch64 (Apple Silicon)

Download page: [GitHub Releases](https://github.com/yashG0/resonance-api-testing-platform/releases)

See the release notes for platform-specific installation instructions and known limitations.

## Build from source

### Requirements

- Rust toolchain
- Cargo
- Platform dependencies required by Slint and the application

### Build

```bash
git clone https://github.com/yashG0/resonance-api-testing-platform.git
cd resonance-api-testing-platform

cargo build --release
```

Run the resulting executable from `target/release/`.

## Development checks

```bash
cargo fmt --check
cargo check
cargo test
cargo build --release
```

## Technology

- Rust
- Slint
- Reqwest
- Tokio
- SQLite
- OS credential storage

## Project status

Resonance is transitioning toward its first stable release. Check the GitHub Releases page for available binaries, supported platforms, installation guidance, and release-specific limitations.

## License

See the repository's `LICENSE` file for licensing terms.
