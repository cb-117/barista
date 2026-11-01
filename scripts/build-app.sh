#!/bin/zsh
set -euo pipefail
cd "$(dirname "$0")/.."

# Pass --universal for Intel + Apple Silicon. Other flags go to SwiftPM.
architectures=("$(uname -m)")
if [[ "${1:-}" == "--universal" ]]; then
    architectures=(arm64 x86_64)
    shift
fi
mkdir -p build
staging="$PWD/build/.Barista-stage-$$.app"
trap 'rm -rf "$staging"' EXIT
mkdir -p "$staging/Contents/MacOS" "$staging/Contents/Helpers" "$staging/Contents/Resources"
app_binaries=()
cli_binaries=()
for architecture in "${architectures[@]}"; do
    triple="${architecture}-apple-macosx13.0"
    swift build -c release --triple "$triple" --product BaristaApp "$@"
    swift build -c release --triple "$triple" --product barista "$@"
    bin_dir="$(swift build -c release --triple "$triple" --show-bin-path "$@")"
    app_binaries+=("$bin_dir/BaristaApp")
    cli_binaries+=("$bin_dir/barista")
done
lipo -create "${app_binaries[@]}" -output "$staging/Contents/MacOS/Barista"
lipo -create "${cli_binaries[@]}" -output "$staging/Contents/Helpers/barista"
cp Support/Barista.icns "$staging/Contents/Resources/Barista.icns"
cp Support/Info.plist "$staging/Contents/Info.plist"
cp docs/AGENTS.md "$staging/Contents/Resources/AgentGuide.md"
codesign --force --sign - --options runtime "$staging/Contents/Helpers/barista"
codesign --force --sign - --options runtime "$staging"

# Swap complete bundles; never overwrite a running executable in place.
if [[ -e "$PWD/build/Barista.app" ]]; then
    previous="$PWD/build/.Barista-previous-$$.app"
    mv "$PWD/build/Barista.app" "$previous"
    if ! mv "$staging" "$PWD/build/Barista.app"; then
        mv "$previous" "$PWD/build/Barista.app"
        exit 1
    fi
    rm -rf "$previous"
else
    mv "$staging" "$PWD/build/Barista.app"
fi
ln -sfn Barista.app/Contents/Helpers/barista "$PWD/build/barista"
print "Built: $PWD/build/Barista.app"
print "Agent CLI: $PWD/build/barista"
