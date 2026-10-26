# Kynex

A native desktop workspace for Claude Code, written in Rust (eframe/egui). It drives
the installed `claude` CLI over its stream-json protocol, so no API key is needed.

## Platform

- Kynex is **OS-agnostic**: it must build and run on Windows, macOS and Linux. Keep
  platform-specific code behind `#[cfg(...)]` with a working fallback for the other
  OSes; don't hard-code Windows paths, shells or tools in shared code.
- Development and testing happen on **native Windows only**. Use PowerShell / Windows
  tooling; don't reach for WSL.
- `src/wsl.rs` and the `\\wsl.localhost` handling exist but are out of scope for now.
  Don't extend or debug WSL paths unless asked; just don't break them.

## You are running inside Kynex

The dev app is started with `./dev.ps1` (`dx serve --hot-patch --features hotpatch`),
and this Claude session is usually a child process of that Kynex window. That means:

- **Every save under `src/` hot-patches the app hosting this conversation** (via
  subsecond; everything reached from `KynexApp::render` is patched live).
- **A compile error is harmless**: dx keeps the old app running.
- **Changing a struct's layout (adding/removing/reordering fields) can't be
  hot-patched and crashes the app**, cutting off the current turn. `dev.ps1` then does a
  full rebuild and relaunches only once it succeeds; tabs and sessions are restored.

Rules:

- Small edits to function bodies in the main tree are fine; they apply live.
- For anything that changes struct fields, adds state, or touches startup code
  (`main.rs`, `KynexApp::new`), **work in a git worktree on a branch**, build and verify
  there, and let the user merge and swap to the new version when it's ready.
- The build must never take down the running app. Don't kill `dx`/`kynex` processes,
  and don't build into the dx output directory. For checks in the main tree use a
  separate target dir, e.g. `cargo check --target-dir target/claude`.
- Make related edits so each saved state compiles, or batch them, to avoid patching the
  live app with half-finished changes.

## Commands

```powershell
./dev.ps1                                    # hot-patch dev loop (dx serve) with auto-restart
cargo run --release                          # plain release run
cargo check --target-dir target/claude       # type-check without touching the dev build
cargo test                                   # unit tests
cargo test -- --ignored                      # live round-trip against the real claude CLI (uses quota)
```

One-time setup: `cargo install dioxus-cli@0.7.10 --locked`, and `claude` logged in.

## Layout

- `src/main.rs` — window setup (frameless, transparent), subsecond hookup
- `src/app.rs` — `KynexApp`: projects sidebar, tabs, persisted state, `render` (hot-patch entry)
- `src/session.rs` — Claude CLI session process and stream-json protocol, session listing
- `src/project.rs` — per-project settings folders (`<data dir>/projects/<name>-<hash>/`, one
  JSON file per section); permissions are passed to each session via `claude --settings`
- `src/git.rs` — Git view (branch, changes, diffs, history)
- `src/chrome.rs` — custom Hyprland-style window frame
- `src/theme.rs` — palettes and fonts
- `src/wsl.rs` — WSL path handling (out of scope for now)

State persists to `state.json` in the per-user data dir (`data_dir()` in `app.rs`:
`%APPDATA%\kynex` on Windows, `~/Library/Application Support/kynex` on macOS,
`$XDG_DATA_HOME/kynex` or `~/.local/share/kynex` on Linux). Set `KYNEX_DATA_DIR` to run a test
instance (e.g. a worktree build) without touching the real state.

## Conventions

- Rust 2024 edition. Hot-patch code is gated behind the `hotpatch` feature; keep
  release builds free of subsecond/devtools.
- Dependencies are built with `opt-level = 2` in dev; our crate stays fast to compile
  and patch. Keep heavy logic out of `render` paths that would slow patching.
