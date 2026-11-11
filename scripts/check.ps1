# The one "is it done?" check: formatting, lints and tests for the whole
# workspace. Builds into target/claude, so it never touches the dev build that
# dx is serving. Exit code 0 means every step passed. Leaves the caller's shell
# as it found it (location and environment).
#
# Usage: ./scripts/check.ps1          check everything
#        ./scripts/check.ps1 -Fix     format and apply clippy's fixes first
#        ./scripts/check.ps1 -Core    only kynex-core (no gpui, seconds)

param([switch]$Fix, [switch]$Core)

$ErrorActionPreference = 'Stop'
$root = Split-Path $PSScriptRoot
$targetDir = Join-Path $root 'target/claude'
$target = @('--target-dir', $targetDir)

# Every rebuild leaves the old test binaries and their PDBs behind, so this folder only grows.
# Past the limit it is cleared (the next run rebuilds the dependencies, a few minutes).
$limitGB = 12
if (Test-Path $targetDir) {
    $bytes = (Get-ChildItem $targetDir -Recurse -File -Force -ErrorAction SilentlyContinue | Measure-Object Length -Sum).Sum
    if ($bytes / 1GB -gt $limitGB) {
        Write-Host ("target/claude is {0:N1} GB (limit $limitGB): clearing it" -f ($bytes / 1GB)) -ForegroundColor Yellow
        Remove-Item $targetDir -Recurse -Force -ErrorAction SilentlyContinue
    }
}
# Incremental caches are for the dev build; here they only add size.
$incremental = $env:CARGO_INCREMENTAL
$env:CARGO_INCREMENTAL = '0'
$scope = if ($Core) { @('-p', 'kynex-core') } else { @('--workspace') }
# Inherited from dx when run inside Kynex; cargo only warns about it.
$makeflags = $env:CARGO_MAKEFLAGS
Remove-Item Env:CARGO_MAKEFLAGS -ErrorAction SilentlyContinue

function Step([string]$name, [string[]]$cargoArgs) {
    Write-Host "==> cargo $($cargoArgs -join ' ')" -ForegroundColor Cyan
    # To the console, not into this function's return value.
    & cargo @cargoArgs | Out-Host
    if ($LASTEXITCODE -ne 0) {
        Write-Host "FAILED: $name" -ForegroundColor Red
        return $false
    }
    return $true
}

Push-Location $root
try {
    $steps = @()
    if ($Fix) {
        $steps += , @('fmt', @('fmt', '--all'))
        $steps += , @('clippy --fix', (@('clippy') + $target + @('--fix', '--allow-dirty', '--allow-staged') + $scope + @('--all-targets')))
    }
    $steps += , @('fmt', @('fmt', '--all', '--', '--check'))
    $steps += , @('clippy', (@('clippy') + $target + $scope + @('--all-targets', '--', '-D', 'warnings')))
    $steps += , @('test', (@('test') + $target + $scope))
    foreach ($s in $steps) {
        if (-not (Step $s[0] $s[1])) { exit 1 }
    }
    Write-Host 'All checks passed' -ForegroundColor Green
} finally {
    Pop-Location
    if ($null -ne $incremental) { $env:CARGO_INCREMENTAL = $incremental } else { Remove-Item Env:CARGO_INCREMENTAL -ErrorAction SilentlyContinue }
    if ($null -ne $makeflags) { $env:CARGO_MAKEFLAGS = $makeflags }
}
