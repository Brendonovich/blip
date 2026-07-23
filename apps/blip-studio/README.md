# Blip Studio

## Production Build

Build an optimized, validated macOS app with an ad hoc signature:

```sh
apps/blip-studio/build-app.sh
```

Add `--dmg` to produce an installable disk image or `--open` to launch the
finished app. Artifacts are written under `target/release/bundle/`, including a
native-architecture `.app` bundle.

For distribution, provide a Developer ID Application certificate installed in
your keychain:

```sh
APPLE_SIGNING_IDENTITY="Developer ID Application: Your Name (TEAMID)" \
  apps/blip-studio/build-app.sh --dmg
```

To notarize, first store App Store Connect credentials in the keychain:

```sh
xcrun notarytool store-credentials blip-notary
```

Then build, sign, notarize, and staple the disk image:

```sh
APPLE_SIGNING_IDENTITY="Developer ID Application: Your Name (TEAMID)" \
APPLE_NOTARY_PROFILE="blip-notary" \
  apps/blip-studio/build-app.sh --notarize
```

Set `BLIP_BUILD_NUMBER` to override `CFBundleVersion`. Set
`APPLE_ENTITLEMENTS` to an entitlements plist if future features require one.
