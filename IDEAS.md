# Ideas

## Fix soon

- The composer draft follows you across sessions: start typing in one session, click
  another, and the half-written text comes along. Each session should keep its own draft
  (restored when you switch back), and a session you haven't typed in should open with an
  empty composer.

## Later

- When a tool call's input JSON has a `timeout` (e.g. Bash, in ms), show it on the tool
  row in the transcript: "will run for max Xs".
- Background tasks (`run_in_background` Bash, background agents): let us "spy" on them —
  see that they're still running and their output so far — ideally by bringing them into
  the terminal panel so the output streams live.
