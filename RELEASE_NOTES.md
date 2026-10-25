# Resonance v1.2.0

Resonance v1.2.0 improves request collection management and keyboard-driven workflows while retaining the response inspection and performance improvements from v1.1.0.

## Highlights

- Fresh installs start with the Public API collection; user-created collections remain available.
- Upgrades move saved requests and collection variables from the old built-in My Backend collection into Public API.
- Deleting a saved request closes its request tab and returns an empty collection to the empty editor state.
- Added application shortcuts for quitting, closing request tabs, and switching between request tabs.
- Application shortcuts are disabled while the About and shortcut dialogs are open.
- Shortcut labels use platform-appropriate modifiers on macOS, Linux, and Windows.
- Expandable, syntax-colored JSON Tree view, alongside Pretty and Raw views.
- Pretty is selected by default after a response arrives; HTML, XML, JSON, and other textual formats are detected and displayed appropriately.
- Slow requests show actionable timeout and connection errors with an animated in-flight state.
- Request tabs preserve response bodies when switching between requests, and opening a collection restores its most recently saved request.
- Response formatting and history updates run outside the UI thread; expanded JSON trees are capped to keep large responses responsive.

## Downloads and installation

| Platform | Artifact |
|---|---|
| Linux x86_64 | `resonance-1.2.0-linux-x86_64.tar.gz` |
| Windows x86_64 | `resonance-1.2.0-windows-x86_64.zip` |
| macOS Intel | `resonance-1.2.0-macos-x86_64.zip` |
| macOS Apple Silicon | `resonance-1.2.0-macos-aarch64.zip` |

Linux requires the system X11 or Wayland libraries used by Slint. On Debian/Ubuntu, install `libx11-xcb1`, `libxinput1`, `libxcursor1`, `libxkbcommon-x11-0`, `libx11-6`, and the corresponding Wayland libraries if needed.

The macOS application bundles are unsigned and not notarized. macOS may require using Finder's **Open** command once to approve the downloaded app.

SHA-256 checksums are included as `SHA256SUMS.txt` in the release.

## Validation notes

Linux x86_64 is built and checked locally. Windows and macOS artifacts are built on GitHub-hosted runners; the binaries have not been interactively exercised on those operating systems from this Linux environment.
