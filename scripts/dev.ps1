# Runs `dx serve --hot-patch` and keeps the app alive across bad patches.
#
# Hot-patching can't change a struct's layout in a running process, so a patch
# that adds or removes fields crashes the app. When that happens this restarts
# dx, which does a full build and only relaunches the app if the build succeeds.
# A plain compile error never closes the app: dx keeps the old one running.
#
# Usage: ./scripts/dev.ps1 [extra dx args]

$ErrorActionPreference = 'Stop'
$root = Split-Path $PSScriptRoot
Set-Location $root

$dxArgs = @('serve', '--hot-patch', '--features', 'hotpatch', '--interactive', 'false') + $args

while ($true) {
    $psi = [System.Diagnostics.ProcessStartInfo]::new('dx')
    foreach ($a in $dxArgs) { $psi.ArgumentList.Add($a) }
    # Set-Location doesn't change the process cwd that child processes inherit.
    $psi.WorkingDirectory = $root
    $psi.UseShellExecute = $false
    $psi.RedirectStandardOutput = $true
    $dx = [System.Diagnostics.Process]::Start($psi)

    $restart = $false
    try {
        while ($null -ne ($line = $dx.StandardOutput.ReadLine())) {
            Write-Host $line
            if ($line -match 'Application \[\w+\] exited with error') {
                Write-Host "`n[dev] app crashed (likely a struct layout change); full rebuild...`n" -ForegroundColor Yellow
                $restart = $true
                break
            }
            if ($line -match 'Application \[\w+\] exited gracefully') {
                break
            }
        }
    } finally {
        if (-not $dx.HasExited) { $dx.Kill($true) }
    }

    if (-not $restart) { break }
}
