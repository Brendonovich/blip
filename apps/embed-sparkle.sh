#!/bin/zsh
set -euo pipefail

if (( $# != 2 )); then
    print -u2 "Usage: embed-sparkle.sh APP_PATH TARGET_DIR"
    exit 2
fi

app="$1"
target_dir="$2"
version="2.9.4"
checksum="ce89daf967db1e1893ed3ebd67575ed82d3902563e3191ca92aaec9164fbdef9"
sparkle_dir="$target_dir/sparkle-$version"
archive="$target_dir/Sparkle-$version.tar.xz"

if [[ ! -d "$sparkle_dir/Sparkle.framework" || ! -x "$sparkle_dir/bin/sign_update" ]]; then
    mkdir -p "$sparkle_dir"
    if [[ ! -f "$archive" ]]; then
        curl --fail --location --silent --show-error \
            "https://github.com/sparkle-project/Sparkle/releases/download/$version/Sparkle-$version.tar.xz" \
            --output "$archive"
    fi
    actual_checksum="$(shasum -a 256 "$archive" | cut -d ' ' -f 1)"
    if [[ "$actual_checksum" != "$checksum" ]]; then
        print -u2 "Sparkle archive checksum mismatch."
        exit 1
    fi
    tar -xJf "$archive" -C "$sparkle_dir" \
        ./Sparkle.framework \
        ./bin/generate_keys \
        ./bin/sign_update
fi

mkdir -p "$app/Contents/Frameworks"
ditto "$sparkle_dir/Sparkle.framework" "$app/Contents/Frameworks/Sparkle.framework"
