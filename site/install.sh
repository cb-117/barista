#!/bin/sh
# Barista installer · https://github.com/cb-117/barista
#
#   curl -fsSL https://cb-117.github.io/barista/install.sh | sh
#
# Installs the latest Barista.app (a free, open-source Mac menu-bar app that keeps the Mac
# awake while timers, commands, and AI agents work), then runs `barista setup`, which
# connects Claude Code and Codex if they're set up for this user and starts the app.
# Safe to re-run: an existing install is kept (Barista updates itself from its menu).
set -eu

base="https://github.com/cb-117/barista/releases/latest/download"

[ "$(uname -s)" = "Darwin" ] || { echo "Barista is a macOS app." >&2; exit 1; }
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
