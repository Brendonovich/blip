# Vendored GPUI

This directory contains all `gpui*` crates from
[`zed-industries/zed`](https://github.com/zed-industries/zed) at commit
`aba12fc8a0fe44a0742acc0d096e843d07385962`.

The crate manifests replace Zed's workspace lint inheritance with its
`unexpected_cfgs` allowance so they can build as members of this workspace.
Their unpublished Zed support dependencies are pinned to the same upstream
commit in the root workspace manifest.

Run `cargo lint` to lint the workspace application and libraries while excluding
the vendored GPUI packages.

## Applied Patches

- [zed-industries/zed#61291](https://github.com/zed-industries/zed/pull/61291),
  commit `efa21a1bbb0831fa4e27604c2ed4481932c9537f`: support zero-copy,
  single-plane BGRA `CVPixelBuffer` surfaces in `gpui_macos`.
