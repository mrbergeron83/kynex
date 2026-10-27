# Changelog

Notable changes to Kynex. The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and versions follow [Semantic Versioning](https://semver.org/).

## [Unreleased]

### Added

- Queue messages while Claude works, with Claude Code's keys: Enter queues them above the
  composer, and they all go out together when the turn ends, or when you stop it with Esc.
  **Send now** (Ctrl+Enter) stops Claude and sends them right away; ↑ from the first line
  takes them back to edit, and clicking one edits just that one.
- Shift+Tab on a permission prompt allows the action for the rest of the session, as in
  Claude Code.
- More of Claude Code's keys in the message box: ↑ / ↓ go through what you've sent in the
  project, Ctrl+R searches it, Ctrl+S stashes the draft (again on an empty box brings it
  back), Esc Esc clears it, Ctrl+G writes it in your editor (`$VISUAL` / `$EDITOR`, else
  Notepad / TextEdit), Ctrl+J adds a line, and Alt+P opens the model picker. Ctrl+Shift+O
  (Ctrl+O on macOS) opens every tool call in the transcript. All of them can be rebound in
  `keymap.json`.
- Rewind, as in Claude Code: Esc Esc on an empty message box (or `/rewind`) lists what you've
  sent, newest first; pick one to go back to just before it. Restore the conversation, the
  files Claude changed since, or both; the message comes back into the box to edit and send
  again. Sessions now keep file checkpoints so their files can be restored. Rebind it with
  `rewind` in `keymap.json`.

### Changed

- The slash-command popup is full on the first `/`, right after a restart too: each project's
  list is saved, and sessions refresh it as they start instead of Kynex running an extra
  `claude` process (and again every minute). The model picker's list is saved the same way.
- A quieter transcript: each tool result folds into its call (open the call to see it, and a
  failed call is marked), edits no longer echo "updated successfully", and a run of tool calls
  collapses to one summary row, e.g. "Edited app.rs ×3, read 2 files  +22 −6". While Claude
  works, the latest call shows under it.
- Prompts work like Claude Code's: a cursor over numbered choices instead of outlined rows.
  Several questions get a tab each (←→ or Tab) and a Submit tab that sums up the answers;
  picking an answer moves on by itself, multi-select has checkboxes (Space toggles), and
  "Type something" / "No, and tell Claude what to do differently" open their field in place.
- Releases are gated: the Release workflow publishes only if the tag is on `main`, CI passes
  on every OS for the tagged commit, and cargo-deny finds no advisories at release time.

### Fixed

- Restart after an update opens the new version on Windows instead of just closing Kynex.
- Question prompts show which option you picked: its row is highlighted, with a check mark
  for single-choice questions and a ticked box for multi-select.

## [0.0.3] - 2026-10-27

### Added

- Slash commands in the composer: typing `/` lists the session's commands (skills, plugin and
  project commands, built-ins) to pick with the keyboard. `/clear`, `/model`, `/effort` and
  `/resume` run in Kynex; commands show as a chip in the transcript.
- Session groups and an archive in the sessions panel: group sessions from their right-click
  menu or by dragging, collapse and rename groups, and archive sessions without deleting them.
- Check for Claude Code updates on demand, from the Kynex menu or the About modal, even a
  version you skipped; failed checks are shown, and the installed version can be copied.
- Tool calls with a time limit show it, e.g. "max 2m".
- The Kynex mark is lit and comes alive while Claude works.
- Security checks on every change and weekly: cargo-deny (RustSec advisories, licenses, sources),
  CodeQL, zizmor for the workflows, and OpenSSF Scorecard.
- Build provenance attestations for release archives.
- Contributing guide, security policy, code of conduct, and this changelog.

### Changed

- Licensed under GPL-3.0-or-later.

### Fixed

- Each session keeps its own unsent message; switching sessions no longer carries the draft
  along, and drafts survive a restart.
- Text on macOS: 0.0.2 drew its window but no glyphs, because gpui only installs a text
  system when its `font-kit` feature is enabled. Tab labels, the setup card and button
  captions are visible again.

## [0.0.2] - 2026-10-26

### Added

- Show how full the context is, and compact it in one click.
- `knx` command on Windows; `install.sh` for macOS and Linux.
- Kynex updates move out of the chat: a toast, a Kynex menu on the logo, and an About modal.

### Changed

- Child processes end with Kynex (a job object on Windows, `PR_SET_PDEATHSIG` on Linux).

## [0.0.1] - 2026-10-26

First public version.

### Added

- Claude Code sessions per project, driven over the CLI's stream-json protocol: streaming
  transcripts, tool calls, edits as diffs, selectable text, turn recaps and unread markers.
- Permission, plan and question prompts docked in the composer; permission mode, effort and
  extra directories per project.
- Git view: stage and commit, diffs, history, branches, fetch / pull / push, worktrees.
- Built-in terminal under the conversation, with play buttons on suggested shell commands.
- Install, sign in to and update Claude Code from inside Kynex.
- Self-update from GitHub releases; one-line Windows installer.
- Kynex brand: Instrument · Petrol themes, bundled fonts, app icon.
- Release workflow building Windows, macOS and Linux archives with third-party licenses.

[Unreleased]: https://github.com/mrbergeron83/kynex/compare/v0.0.3...HEAD
[0.0.3]: https://github.com/mrbergeron83/kynex/compare/v0.0.2...v0.0.3
[0.0.2]: https://github.com/mrbergeron83/kynex/compare/v0.0.1...v0.0.2
[0.0.1]: https://github.com/mrbergeron83/kynex/releases/tag/v0.0.1
