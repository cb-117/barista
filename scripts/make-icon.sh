#!/bin/zsh
# Builds Support/Barista.icns from the 1024px artwork in site/assets.
set -euo pipefail
cd "$(dirname "$0")/.."
source=site/assets/app-icon-1024.png
iconset="$(mktemp -d)/Barista.iconset"
mkdir -p "$iconset"
for size in 16 32 128 256 512; do
    sips -z $size $size "$source" --out "$iconset/icon_${size}x${size}.png" >/dev/null
    sips -z $((size * 2)) $((size * 2)) "$source" --out "$iconset/icon_${size}x${size}@2x.png" >/dev/null
done
iconutil -c icns "$iconset" -o Support/Barista.icns
print "Built Support/Barista.icns"
