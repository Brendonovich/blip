# Blip Capture

## Remote Recordings

Create a Capture key on your Blip server and choose **Add to Blip Capture** to
create and select its recording profile. You can also open **Profile > Edit
Recording Profiles**, add a profile, select **Blip server**, and paste the URL:

```text
https://blip-server.example.workers.dev#upload-token
```

Blip writes the recording to a temporary MP4, uploads it in the chunk size
selected by the server, then opens and copies the private viewer link. A failed
upload leaves the MP4 on disk and reports its location.

### Headless Upload Test

Run a timed recording of the main display through the same recording and upload
path as the app. The private viewer URL is printed to stdout after the upload
completes:

```sh
BLIP_SERVER_URL='https://blip-server.example.workers.dev#upload-token' \
  cargo run -p blip-capture -- --headless --duration 5
```

Use `--display ID` to select another display and `--format hls` to test the HLS
upload path. Screen Recording permission must already be granted to the binary
running the command. Failed uploads leave the temporary recording in place and
print its path to stderr for inspection.

## Production Build

Build an optimized, validated macOS app with an ad hoc signature:

```sh
apps/blip-capture/build-app.sh
```

Add `--dmg` to produce an installable disk image or `--open` to launch the
finished app. Artifacts are written under `target/release/bundle/`.

For Developer ID signing and notarization, use the same
`APPLE_SIGNING_IDENTITY`, `APPLE_NOTARY_PROFILE`, `APPLE_ENTITLEMENTS`, and
`BLIP_BUILD_NUMBER` settings documented by Blip Studio.

Production bundles include Sparkle and check the app-specific GitHub release
feed automatically. The release workflow publishes the corresponding appcast
alongside each signed and notarized DMG. Developer ID builds require the public
EdDSA key in `SPARKLE_PUBLIC_KEY`; releases use the matching base64 private key
from the `SPARKLE_PRIVATE_KEY` Actions secret.
