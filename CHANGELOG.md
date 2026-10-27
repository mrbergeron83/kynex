# Changelog

Notable changes to Kynex. The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and versions follow [Semantic Versioning](https://semver.org/).

## [Unreleased]

### Changed

- Mouse-wheel scrolling glides everywhere, not just in the conversation: the sessions panel,
  timeline, Git view, settings, menus and terminal too. A notch also goes about twice as far,
  like a browser. Touchpads still scroll 1:1, and with animations off a notch moves at once.

### Fixed

- A reopened session no longer shows the CLI's background-task notices
  (`<task-notification>…`) as messages you sent.

## [0.0.4] - 2026-10-27

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
- Each project has a colour, so you can tell at a glance which one you're in: it tints the title
  bar (with a line of it along the bottom), colours the project's tab icon, edges the message
  box, and names the project on a new conversation. It's picked from the folder automatically;
  choose another or turn it off by right-clicking the project's tab, or under Claude settings →
  Appearance.
- The branch chips above a conversation show what's changed there (`3 files +42 −7`) and how
  far the branch is ahead of / behind its upstream, refreshed as each tool call and turn ends
  and when you come back to Kynex. Clicking one opens that worktree's Git view in a modal over
  the conversation, with **Open as tab** to keep it.
- Things move instead of popping in: menus, modals, tooltips, the slash-command popup and
  prompts fade and settle into place, the sessions panel slides open, and switching sessions
  or to the Git view fades the new one in. The mouse wheel scrolls the conversation smoothly
  (touchpads stay 1:1), and sending a message glides down to the end. Short and time-based, so
  it looks the same at 60 and 240 Hz. Follows the system's "reduce motion" setting, or pick
  Animations on / off in the Appearance menu.
- A timeline beside the conversation (Ctrl+Shift+T, or the button in the toolbar): each message
  you sent, with its recap and the steps worth finding again — files edited, commands run,
  agents, plans, prompts and errors. Click one to glide there; the part you're reading is
  marked as you scroll, Alt+↑ / Alt+↓ go to the previous / next message, and **Rewind to
  here** on a message opens rewind on it.

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
- The Code / Git switch no longer jumps sideways when you change views: the sessions panel
  stays in the Git view, and picking a session there goes back to the conversation.

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

[Unreleased]: https://github.com/mrbergeron83/kynex/compare/v0.0.4...HEAD
[0.0.4]: https://github.com/mrbergeron83/kynex/compare/v0.0.3...v0.0.4
[0.0.3]: https://github.com/mrbergeron83/kynex/compare/v0.0.2...v0.0.3
[0.0.2]: https://github.com/mrbergeron83/kynex/compare/v0.0.1...v0.0.2
[0.0.1]: https://github.com/mrbergeron83/kynex/releases/tag/v0.0.1
