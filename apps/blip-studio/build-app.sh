#!/bin/zsh
set -euo pipefail

script_dir="${0:A:h}"
workspace="${script_dir:h:h}"
target_dir="$workspace/target"
bundle_dir="$target_dir/release/bundle"
app="$bundle_dir/macos/Blip Studio.app"
dmg="$bundle_dir/dmg/Blip-Studio.dmg"
binary="$target_dir/release/blip-studio"
identity="${APPLE_SIGNING_IDENTITY:--}"
build_number="${BLIP_BUILD_NUMBER:-1}"
make_dmg=false
notarize=false
open_app=false

usage() {
    cat <<'EOF'
Usage: build-app.sh [--dmg] [--notarize] [--open]

Builds a release Blip Studio.app. The app is ad hoc signed unless
APPLE_SIGNING_IDENTITY is set to a Developer ID Application identity.

Options:
  --dmg       Also create a compressed disk image.
  --notarize  Notarize and staple the DMG using APPLE_NOTARY_PROFILE.
  --open      Open the finished app after validation.
  -h, --help  Show this help.
EOF
}

while (( $# > 0 )); do
    case "$1" in
        --dmg)
            make_dmg=true
            ;;
        --notarize)
            notarize=true
            make_dmg=true
            ;;
        --open)
            open_app=true
            ;;
        -h|--help)
            usage
            exit 0
            ;;
        *)
            print -u2 "Unknown option: $1"
            usage >&2
            exit 2
            ;;
    esac
    shift
done

if [[ "$notarize" == true && "$identity" == "-" ]]; then
    print -u2 "--notarize requires APPLE_SIGNING_IDENTITY."
    exit 2
fi
if [[ "$notarize" == true && -z "${APPLE_NOTARY_PROFILE:-}" ]]; then
    if [[ -z "${APPLE_NOTARY_KEY_PATH:-}" || -z "${APPLE_NOTARY_KEY_ID:-}" ]]; then
        print -u2 "--notarize requires APPLE_NOTARY_PROFILE or an API key path and ID."
        exit 2
    fi
fi
if [[ ! "$build_number" =~ '^[0-9]+([.][0-9]+)*$' ]]; then
    print -u2 "BLIP_BUILD_NUMBER must contain only integers separated by periods."
    exit 2
fi

package_id="$(cargo pkgid --manifest-path "$workspace/Cargo.toml" -p blip-studio)"
version="${package_id##*@}"
if [[ "$version" == "$package_id" ]]; then
    version="${package_id##*#}"
fi

print "Building Blip Studio $version ($build_number)..."
CARGO_TARGET_DIR="$target_dir" cargo build \
    --manifest-path "$workspace/Cargo.toml" \
    --package blip-studio \
    --release \
    --locked

rm -rf "$app" "$bundle_dir/Blip Studio.dSYM"
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"
cp "$binary" "$app/Contents/MacOS/blip-studio"
cp "$script_dir/Info.plist" "$app/Contents/Info.plist"

plutil -replace CFBundleShortVersionString -string "$version" "$app/Contents/Info.plist"
plutil -replace CFBundleVersion -string "$build_number" "$app/Contents/Info.plist"
if [[ -f "$script_dir/AppIcon.icns" ]]; then
    cp "$script_dir/AppIcon.icns" "$app/Contents/Resources/AppIcon.icns"
    plutil -insert CFBundleIconFile -string "AppIcon" "$app/Contents/Info.plist"
fi

strip -S -x "$app/Contents/MacOS/blip-studio"

sign_args=(--force --sign "$identity")
if [[ "$identity" != "-" ]]; then
    sign_args+=(--options runtime --timestamp)
fi
if [[ -n "${APPLE_ENTITLEMENTS:-}" ]]; then
    sign_args+=(--entitlements "$APPLE_ENTITLEMENTS")
fi
codesign "${sign_args[@]}" "$app"

plutil -lint "$app/Contents/Info.plist"
codesign --verify --deep --strict --verbose=2 "$app"
test "$(plutil -extract CFBundleIdentifier raw "$app/Contents/Info.plist")" = \
    "dev.brendonovich.blip.studio"
test "$(plutil -extract CFBundleShortVersionString raw "$app/Contents/Info.plist")" = "$version"

if [[ "$make_dmg" == true ]]; then
    staging="$bundle_dir/dmg/staging"
    rm -rf "$staging" "$dmg"
    mkdir -p "$staging"
    ditto "$app" "$staging/Blip Studio.app"
    ln -s /Applications "$staging/Applications"
    hdiutil create \
        -volname "Blip Studio" \
        -srcfolder "$staging" \
        -ov \
        -format UDZO \
        "$dmg"
    rm -rf "$staging"
    hdiutil verify "$dmg"

    if [[ "$identity" != "-" ]]; then
        codesign --force --sign "$identity" --timestamp "$dmg"
        codesign --verify --verbose=2 "$dmg"
    fi
fi

if [[ "$notarize" == true ]]; then
    if [[ -n "${APPLE_NOTARY_PROFILE:-}" ]]; then
        notary_args=(--keychain-profile "$APPLE_NOTARY_PROFILE")
    else
        notary_args=(--key "$APPLE_NOTARY_KEY_PATH" --key-id "$APPLE_NOTARY_KEY_ID")
        if [[ -n "${APPLE_NOTARY_ISSUER_ID:-}" ]]; then
            notary_args+=(--issuer "$APPLE_NOTARY_ISSUER_ID")
        fi
    fi
    xcrun notarytool submit "$dmg" "${notary_args[@]}" --wait
    xcrun stapler staple "$dmg"
    xcrun stapler validate "$dmg"
    spctl --assess --type open --context context:primary-signature --verbose=2 "$dmg"
fi

print "App:  $app"
if [[ "$make_dmg" == true ]]; then
    print "DMG:  $dmg"
fi
if [[ "$open_app" == true ]]; then
    open -n "$app"
fi
