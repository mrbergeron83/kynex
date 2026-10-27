# Installs (or updates) Kynex for the current user, no admin needed:
#   irm https://raw.githubusercontent.com/mrbergeron83/kynex/main/install.ps1 | iex
#
# Downloads the latest release, checks it against SHA256SUMS, unpacks it into
# %LOCALAPPDATA%\Programs\kynex and adds Start menu and desktop shortcuts.
# KYNEX_VERSION=v0.2.0 picks a release, KYNEX_INSTALL_DIR another folder.

# A script block, so `irm | iex` doesn't leave these settings in the caller's session.
& {
    $ErrorActionPreference = 'Stop'
    $ProgressPreference = 'SilentlyContinue' # Invoke-WebRequest is far slower with the progress bar

    $repo = 'mrbergeron83/kynex'
    $asset = 'kynex-x86_64-pc-windows-msvc.zip'
    $dir = if ($env:KYNEX_INSTALL_DIR) { $env:KYNEX_INSTALL_DIR } else { Join-Path $env:LOCALAPPDATA 'Programs\kynex' }
    $base = if ($env:KYNEX_VERSION) { "https://github.com/$repo/releases/download/$env:KYNEX_VERSION" } else { "https://github.com/$repo/releases/latest/download" }
    $exe = Join-Path $dir 'kynex.exe'

    # A running kynex.exe can't be overwritten.
    $running = Get-Process kynex -ErrorAction SilentlyContinue | Where-Object { $_.Path -eq $exe }
    if ($running) { throw "Kynex is running from $dir; close it and run the installer again." }

    $tmp = Join-Path ([IO.Path]::GetTempPath()) "kynex-install-$([guid]::NewGuid())"
    New-Item -ItemType Directory $tmp | Out-Null
    try {
        Write-Host "Downloading $asset..."
        $zip = Join-Path $tmp $asset
        Invoke-WebRequest "$base/$asset" -OutFile $zip -UseBasicParsing
        $sums = (Invoke-WebRequest "$base/SHA256SUMS" -UseBasicParsing).Content
        if ($sums -is [byte[]]) { $sums = [Text.Encoding]::UTF8.GetString($sums) }

        $expected = ($sums -split "`n" | Where-Object { $_ -match "^([0-9a-f]{64})\s+\*?$([regex]::Escape($asset))\s*$" } | ForEach-Object { $Matches[1] })
        if (-not $expected) { throw "SHA256SUMS has no entry for $asset." }
        $actual = (Get-FileHash $zip -Algorithm SHA256).Hash.ToLower()
        if ($actual -ne $expected) { throw "Checksum mismatch for $asset (expected $expected, got $actual)." }

        $unpacked = Join-Path $tmp 'unpacked'
        Expand-Archive $zip $unpacked
        New-Item -ItemType Directory -Force $dir | Out-Null
        Copy-Item (Join-Path $unpacked '*') $dir -Recurse -Force
    } finally {
        Remove-Item $tmp -Recurse -Force -ErrorAction SilentlyContinue
    }

    $shell = New-Object -ComObject WScript.Shell
    foreach ($folder in @([Environment]::GetFolderPath('Programs'), [Environment]::GetFolderPath('Desktop'))) {
        $link = $shell.CreateShortcut((Join-Path $folder 'Kynex.lnk'))
        $link.TargetPath = $exe
        $link.WorkingDirectory = $dir
        $link.Save()
    }

    Write-Host "Kynex installed to $dir"
    Start-Process $exe
}
