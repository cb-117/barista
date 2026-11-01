#!/bin/zsh
# Packages an app (default build/Barista.app) into build/Barista-<version>.dmg with a
# drag-to-Applications window. Usage: zsh scripts/make-dmg.sh [PATH/TO/Barista.app]
# Needs dmgbuild (pip install dmgbuild==1.6.5); pass a Python that has it with DMGBUILD_PYTHON.
set -euo pipefail
cd "$(dirname "$0")/.."
app="${1:-$PWD/build/Barista.app}"
app="${app:A}"
[[ -d "$app" ]] || { print -u2 "Build the app first: zsh scripts/build-app.sh"; exit 1; }
version="$(/usr/libexec/PlistBuddy -c 'Print :CFBundleShortVersionString' "$app/Contents/Info.plist")"
mkdir -p build
output="$PWD/build/Barista-$version.dmg"
python="${DMGBUILD_PYTHON:-python3}"
"$python" -I -c 'import dmgbuild' 2>/dev/null || { print -u2 "dmgbuild isn't installed for $python (pip install dmgbuild==1.6.5)"; exit 1; }
rm -f "$output"
"$python" -I -m dmgbuild -s scripts/dmg-settings.py \
    -D app="$app" -D background="$PWD/Support/dmg-background.tiff" -D volume_icon="$PWD/Support/Barista.icns" \
    "Barista $version" "$output"
codesign --force --sign - "$output"
hdiutil verify "$output" >/dev/null
print "Built: $output"
