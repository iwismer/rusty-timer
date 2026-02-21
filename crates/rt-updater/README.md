# rt-updater

Self-update checker and downloader for rusty-timer services.

## Purpose

Checks GitHub Releases for newer versions of a given service, downloads and verifies release archives (SHA-256), and stages updated binaries for atomic replacement. Used by the forwarder and receiver to support over-the-air updates.

Release tags follow the format `{service}-v{semver}` (e.g. `forwarder-v0.2.0`).

## Key types

- **`UpdateChecker`** -- Main entry point. Constructed with a GitHub repo, service name, and current version. Provides:
  - `check()` -- Query GitHub Releases for a newer version.
  - `download(version)` -- Download, verify, and stage the release binary.
  - `apply_and_exit(staged_path)` -- Atomically replace the running binary and exit.
- **`UpdateStatus`** -- Tagged enum representing the result of an update cycle:
  - `UpToDate` -- No newer version available.
  - `Available { version }` -- A newer version exists.
  - `Downloaded { version }` -- The newer version has been staged.
  - `Failed { error }` -- The update check or download failed.
