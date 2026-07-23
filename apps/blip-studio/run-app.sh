#!/bin/zsh
set -euo pipefail

script_dir="${0:A:h}"
workspace="${script_dir:h:h}"
app="$workspace/target/debug/Blip Studio.app"

cargo build --manifest-path "$workspace/Cargo.toml" -p blip-studio
mkdir -p "$app/Contents/MacOS"
cp "$workspace/target/debug/blip-studio" "$app/Contents/MacOS/blip-studio"
cp "$script_dir/Info.plist" "$app/Contents/Info.plist"
codesign --force --sign - "$app"
open -n "$app" --args "$@"
