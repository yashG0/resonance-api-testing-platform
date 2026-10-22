# Resonance v1.0.0

**Release status: Preparation — publish only after release validation is complete.**

Resonance v1.0.0 is intended to be the first stable release of the native desktop API testing client.

## Highlights

- Native desktop interface built with Rust and Slint.
- HTTP request configuration and response inspection.
- Request collections, environment variables, and history.
- Local SQLite storage.
- Operating-system credential-store integration.
- Light and dark themes.

## Downloads

Release artifacts will be listed below once their builds and validation are complete.

| Platform | Artifact |
|---|---|
| Linux x86_64 | Linux archive or package |
| Windows x86_64 | Windows ZIP containing the executable |
| macOS | DMG or ZIP for each supported architecture |

Download: https://github.com/yashG0/resonance-api-testing-platform/releases

## Installation

### Linux

Download the Linux artifact, extract it if necessary, and follow the included instructions. Required system libraries may vary by distribution.

### Windows

Download and extract the Windows package, then run the included executable. Any required runtime dependencies will be documented with the artifact.

### macOS

Download the package matching your Mac's architecture. Installation, signing, notarization, and first-launch instructions will be documented with the published build.

## Validation

Before this release is marked stable, validate:

- Formatting, compilation, and automated tests.
- Optimized builds for all supported targets.
- Application launch and basic request workflows on each operating system.
- Data persistence and credential-store behavior.
- Packaged artifacts on clean environments.
- SHA-256 checksums and installation instructions.

## Known limitations

Platform-specific limitations and unverified behavior will be documented here before publication.

## Checksums

SHA-256 checksums will be published alongside the final release artifacts.
