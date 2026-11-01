#!/bin/zsh
# Prints CHANGELOG.md's entry for a version (default: the one in Version.swift).
# Fails if the entry is missing or empty, so a release can't ship without notes.
# Usage: zsh scripts/release-notes.sh [VERSION]
set -euo pipefail
cd "$(dirname "$0")/.."
version="${1:-$(sed -n 's/.*static let current = "\(.*\)".*/\1/p' Sources/BaristaCore/Version.swift)}"
notes="$(awk -v version="$version" '
    index($0, "## [" version "]") == 1 { found = 1; next }
    found && /^## \[/ { exit }
    found { print }
' CHANGELOG.md | sed -e '/./,$!d' | sed -e :a -e '/^\n*$/{$d;N;ba' -e '}')"
if [[ -z "${notes//[[:space:]]/}" ]]; then
    print -u2 "CHANGELOG.md has no entry for $version. Add a '## [$version] - $(date +%Y-%m-%d)' section describing what changed."
    exit 1
fi
if ! grep -qE "^## \[$version\] - [0-9]{4}-[0-9]{2}-[0-9]{2}$" CHANGELOG.md; then
    print -u2 "CHANGELOG.md's heading for $version needs a date: '## [$version] - YYYY-MM-DD'."
    exit 1
fi
print -r -- "$notes"
