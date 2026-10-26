# The one "is it done?" check: formatting, lints and tests for the whole
# workspace. Builds into target/claude, so it never touches the dev build that
# dx is serving. Exit code 0 means every step passed.
#
# Usage: ./check.ps1          check everything
#        ./check.ps1 -Fix     format and apply clippy's fixes first
#        ./check.ps1 -Core    only kynex-core (no gpui, seconds)

param([switch]$Fix, [switch]$Core)

$ErrorActionPreference = 'Stop'
Set-Location $PSScriptRoot
# Inherited from dx when run inside Kynex; cargo only warns about it.
Remove-Item Env:CARGO_MAKEFLAGS -ErrorAction SilentlyContinue
$env:CARGO_TARGET_DIR = Join-Path $PSScriptRoot 'target/claude'

$scope = if ($Core) { @('-p', 'kynex-core') } else { @('--workspace') }

function Step([string]$name, [string[]]$cargoArgs) {
    Write-Host "==> cargo $($cargoArgs -join ' ')" -ForegroundColor Cyan
    & cargo @cargoArgs
    if ($LASTEXITCODE -ne 0) {
        Write-Host "FAILED: $name" -ForegroundColor Red
        exit 1
    }
}

if ($Fix) {
    Step 'fmt' @('fmt', '--all')
    Step 'clippy --fix' (@('clippy', '--fix', '--allow-dirty', '--allow-staged') + $scope + @('--all-targets'))
}
Step 'fmt' @('fmt', '--all', '--', '--check')
Step 'clippy' (@('clippy') + $scope + @('--all-targets', '--', '-D', 'warnings'))
Step 'test' (@('test') + $scope)
Write-Host 'All checks passed' -ForegroundColor Green
