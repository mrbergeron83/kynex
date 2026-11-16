# Kynex's hooks: tell the terminal when a command starts and ends (and its exit code), and link
# what it prints, so `knx pane run --wait` can answer. Run after the profile (-EncodedCommand),
# so they wrap whatever prompt the profile set. Nothing is printed. Not in ConstrainedLanguage,
# where the .NET calls aren't allowed.
if (-not $global:__kynex -and $ExecutionContext.SessionState.LanguageMode -eq 'FullLanguage') {
    $global:__kynex = @{ p = 0; n = 0; prompt = $function:prompt; readLine = $null; code = $null; error = $null }
    function global:prompt {
        $ok = $global:?
        $k = $global:__kynex
        # A failed cmdlet leaves a new error and $LASTEXITCODE as it was (a native command's
        # from before); a failed native command sets $LASTEXITCODE.
        $newError = $global:Error.Count -gt 0 -and -not [object]::ReferenceEquals($global:Error[0], $k.error)
        $code = if ($ok) { 0 }
            elseif ($newError -and $global:LASTEXITCODE -eq $k.code) { 1 }
            elseif ($global:LASTEXITCODE) { $global:LASTEXITCODE }
            else { 1 }
        $e = [char]27
        $k.p++
        [Console]::Write("$e]8;;$e\")
        $Host.UI.RawUI.WindowTitle = "knx-prompt:$($k.p):$($k.n):$code"
        # The prompt it wraps sees the command's `$?`, as without the hooks (themes show it).
        if (-not $ok) { Write-Error '' -ErrorAction Ignore }
        & $k.prompt
    }
    if (Test-Path Function:\PSConsoleHostReadLine) {
        $global:__kynex.readLine = $function:PSConsoleHostReadLine
        function global:PSConsoleHostReadLine {
            $k = $global:__kynex
            $line = & $k.readLine
            $k.n++
            $k.code = $global:LASTEXITCODE
            $k.error = if ($global:Error.Count -gt 0) { $global:Error[0] } else { $null }
            $e = [char]27
            [Console]::Write("$e]8;;knx:out:$($k.n)$e\")
            $Host.UI.RawUI.WindowTitle = "knx-exec:$($k.n)"
            $line
        }
    }
}
