# Blip Capture

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
