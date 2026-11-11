#!/bin/bash
# Signs Cutaway.app with an Apple Developer ID, and has Apple notarize it, so
# a Mac opens it without "Apple could not verify Cutaway". The Release
# workflow runs this; it works on your own Mac too. Run ./macos/bundle.sh first.
#
#   ./macos/sign.sh sign "Developer ID Application: Your Name (TEAMID)" [profile.provisionprofile]
#   ./macos/sign.sh notarize path/to/AuthKey_XXXX.p8 KEY_ID ISSUER_ID
#
# sign:     hardened runtime, a secure timestamp, and the permissions in
#           macos/Cutaway.entitlements. With a Developer ID provisioning
#           profile (System Extension capability) it also adds the virtual
#           camera (macos/virtualcam/build.sh).
# notarize: sends the app to Apple's notary service, waits, staples the
#           approval onto the app (so it opens offline too) and checks it the
#           way Gatekeeper on a customer's Mac would.
set -euo pipefail
cd "$(dirname "$0")/.."

app=target/Cutaway.app
[[ -d "$app" ]] || { echo "Run ./macos/bundle.sh first" >&2; exit 1; }

case "${1:-}" in
sign)
    identity="${2:?the signing identity, e.g. Developer ID Application: Your Name (TEAMID)}"
    profile="${3:-}"
    # The team ID is the bit in brackets at the end of the identity's name.
    team=$(sed -nE 's/.*\(([A-Z0-9]{10})\)$/\1/p' <<< "$identity")
    [[ -n "$team" ]] || { echo "Couldn't read the team ID from \"$identity\"" >&2; exit 1; }
    if [[ -n "$profile" ]]; then
        ./macos/virtualcam/build.sh "$identity" "$team" "$profile"
    else
        codesign --force --options runtime --timestamp --sign "$identity" \
            --entitlements macos/Cutaway.entitlements "$app"
    fi
    codesign --verify --deep --strict --verbose=2 "$app"
    codesign --display --verbose=2 "$app" 2>&1 | grep -E '^(Authority|TeamIdentifier|Timestamp|Runtime)' || true
    echo "Signed $app as $identity"
    ;;
notarize)
    key="${2:?path to the App Store Connect API key (.p8)}"
    key_id="${3:?the Key ID of that key}"
    issuer="${4:?the Issuer ID}"
    work=target/notarize
    rm -rf "$work" && mkdir -p "$work"
    ditto -c -k --keepParent "$app" "$work/Cutaway.zip"
    echo "Sending to Apple's notary service (usually a few minutes)…"
    xcrun notarytool submit "$work/Cutaway.zip" --key "$key" --key-id "$key_id" --issuer "$issuer" \
        --wait --timeout 45m --output-format json > "$work/result.json" || true
    cat "$work/result.json"
    status=$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1])).get("status",""))' "$work/result.json" 2>/dev/null || true)
    if [[ "$status" != "Accepted" ]]; then
        id=$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1])).get("id",""))' "$work/result.json" 2>/dev/null || true)
        if [[ -n "$id" ]]; then
            # Apple's list of what it didn't like.
            xcrun notarytool log "$id" --key "$key" --key-id "$key_id" --issuer "$issuer" || true
        fi
        echo "Apple didn't accept the app (status: ${status:-none})." >&2
        exit 1
    fi
    xcrun stapler staple "$app"
    xcrun stapler validate "$app"
    spctl --assess --type execute --verbose=4 "$app"
    echo "Notarized and stapled $app"
    ;;
*)
    sed -n '2,15p' "$0" >&2
    exit 1
    ;;
esac
