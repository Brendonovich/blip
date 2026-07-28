#!/bin/zsh
set -euo pipefail

if (( $# != 8 )); then
    print -u2 "Usage: generate-appcast.sh TITLE ARTIFACT ASSET_NAME OUTPUT VERSION BUILD TAG SIGNATURE"
    exit 2
fi

title="$1"
artifact="$2"
asset_name="$3"
output="$4"
version="$5"
build="$6"
tag="$7"
signature="$8"
length="$(stat -f '%z' "$artifact")"
publication_date="$(LC_ALL=C date -u '+%a, %d %b %Y %H:%M:%S %z')"

cat > "$output" <<EOF
<?xml version="1.0" encoding="utf-8"?>
<rss version="2.0" xmlns:sparkle="http://www.andymatuschak.org/xml-namespaces/sparkle">
  <channel>
    <title>$title updates</title>
    <item>
      <title>Version $version</title>
      <pubDate>$publication_date</pubDate>
      <enclosure
        url="https://github.com/Brendonovich/blip/releases/download/$tag/$asset_name"
        sparkle:version="$build"
        sparkle:shortVersionString="$version"
        sparkle:edSignature="$signature"
        length="$length"
        type="application/octet-stream" />
    </item>
  </channel>
</rss>
EOF
