# Changelog

All notable changes to Bridge are documented here. The project follows
[Semantic Versioning](https://semver.org/).

## [Unreleased]

The next release line is `0.2.x`. This creates an unambiguous version boundary
between the published MIT-licensed `v0.1.0` release and Apache-2.0 builds from
current source.

### Changed

- Relicensed future Bridge distributions from the MIT License to the Apache
  License, Version 2.0. The previously published `v0.1.0` release remains
  available under the MIT License that accompanied that release.

### Added

- Native Windows and macOS CI coverage for formatting, tests, builds, and
  Clippy.
- Repository-local Windows and macOS application icons.
- Open-source contribution, security, review, and rectification guidance.
- Reproducible Node and Rust toolchain baselines, installer smoke builds, and
  complete lockfile-to-license-inventory checks.

### Security

- Updated the Tauri runtime to 2.11.5 and tauri-runtime-wry to 2.11.4.
- HTTPS-only AXAL endpoints with redirect blocking, bounded responses, and
  credential validation.
- Safer DSC PIN transport and PKCS#11 library discovery without exposing
  arbitrary native-library loading to the webview.
- Bounded Tally and document responses with endpoint and upload validation.

## [0.1.0] - 2026-07-12

### Added

- Initial open-source Bridge application with React, Rust, and Tauri support
  for Tally, GST, DSC, document, sync, and local database workflows.

[Unreleased]: https://github.com/lamemustafa/bridge/compare/v0.1.0...HEAD
[0.1.0]: https://github.com/lamemustafa/bridge/releases/tag/v0.1.0
