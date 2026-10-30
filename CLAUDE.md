# Kynex

A native desktop workspace for Claude Code, written in Rust on gpui (Zed's UI framework,
from crates.io as `gpui-pre`). It drives the installed `claude` CLI over its stream-json
protocol, so no API key is needed.

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
- `crates/kynex-core/src/wsl.rs` and the `\\wsl.localhost` handling exist but are out of scope for now.
  Don't extend or debug WSL paths unless asked; just don't break them.

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

**Done means `./scripts/check.ps1` passes.** Run it before saying a change is finished; formatting
is `cargo fmt` (rustfmt.toml), not hand-tuned.

## Layout

A cargo workspace with two crates:

- **`crates/kynex-core`** — everything that isn't drawing. No gpui; plain Rust, unit-tested.
  - `session.rs` — a `claude` process and the stream-json protocol, session listing; reader
    threads ring a `Wake` callback and the app drains every session on one task
  - `live.rs` — `Live`: the open sessions in one folder, which is shown, which close
  - `cli.rs` — managing the `claude` CLI so users never need a terminal: finding it (PATH,
    then `~/.local/bin`), installing it with the official script, `claude auth login`, and
    update checks against npm dist-tags with `claude update`
  - `project.rs` — per-project settings folders (`<data dir>/projects/<name>-<hash>/`, one
    JSON file per section); rules go to each session via `claude --settings`, mode / effort /
    extra dirs via `--permission-mode`, `--effort`, `--add-dir`; the mode switches live, and
    effort and added rules / dirs reach a running process via `apply_flag_settings` (`Launched`)
  - `commands.rs` — slash commands: listing a folder's (the CLI's free `initialize`
    handshake), matching as you type, and the built-ins Kynex runs itself (`/clear`, `/model`…)
  - `history.rs` — sent messages per project for the composer's ↑ / ↓, Ctrl+R and stash;
    `queue.rs` — messages written while Claude works, sent together when the turn ends
  - `mcp.rs` — MCP tools and servers in words, server status, browser handoff;
    `tasks.rs` — background tasks (`run_in_background`, agents, Monitors) from stream-json
  - `attention.rs` — why a session wants you, for OS notifications and the icon's "needs
    you" dot (Dock badge on macOS); `win_shell.rs` — the taskbar dot and app ID on Kynex's
    shortcuts (Windows)
  - `zoom.rs` — the image viewer's fit, zoom and pan; `column.rs` — the Transcript width
    setting and the prose / tool-row widths it gives; `env_path.rs` — the login shell's PATH
    for an app started from Finder or the Dock (macOS)
  - `rewind.rs` — going back to an earlier message: the log's current branch, the points with
    their file checkpoints, restoring files (`--rewind-files`); the session resumes with
    `--resume-session-at`
  - `groups.rs` — session groups and the archive in the sessions panel (`sessions.json` per project)
  - `timeline.rs` — a session's turns and their notable steps, for the timeline panel;
    `transcript.rs` lays items out as transcript rows (tool runs folded) and maps between them
  - `markdown.rs` — markdown (pulldown-cmark) parsed into plain blocks the app draws
  - `motion.rs` — the Animations setting, the OS's "reduce motion", `Glide` (smooth,
    time-based scrolling) and `Wheel` (wheel notches as gliding pixel steps)
  - `git/` — everything git, parsed from machine formats: `status`, `diff` (hunks with line
    numbers), `log` and commit details, `branch` (list / switch / create), `ops` (stage, discard,
    commit, fetch / pull / push), `worktree`; `open.rs` — open a folder in VS Code / file
    manager / terminal; `attach.rs` — image attachments; `fmt.rs` — times and turn recaps;
    `paths.rs` — `data_dir()` and friends; `process.rs` — child processes die with Kynex (a
    job object on Windows, `PR_SET_PDEATHSIG` on Linux) unless `detach`ed, like editors it opens; `wsl.rs` — WSL paths (out of scope for now)
  - `terminal/` — interactive terminals on `alacritty_terminal` (ConPTY on Windows): spawning
    PowerShell / Git Bash, keys to bytes, render snapshots with resolved colours, selection;
    shells inherit Kynex's rights (`elevated()` says if that's administrator)
- **`kynex`** (`src/`) — the gpui app.
  - `main.rs` — app and window setup (custom title bar), key bindings, theme
  - `app/` — `KynexApp`, the root view. `app/mod.rs` has its fields, startup and top-level
    layout; every other file is one `impl KynexApp` block for one part of the window
    (the table at the top of `app/mod.rs` lists them: title bar, sessions panel, transcript,
    composer, prompts, menus, settings modal, setup screen…)
  - `prompt/` — `PromptView`: permission / plan / question prompts in the composer's place,
    Claude Code-style (cursor, numbered choices, a tab per question); `questions.rs` draws the
    questions, whose state is `kynex_core::questions::Form`; `preview.rs` has titles and diffs
  - `markdown.rs` — `kynex_core::markdown` blocks to gpui elements; parses are cached per
    message, so frames only draw
  - `git/` — the Git view (`GitView` entity): top bar with branch picker and fetch / pull / push,
    Changes tab (commit box, staged / unstaged files), History tab, diff viewer; the table at the
    top of `git/mod.rs` lists the files
  - `terminal/` — the terminal panel under the conversation (`TerminalPanel`, tabs of
    `TerminalView`s drawn by a custom GPU element); play buttons on the ```bash / ```powershell commands Claude suggests
    run their command there; Ctrl+` toggles it
  - `ui/` — our small component kit, Zed `ui`-crate style: buttons, tooltips, menus, icons
    (inline SVGs served as assets), spinner; `ui/input.rs` is the multi-line text field (IME,
    selection, clipboard, image paste) built on gpui's input example; `ui/selectable.rs` is
    mouse-selectable text across blocks; `ui/completions.rs` is the popup list of completions
    (slash commands); `ui/motion.rs` has the transitions (`fade_in`, `pop_in`…) on gpui's
    time-based `with_animation`, which `cx.reduce_motion()` turns off; `ui/smooth_scroll.rs`
    catches every mouse-wheel turn at the window root and replays it as per-frame pixel
    scrolls, so every scroll area glides without code of its own
  - `chrome.rs` — title bar drag area and window buttons (native snap on Windows via
    `WindowControlArea`, traffic lights on macOS, drawn buttons on Linux)
  - `theme.rs` — palettes as a gpui global (`theme(cx)`), mono font pick, project colours

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
4. The merge to `main` runs `.github/workflows/release.yml`: it sees a version with no tag
   yet, re-runs CI and cargo-deny, builds every platform, then publishes the release and
   creates `vX.Y.Z` on that commit. Check it with `gh run list --workflow release.yml`.
   A failed run publishes nothing and creates no tag; fix it in a PR, and the next push that
   touches `Cargo.toml` (or a re-run) tries again.
