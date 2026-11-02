#!/bin/zsh
# Notarizes a signed .zip, .dmg, or .app with Apple and staples the ticket.
# Usage: zsh scripts/notarize.sh PATH [STAPLE_TARGET]
#   PATH           what to submit (an .app is zipped first)
#   STAPLE_TARGET  what to staple (defaults to PATH; pass the .app when submitting its zip)
# Needs an App Store Connect API key: NOTARY_KEY_PATH (.p8 file), NOTARY_KEY_ID, NOTARY_ISSUER_ID.
set -euo pipefail
submit="${1:?Usage: notarize.sh PATH [STAPLE_TARGET]}"
staple="${2:-$submit}"
: "${NOTARY_KEY_PATH:?}" "${NOTARY_KEY_ID:?}" "${NOTARY_ISSUER_ID:?}"
auth=(--key "$NOTARY_KEY_PATH" --key-id "$NOTARY_KEY_ID" --issuer "$NOTARY_ISSUER_ID")

if [[ "$submit" == *.app ]]; then
    upload="$(mktemp -d)/${submit:t:r}.zip"
    ditto -c -k --sequesterRsrc --keepParent "$submit" "$upload"
else
    upload="$submit"
fi

print "Submitting ${upload:t} for notarization…"
# notarytool exits 0 even when Apple rejects the upload, so check the status itself. It exits
# nonzero on upload errors and timeouts; keep going then, to report what it said.
result="$(xcrun notarytool submit "$upload" "${auth[@]}" --wait --timeout 3h --output-format json)" || true
print -r -- "$result"
# Not "status": that's a read-only variable in zsh.
state="$(print -r -- "$result" | plutil -extract status raw -o - - 2>/dev/null)" || state=""
id="$(print -r -- "$result" | plutil -extract id raw -o - - 2>/dev/null)" || id=""
if [[ "$state" != "Accepted" ]]; then
    print -u2 "Notarization didn't succeed (status: '${state:-unknown}', submission: '${id:-none}')."
    if [[ -n "$id" ]]; then
        print -u2 "Apple's log:"
        xcrun notarytool log "$id" "${auth[@]}" >&2 || true
    fi
    exit 1
fi
xcrun stapler staple "$staple"
xcrun stapler validate "$staple"
