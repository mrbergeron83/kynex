# Installs (or updates) Kynex for the current user, no admin needed:
#   irm https://raw.githubusercontent.com/mrbergeron83/kynex/main/install.ps1 | iex
#
# Downloads the latest release, checks it against SHA256SUMS, unpacks it into
# %LOCALAPPDATA%\Programs\kynex, adds Start menu and desktop shortcuts, and puts
# a `knx` command on the user PATH.
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

    # The shortcuts carry Kynex's AppUserModelID (attention::APP_ID), as its window
    # does: the taskbar keeps a pinned Kynex with the running one, and notifications
    # come from "Kynex". WScript.Shell can't set it, so a little C# does.
    if (-not ('KynexInstall.Shortcut' -as [type])) {
        Add-Type -TypeDefinition @'
using System;
using System.Runtime.InteropServices;
namespace KynexInstall {
    [StructLayout(LayoutKind.Sequential, Pack = 4)]
    public struct PropertyKey { public Guid fmtid; public uint pid; }
    [StructLayout(LayoutKind.Explicit, Size = 24)]
    public struct PropVariant { [FieldOffset(0)] public ushort vt; [FieldOffset(8)] public IntPtr p; }
    [ComImport, Guid("886D8EEB-8CF2-4446-8D02-CDBA1DBDCF99"), InterfaceType(ComInterfaceType.InterfaceIsIUnknown)]
    interface IPropertyStore {
        int GetCount(out uint count);
        int GetAt(uint index, out PropertyKey key);
        int GetValue(ref PropertyKey key, out PropVariant value);
        int SetValue(ref PropertyKey key, ref PropVariant value);
        int Commit();
    }
    public static class Shortcut {
        [DllImport("shell32.dll", CharSet = CharSet.Unicode, PreserveSig = false)]
        static extern void SHGetPropertyStoreFromParsingName(string path, IntPtr bindContext, int flags, ref Guid iid,
            [MarshalAs(UnmanagedType.Interface)] out IPropertyStore store);
        public static void SetAppId(string path, string id) {
            Guid iid = typeof(IPropertyStore).GUID;
            IPropertyStore store;
            SHGetPropertyStoreFromParsingName(path, IntPtr.Zero, 2 /* GPS_READWRITE */, ref iid, out store);
            // PKEY_AppUserModel_ID, as a VT_LPWSTR.
            PropertyKey key = new PropertyKey { fmtid = new Guid("9F4C2855-9F79-4B39-A8D0-E1D42DE1D5F3"), pid = 5 };
            PropVariant value = new PropVariant { vt = 31, p = Marshal.StringToCoTaskMemUni(id) };
            try {
                Marshal.ThrowExceptionForHR(store.SetValue(ref key, ref value));
                Marshal.ThrowExceptionForHR(store.Commit());
            } finally {
                Marshal.FreeCoTaskMem(value.p);
                Marshal.ReleaseComObject(store);
            }
        }
    }
}
'@
    }
    $shell = New-Object -ComObject WScript.Shell
    foreach ($folder in @([Environment]::GetFolderPath('Programs'), [Environment]::GetFolderPath('Desktop'))) {
        $path = Join-Path $folder 'Kynex.lnk'
        $link = $shell.CreateShortcut($path)
        $link.TargetPath = $exe
        $link.WorkingDirectory = $dir
        $link.Save()
        try { [KynexInstall.Shortcut]::SetAppId($path, 'Kynex.Kynex') } catch { Write-Warning "Couldn't set Kynex's app ID on ${path}: $_" }
    }

    # `knx` from any terminal: a shim that starts Kynex without holding the console.
    Set-Content (Join-Path $dir 'knx.cmd') "@start `"`" `"%~dp0kynex.exe`" %*" -Encoding Ascii
    # Edit the raw registry value so %VARS% in the user PATH stay unexpanded.
    $envKey = (Get-Item 'HKCU:\').OpenSubKey('Environment', $true)
    try {
        $path = $envKey.GetValue('Path', '', 'DoNotExpandEnvironmentNames')
        if (($path -split ';') -notcontains $dir) {
            $kind = if ($path -match '%') { 'ExpandString' } else { 'String' }
            $path = $path.TrimEnd(';')
            $envKey.SetValue('Path', $(if ($path) { "$path;$dir" } else { $dir }), $kind)
            # Tell Explorer the environment changed, so terminals opened from now on see the new PATH.
            Add-Type -Namespace Kynex -Name Win32 -MemberDefinition '[DllImport("user32.dll", CharSet = CharSet.Unicode)] public static extern IntPtr SendMessageTimeout(IntPtr hWnd, uint msg, UIntPtr wParam, string lParam, uint flags, uint timeout, out UIntPtr result);'
            $result = [UIntPtr]::Zero
            [Kynex.Win32]::SendMessageTimeout([IntPtr]0xffff, 0x1A, [UIntPtr]::Zero, 'Environment', 2, 5000, [ref]$result) | Out-Null
            $env:Path = "$env:Path;$dir"
            Write-Host "Added $dir to your PATH; open a new terminal to use 'knx'."
        }
    } finally {
        $envKey.Close()
    }

    Write-Host "Kynex installed to $dir"
    Start-Process $exe
}
