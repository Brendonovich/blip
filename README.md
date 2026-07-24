# Blip

Blip is a little Rust workspace for experimenting with native macOS screen
capture, recording, compositing, and streaming. There are three apps in here:

## Blip Capture

The simple, everyday recorder. Pick a display, window, or region, hit record,
and save the result to a folder or the clipboard. See
[`apps/blip-capture`](apps/blip-capture) for macOS build instructions.

## Blip Studio

The more ambitious one: a real-time scene compositor for captured screens and
cameras, with an interactive preview and RTMP streaming. It can also run
headlessly from a JSON scene. See [`apps/blip-studio`](apps/blip-studio) for
macOS build and distribution notes.

## Blip CLI

The useful low-level tool for poking at the capture stack. It lists available
displays and windows, streams frames for inspection, and records directly to
MP4. Run `cargo run -p blip-cli -- --help` to see the commands.

The shared capture plumbing lives in `crates/`, while `gpui/` contains the UI
framework used by the desktop apps.
