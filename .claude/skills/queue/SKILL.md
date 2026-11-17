---
name: queue
description: Work through a list of GitHub issues one at a time, in this chat, handing each to subagents the user can watch, and ending in a pull request per issue. Use when the user runs /queue with issue numbers.
argument-hint: "<step numbers like 01-03, or issue numbers> [--no-pr] [--headless [--model haiku|sonnet|opus] [--budget <dollars>]]"
disable-model-invocation: true
---

# Queue

Arguments: `$ARGUMENTS`

Work through the issues **in this chat**, in the order given, so the user sees every step:
each piece of work goes to a subagent with the Agent tool, in the foreground, and you say in
one line what is happening before each one. Never start a hidden process for this. Only
`--headless` (last section) runs outside the chat.

You plan, brief and judge; the subagents type, build and review. Keep your own context
small: don't read whole files a subagent can read, and don't paste their reports back.

## Before starting

1. **Issues.** Take the numbers from the arguments. With none, ask which; never pick them
   yourself.
   - A number under 100, or written with a leading zero (`01`, `7`), is a **step number**:
     the issue whose title starts with that `[NN]` prefix. A range like `01-03` is every
     step in it. Resolve them with
     `gh issue list --state all --limit 200 --json number,title,state`, matching the
     `[NN]` prefix exactly (GitHub's search doesn't match brackets, so filter the list).
   - A larger number (`302`) is an issue number.
   - Steps always run in step order, lowest first, whatever order they were typed in. Say
     which issue each step resolved to before starting.
2. **Read each** with `gh issue view <N> --json number,title,state,body`. Drop closed ones
   and say so. A body may say `Depends on #X`: if `#X` is open and not earlier in this list,
   stop and tell the user.
3. **The tree must be clean** (`git status --short`; untracked files are fine). If it
   isn't, stop and say what is in the way.
4. Tell the user the order and which agent each issue goes to, then start.

## For each issue

Say `Issue #N: <title>` first, then:

1. **Branch.** `git fetch origin`, then `git switch -c issue-<N>` from `origin/main`, or
   from the previous issue's branch when this one depends on it.
2. **Brief.** Find the files and the exact change the issue needs (use Explore for the
   search), and write a brief a fresh agent can follow: files, change, what not to touch,
   and how it will be checked.
3. **Implement** with the agent for the issue's `Model:` line: `haiku` → `haiku-worker`,
   `sonnet` → `sonnet-worker`, `opus` or no line → do it yourself, handing the mechanical
   parts to the workers. A wide issue goes out as several briefs, one area at a time.
4. **Check** with `checker`: `./scripts/check.ps1` (`-Core` while iterating on a
   `kynex-core`-only change, the full one at the end). On failures, fix them or send a
   sharper brief to a stronger worker; don't resend the same task. At most three rounds.
5. **Review** with `reviewer`: give it the issue number and `git diff origin/main...HEAD`
   (or against the previous branch). Fix the real problems it reports, then check again.
6. **Ship.** Commit with the issue's title (without its `[NN]` prefix) as the message, and
   no AI attribution. Unless `--no-pr`: push the branch and `gh pr create` against `main`
   (or the previous issue's branch when stacked), with a few plain lines on what changed
   and `Fixes #N`.
7. **Report one line**: the PR link, or what stopped it.

## Rules

- Stop at the first issue that can't be finished: leave the work on its branch, say exactly
  what failed, and don't start the next one.
- Never merge, push to `main`, tag, or use `--admin`.
- Nobody may be watching: don't ask questions mid-queue. Decide, and put the decision in
  the PR description.
- When the queue ends, switch back to the branch you started on and list each issue with
  its PR or its failure.

## `--headless`

Only when asked for with `--headless`: run `scripts/queue.ps1` in a **visible** window
instead, for a night away from the chat. It does the same per issue in separate
`claude -p` sessions with a spending cap (`--budget`, default 5) and a default model
(`--model`, default `sonnet`; an issue's `Model:` line wins).

```powershell
Start-Process pwsh -ArgumentList '-NoProfile', '-NoExit', '-File', 'scripts/queue.ps1', '-Issues', '302,303', '-Budget', '5'
```

Add `-Stack` when an issue depends on another in the list. Tell the user that progress is
in that window and in `summary.md` under the folder named by `target/queue/latest.txt`, and
that the run stops if Kynex quits unless they start that line from their own terminal.
