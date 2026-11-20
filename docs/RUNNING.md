# Build and run a title on Windows

## Requirements

- Windows 10/11 x64 with a Direct3D 12 GPU.
- Microsoft Gaming Services (provides `xgameruntime.dll`).
- Rust toolchain (`x86_64-pc-windows-msvc`) and the Windows SDK (`dxcompiler.dll` and
  `dxil.dll`, found automatically or through `TOMBOX_DXC_DIR`).
- Your own authorised, decrypted title dump.

## Build

```
cargo build --release
cargo test --release
```

The compatibility DLLs must be built with a static CRT. Run these commands in Command Prompt.

```bat
set RUSTFLAGS=-C target-feature=+crt-static
cargo build --release -p d3d12_xs -p xmem -p xcrdapi -p acphal -p dxcompiler_xs -p mfplat
```

## Run the tested Fortnite build

The steps below apply to build 2.0.1461.7919. The development scripts use machine-specific paths. [Development tools](../tools/dev/README.md)

1. Extract the dump to `input/extracted/` (game files end up under `input/extracted/Mount/`).
2. Map that folder to drive `G:`, where the game expects its filesystem root:
   `subst G: "%CD%\input\extracted\Mount"`
3. Copy `acphal.dll`, `d3d12_xs.dll`, `dxcompiler_xs.dll`, `mfplat.dll`, `xcrdapi.dll` and
   `xmem.dll` from `target/release/` into the folder of the game executable
   (`G:\FortniteGame\Binaries\XSX\`).
4. Copy the **real** `xg_xs.dll` from the dump (`EmbeddedXvd/Windows/System32/`) into the
   same folder. Texture layouts depend on it; do not use the stub crate of that name.
5. Optional: create `tombox.cfg` in that folder:

   ```
   net_redirect=<your server>   # send all traffic to one host (omit to stay offline)
   gamertag=<name>              # local offline user (default TomBoxPlayer)
   xuid=<decimal id>
   xbl_token=<token string your server expects>
   ```
6. Run the game executable (`FortniteClient-XSX-Shipping.exe`).

Compatibility-layer calls are logged to `%TEMP%\tombox_compat.log` (or `%TOMBOX_LOG%`). The
game logs only through the debugger channel; `tools/dev/mdbg.rs` is a small tracer for it.

| Variable | Effect |
|---|---|
| `TOMBOX_NET_REDIRECT` | Same as `net_redirect` |
| `TOMBOX_ALLOW_NETWORK=1` | Turn the network guard off (unrestricted access) |
| `TOMBOX_D3D12_DEBUG=1` | Direct3D validation layer |
| `TOMBOX_RTCAP=1` | Diagnostics: render-target capture, per-draw trace, shader skip list and override, binding trace (see `docs/DEVELOPMENT_LOG.md`) |
| `TOMBOX_OWN_WINDOW=1` | Present into a separate window |
