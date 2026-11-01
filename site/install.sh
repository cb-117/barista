#!/bin/sh
# Barista installer · https://github.com/cb-117/barista
#
#   curl -fsSL https://baristatime.app/install.sh | sh
#
# Installs the latest Barista (a free, open-source app that keeps the computer awake while
# timers, commands, and AI agents work), then runs `barista setup`, which connects Claude Code
# and Codex if they're set up for this user and starts the app.
#   macOS: Barista.app in Applications. Safe to re-run: an existing install is kept
#          (Barista updates itself from its menu).
#   Linux: barista and barista-app in ~/.local/bin, plus an app-menu entry. Re-running updates it.
# Windows: use install.ps1 instead.
set -eu

# Everything runs from main, called on the last line, so a download cut short runs nothing.
main() {
base="https://github.com/cb-117/barista/releases/latest/download"

case "$(uname -s)" in
    Darwin) ;;
    Linux) install_linux; return ;;
    *) echo "This installer supports macOS and Linux. On Windows, run in PowerShell: irm https://baristatime.app/install.ps1 | iex" >&2; exit 1 ;;
esac
major="$(sw_vers -productVersion | cut -d. -f1)"
[ "$major" -ge 13 ] || { echo "Barista needs macOS 13 or later." >&2; exit 1; }

app=""
for candidate in "/Applications/Barista.app" "$HOME/Applications/Barista.app"; do
    if [ -d "$candidate" ]; then app="$candidate"; break; fi
done

if [ -n "$app" ]; then
    echo "Barista is already installed at $app. It updates itself from its menu."
else
    if [ -w /Applications ]; then destination="/Applications"; else destination="$HOME/Applications"; mkdir -p "$destination"; fi
    app="$destination/Barista.app"
    work="$(mktemp -d)"
    trap 'rm -rf "$work"' EXIT INT TERM
    echo "Downloading Barista..."
    curl -fsSL "$base/Barista.zip" -o "$work/Barista.zip"
    curl -fsSL "$base/SHA256SUMS.txt" -o "$work/SHA256SUMS.txt"
    expected="$(awk '$2 == "Barista.zip" { print $1 }' "$work/SHA256SUMS.txt")"
    actual="$(shasum -a 256 "$work/Barista.zip" | awk '{ print $1 }')"
    if [ -z "$expected" ] || [ "$expected" != "$actual" ]; then
        echo "The download didn't match its published checksum, so nothing was installed." >&2
        exit 1
    fi
    ditto -x -k "$work/Barista.zip" "$work/unpacked"
    codesign --verify --deep --strict "$work/unpacked/Barista.app"
    ditto "$work/unpacked/Barista.app" "$app"
    echo "Installed $app"
fi

"$app/Contents/Helpers/barista" setup
}

install_linux() {
case "$(uname -m)" in
    x86_64|amd64) arch="x86_64" ;;
    aarch64|arm64) arch="arm64" ;;
    *) echo "Barista for Linux runs on x86_64 and arm64." >&2; exit 1 ;;
esac
archive="barista-linux-$arch.tar.gz"
bin="$HOME/.local/bin"
share="${XDG_DATA_HOME:-$HOME/.local/share}"
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT INT TERM
echo "Downloading Barista..."
curl -fsSL "$base/$archive" -o "$work/$archive" || { echo "Couldn't download $archive. Is there a Linux build in the latest release yet?" >&2; exit 1; }
curl -fsSL "$base/SHA256SUMS.txt" -o "$work/SHA256SUMS.txt"
expected="$(awk -v name="$archive" '$2 == name { print $1 }' "$work/SHA256SUMS.txt")"
if command -v sha256sum >/dev/null 2>&1; then actual="$(sha256sum "$work/$archive" | awk '{ print $1 }')"
else actual="$(shasum -a 256 "$work/$archive" | awk '{ print $1 }')"; fi
if [ -z "$expected" ] || [ "$expected" != "$actual" ]; then
    echo "The download didn't match its published checksum, so nothing was installed." >&2
    exit 1
fi
tar -xzf "$work/$archive" -C "$work"
unpacked="$(find "$work" -mindepth 1 -maxdepth 1 -type d -name 'barista-*' | head -n 1)"
[ -n "$unpacked" ] || { echo "The download was empty, so nothing was installed." >&2; exit 1; }
mkdir -p "$bin" "$share/applications" "$share/icons/hicolor/256x256/apps"
# A running copy keeps the old version; quit it so setup starts the new one.
if pkill -x barista-app 2>/dev/null; then sleep 1; fi
for file in barista barista-app; do
    cp "$unpacked/$file" "$bin/.$file.new" && chmod 755 "$bin/.$file.new" && mv -f "$bin/.$file.new" "$bin/$file"
done
cp "$unpacked/barista.png" "$share/icons/hicolor/256x256/apps/barista.png"
cat > "$share/applications/barista.desktop" <<DESKTOP
[Desktop Entry]
Type=Application
Name=Barista
Comment=Keeps your computer awake while work happens
Exec="$bin/barista-app"
Icon=barista
Terminal=false
Categories=Utility;
DESKTOP
echo "Installed Barista to $bin"
case ":$PATH:" in *":$bin:"*) ;; *) echo "Add $bin to your PATH to use the barista command from anywhere." ;; esac
"$bin/barista" setup
}

main "$@"
