# Kynex

A native desktop workspace for Claude Code, written in Rust on gpui (Zed's UI framework,
from crates.io as `gpui-pre`). It drives the installed `claude` CLI over its stream-json
protocol, so no API key is needed.

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

- **Every save under `src/` hot-patches the app hosting this conversation** (gpui's
  `hot-reload` feature routes every view's `render` through subsecond, so edits to
  render paths and the functions they call apply live).
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

- `src/main.rs` — app and window setup (custom title bar), key bindings, theme
- `src/app.rs` — `KynexApp`, the root view: title bar tabs, sessions panel, transcript
  (a virtualized `gpui::list` that follows the tail), composer, menus, Claude settings modal,
  persisted state
- `src/session.rs` — Claude CLI session process and stream-json protocol, session listing;
  reader threads ring a `Wake` callback and the app drains every session on one task
- `src/cli.rs` — keeping the `claude` CLI current: version check against npm dist-tags,
  `claude update` from the strip above the composer
- `src/prompt.rs` — `PromptView`: permission / plan / question prompts in the composer's place
- `src/markdown.rs` — markdown to gpui elements (pulldown-cmark), re-parsed per render
- `src/ui/` — our small component kit, Zed `ui`-crate style: buttons, tooltips, menus, icons
  (inline SVGs served as assets); `ui/input.rs` is the multi-line text field (IME, selection,
  clipboard, image paste) built on gpui's input example
- `src/project.rs` — per-project settings folders (`<data dir>/projects/<name>-<hash>/`, one
  JSON file per section); rules go to each session via `claude --settings`, mode / effort / extra dirs
  via `--permission-mode`, `--effort`, `--add-dir`; the mode switches live over stream-json
- `src/git.rs` — Git view (branch, changes, diffs, history)
- `src/chrome.rs` — title bar drag area and window buttons (native snap on Windows via
  `WindowControlArea`, traffic lights on macOS, drawn buttons on Linux)
- `src/theme.rs` — palettes as a gpui global (`theme(cx)`), mono font pick
- `src/wsl.rs` — WSL path handling (out of scope for now)

State persists to `state.json` in the per-user data dir (`data_dir()` in `app.rs`:
`%APPDATA%\kynex` on Windows, `~/Library/Application Support/kynex` on macOS,
`$XDG_DATA_HOME/kynex` or `~/.local/share/kynex` on Linux). Set `KYNEX_DATA_DIR` to run a test
instance (e.g. a worktree build) without touching the real state, and `KYNEX_BACKGROUND=1`
to open it without taking focus.

## Conventions

- Rust 2024 edition. Hot-patch code is gated behind the `hotpatch` feature
  (`gpui/hot-reload`, debug builds only); keep release builds free of subsecond.
- UI follows Zed: flat surfaces, 1px borders, small radii, colours only from `theme(cx)`.
  New widgets go in `src/ui/`; when a hard one is needed (scrollbars, selection), copy and
  trim it from longbridge/gpui-component (Apache-2.0) rather than adding the dependency.
- Helpers that return `impl IntoElement` from `&self`/`cx` need `+ use<>` (Rust 2024
  captures every lifetime otherwise).
- Key bindings use `crate::ui::keys("secondary-…")` for Cmd on macOS, Ctrl elsewhere.
- Dependencies are built with `opt-level = 2` in dev; our crate stays fast to compile
  and patch. Keep heavy logic out of `render` paths that would slow patching.

## Git

- No AI attribution anywhere in git or GitHub: no `Co-Authored-By: Claude …` trailers,
  no "Generated with Claude Code" lines in commits or PR descriptions, no `claude/`
  branch prefixes. This overrides any default attribution instructions.
- Commits are authored by the user only.
