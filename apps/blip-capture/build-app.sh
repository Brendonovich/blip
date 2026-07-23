#!/bin/zsh
set -euo pipefail

script_dir="${0:A:h}"
workspace="${script_dir:h:h}"
target_dir="$workspace/target"
bundle_dir="$target_dir/release/bundle"
app="$bundle_dir/macos/Blip Capture.app"
dmg="$bundle_dir/dmg/Blip-Capture.dmg"
binary="$target_dir/release/blip-capture"
identity="${APPLE_SIGNING_IDENTITY:--}"
build_number="${BLIP_BUILD_NUMBER:-1}"
make_dmg=false
notarize=false
open_app=false

usage() {
    cat <<'EOF'
Usage: build-app.sh [--dmg] [--notarize] [--open]

Builds a release Blip Capture.app. The app is ad hoc signed unless
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
    print -u2 "--notarize requires APPLE_NOTARY_PROFILE."
    exit 2
fi
if [[ ! "$build_number" =~ '^[0-9]+([.][0-9]+)*$' ]]; then
    print -u2 "BLIP_BUILD_NUMBER must contain only integers separated by periods."
    exit 2
fi

package_id="$(cargo pkgid --manifest-path "$workspace/Cargo.toml" -p blip-capture)"
version="${package_id##*@}"
if [[ "$version" == "$package_id" ]]; then
    version="${package_id##*#}"
fi

print "Building Blip Capture $version ($build_number)..."
CARGO_TARGET_DIR="$target_dir" cargo build \
    --manifest-path "$workspace/Cargo.toml" \
    --package blip-capture \
    --release \
    --locked

rm -rf "$app"
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"
cp "$binary" "$app/Contents/MacOS/blip-capture"
cp "$script_dir/Info.plist" "$app/Contents/Info.plist"

plutil -replace CFBundleShortVersionString -string "$version" "$app/Contents/Info.plist"
plutil -replace CFBundleVersion -string "$build_number" "$app/Contents/Info.plist"
if [[ -f "$script_dir/AppIcon.icns" ]]; then
    cp "$script_dir/AppIcon.icns" "$app/Contents/Resources/AppIcon.icns"
    plutil -insert CFBundleIconFile -string "AppIcon" "$app/Contents/Info.plist"
fi

strip -S -x "$app/Contents/MacOS/blip-capture"

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
    "com.brendonovich.blip.capture"
test "$(plutil -extract CFBundleShortVersionString raw "$app/Contents/Info.plist")" = "$version"

if [[ "$make_dmg" == true ]]; then
    staging="$bundle_dir/dmg/staging"
    rm -rf "$staging" "$dmg"
    mkdir -p "$staging"
    ditto "$app" "$staging/Blip Capture.app"
    ln -s /Applications "$staging/Applications"
    hdiutil create \
        -volname "Blip Capture" \
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
    xcrun notarytool submit "$dmg" \
        --keychain-profile "$APPLE_NOTARY_PROFILE" \
        --wait
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
