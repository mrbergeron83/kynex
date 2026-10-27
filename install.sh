#!/bin/sh
# Installs (or updates) Kynex for the current user on macOS or Linux, no sudo needed:
#   curl -fsSL https://raw.githubusercontent.com/mrbergeron83/kynex/main/install.sh | sh
#
# Downloads the latest release and checks it against SHA256SUMS. On macOS it
# builds ~/Applications/Kynex.app (Launchpad, Spotlight); on Linux it unpacks
# into ~/.local/share/kynex and adds an app menu entry. Both get a `knx`
# command in ~/.local/bin. KYNEX_VERSION=v0.2.0 picks a release, KYNEX_INSTALL_DIR
# another folder (the .app's parent on macOS). Windows uses install.ps1.

set -eu

repo=mrbergeron83/kynex
bin_dir="$HOME/.local/bin"

case "$(uname -s)-$(uname -m)" in
    Darwin-arm64) target=aarch64-apple-darwin ;;
    Linux-x86_64) target=x86_64-unknown-linux-gnu ;;
    *) echo "No Kynex build for $(uname -s) $(uname -m) yet; see https://github.com/$repo/releases" >&2; exit 1 ;;
esac
asset="kynex-$target.tar.gz"
if [ -n "${KYNEX_VERSION:-}" ]; then
    base="https://github.com/$repo/releases/download/$KYNEX_VERSION"
else
    base="https://github.com/$repo/releases/latest/download"
fi

tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT

echo "Downloading $asset..."
curl -fsSL "$base/$asset" -o "$tmp/$asset"
curl -fsSL "$base/SHA256SUMS" -o "$tmp/SHA256SUMS"
expected=$(awk -v f="$asset" '$2 == f || $2 == "*" f { print $1 }' "$tmp/SHA256SUMS")
[ -n "$expected" ] || { echo "SHA256SUMS has no entry for $asset." >&2; exit 1; }
if command -v sha256sum >/dev/null 2>&1; then
    actual=$(sha256sum "$tmp/$asset" | cut -d ' ' -f 1)
else
    actual=$(shasum -a 256 "$tmp/$asset" | cut -d ' ' -f 1)
fi
[ "$actual" = "$expected" ] || { echo "Checksum mismatch for $asset (expected $expected, got $actual)." >&2; exit 1; }

mkdir "$tmp/unpacked"
tar -xzf "$tmp/$asset" -C "$tmp/unpacked"
mkdir -p "$bin_dir"

if [ "$(uname -s)" = Darwin ]; then
    app="${KYNEX_INSTALL_DIR:-$HOME/Applications}/Kynex.app"
    rm -rf "$app"
    mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"
    cp "$tmp/unpacked/kynex" "$app/Contents/MacOS/kynex"
    cp "$tmp/unpacked/"*.md "$tmp/unpacked/"*.txt "$app/Contents/Resources/"
    cat >"$app/Contents/Info.plist" <<'EOF'
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleName</key><string>Kynex</string>
    <key>CFBundleDisplayName</key><string>Kynex</string>
    <key>CFBundleIdentifier</key><string>dev.kynex.Kynex</string>
    <key>CFBundleExecutable</key><string>kynex</string>
    <key>CFBundlePackageType</key><string>APPL</string>
    <key>NSHighResolutionCapable</key><true/>
</dict>
</plist>
EOF
    # Ad-hoc sign the bundle so macOS accepts it as a whole, not just the binary.
    codesign --force --sign - "$app" >/dev/null 2>&1 || true
    printf '#!/bin/sh\nexec open -a "%s" --args "$@"\n' "$app" >"$tmp/knx"
    where="$app"
else
    dir="${KYNEX_INSTALL_DIR:-$HOME/.local/share/kynex}"
    mkdir -p "$dir"
    cp "$tmp/unpacked/"*.md "$tmp/unpacked/"*.txt "$dir/"
    # Copy then rename, so a running kynex keeps its old binary instead of failing with "text file busy".
    cp "$tmp/unpacked/kynex" "$dir/kynex.new"
    chmod +x "$dir/kynex.new"
    mv -f "$dir/kynex.new" "$dir/kynex"

    apps="${XDG_DATA_HOME:-$HOME/.local/share}/applications"
    mkdir -p "$apps"
    cat >"$apps/kynex.desktop" <<EOF
[Desktop Entry]
Type=Application
Name=Kynex
Comment=A native desktop workspace for Claude Code
Exec="$dir/kynex"
Path=$dir
Terminal=false
Categories=Development;
EOF
    command -v update-desktop-database >/dev/null 2>&1 && update-desktop-database "$apps" >/dev/null 2>&1 || true
    printf '#!/bin/sh\nnohup "%s" "$@" >/dev/null 2>&1 &\n' "$dir/kynex" >"$tmp/knx"
    where="$dir"
fi

# `knx` from any terminal: starts Kynex without holding the shell.
chmod +x "$tmp/knx"
mv -f "$tmp/knx" "$bin_dir/knx"

echo "Kynex installed to $where"
case ":$PATH:" in
    *":$bin_dir:"*) ;;
    *) echo "Add $bin_dir to your PATH to use 'knx', e.g. in ~/.profile: export PATH=\"\$HOME/.local/bin:\$PATH\"" ;;
esac

if [ "$(uname -s)" = Darwin ]; then
    open "$app"
elif [ -n "${DISPLAY:-}${WAYLAND_DISPLAY:-}" ]; then
    nohup "$dir/kynex" >/dev/null 2>&1 &
fi
