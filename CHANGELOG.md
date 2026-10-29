# Changelog

Notable changes to Kynex. The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and versions follow [Semantic Versioning](https://semver.org/).

## [Unreleased]

### Added

- Closing the window keeps Kynex running, so turns and background tasks carry on: on
  Windows it goes to the notification area (click the icon to come back; its menu has
  Quit), on macOS the Dock brings it back. Turn it off in Settings → General. Ctrl+Q
  (Cmd+Q) quits. Linux closes as before.
- Keep awake (Settings → General): the computer doesn't idle-sleep while Claude works or
  background tasks run, or, if you pick it, whenever Kynex is open. The screen still
  turns off and locks.
- Sending a message shows "Starting Claude…" / "Waiting for Claude…" with the seconds
  until the first reply comes. Claude starts when you focus the message box or open a
  session, so sending no longer waits for it.
- Notifications (Settings → General, off by default): while Kynex is in the background, an
  OS notification says which session finished or needs your answer; clicking it opens
  that session. Kynex's icon shows a dot while any session needs you (taskbar on
  Windows, Dock on macOS). On Windows, `install.ps1` gives Kynex's shortcuts its app ID,
  and Kynex does it once for shortcuts from an older install; if Kynex is pinned to the
  taskbar from before, pin it again once.
- A running command with a time limit shows how long it has been going, "1m 12s / max 2m",
  counting up once a second; once it's done, how long it took.
- Syntax highlighting in code blocks, the diffs under edits, and the Git view, in every theme
  (Rust, TypeScript / JavaScript, Python, Go, C-family, shell, PowerShell, JSON, TOML, YAML,
  CSS, HTML and SQL).
- Scrollbars on the transcript, sessions list, timeline, settings, menus, prompts and the Git
  view. They stay out of the way: a thin bar that shows while you scroll or point at it.
  Drag one to scroll, or click the track to page.
- File paths in Claude's replies, like `src/app/mod.rs:123`, are links when the file exists:
  click to open it in VS Code at that line (or in the default app). Tool calls on a file
  open it from a button on hover.
- A turn now ends with a quiet rule giving its time and cost, and a chip for each file it
  edited; click one to see that file's changes in the Git view.
- Background tasks can be stopped: each one in the strip over the composer has Stop next
  to Watch.

### Changed

- Code blocks have a header row with the language, Copy and Run, so the buttons no longer
  cover the code. Long lines stay on one line and the block scrolls sideways: drag its
  scrollbar, use Shift+wheel, or drag a selection past its edge to select a whole line.

### Fixed

- Selected text in the transcript is easier to see, and the selection no longer has gaps
  over inline code.
- The "Claude is waiting for you in the browser" banner no longer shows when Claude only
  mentions a login or ends with "let me know when…", or after a background task's turn
  proposed something else. Its Continue message now only says the browser step is done,
  so it can't be read as a yes to another proposal.
- Opening a project or a past session no longer freezes the window. Session titles come
  from the ends of each transcript, read in the background, and a session's history loads
  off the UI thread: its newest turns show at once and older ones follow above them. A
  message sent while it loads waits in the queue.
- Open sessions no longer hold every screenshot in their history in memory. They stay
  in the session log and are read, as small thumbnails, when they scroll into view; the
  viewer reads the full image when one is opened.

## [0.0.8] - 2026-10-28

### Added

- Shortcuts for a new session (Ctrl+N / Cmd+N), the next and previous session in the
  project (Ctrl+PageDown / PageUp; Cmd+Shift+] / [ on macOS), and closing the project
  tab (Ctrl+W / Cmd+W; if Claude is working there, press it twice). All rebindable in
  `keymap.json`.
- A project can have its own model: "This project" at the top of the composer's model
  menu pins one, "Everywhere" goes back to the one for every tab. The chip shows a folder
  when the project's own model applies.
- Sessions can be renamed (Rename in a session's right-click menu, or double-click its
  name above the conversation) and pinned above the date groups (Pin to top). A blank
  name goes back to the generated one.

### Changed

- Only the session you open is named by Claude Haiku, so opening a project no longer
  spends quota naming old sessions. Naming the 10 most recent too is a setting (General
  → Session names).
- On wide windows, tool calls, diffs and their output grow to the right, up to 1040 px,
  lined up with the text, which keeps its readable width. A Transcript width setting
  (Appearance) offers Wide and Full width for big screens.

### Fixed

- Screenshots and other images in tool results no longer flash and disappear when Claude
  takes several in a turn.
- Clicking a menu item, toast or popup no longer also clicks what's underneath it.

## [0.0.7] - 2026-10-28

### Added

- Ungrouped sessions sit under date headers: Today, Yesterday, Previous 7 and 30 days,
  then months and years. Older ones start folded; a click folds or opens a header, and
  it's remembered per project.
- A search button in the sessions panel filters sessions by title.
- Thinking shows how long it took: it counts up while it runs, then reads "Thought for
  6s", and opens to the thinking text when Claude Code shares it.
- The usage meter in the title bar shows Fable's own weekly limit next to the 5-hour and
  weekly ones, once a Fable turn has reported it.

### Changed

- Notifications are one compact stack at the bottom right, above the composer, instead of
  toasts over the top-right controls. They close on their own (errors wait for you), pause
  while you point at them or are away, show three at a time, and several background tasks
  ending in one session merge into one.
- The sessions panel is quieter: rows show just their title (the time is in the tooltip)
  and a lamp only when a session is working, waiting or just finished. Claude settings
  moved to a shield in the toolbar.
- The session header shows the session's name, or its first few words (muted) until the
  name comes, instead of repeating the whole first message; the shown session is named
  first. Its context chip is just the percentage, with the tokens in the tooltip.
- Quieter while Claude works: the "Working…" strip above the composer and the stop square
  in the Stop button are gone; the lamps, Stop's spinner and the running step say it.
- A terminal whose shell exited shrinks to a one-line strip instead of keeping its full
  height; Enter or a click on it restarts the shell at the dock's saved height.
- The theme picker left the title bar: theme, fonts and motion are under Appearance in the
  Kynex menu, and in Settings.

### Fixed

- Scrolling the transcript while Claude runs commands no longer jitters or jumps back near
  the end, and new output is followed again once you scroll back down. A wheel turned over
  a code block scrolls the conversation instead of the code sideways.
- Smoother scrolling and streaming: no frame reads files or searches PATH any more (the
  composer's effort label, task output buttons, the CLI path in Settings), and the timeline
  only goes over the messages that changed instead of the whole session every frame.
- Finished turns no longer say "Thinking…" forever.
- Starting a new session or picking one in the sessions panel puts the cursor in the
  composer, or in the session's waiting prompt, so you can type right away.

## [0.0.6] - 2026-10-28

### Added

- Your Claude usage limits in the title bar: a small meter each for the 5-hour and weekly (7d)
  windows, amber from 80%, and "Limited · 2h 10m" once a limit is hit. The tooltip says when
  they reset. They're account-wide, update with every turn and are kept for the next start.
- An "Update" pill in the title bar while a Kynex or Claude Code update waits, until it's
  installed, even after the toast is closed. It opens About Kynex. It replaces the dot by the
  Kynex menu.
- An Extensions page (Kynex menu, or click the MCP dot in the session header) listing the
  folder's MCP servers with where each is configured (user, local, project, plugin, claude.ai),
  what it runs or connects to, and how it's doing, with a hint when one failed, needs signing in
  or waits for approval. A Plugins tab shows installed plugins and whether they're on.
- Add and remove MCP servers from the Extensions page, without a terminal: a one-click browser
  server for Edge or Chrome (your own profile through the Playwright extension, or a fresh
  browser), or any command or URL for this folder, the repo or every folder. "Check status"
  starts every server to show how it's doing when no session is open.

### Changed

- When Claude finishes a turn, the transcript glides to the end of it, even if you'd scrolled
  up mid-turn.
- Menus, modals, the image viewer, the sessions and terminal panels, the slash-command list and
  a closing prompt animate out as they close, as they did coming in. They take no clicks on the
  way out, and Animations off still removes them at once.
- Hover and selection backgrounds on buttons, menu items, session, commit and file rows fade
  instead of flipping.
- Opening a tool block or result, growing the composer, and folding a timeline turn ease to
  the new height instead of shoving everything below in one frame.

### Fixed

- The transcript no longer jumps back to the start of a reply (or an earlier message) on its
  own while a reply streams or the session's rows are rebuilt.
- Screenshots and images Claude reads show in the transcript even when their tool calls are
  folded into a run ("Ran 2 commands, read 2 files"); they were hidden until you opened it.

## [0.0.5] - 2026-10-27

### Added

- **Settings** (the gear in the title bar, Ctrl+, / Cmd+,), apart from a project's Claude
  settings: the `claude` CLI to use (found automatically, or one you pick, with the version it
  reports), the shell a new terminal opens, theme, fonts and animations, and a shortcut to
  `keymap.json`.
- **Comments on a yes**: Tab on a permission prompt's Yes opens a field in place, and what you
  type goes to Claude with the answer ("yes, but use the staging config"); Esc closes it.
- **Browser and MCP tools in words**: MCP calls read as what they do (`Edge · Click "Sign in"`,
  `Edge · Open https://…`) instead of `mcp__playwright-edge__browser_click {…}`, and a run of
  them folds to `Edge: 14 actions (3 screenshots)`. Their permission prompts ask the same way
  (`Click "Delete repository" in Edge?`) and offer **allow all Edge actions this session**.
- When Claude drives a browser and stops to ask you to log in, pass MFA or accept something, a
  banner above the message box says it's waiting for you, with the latest screenshot and a
  **Continue** button.
- An **MCP** dot in the session header: green when the session's MCP servers connected, amber
  while one connects or needs auth, red when one failed; hover to see each server's status.
- **Image viewer**: click an image in the transcript or an attachment in the message box to see
  it over the window, fitted; zoom with the wheel or + / − about the pointer, drag to pan, 0 to
  fit again and 1 for actual size, Esc or a click beside it to close. Removing an attachment is
  now the × that shows on hover.
- Images Claude gets from tools (a `Read` of a picture, a browser screenshot from an MCP server)
  show as thumbnails under the call instead of as text, and reopened sessions show the images
  you sent instead of "[N image(s)]".
- **Background tasks** (`run_in_background` commands, background agents, Monitors): the ones
  still running show over the message box with how long they've run, and **Watch** streams a
  task's output live in the terminal panel. When one ends, the transcript gets a row with its
  status, summary, run time and an **Output** button, the session lights up and the taskbar
  flashes as when a turn ends, and a toast says so if the session isn't shown (click it to go
  there). Reopened sessions show these rows too.
- **Jump to latest**: a button over the transcript when it's scrolled up, and in the timeline's
  header, glides back to the end and follows new output again.
- Timeline turns fold: a chevron opens or closes a turn's steps, and **Collapse all** /
  **Expand all** in the header does every turn. Until you choose, only the latest turn and the
  one in view are open.
- Scroll the conversation from the keyboard: Page Up / Page Down a page at a time, even while
  typing; Ctrl+Home / Ctrl+End (Cmd on macOS) to the start or the end, which follows new output
  again; and Home / End and ↑ / ↓ when the message box isn't focused. Rebind them with
  `page_up`, `page_down`, `scroll_top` and `scroll_bottom` in `keymap.json`. In the terminal
  these keys still go to the shell.
- A **New group** button in the Sessions header, beside **+**: it adds an empty group with its
  name open for editing; drag sessions into it or move them with the right-click menu.

### Changed

- The Kynex logo has one look, the dark mark, under every theme and in the README.
- Mouse-wheel scrolling glides everywhere, not just in the conversation: the sessions panel,
  timeline, Git view, settings, menus and terminal too. A notch also goes about twice as far,
  like a browser. Touchpads still scroll 1:1, and with animations off a notch moves at once.
- A streaming reply glides down as it grows instead of jumping a line at a time.
- Scrolling and animating a long conversation is lighter: a message's markdown is parsed once
  per change rather than on every frame.

### Fixed

- At 125% or 175% display scaling, the message box's caret and the terminal's cursor no longer
  change width as they move, and terminal lines are evenly spaced.

- macOS: Kynex opened from Finder or the Dock finds a `claude`, `pwsh` or `code` installed with
  Homebrew or npm; it takes PATH from your login shell, as editors do.
- macOS: Cmd+C / Cmd+V copy and paste in the terminal; Ctrl+C and Ctrl+V go to the shell.
- Shortcut hints in tooltips show the keys actually bound: ⌘ on macOS, and your `keymap.json`
  rebinds.
- Enter in an empty commit message no longer stages every change before failing.
- Closing a terminal tab keeps showing the same shell, and focuses it, instead of jumping to
  another; closing a project ends its shells.
- A permission prompt reads as cancelled once Claude stops or drops the request, instead of
  staying open with nothing to answer it.
- When `claude` exits with an error, the reason it gives is shown, not just "claude process exited".
- Session titles and the model list are saved safely: a crash mid-save no longer loses them.
- **Watch** / **Output** find a background task's output when its path contains ". "
  (e.g. a user folder named `J. Doe`).
- Switching away from a session, or changing a project's rules, no longer closes a session
  whose background tasks are still running (which stopped them).
- A reopened session no longer shows the CLI's background-task notices
  (`<task-notification>…`) as messages you sent.
- The transcript follows new output again after you glide back down to the end, instead of
  letting a growing reply run off the bottom.
- The timeline runs the full height of the window beside the transcript, with the message box
  under the transcript only.
- The Git view no longer crashes Kynex on unusual git output: file names with accented or other
  non-ASCII characters in certain places, very short status entries, or huge line counts and
  hunk numbers. Found by the new fuzz tests.

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

[Unreleased]: https://github.com/mrbergeron83/kynex/compare/v0.0.7...HEAD
[0.0.7]: https://github.com/mrbergeron83/kynex/compare/v0.0.6...v0.0.7
[0.0.6]: https://github.com/mrbergeron83/kynex/compare/v0.0.5...v0.0.6
[0.0.5]: https://github.com/mrbergeron83/kynex/compare/v0.0.4...v0.0.5
[0.0.4]: https://github.com/mrbergeron83/kynex/compare/v0.0.3...v0.0.4
[0.0.3]: https://github.com/mrbergeron83/kynex/compare/v0.0.2...v0.0.3
[0.0.2]: https://github.com/mrbergeron83/kynex/compare/v0.0.1...v0.0.2
[0.0.1]: https://github.com/mrbergeron83/kynex/releases/tag/v0.0.1
