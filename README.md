# Kynex

A fast, native workspace for Claude Code, written in Rust.

Kynex drives your installed `claude` CLI over its stream-json protocol, so it
uses whatever account `claude` is logged into. No API key needed.

## Features (milestone 1)

- **Projects** sidebar: add folders (button or drag-and-drop), open in VS Code / Explorer / Windows Terminal
- **Code**: one Claude Code agent session per project, with streaming output, tool calls, and permission prompts as buttons
- **Chat**: general conversation (same login, tools disabled)
- **Git**: branch, changed files, diffs, recent history
- Paste images with Ctrl+V, drop image files, or use "Attach image"
- Model picker, Stop (interrupt), New session, running cost

## Run

```sh
claude          # once, to log in
cargo run --release
```

### Hot-patching (dev)

Edit UI code and see it live without restarting, via [subsecond](https://crates.io/crates/subsecond):

```sh
cargo install dioxus-cli@0.7.10 --locked   # once
dx serve --hot-patch --features hotpatch
```

Everything reached from `KynexApp::render` is patched on save. Changing a struct's fields
(adding state) still needs a restart; dependencies aren't patched.

`cargo test -- --ignored` runs a live round-trip against the real CLI (uses a little quota).

State is saved to `%APPDATA%\kynex\state.json`.
