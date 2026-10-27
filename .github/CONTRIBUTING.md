# Contributing to Kynex

Thanks for helping. Bug reports and ideas are the most useful contributions right now.

## Issues first

Bugs and ideas live in [GitHub issues](https://github.com/mrbergeron83/kynex/issues), not TODO
files or code comments. Use the templates (bug or idea); include your OS and Kynex version
(Kynex menu → About) for bugs. Security problems go through the [security policy](SECURITY.md),
not public issues.

## Code contributions

Kynex is under the [PolyForm Strict License](../LICENSE), which doesn't allow changes or
derivative works without permission. So **open an issue before writing code**, and agree on the
change there; pull requests that weren't discussed may be closed.

## Building from source

You need a recent stable Rust toolchain and `claude` installed and logged in.

```sh
claude              # once, to log in
cargo run --release
```

On Linux, gpui needs a few system libraries:

```sh
sudo apt-get install -y libfontconfig-dev libfreetype-dev libwayland-dev libxkbcommon-x11-dev \
  libx11-xcb-dev libxcb1-dev libvulkan-dev libasound2-dev
```

### Hot-patching

Edit UI code and see it live without restarting, via [subsecond](https://crates.io/crates/subsecond):

```powershell
cargo install dioxus-cli@0.7.10 --locked   # once
./scripts/dev.ps1                          # dx serve --hot-patch, with auto-restart
```

Everything reached from `KynexApp::render` is patched on save, in both workspace crates;
third-party dependencies aren't patched. A compile error leaves the running app alone. Changing
a struct's fields (adding state) can't be hot-patched and crashes the app, so `dev.ps1` restarts
dx, which relaunches the app once a full build succeeds. Tabs and Claude sessions are restored.

Set `KYNEX_DATA_DIR` to run a test instance without touching your real state.

## Checks

**Done means `./scripts/check.ps1` passes**: `cargo fmt --check`, clippy with `-D warnings`, and every
test, built into `target/claude` so it never touches the dev build. `./scripts/check.ps1 -Fix` formats
and applies clippy's fixes first; `./scripts/check.ps1 -Core` checks only `kynex-core` (seconds).

`cargo test -p kynex-core -- --ignored` runs a live round-trip against the real CLI (uses a
little quota).

CI runs the same checks on Windows, macOS and Linux. The Security workflow runs
[cargo-deny](https://github.com/EmbarkStudios/cargo-deny) (`cargo deny check`, configured in
`deny.toml`) and [zizmor](https://github.com/zizmorcore/zizmor) on the workflows; CodeQL and
OpenSSF Scorecard run too. A new dependency must have a license listed in both `deny.toml` and
`packaging/about.toml`.

## Layout

A cargo workspace: `crates/kynex-core` is the logic with no UI (the `claude` protocol, sessions,
project settings, git, terminals), and `src/` is the gpui app. [CLAUDE.md](../CLAUDE.md) has the
full map and the conventions (where code goes, UI style, Rust 2024 notes).

Kynex must build and run on Windows, macOS and Linux: keep platform-specific code behind
`#[cfg(...)]` with a working fallback for the other OSes.

State is saved to `state.json` in the per-user data dir: `%APPDATA%\kynex` on Windows,
`~/Library/Application Support/kynex` on macOS, `~/.local/share/kynex` on Linux.

## Commits and releases

- Small commits with a sentence-case summary saying what changes for the user
  ("Show how full the context is, and compact it in one click"); `Fixes #N` closes an issue.
- Versions follow [SemVer](https://semver.org) and are tagged `vX.Y.Z`. To release: bump the
  version in `Cargo.toml`, add the section to [CHANGELOG.md](../CHANGELOG.md), commit, then push a
  matching tag on `main`; the Release workflow re-runs CI and cargo-deny on the tagged commit,
  then builds every OS and publishes the release. A tag that isn't on `main`, or fails a
  check, publishes nothing.
- Label pull requests (`enhancement`, `bug`, `security`, …): generated release notes are grouped
  by label.
