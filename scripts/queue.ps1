# Works through GitHub issues unattended, one at a time: each issue gets a fresh
# headless `claude -p` session (a new context, so cost doesn't pile up), a
# spending cap, and ./scripts/check.ps1 as the "is it done?" gate. A passing
# issue is committed, pushed as branch issue-<N> and opened as a PR. The queue
# stops at the first issue that fails, so nothing builds on broken work.
#
# It never merges, never pushes to main and never tags. It works in its own
# worktree (a sibling folder, reused between runs so cargo's build is too).
#
# Usage: ./scripts/queue.ps1 -Issues 302,303,311
#        ./scripts/queue.ps1 -Issues 303,305 -Stack     305 branches from 303's branch
#        ./scripts/queue.ps1 -Issues 311 -Model haiku -Budget 3
#        ./scripts/queue.ps1 -Issues 302 -NoPr          commit locally, don't push
#
# Logs and summary.md land in target/queue/<time>/; target/queue/latest.txt
# names the newest run.

param(
    # Issue numbers, in the order to do them: 302,303 or "302 303".
    [Parameter(Mandatory)][string[]]$Issues,
    # The model doing the work: an alias (haiku, sonnet, opus) or a full name.
    [string]$Model = 'sonnet',
    # The most one session may spend on one issue, in dollars.
    [double]$Budget = 5,
    # Branch each issue from the one before it instead of from main.
    [switch]$Stack,
    [switch]$NoPr,
    [string]$Worktree,
    # On a usage limit: wait this long between tries, for at most this long.
    [int]$RetryMinutes = 30,
    [double]$MaxWaitHours = 6
)

$ErrorActionPreference = 'Stop'
# One string when started with `pwsh -File`, an array from a PowerShell prompt.
$Issues = @($Issues -split '[,\s]+' | Where-Object { $_ } | ForEach-Object { [int]($_ -replace '^#', '') })
$root = Split-Path $PSScriptRoot
if (-not $Worktree) { $Worktree = Join-Path (Split-Path $root) ((Split-Path $root -Leaf) + '-queue') }
$run = Join-Path $root ('target/queue/' + (Get-Date -Format 'yyyyMMdd-HHmmss'))
New-Item -ItemType Directory -Force $run | Out-Null
Set-Content -LiteralPath (Join-Path $root 'target/queue/latest.txt') -Value $run
$summary = Join-Path $run 'summary.md'

# What a session may run without anyone there to ask. Edits are allowed by the
# permission mode; anything else that would prompt is refused.
$allowed = foreach ($shell in 'Bash', 'PowerShell') {
    foreach ($cmd in 'cargo *', './scripts/check.ps1*', 'pwsh ./scripts/check.ps1*', 'pwsh -File ./scripts/check.ps1*',
        'pwsh -NoProfile -File ./scripts/check.ps1*', 'gh issue view *', 'git status*', 'git diff*', 'git log*', 'git show*') {
        "$shell($cmd)"
    }
}
$denied = foreach ($shell in 'Bash', 'PowerShell') {
    foreach ($cmd in 'git push*', 'git tag*', 'gh pr *', 'gh release *') { "$shell($cmd)" }
}

function Note([string]$line) {
    Write-Host $line
    Add-Content -LiteralPath $summary -Value $line
}

function Tree {
    $out = & git -C $Worktree @args
    if ($LASTEXITCODE -ne 0) { throw "git $($args -join ' ') failed" }
    $out
}

# The subagents a session gets, as Kynex gives them to the sessions it starts:
# Kynex's own list once it has been edited (<data dir>/agents.json), else these.
function Agents([string]$model) {
    $run_and_read = if ($IsWindows) { 'Bash', 'PowerShell', 'Read', 'Grep', 'Glob' } else { 'Bash', 'Read', 'Grep', 'Glob' }
    $doer = 'You carry out the task you are given, exactly as described. Make the changes and run the checks it names. Report what you changed, what the checks said, and anything that did not match the task. If you would have to guess, stop and say what is different.'
    $list = @(
        [pscustomobject]@{ name = 'haiku-worker'; model = 'haiku'; tools = @(); on = $true; prompt = $doer
            description = 'Fast, cheap: contained, clear work where you can say which files and what change. Independent items can go to several at once.' }
        [pscustomobject]@{ name = 'sonnet-worker'; model = 'sonnet'; tools = @(); on = $true; prompt = $doer
            description = 'Mid-priced: multi-file changes you have designed that are too much for Haiku.' }
        [pscustomobject]@{ name = 'checker'; model = 'haiku'; tools = $run_and_read; on = $true
            description = 'Every build, test or lint run; it returns only the failures. Never edits.'
            prompt = 'Run the checks you are asked to. Change nothing. Report PASS, or each failure as file:line and the error in one line.' }
        [pscustomobject]@{ name = 'reviewer'; model = 'sonnet'; tools = $run_and_read; on = $true
            description = 'A second read of a finished change: give it the task and how to see the diff. It reports only real problems. Never edits.'
            prompt = 'Review the change you are pointed at against the task it was for. Read the diff and the code around it. Change nothing. Report only real problems, each as file:line and one line. If there are none, say it is clean.' }
    )
    $dir = if ($env:KYNEX_DATA_DIR) { $env:KYNEX_DATA_DIR }
    elseif ($IsWindows) { Join-Path $env:APPDATA 'kynex' }
    elseif ($IsMacOS) { Join-Path $HOME 'Library/Application Support/kynex' }
    elseif ($env:XDG_DATA_HOME) { Join-Path $env:XDG_DATA_HOME 'kynex' }
    else { Join-Path $HOME '.local/share/kynex' }
    $file = Join-Path $dir 'agents.json'
    if (Test-Path $file) { try { $list = @(Get-Content -Raw -LiteralPath $file | ConvertFrom-Json) } catch { } }

    $map = [ordered]@{}
    foreach ($a in $list | Where-Object on) {
        # A worker on the session's own model saves nothing.
        if (-not $a.tools -and $a.model -and $model -like "*$($a.model)*") { continue }
        $entry = [ordered]@{
            description = $a.description
            prompt      = "$($a.prompt) Your report goes into a more expensive model's context: at most 10 lines."
        }
        if ($a.model) { $entry.model = $a.model }
        if ($a.tools) { $entry.tools = @($a.tools) }
        $map[$a.name] = $entry
    }
    $map
}

# One headless session in the worktree. Returns its text, cost and whether it
# ended in an error; waits out a usage limit and tries again.
function Ask([string]$prompt, [string]$log, [string]$model) {
    $agents = Agents $model
    $agentsFile = Join-Path $run 'agents.json'
    $ruleFile = Join-Path $run 'rule.txt'
    Set-Content -LiteralPath $agentsFile -Value ($agents | ConvertTo-Json -Depth 5)
    $rule = @('Delegate to save your tokens and context. Subagents start fresh: give them the files, the exact task and how to check it.')
    $rule += $agents.Keys | ForEach-Object { "- ${_}: $($agents[$_].description)" }
    if ($agents.Contains('checker')) { $rule += 'Run every build, test and check through checker, never yourself.' }
    if ($agents.Contains('reviewer')) { $rule += 'Before you finish, have reviewer read the diff against the issue, and fix the real problems it finds.' }
    Set-Content -LiteralPath $ruleFile -Value ($rule -join "`n")

    $waited = 0
    while ($true) {
        Push-Location $Worktree
        try {
            $out = $prompt | & claude -p --model $model --max-budget-usd $Budget --permission-mode acceptEdits `
                --permission-prompts none --output-format json --agents $agentsFile --append-system-prompt-file $ruleFile `
                --allowedTools @allowed --disallowedTools @denied 2>&1 | Out-String
        } finally { Pop-Location }
        $failed = $LASTEXITCODE -ne 0
        Add-Content -LiteralPath $log -Value $out
        $reply = $null
        try { $reply = $out | ConvertFrom-Json } catch { }
        if ($reply -and $reply.is_error) { $failed = $true }
        $limited = $failed -and $out -match '(?i)usage limit|rate limit|limit reached|out of extra usage'
        if (-not $limited) {
            return [pscustomobject]@{
                Text   = if ($reply) { [string]$reply.result } else { $out }
                Cost   = if ($reply) { [double]$reply.total_cost_usd } else { 0 }
                Failed = $failed
            }
        }
        if ($waited -ge $MaxWaitHours * 60) { throw "usage limit: still limited after $MaxWaitHours hours" }
        Note "  usage limit, trying again in $RetryMinutes minutes"
        Start-Sleep -Seconds ($RetryMinutes * 60)
        $waited += $RetryMinutes
    }
}

function Check([string]$log) {
    & pwsh -NoProfile -File (Join-Path $Worktree 'scripts/check.ps1') *> $log
    $LASTEXITCODE -eq 0
}

& git -C $root fetch origin --quiet
if ($LASTEXITCODE -ne 0) { throw 'git fetch failed' }
if (-not (Test-Path $Worktree)) {
    & git -C $root worktree add --detach $Worktree origin/main | Out-Null
    if ($LASTEXITCODE -ne 0) { throw "couldn't create the worktree at $Worktree" }
}
if (Tree status --porcelain) { throw "the queue's worktree at $Worktree has uncommitted changes; clean it first" }

Note "# Queue $(Get-Date -Format 'yyyy-MM-dd HH:mm'): issues $($Issues -join ', ') on $Model, at most `$$Budget each"
$previous = $null
foreach ($n in $Issues) {
    $issue = gh issue view $n --json 'title,state,body' | ConvertFrom-Json
    if ($LASTEXITCODE -ne 0) { Note "- #${n}: couldn't read the issue; stopped"; break }
    if ($issue.state -ne 'OPEN') { Note "- #${n}: already closed, skipped"; continue }
    # An issue may name its own model on a line like "Model: haiku".
    $model = if ($issue.body -match '(?im)^Model:\s*([\w.-]+)') { $Matches[1] } else { $Model }
    $title = $issue.title -replace '^\[\d+\]\s*', ''
    $branch = "issue-$n"
    $base = if ($Stack -and $previous) { $previous } else { 'origin/main' }
    $log = Join-Path $run "issue-$n.log"

    if (& git -C $Worktree branch --list $branch) { Tree switch --quiet $branch | Out-Null }
    else { Tree switch --quiet -c $branch $base | Out-Null }

    $answer = Ask @"
Implement GitHub issue #$n of this repository. Read it first: gh issue view $n
Follow CLAUDE.md. Work only in this folder; it is a git worktree on branch $branch, and work
from an earlier attempt may already be here, so look before you start.
Do only what the issue asks. You are done when ./scripts/check.ps1 passes.
Nobody is here to answer questions: decide, and write what you decided in your final message.
Don't commit, push or open a pull request; the runner does that.
Your final message becomes the pull request description: a few plain lines on what changed
and anything the reviewer must look at.
"@ $log $model
    $cost = $answer.Cost

    $passed = -not $answer.Failed -and (Check "$log.check")
    if (-not $passed -and -not $answer.Failed) {
        # One repair pass with what the check said.
        $tail = (Get-Content -LiteralPath "$log.check" -Tail 60) -join "`n"
        $repair = Ask @"
You implemented GitHub issue #$n in this folder (gh issue view $n), but ./scripts/check.ps1 fails.
Fix it without changing what the issue asks for. The end of its output:

$tail

Don't commit or push. Your final message becomes the pull request description.
"@ $log $model
        $cost += $repair.Cost
        if (-not $repair.Failed) { $answer = $repair }
        $passed = -not $repair.Failed -and (Check "$log.check")
    }
    $spent = '${0:n2}' -f $cost

    if (-not $passed) {
        $why = if ($answer.Failed) { 'the session ended in an error (budget or failure)' } else { 'the check still fails' }
        Note "- #${n} ${title}: FAILED, $why ($spent). Work is on branch $branch in $Worktree; log $log. Queue stopped."
        break
    }

    if (Tree status --porcelain) {
        Tree add -A | Out-Null
        Tree commit --quiet -m $title | Out-Null
    }
    if (-not (Tree rev-list "$base..HEAD")) {
        Note "- #${n} ${title}: FAILED, nothing was changed ($spent). Queue stopped."
        break
    }

    if ($NoPr) {
        Note "- #${n} ${title}: done on branch $branch, not pushed ($spent)"
    } else {
        Tree push --quiet -u origin $branch | Out-Null
        # No AI attribution in pull requests, whatever the session wrote.
        $text = ($answer.Text -split "`r?`n" | Where-Object { $_ -notmatch 'Generated with|Co-Authored-By' }) -join "`n"
        if ($text -notmatch "#$n\b") { $text = "$($text.Trim())`n`nFixes #$n" }
        $bodyFile = Join-Path $run "issue-$n.pr.md"
        Set-Content -LiteralPath $bodyFile -Value $text
        $prBase = if ($Stack -and $previous) { $previous } else { 'main' }
        $url = gh pr create --base $prBase --head $branch --title $title --body-file $bodyFile
        if ($LASTEXITCODE -ne 0) { Note "- #${n} ${title}: pushed as $branch, but opening the PR failed ($spent). Queue stopped."; break }
        Note "- #${n} ${title}: $url ($model, $spent)"
    }
    $previous = $branch
}
Note "Finished $(Get-Date -Format 'HH:mm')."
