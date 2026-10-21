# Resonance

**A lightweight desktop API testing client built with Rust and Slint.**

Resonance started with a simple idea: I wanted an API testing tool that felt fast, reliable, and straightforward to use, with a local-first approach to storing my requests.

As a backend developer familiar with Python and Go, I wanted to build something useful while exploring Rust and desktop application development.

This project is my attempt to do both.

## Screenshots

<table>
  <tr>
    <td align="center"><strong>Dark Mode</strong></td>
    <td align="center"><strong>Light Mode</strong></td>
  </tr>
  <tr>
    <td><img src="assets/images/1.png" alt="Resonance running in dark mode" width="480"></td>
    <td><img src="assets/images/2.png" alt="Resonance running in light mode" width="480"></td>
  </tr>
</table>



## Why I Started This

Testing APIs is a regular part of backend development. I wanted a desktop tool that kept the workflow simple and didn't introduce unnecessary complexity.

My main goals are:

- **Speed:** A responsive desktop experience with minimal unnecessary overhead.
- **Reliability:** Predictable request handling and dependable local storage.
- **Local-first:** Keep saved requests and application data on the local machine.
- **Security-conscious:** Be deliberate about how request data is handled and avoid unnecessary data collection.
- **Simplicity:** Focus on the things I need when working with APIs.

These are the principles guiding the project, not claims that every goal has already been achieved.

## System Design

Resonance uses a desktop UI built with Slint, a Rust application layer, an HTTP client powered by Reqwest and Tokio, and SQLite for local persistence.

![Resonance system design](assets/images/resonance-system-design.png)

The architecture will evolve as the application grows. The diagram should be read alongside the current implementation, since some components and ideas may still be planned.

## Why Rust?

Python and Go are the languages I'm already familiar with from backend development. Choosing Rust was an opportunity to step outside my comfort zone and understand a different approach to building software.

While working on Resonance, I want to get more comfortable with:

- Ownership and borrowing.
- Error handling and resource management.
- Asynchronous programming.
- Building native desktop applications.
- Designing maintainable application architecture.

I'm not choosing Rust just because it's popular. I want to understand what it offers and where it makes sense for the kind of software I'm building.

## Built With AI Assistance

I'm being open about the development process: **Resonance is being built with AI assistance, including what people often call vibe coding.**

I use AI to explore implementation approaches, generate and refine code, troubleshoot compiler errors, and work through unfamiliar parts of the Rust ecosystem.

That doesn't mean I assume generated code is correct. I still need to test the application, understand the implementation, debug failures, and make decisions about how the pieces fit together.

For me, this project is about building a useful tool while learning through practical experience. I would rather be honest about the process than pretend every line was written without assistance.

## Current Features

### Requests and collections

- Create, rename, save, and delete requests organized into collections.
- Configure HTTP methods, URLs, query parameters, headers, request bodies, and None, Bearer, Basic, or API Key authentication.
- Define reusable variables for each collection and use them in templated requests.
- Review request history with status filters and sort order controls.

### Responses

- Inspect response status, elapsed time, body, headers, and cookies.
- View responses in Pretty or Raw mode. JSON Pretty mode includes syntax colors and line numbers.
- Wrap long response lines and copy the response body. Turn wrapping off to keep JSON syntax colors and line numbers in Pretty mode.
- Edit request bodies and format them as JSON, text, HTML, or XML.

### Desktop workflow

- Keep the request editor above the response panel by default, with a draggable divider to resize both areas.
- Collapse the navigation sidebar or switch to its compact icon rail.
- Choose System, Light, or Dark theme.
- Use the integrated GNOME/Adwaita-inspired title bar, application menu, About dialog, and keyboard shortcuts.

## Version 0.1.1 Highlights

- Refined the application frame, title bar, compact controls, tab alignment, and light and dark themes.
- Made the stacked request and response layout the default and removed the layout-cycling control while keeping the sidebar toggle.
- Fixed the request URL field so it updates when a different saved request is selected.
- Fixed line wrapping in Pretty JSON responses.
- Added the About dialog and improved the application menu and control tooltips.

## Downloads and Installation

Release archives and SHA-256 checksum files are available on the [latest GitHub release](https://github.com/yashG0/resonance-api-testing-platform/releases/latest).

### Linux x86-64

Download `resonance-v0.1.1-linux-x86_64.tar.gz` and extract it:

```bash
tar -xzf resonance-v0.1.1-linux-x86_64.tar.gz
./resonance
```

The archive contains the application executable only. Required Linux system libraries may need to be installed separately.

### Windows x86-64

Download and extract `resonance-windows-x86_64.zip`, then run `resonance.exe`. The executable and package are built by GitHub Actions; runtime behavior on Windows has not yet been verified.

### macOS Apple Silicon (ARM64)

Download and extract `resonance-macos-aarch64.tar.gz`, then run the `resonance` executable from the extracted folder. The package is built by GitHub Actions; runtime behavior on macOS has not yet been verified.

### Build from source on Linux

#### Requirements

- Rust and Cargo 1.92 or newer.
- A C/C++ build toolchain and the Linux libraries used by Slint's desktop backend.

On Debian or Ubuntu, install the system packages with:

```bash
sudo apt update
sudo apt install -y build-essential pkg-config libx11-xcb1 libx11-dev libxcb1-dev libxkbcommon0 libinput10 libinput-dev libgbm1 libgbm-dev
```

Install Rust with [rustup](https://rustup.rs/) if it is not already available. Then clone this repository and build the optimized release binary:

```bash
git clone https://github.com/yashG0/resonance-api-testing-platform.git
cd resonance-api-testing-platform
cargo build --release
```

Run the application with:

```bash
./target/release/resonance
```

The release profile enables thin LTO, one code generation unit, and symbol stripping. Building on the target machine selects its architecture automatically. Slint's Linux package requirements are documented in its [manual setup guide](https://docs.slint.dev/latest/docs/slint/guide/tooling/manual-setup/).

## Local-First, Not Offline HTTP

Resonance stores saved requests locally, helping keep request collections on your machine.

However, sending a request to a remote API still requires network connectivity. Local APIs can be tested without an internet connection when the local server is running and reachable.

Local storage is also not a complete security guarantee. Protecting sensitive data, handling credentials responsibly, and improving error handling remain important considerations as the project develops.

## What's Next?

Resonance is in early development. Next areas of work include:

- Broader runtime testing across Linux, Windows, and macOS.
- Improving diagnostics and handling for failed or unusual requests.
- Strengthening the handling of credentials and other sensitive local data.
- Exploring import and export for collections and requests.

## Technology Stack

| Technology | Purpose |
|---|---|
| Rust | Application logic |
| Slint | Desktop user interface |
| Reqwest | HTTP client |
| Tokio | Asynchronous execution |
| SQLite | Local persistence |

## Project Status

**Early development — actively evolving.**

Version 0.1.1 is available for Linux x86-64, Windows x86-64, and macOS Apple Silicon. Formatting, the test suite, and the optimized Linux build passed for this release; Windows and macOS CI builds also passed. Runtime behavior on Windows and macOS has not yet been verified.

Resonance is a personal project and a learning experience. I'm building it to solve a workflow problem of my own while improving my understanding of Rust, systems programming, and software design.

Feedback, suggestions, and constructive criticism are welcome.

---

*Built with Rust, curiosity, and AI-assisted development.*
