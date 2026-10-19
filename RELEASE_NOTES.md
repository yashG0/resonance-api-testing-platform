# Resonance v0.1.0

The first tagged release of **Resonance**, a lightweight desktop API testing client built with Rust and Slint.

## Features

- Send HTTP requests and inspect responses.
- Configure request methods and URLs.
- Add request headers, query parameters, and request bodies.
- Inspect HTTP status codes, response times, and response bodies.
- Save, load, update, and delete requests using SQLite.
- Switch between dark, light, and system themes.

## Downloads

### Linux x86-64

Download `resonance-v0.1.0-linux-x86_64.tar.gz` and extract it:

```bash
tar -xzf resonance-v0.1.0-linux-x86_64.tar.gz
./resonance
```

The archive contains the application executable only. Required Linux system libraries may need to be installed separately.

Verify the download with the checksum file in the same directory:

```bash
sha256sum -c resonance-v0.1.0-linux-x86_64.tar.gz.sha256
```

### Windows x86-64

Download `resonance-windows-x86_64.zip` and extract it. The archive includes `resonance.exe`, `LICENSE`, and `README.md`.

The executable was built successfully using GitHub Actions, but runtime testing on a Windows machine has not yet been performed.

### macOS Apple Silicon (ARM64)

Download `resonance-macos-aarch64.tar.gz` and extract it:

```bash
tar -xzf resonance-macos-aarch64.tar.gz
cd resonance-macos-aarch64
./resonance
```

The executable was built successfully using GitHub Actions, but runtime testing on a macOS machine has not yet been performed.

Verify the macOS archive with its checksum file:

```bash
shasum -a 256 -c resonance-macos-aarch64.tar.gz.sha256
```

### Checksums

Each downloadable archive has a corresponding `.sha256` file. Keep the archive and its checksum file in the same directory when verifying downloads.

## Project Status

Resonance is in early development. Linux compatibility with other distributions and environments has not been established. Windows and macOS builds have been compiled by GitHub Actions, but their runtime compatibility remains unverified.

Resonance was developed with AI assistance.

Feedback and constructive criticism are welcome.
