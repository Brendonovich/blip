# Blip

Blip contains macOS capture, recording, compositing, streaming, and hosting
projects.

## Blip Capture

Blip Capture records a display, window, or region with an optional camera
preview, then saves, edits, or uploads the recording. It uses ScreenCaptureKit
and AVFoundation for capture and encoding, WGPU for composition, and GPUI for
the interface. See [`apps/blip-capture`](apps/blip-capture) for build and remote
recording instructions.

## Blip Server

Blip Server accepts recordings, stores them in each user's S3-compatible
bucket, and serves private playback links. A shared SolidStart and Effect
application runs as either a [Node process with SQLite](apps/blip-server) or a
[Cloudflare Worker with D1](apps/blip-server-cloudflare), with authentication
provided by Better Auth.

## Blip Studio

Blip Studio combines screen, window, camera, and graphic sources into a scene
and streams it over RTMP. It uses ScreenCaptureKit and AVFoundation for inputs,
WGPU for composition, FFmpeg for streaming, and GPUI for interactive control;
it can also run a JSON scene without the interface. See
[`apps/blip-studio`](apps/blip-studio) for build and distribution instructions.

## Blip CLI

Blip CLI lists and inspects capture targets, streams frames for diagnostics,
and records displays or windows to MP4. It calls the shared ScreenCaptureKit
and AVFoundation crates without a graphical interface. Run
`cargo run -p blip-cli -- --help` to see the commands.

Shared capture, media, compositor, and updater code lives in `crates/`. The
desktop interfaces use the GPUI source in `gpui/`.
