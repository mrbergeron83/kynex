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
./dev.ps1                                  # dx serve --hot-patch, with auto-restart
```

Everything reached from `KynexApp::render` is patched on save, in both workspace crates;
third-party dependencies aren't patched.
A compile error leaves the running app alone. Changing a struct's fields (adding state)
can't be hot-patched and crashes the app, so `dev.ps1` restarts dx, which relaunches the
app once a full build succeeds. Tabs and Claude sessions are restored on relaunch.

`./check.ps1` runs formatting, clippy and every test. `cargo test -p kynex-core -- --ignored`
runs a live round-trip against the real CLI (uses a little quota).

## Layout

A cargo workspace: `crates/kynex-core` is the logic with no UI (the `claude` protocol,
sessions, project settings, git), and `src/` is the gpui app. See `CLAUDE.md` for a map.

State is saved to `state.json` in the per-user data dir: `%APPDATA%\kynex` on Windows,
`~/Library/Application Support/kynex` on macOS, `~/.local/share/kynex` on Linux.

## License

Kynex is free to use but source-available, not open source: it is licensed under the
[PolyForm Strict License 1.0.0](LICENSE.md), which allows personal and noncommercial use
but not modifying, redistributing or commercial use without permission. Third-party
notices are in [NOTICE.md](NOTICE.md).
