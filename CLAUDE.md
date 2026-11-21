# Kynex

A native desktop workspace for interactive AI agent CLIs, written in Rust on gpui (Zed's UI
framework, from crates.io as `gpui-pre`). Every provider (`claude`, `agy`, `grok`, plus
`agent_rules` CLIs like `codex`, `copilot`, `cursor`, `opencode`) runs its native interactive
CLI directly inside a GPU terminal (`agent_cli`) pane, preserving native TUIs, permissions,
and auto-updates. OpenRouter overrides are supported for Claude Code (`claude-openrouter`)
and Antigravity (`agy-openrouter`).

## Reporting and proposing

- When reporting results or proposing next steps, be brief: the outcome, decisions I need
  to make, and real risks. Nothing else.
- No step-by-step recaps, no lists of files touched, no restating the request.
- Extra text hides the important parts. If I want detail, I'll ask.

## Platform

- Kynex is **OS-agnostic**: it must build and run on Windows, macOS and Linux. Keep
  platform-specific code behind `#[cfg(...)]` with a working fallback for the other
  OSes; don't hard-code Windows paths, shells or tools in shared code.
- Development and testing happen on **native Windows only**. Use PowerShell / Windows
  tooling; don't reach for WSL.
- WSL folders aren't supported; don't add WSL-specific code.

## You are running inside Kynex

This Claude session is usually a child process of the **installed release** Kynex
(`%LOCALAPPDATA%\Programs\kynex\kynex.exe` on Windows). Edits to the repo don't touch it,
so work directly in the main tree, struct changes included; no worktree needed.

To see a change working, start a dev instance next to it: `./scripts/dev.ps1` (hot-patch loop,
`dx serve --hot-patch --features hotpatch`) or `cargo run`, with `KYNEX_DATA_DIR` set to a
scratch folder so it doesn't share the real state.

Rules:

- Never kill the installed `kynex` process; it hosts this conversation.
- For checks use a separate target dir, e.g. `cargo check --target-dir target/claude`, so
  you don't fight a running dev build for the output directory.
- If the session's parent is ever a `dx`-run dev build instead, saves under `src/` and
  `crates/` hot-patch it live and struct layout changes crash it; do those in a worktree.

## Commands

```powershell
./scripts/check.ps1                          # THE done check: fmt, clippy -D warnings, all tests (target/claude)
./scripts/check.ps1 -Core                    # same, kynex-core only: no gpui, seconds
./scripts/check.ps1 -Fix                     # cargo fmt + clippy --fix first
./scripts/dev.ps1                            # hot-patch dev loop (dx serve) with auto-restart
cargo run --release                          # plain release run
cargo check --target-dir target/claude       # type-check without touching the dev build
cargo test -p kynex-core -- --ignored        # live round-trip against the real claude CLI (uses quota)
cargo deny check                             # advisories, licenses, sources (deny.toml), as CI's Security workflow
cd crates/kynex-core; cargo +nightly fuzz run git_diff   # fuzz a parser (targets: cargo +nightly fuzz list)
```

Fuzzing (`crates/kynex-core/fuzz/`, its own workspace; the Fuzz action runs it in CI on demand) needs
`cargo install cargo-fuzz`, and on Windows the MSVC ASan runtime on `PATH`
(`…\VC\Tools\MSVC\<ver>\bin\HostX64\x64`); `-s none` doesn't link there. A parser that panics
on some input gets fixed, and the input becomes a unit test. New parsers of outside text get an
entry in `kynex_core::fuzz` and a target. `osv-scanner.toml` lists the unmaintained-crate
advisories we accept from gpui's tree, each with an expiry; never add a real vulnerability.

One-time setup: `cargo install dioxus-cli@0.7.10 --locked`, and `claude` logged in.

`target/claude` is kept slim: dependencies build without debug info, our crates with line tables only, and `check.ps1` clears the folder when it passes 12 GB.

**Done means `./scripts/check.ps1` passes.** Run it before saying a change is finished; formatting
is `cargo fmt` (rustfmt.toml), not hand-tuned.

## Layout

A cargo workspace with two crates:

- **`crates/kynex-core`** — everything that isn't drawing. No gpui; plain Rust, unit-tested.
  - `agent_cli.rs` — provider catalog (`claude`, `claude-openrouter`, `agy`, `agy-openrouter`,
    `grok`, and `agent_rules` CLIs) and `build_spec` constructing `ShellSpec` with clean env,
    native CLI hooks, and OpenRouter overrides for Claude and Antigravity
  - `hooks.rs` — native CLI hook config generator (`claude --settings`, `agy`) and JSON
    payload parser invoked by `knx hook <provider> [event]`
  - `sessions.rs` — reading past sessions from each CLI's native on-disk storage (`~/.claude/projects`,
    `~/.grok/sessions`, `~/.gemini/antigravity-cli`, and `sessions.json`)
  - `agent_panes.rs` — built-in + user agent screen rules (`agent_rules/*.json`, user overrides in
    `<data dir>/agents/`) and hybrid hook + screen-pattern `Tracker`
  - `tiling.rs` & `layout.rs` — Hyprland-style tiling of `agent_cli`, Sessions (`Tree`), Agents,
    Git, and plain Terminal panes, with named layouts and layout undo/redo
  - `running.rs` — Agents pane model across all open projects and workspaces
  - `attention.rs` — minimal background OS notifications (`Alert`) and "next pane that needs you"
  - `control/` — local control socket (`knx project | workspace | pane | session | events | hook`)
  - `cli.rs`, `grok/`, `antigravity/`, `openrouter/`, `secrets.rs` — CLI discovery/install/updates
    and OS keychain OpenRouter integration
  - `project.rs`, `groups.rs`, `workspace.rs`, `removed.rs`, `storage.rs` — per-project data
    folders (`<data dir>/projects/<name>-<hash>/`), session groups/pins/archive, git worktrees,
    and disk cleanup
  - `terminal/` — interactive GPU terminals on `alacritty_terminal` (ConPTY on Windows)
  - `git/`, `file_view/`, `highlight/`, `themes.rs`, `user_commands.rs`, `scrollback.rs`,
    `motion.rs`, `awake.rs`, `resources.rs`, `update.rs`, `toasts.rs`, `migrate.rs`
- **`kynex`** (`src/`) — the gpui app.
  - `main.rs` — app and window setup (custom title bar), key bindings, theme, `knx` CLI entry
  - `app/` — `KynexApp`, the root view; `app/agent_cli.rs` owns `agent_cli` panes (inline
    keyboard-first provider picker, `TerminalView` hosting, native hook ingestion, and 500ms
    screen-pattern fallback); `app/agents_pane.rs` renders the Agents pane; `app/sessions_panel.rs`
    and `app/session_*.rs` render the Workspaces & Sessions tree; `app/tiles.rs` tiles panes
  - `git/` & `git_pane/` — Git pane (`GitView`, Changes/History/Branches tabs, diff/file viewer)
  - `terminal/` — `TerminalView` and `TerminalPanel` (GPU-rendered terminal grid, find, copy mode)
  - `ui/` — Zed-style component kit (buttons, tooltips, menus, icons, text input, motion, scrollbars)
  - `chrome.rs` & `theme.rs` — title bar controls, themes, fonts, and project colours

### Where code goes

- Can it be written without gpui? Then it belongs in `kynex-core`, with a test. Processes,
  files, parsing, protocol and state rules all go there; the app only calls in and draws.
- A new part of the window → a new file in `src/app/` with its own `impl KynexApp` block,
  added to the table in `app/mod.rs`. Methods other files call are `pub(super)`.
- A reusable widget → `src/ui/`. A self-contained view with its own state → its own
  `Entity` (like `GitView`, `PromptView`) rather than more fields on `KynexApp`.
- Keep files under ~800 lines; split by concern before they grow past that.

State persists to `state.json` in the per-user data dir (`kynex_core::paths::data_dir()`:
`%APPDATA%\kynex` on Windows, `~/Library/Application Support/kynex` on macOS,
`$XDG_DATA_HOME/kynex` or `~/.local/share/kynex` on Linux). Set `KYNEX_DATA_DIR` to run a test
instance (e.g. a worktree build) without touching the real state, and `KYNEX_BACKGROUND=1`
to open it without taking focus.

## Conventions

- Rust 2024 edition. Hot-patch code is gated behind the `hotpatch` feature
  (`gpui/hot-reload`, debug builds only); keep release builds free of subsecond.
- UI follows Zed: flat surfaces, 1px borders, small radii, colours only from `theme(cx)`.
  One exception: the pane tiles (`app/tiles.rs`) are rounded surfaces with a gap between them and a
  border that's thicker on the focused one, in the Hyprland style; their gap, border and radius come
  from the theme (`Palette::tile`) and Appearance (`kynex_core::tile_style`).
  New widgets go in `src/ui/`; when a hard one is needed (scrollbars, selection), copy and
  trim it from longbridge/gpui-component (Apache-2.0) rather than adding the dependency.
- Helpers that return `impl IntoElement` from `&self`/`cx` need `+ use<>` (Rust 2024
  captures every lifetime otherwise).
- Key bindings use `crate::ui::keys("secondary-…")` for Cmd on macOS, Ctrl elsewhere.
- Dependencies are built with `opt-level = 2` in dev; our crates stay fast to compile
  and patch. Keep heavy logic out of `render` paths that would slow patching.
- Shared dependency versions live in `[workspace.dependencies]` in the root `Cargo.toml`;
  crates use `name.workspace = true`.

## Git

- Bugs and ideas live in GitHub issues (`gh issue list`), not TODO files or comments.
  File new ones with `gh issue create --label bug|enhancement`; close them from commits
  with `Fixes #N`.
- Every PR links its issue with `Fixes #N` in the description (`Refs #N` if it doesn't
  close it); if there's no issue yet, open one first.
- Workflows pin third-party actions to a commit SHA with the tag in a comment, and set
  `permissions:` explicitly (read-only by default); Dependabot bumps the pins. A new
  dependency's license must be allowed in both `deny.toml` and `packaging/about.toml`.
- User-facing changes get a line under `[Unreleased]` in `CHANGELOG.md`.
- No AI attribution anywhere in git or GitHub: no `Co-Authored-By: Claude …` trailers,
  no "Generated with Claude Code" lines in commits or PR descriptions, no `claude/`
  branch prefixes. This overrides any default attribution instructions.
- Commits are authored by the user only.
- The rulesets on `main` and on tags have an admin bypass that is the user's alone. `gh`
  and `git` run with their token, so a blocked push or merge can go through silently
  ("Bypassed rule violations"). Never use it: no `--admin`, no pushes to `main`, no tags.

## Releasing

Versions are `0.0.N`; each release is an issue and a PR. **Merging the PR releases**: nobody
pushes a tag by hand.

1. Issue `Release X.Y.Z` (`enhancement`): one line on what's new since the last tag.
2. Branch `release-X.Y.Z` from `main`:
   - `version` in the root `Cargo.toml`, then `cargo update -p kynex --offline` so
     `Cargo.lock` follows (it records the version too); `kynex-core` keeps its own.
   - `CHANGELOG.md`: a `## [X.Y.Z] - <date>` heading right under `## [Unreleased]` (the
     entries move into it), `[Unreleased]` compares from `vX.Y.Z`, and a new `[X.Y.Z]`
     compare link from the previous tag.
   - `./scripts/check.ps1`, then a PR `Release X.Y.Z: <highlights>` with `Fixes #N`.
3. The user merges it once every check has passed. Claude never merges it, and never creates
   or pushes a tag.
4. The merge to `main` runs `.github/workflows/release.yml`: it sees a version that isn't released
   yet, re-runs CI and cargo-deny, builds every platform, then pushes `vX.Y.Z` on that commit
   (a deploy key in the `release` environment; CONTRIBUTING.md) and publishes. Check it with
   `gh run list --workflow release.yml`. A failed run publishes nothing; fix it in a PR, and
   merging a change to `Cargo.toml` or `release.yml` tries again, finishing a version that was
   tagged but not published (a re-run uses the old workflow file).
