# Coin Run and Linux test guide

## Linux research tools

Run the shell commands in this guide from the repository root unless a command changes directory.

On x86-64 Linux you can inspect PE files and ZIP archives, disassemble x86-64 code,
and translate Xbox shaders to HLSL. The checked-in fixtures let you start without a
game dump. From the repository root, run the tests and scan a sample pixel shader:

```sh
./tuxbox test
cargo run --locked -p tuxbox-xshader --bin xscan -- analysis/shaders/fortnite_ps_sample.dxbc
```

The scan should report one translated pixel shader and zero failures. To read the
generated HLSL, run:

```sh
cargo run --locked -p tuxbox-xshader --bin xshader -- analysis/shaders/fortnite_ps_sample.dxbc --hlsl-out sample.hlsl
```

TuxBox builds the runtime and DXC integration as Windows binaries for Proton and native Windows. Coin Run exercises those DLLs under Proton. A retail title run on Linux remains unverified.

## Terminal Coin Run

`coin_run` is a terminal game that imports `XMemAlloc` and `XMemFree` from
TuxBox's `xmem.dll`. Its board lives in that allocation. A scripted win exercises
DLL loading, allocation, reads, writes and release without a game dump or console SDK.
Input and output use the host terminal. Graphics, Xbox input and platform services
need their own tests.

On Linux with Wine, cargo-xwin and the Rust `x86_64-pc-windows-msvc` target:

```sh
RUSTFLAGS='-C target-feature=+crt-static' cargo xwin build --locked --release \
  --target x86_64-pc-windows-msvc -p xmem --lib --example coin_run
cd target/x86_64-pc-windows-msvc/release
cp examples/coin_run.exe .
printf 'dddaaass\n' | TOMBOX_LOG=coin_run.log wine ./coin_run.exe
```

A win prints `Both coins collected!` and exits with status 0. The log records
allocation and release of the same address. Run `wine ./coin_run.exe` interactively
to play. On Windows, build with Cargo for the same target and run the example EXE
beside the freshly built DLL.

## Headless graphics test

`clear_pixel.exe` uses TuxBox's `d3d12_xs.dll` to create a device and an Xbox
command queue and list. It clears a one-pixel RGBA8 render target to magenta,
copies it to a readback buffer and checks all four bytes after the GPU fence
completes. It also serializes and deserializes a synthetic root signature.
The test requires no game assets, middleware or shader compiler.

On Linux, build from the repo root with cargo-xwin, the x64 MSVC Rust target,
Python 3, clang-cl and lld-link. The script uses cargo-xwin's SDK and CRT cache. Set
`XWIN_SDK_ROOT` to the directory containing `sdk/` and `crt/` for a custom cache.
Set `PROTON` to the absolute path of the Proton Experimental `proton` executable.

```sh
tools/homebrew/build-clear.sh
cd target/x86_64-pc-windows-msvc/release
export STEAM_COMPAT_DATA_PATH="$PWD/clear-pixel-prefix"
export STEAM_COMPAT_CLIENT_INSTALL_PATH="$HOME/.steam/root"
mkdir -p "$STEAM_COMPAT_DATA_PATH"
TOMBOX_LOG=clear_pixel.log "$PROTON" run cmd.exe /d /c 'clear_pixel.exe > clear-result.txt 2>&1'
cat clear-result.txt
```

The verified result under Proton Experimental on an RTX 4070 Ti SUPER was
`RGBA readback: 255 0 255 255` with exit code 0. The stock Wine backend on
the same machine rejected D3D feature level 12.0 at device creation.

Run the same executable with `--triangle` to exercise the shader translator and
draw path. The build script generates original vertex and pixel shader containers
from short RDNA2 programs. At runtime TuxBox translates both during Xbox graphics
pipeline creation. This mode needs DXC or the D3DCompile fallback provided by Proton.
The vertex shader passes through three clip-space positions. The pixel shader writes
opaque green. Readback checks all 256 pixels against the triangle on a magenta
background, returning a failure exit code on any mismatch.

```sh
TOMBOX_LOG=triangle.log "$PROTON" run cmd.exe /d /c 'clear_pixel.exe --triangle > triangle-result.txt 2>&1'
cat triangle-result.txt
```

## Graphical Coin Run

Coin Run is a homebrew game you can build and play on Linux with Proton. All
sprites and shaders are generated from source in this repo. Move the cyan player
with WASD, the D-pad or the left stick and collect both coins. A held controller direction
moves one tile. The last coin turns silver. Press R or the controller’s Menu
(Start) button to restart, or Q or Escape on the keyboard to quit.

Set `PROTON` to the absolute path of Proton Experimental's `proton` executable.
With the Linux build tools above installed, run these commands from the repo root.

```sh
tools/homebrew/build-clear.sh
tools/homebrew/play-coins.sh
```

The launcher also builds on first use. Re-run the build script after changing the
examples. The graphical and terminal games share their Rust rules and XMem-backed
board. The default 2D view draws two triangles per tile. Its synthetic RDNA2 pixel
shader samples a 16×4 RGBA8 sprite atlas through separate texture and point-sampler
descriptor tables. Picking up the first coin copies a single 4×4 region into the
atlas. Restarting restores the original pixels using the same GPU resources.

| Command from the repo root | What it exercises |
|---|---|
| `tools/homebrew/play-coins.sh --device-diagnostics-test` | Calls each Device slot from 91 through 159 and checks its HRESULT, slot number and caller location in the trace. |
| `tools/homebrew/play-coins.sh --command-list-test` | Checks 89 diagnostic paths through slot 148 and their caller traces. A tagged BVH fixture clears a GPU counter and verifies both neighboring words. |
| `tools/homebrew/play-coins.sh --gamechat-test /path/to/GameChat2.dll` | Loads the supplied desktop provider by basename, changes audio and networking thread affinity, checks both getters, then restores the previous settings. |
| `tools/homebrew/play-coins.sh --scarlett-libraries-test /path/to/dlls` | Checks the 16 supplied libraries using native calls, exported symbols and embedded GPU programs. [Probe scope](#local-scarlett-libraries) |
| `tools/homebrew/play-coins.sh --rdna2-test` | Verifies arithmetic and branches through the game's Xbox compute pipeline, runs the memory conformance suite, and checks rendering and dispatch. [Requirements and coverage report](#rdna2-verification) |
| `tools/homebrew/play-coins.sh --3d` | Play on a perspective board with raised walls, octagonal coins and a camera that follows the player. Uses the same controls and rules. |
| `tools/homebrew/play-coins.sh --3d-test` | Runs the 20-frame route in 3D, checking textured top and side faces, camera movement and depth occlusion by GPU readback. Requires a display. |
| `tools/homebrew/play-coins.sh --audio-test` | Plays the board twice, checks which stereo channel carries each pickup, and measures both voices playing together on a private Linux audio monitor. |
| `tools/homebrew/play-coins.sh --controller-test` | Drives the 3D board with two virtual Linux pads through Proton. Checks collisions, pickup rumble on the active pad, handover after unplugging it and Menu to restart. |
| `tools/homebrew/play-coins.sh --gameinput-test` | Imports `GameInputCreate` by ordinal 1, checks reading history, callbacks, hotplug and motor output, then plays the controller route through the TuxBox GameInput DLL. |
| `tools/homebrew/play-coins.sh --test` | Upload, draw and GPU readback using a committed texture, headless |
| `tools/homebrew/play-coins.sh --placed-test` | Same route using `CreatePlacedResourceX` with a 64 KiB-aligned XMem address and Xbox layout 261, headless |
| `tools/homebrew/play-coins.sh --present-test` | Committed texture plus `PresentX` to a window, with the same render-target readback |
| `tools/homebrew/play-coins.sh --tiled` | Play with a CPU-written 64×32 placed atlas. The runtime detects writes during `PresentX` and uploads the decoded texture. |
| `tools/homebrew/play-coins.sh --tiled-test` | Scripted route with the CPU-written atlas, including silver and gold rewrites. Requires a display. |
| `tools/homebrew/play-coins.sh --tiled-reference` | Downloads two hash-pinned public DDS files, draws the Scarlett image and checks every pixel against the separate linear image. Requires a display. |
| `tools/homebrew/play-coins.sh --mip-reference` | Downloads the same two pinned files, samples all nine mips, changes the 1×1 mip in CPU memory and checks all nine again. Requires a display. |
| `tools/homebrew/play-coins.sh --bc1-reference` | Downloads a pinned Scarlett BC1 texture and compares every mip with a committed PC BC1 texture on the same GPU. Repeats after a CPU write to the smallest mip. Requires a display. |
| `tools/homebrew/play-coins.sh --rect-test` | Generates four offline rectangular textures, checks every mip on the GPU, edits the 1×1 mip and checks again. Requires a display. |
| `tools/homebrew/play-coins.sh --array-test` | Point-samples every slice and mip of two offline texture arrays, changes the 1×1 mip of the middle slice and checks every pixel again. Requires a display. |
| `tools/homebrew/play-coins.sh --cube-test` | Generates an RGBA8 cube map, checks face selection and orientation on the GPU, edits the −Y face’s 1×1 mip and checks all six faces again. Requires a display. |
| `tools/homebrew/play-coins.sh --uav-test` | Clears an integer texture and a normalized texture through Xbox command-list slots 53 and 54. GPU readback checks the whole surface and a second, rectangular clear. Runs headless. |
| `tools/homebrew/play-coins.sh --scarlett-copy-test` | Creates two committed textures through device slot 76, uploads a pattern and copies a 5×4 rectangle between different coordinates through list slot 85. Checks the resource flags and all 54 destination pixels. |
| `tools/homebrew/play-coins.sh --indirect-test` | Three synthetic DLLs submit indirect draws on one list. Checks legacy slot 59, GXDK slot 61, argument offsets and a zero count buffer across 72 pixels. Exercises the PIX paths in the tagged DLLs. |
| `tools/homebrew/play-coins.sh --fidelityfx-test` | Downloads the hash-pinned public Xbox FidelityFX loader and upscaler plus the desktop PIX runtime. Checks four resolution queries through the real DLLs under Proton. |
| `tools/homebrew/play-coins.sh --fidelityfx-context` | Diagnoses FSR 3.1.6 context creation on a TuxBox device. Currently returns an error at offline compute PSO deserialization and saves the trace in the release directory as `fidelityfx-context.log`. |
| `tools/homebrew/play-coins.sh --thread-dispatch-test` | Exercises GXDK 250405 slot 80 with one- and four-thread groups across pipeline changes, reset and list creation. Checks all 32 values, including the untouched tail. |
| `tools/homebrew/play-coins.sh --compute-barrier-test` | Issues the GXDK 250405 slot-146 global UAV barrier between two translated compute shaders. Checks the barrier trace and reads back all 32 incremented values. |
| `tools/homebrew/play-coins.sh --pso-fallback-test` | Exercises the upscaler’s fallback from offline pipeline packets at device slot 67 to a translated compute pipeline at slot 62. Reads back 32 shader writes. |
| `tools/homebrew/play-coins.sh --discard-test` | Discards a whole render target and a rectangle through slot 55, overwrites the discarded data and checks all 54 texels. Also checks the shim log for an unmapped call. Runs headless. |
| `tools/homebrew/play-coins.sh --state-test` | Submits two command lists in reverse recording order, edits one mip, copies it back and samples its neighbor. Checks the uploader’s barrier arguments and all 1,280 pixels. Requires a display. |
| `tools/homebrew/play-coins.sh --stream-test` | Streams a two-mip RGBA8 texture through XMem, checks the rendered pixels and verifies upload order across partial mapping, eviction and reload. Runs offline and requires a display. |

### Audio

In interactive 2D and 3D play, each keyboard or controller pickup plays a
120 ms tone through the host’s `XAudio2_9.dll`. The left coin plays at 440 Hz
and the right coin at 880 Hz. Coin Run resolves `XAudio2Create` by ordinal 1
and submits generated 48 kHz, 16-bit mono PCM to two source voices. Each
voice’s output matrix sends the tone to the stereo mastering voice at full
amplitude on the coin’s side and one quarter on the other. Play continues if
audio is unavailable. [XAudio2 output matrices](https://learn.microsoft.com/en-us/windows/win32/api/xaudio2/nf-xaudio2-ixaudio2voice-setoutputmatrix)

The audio test uses Python 3, `dbus-run-session`, PipeWire with
`pipewire-pulse`, WirePlumber with the `policy` profile, and the `pactl` and
`parec` utilities. It starts its own audio server and virtual output, records
both channels, and checks the pitch, duration and stereo gain of all four
pickups across a restart. After the route it plays both voices together and
checks that each retains its solo level in the mix. The fixture needs no
speakers or game assets.

### Controllers

Coin Run requests gamepad readings from the host’s GameInput runtime and falls
back to XInput if the factory is unavailable or reading returns `E_NOTIMPL`.
The controller test requires a display (or Xvfb), Python’s `evdev` package,
write access to `/dev/uinput` and read access to the resulting joystick.
Run it with other controllers disconnected so the virtual pad is the player.
It prints the backend that supplied the readings. The test uses no game files
or physical controller.

The optional adapter builds to `target/x86_64-pc-windows-msvc/release/gameinput-xinput/GameInput.dll`.
It implements Windows SDK GameInput v0 gamepad polling, device callbacks, reading callbacks and two-motor rumble over XInput.
The ordinal probe checks button combinations, both sticks and triggers, the
lifetime of a retained reading and device identity after an observed disconnect.
A second probe starts with no controllers, then attaches two virtual pads and
checks blocking and asynchronous enumeration, device and status filters,
self-stop and timed unregistration. Coin Run disables its direct XInput fallback
during the gameplay check. [GameInput callback semantics](https://learn.microsoft.com/en-us/gaming/gdk/docs/features/common/input/advanced/input-callbacks)

A controller pickup in Coin Run requests a 180 ms pulse at 62.5% low-frequency
and 25% high-frequency strength. Coin Run also stops an active pulse on focus
loss or exit. The route checks the force-feedback events delivered to each Linux
virtual pad, including the stop after each pickup. Proton can report XInput
motors even for an input-only pad.

The adapter clamps each rumble channel to the range 0–1. It approximates each
trigger motor by adding a quarter of its requested strength to the high-frequency
body motor. The ordinal probe checks clamping, trigger mixing, a null-parameter
stop and that a retained handle leaves the replacement pad’s running effect alone.

Coin Run uses `GetNextReading` to process controller changes in order. The
history probe pauses its input loop while one virtual pad taps A and the other
moves its stick. After both return to neutral, the probe checks the four changes
in observation order, then walks backward with `GetPreviousReading` and checks
that both directions return the same reading objects. It also checks per-pad
history and disconnects. An expired reference returns
`GAMEINPUT_E_REFERENCE_READING_TOO_OLD`. A valid reference with no available
predecessor returns `GAMEINPUT_E_READING_NOT_FOUND`. [GameInput backward traversal](https://learn.microsoft.com/en-us/gaming/gdk/docs/reference/input/gameinput/interfaces/igameinput/methods/igameinput_getpreviousreading)

The same probe registers a reading callback on one pad while both pads send
input. The callback receives the A press and release in order. After unregistering,
the next tap still appears in history. For analog filtering the adapter compares
each stick axis and trigger with its last accepted state on that pad. Button
changes always qualify. While the dispatcher is busy, the adapter queues up to
64 further readings per subscription. On overflow it keeps the newest state and
sets the v0 overrun flag on that delivery.

The adapter samples connected controllers on a background loop with an 8 ms wait
between scans and retains half a second of history. Each connected pad’s latest
state stays available while it is idle. A tap or reconnection entirely between
scans can still be missed. Mouse input, adapter-wide focus policy and advanced haptics remain outside
its coverage. [GameInput reading model](https://learn.microsoft.com/en-us/gaming/gdk/docs/features/common/input/overviews/input-readings)
Retail compatibility still needs a title test.

### RDNA2 verification

Set `PROTON` as above. Place the Windows x64 DXC pair (`dxcompiler.dll` and
`dxil.dll`) in `target/x86_64-pc-windows-msvc/release`, or point
`TOMBOX_DXC_DIR` at their directory using a Proton path such as `Z:/opt/dxc/bin/x64`.
The full memory suite needs a D3D12 device with shader model 6.6. For a machine
without a display, run:

```sh
xvfb-run -a tools/homebrew/play-coins.sh --rdna2-test
```

The launcher rebuilds Coin Run and the runtime. The game creates an Xbox compute
pipeline for each generated shader, dispatches one workgroup per input and reads
back the result, VCC, EXEC and a completion marker. The Rust reference evaluates the
same inputs independently and checks the outputs relevant to each instruction.
A packed-root shader reads inline constants and a root UAV. It dispatches three 32-thread groups and checks every output word, including the 25 masked lanes. Multi-thread probes also select one reader per workgroup and exchange values between lanes.
The integer wave-sum probe requests 32-lane waves on a shader model 6.6 device.
Set `TOMBOX_PROBE_WAVE64=1` on a device that runs 64-lane waves to also check the
64-lane sum, its upper-half subtotal and nonzero upper-half lane counts. A second packed-root shader checks both words of a 64-lane ballot against CPU masks for two groups, including seven inactive lanes in each.
Coin Run also checks a three-group prefix sum at each enabled wave width. Every lane is compared with a wrapping CPU sum.
The FidelityFX prepass probes exercise both FP32 quad permutations, including a
read from an inactive neighbor. A separate 64-thread group writes unique values
to shared memory before 44 threads exit. The other 20 read values from the
upper half of the group after a barrier. Both tests check the untouched output rows.
The companion memory suite checks buffer and texture reads,
writes and atomic results. A 16×16 graphics probe checks geometry emission, exports,
flat attributes, both interpolation steps and a pixel-quad swizzle against a CPU image.
The final game runs check the 2D and 3D scenes, texture arrays, cube faces and thread dispatch.
The texture probes cover comparison sampling at a statically zero LOD, UAV stores at relative mip zero and 2D resource dimensions. Other cases check bounded atomics, neighboring subword stores, 64-bit register pairs and packed signed-byte dot products against CPU results. The pixel fixture contains the same bit pattern in a move literal and in the padding after termination, and verifies both boundaries. [RDNA 2 instruction reference](https://docs.amd.com/v/u/en-US/rdna2-shader-instruction-set-architecture)

The launcher publishes the report only after every check passes. On RADV, use the desktop
display for the full run if Xvfb reports missing DRI3 support during the presentation tests.

A successful run writes `coin-rdna2-coverage.tsv` beside `coin_window.exe`.
Each row records retail observation and the type of passing GPU check. The
`coin_run_alu`, `memory_probe` and `graphics_probe` columns count toward direct
reference coverage. The `cache_probe` column records cross-wave UAV visibility for
`buffer_gl0_inv` and `buffer_gl1_inv`, and preservation of a register value and
condition flag in all three defined `s_inst_prefetch` modes. Physical cache contents
and instruction-fetch distance remain unmeasured. [Recorded results](../progress/coin-rdna2-coverage.tsv)

### Local Scarlett libraries

To reproduce the host-import checks on Linux, set `PROTON` to the Proton Experimental executable and run the command below. The standalone homebrew probe uses the DLL names and ordinals from the Scarlett audit. It needs no game files. On success it writes `target/x86_64-pc-windows-msvc/release/scarlett-import-checks.tsv` with the scenario that exercised each import. Copy that report to `tools/progress/` and run `python3 tools/progress/gen.py` to refresh the map.

```sh
tools/homebrew/play-coins.sh --scarlett-import-test
```

Pass the directory containing all 16 DLLs from the audit. The launcher stages the local libraries in a temporary directory and selects their native versions under Proton. Coin Run checks the module path before probing each runtime or middleware DLL. The BVH builder loads through the XMem shim. Coin Run checks the memory query against reserved and committed regions and verifies the failure path. The FidelityFX probe sends four render-resolution queries through the loader and directly to the upscaler. The remaining GPU checks use the actual programs embedded in the upscaler and BVH builder. The four-pass BVH radix sort requires a wave64-capable device. The launcher reports failures after running the independent probes. [Per-DLL operations and limits](../../docs/COMPATIBILITY.md#scarlett-dll-audit)

Fortnite delay-loads `GameChat2.dll` separately. The optional `--gamechat-test` passed with the x64 desktop provider from Microsoft.GDK.Windows 2610.0.9661. It tests local loading and thread configuration. Voice chat and Fortnite’s delay-load path need a title run. [Microsoft GDK package](https://www.nuget.org/packages/Microsoft.GDK.Windows/2610.0.9661)

### Scarlett package startup

From Linux, inventory an extracted package with its game config and Appx manifest. The report checks literal install-layout destinations, records DLL hashes and compares each DLL’s import table with the checked-in audit. Once a declared executable is present, the same command records its PE headers, TLS summary, ordinary and delay-loaded symbols, and the exact imports absent from the existing Scarlett and Fortnite snapshots. An unfamiliar symbol needs provider and behavior checks. Dynamic lookups appear only in runtime tests or further analysis.

```sh
cargo build --locked --release -p tuxbox-cli
python3 tools/homebrew/package_preflight.py /path/to/package > package-preflight.json
```

For the offline HTTP startup test, use the extracted package directory, set `PROTON`, and pass the desktop GDK’s Windows directory containing `include/`, `lib/x64/` and `bin/x64/`. The tested NuGet package provides it under `native/261000/windows`. The launcher stages the supplied Scarlett HTTP client and its imported package DLLs alongside the matching desktop `xgameruntime.dll` in a temporary directory. Two processes test the default initialization and the package config. The latter reads back the title ID before exercising the HTTP client. Both check the origin of the loaded modules, set and read back a POST URL, and close the request without sending traffic. [Microsoft.GDK.Windows 2610.0.9661](https://www.nuget.org/packages/Microsoft.GDK.Windows/2610.0.9661) · [Gaming Runtime initialization](https://learn.microsoft.com/en-us/gaming/gdk/docs/reference/system/xgameruntimeinit/functions/xgameruntimeinitialize) · [Microsoft HTTP client](https://github.com/microsoft/libHttpClient)

```sh
tools/homebrew/play-coins.sh --scarlett-http-test /path/to/package /path/to/native/261000/windows
```

The user startup probe needs only the package’s `MicrosoftGame.config` and the same GDK. It registers for user changes and dispatches the completion of a silent default-user request. It accepts the documented no-default-user result or checks that a returned signed-in user can be found by its local ID. It reports the observed notification count after unregistering. [GDK user selection](https://learn.microsoft.com/en-us/gaming/gdk/docs/reference/system/xuser/enums/xuseraddoptions)

```sh
tools/homebrew/play-coins.sh --scarlett-user-test /path/to/package /path/to/native/261000/windows
```

### Retail FidelityFX prepass

Build the homebrew tools and pass the October Scarlett upscaler as a data file.
The probe creates a compute pipeline from the original 256-thread shader and
its embedded root signature. It checks both outputs across four workgroups.
The texture reference covers reset, valid history and adjacent pixels with NaN
and finite motion. The raw tensor reference uses per-quadrant biases and a
nonzero luminance coefficient. Partial dimensions leave guard words untouched.

```sh
tools/homebrew/build-clear.sh
tools/homebrew/play-coins.sh --fsr-prepass-test /path/to/amd_fidelityfx_upscaler_xbox.dll
```

### Retail FidelityFX sharpening

Coin Run extracts shader `627f8369a1fe920d` from the supplied DLL and dispatches it with the embedded root signature. The original shader reads the color texture at `t18` and exposure at `t17`, and writes the sharpened image to `u11`. The probe leaves the unused `t6` descriptor null.

```sh
tools/homebrew/play-coins.sh --fsr-sharpen-test /path/to/amd_fidelityfx_upscaler_xbox.dll
```

Two partial dispatches over a 32×32 pattern exercise horizontal and vertical workgroup placement at strengths zero and one with different exposure ratios. The CPU reference models the five-tap filter, half-precision intermediates and inverse log encoding. The probe compares every RGBA channel within an absolute tolerance of 0.00001 plus 0.2% of the reference and checks every texel outside the dispatch exactly. [AMD’s RCAS filter reference](https://github.com/GPUOpen-Effects/FidelityFX-FSR/blob/master/ffx-fsr/ffx_fsr1.h)

### Retail FidelityFX postpasses

Pass the Scarlett upscaler DLL to the probe as a data file. It locates the 13
FSR4 postpasses from the tested build by container hash and dispatches each
through TuxBox’s compute pipeline. The CPU reference checks the cleared tensor
borders, every interior word and surrounding guards. Three input sizes exercise
ordinary padding and the right and bottom limits.

```sh
tools/homebrew/play-coins.sh --fsr-postpass-test /path/to/amd_fidelityfx_upscaler_xbox.dll
```

### Retail BVH radix sort

With a wave64-capable shader model 6.6 device and the GXDK 250405 BVH builder,
run the four translated compute kernels as a full sort under Proton.

```bash
tools/homebrew/build-clear.sh
tools/homebrew/play-coins.sh --bvh-sort-test /path/to/XboxBvhBuilder.dll
```

Use the same `PROTON` and DXC setup as for `--rdna2-test`. The probe reads four
shader containers from the supplied file and creates their pipelines through
TuxBox. It sorts 16,384 keys in four byte passes on the GPU. After each pass it
compares keys and original indices with a stable CPU sort and checks the guards
on both buffers. The three-group homebrew prefix tests above run independently
of the supplied library.

### Graphics readback

Each scripted 2D board mode checks 192 RGBA samples per frame, one inside every magnified sprite
texel on every tile. The 20 frames cover wall and boundary collisions, both pickup
orders, movement after winning, and restart. A mismatch reports the frame, tile,
texel and colors and exits with failure. TuxBox writes its graphics log to
`coin-window.log` beside the executable by default.

The 3D mode sends camera-relative positions through a synthetic RDNA2 vertex
shader that tilts the view and projects the board into perspective. Its pixel
shader samples the atlas and shades vertical faces. The game draws raised pieces
before the floor tiles, using a D32_FLOAT depth buffer. The scripted test checks
26 pixel positions per frame against the board state and camera projection,
including the wall covering part of the player on the back row.

The tiled modes run without a game dump or `xg_xs.dll`. The game writes the atlas
at its XMem address. A presentation submits the runtime’s upload, and a subsequent
draw samples the resulting texture. The older `--placed-test` still exercises an
explicit GPU copy. For unattended Linux use, prefix a windowed test command with
`xvfb-run -a`.

### Texture reference data

The single-level reference test checks the 256×256 top mip of a public Scarlett DDS.
The mip-chain test places its entire 352,256-byte tiled payload in XMem. Each draw
uses a shader resource view exposing one mip, from 256×256 down to 1×1. The expected
colors come from a separate linear DDS. For each mip, the fixture averages the
corresponding square of original pixels and rounds once per channel. This gives
87,381 RGBA comparisons per pass. After a one-byte CPU edit in the packed tail,
the uploader transfers the changed four-byte texel and the test repeats every
comparison. [Scarlett test texture](https://github.com/walbourn/directxtextest/blob/b185b8757b86b09257ab4f7701ba921248c8ba6e/xbox/Media/scarlettwindowslogo.dds) · [Linear reference](https://github.com/walbourn/directxtextest/blob/b185b8757b86b09257ab4f7701ba921248c8ba6e/media/windowslogo_X8R8G8B8.dds) · [AMD GFX10 swizzle patterns](https://github.com/GPUOpen-Drivers/pal/blob/c5e800072a32f68b6ccc4422936d96167c6e0728/src/core/imported/addrlib/src/gfx10/gfx10SwizzlePattern.h)

The BC1 test uses 5,463 eight-byte blocks. The linear block stream is checked
against the SHA-256 of an independent encode. To produce that encode we took the
separate linear BGRX logo, generated its nine mip levels with the box average
above, and compressed the resulting BGRX mip-chain DDS with DirectXTex
`texconv -f BC1_UNORM` (2026.5.8.1). The test uploads those verified blocks to
a committed PC texture, point-samples both textures and compares all 87,381 pixels
exactly on the same GPU. A second pass replaces the 1×1 mip with a transparent
BC1 block and checks every level again. [Scarlett BC1 fixture](https://github.com/walbourn/directxtextest/blob/b185b8757b86b09257ab4f7701ba921248c8ba6e/xbox/Media/scarlettwindowslogoBC1.dds) · [DirectXTex release](https://github.com/microsoft/DirectXTex/releases/tag/may2026)

The rectangular tests generate original RGBA8 images at 256×64 and 64×256, and
BC1 images at 256×64 and 128×256. They cover padded tile rows and mip levels
where one dimension has reached one pixel. Before each GPU run, the generator
checks its element addresses against a SHA-256 digest obtained independently
from AMD AddrLib. To reproduce the digests, use the pinned GFX10 implementation
with family 0x8f, revision 0x28, SW_4K_S, a 2D texture, one slice, one sample
and nine levels. Call `Addr2ComputeSurfaceAddrFromCoord` in mip, row, column
order and hash each byte address as a little-endian uint32. Use texel coordinates
and 32 bits per element for RGBA8, block coordinates and 64 bits per element for
BC1. [AMD AddrLib source](https://github.com/GPUOpen-Drivers/pal/tree/c5e800072a32f68b6ccc4422936d96167c6e0728/src/core/imported/addrlib)

The array test uses three slices with distinct colors at every mip. The RGBA8
64×256 texture uses 94,208 bytes per slice and the BC1 256×64 texture uses
16,384. To reproduce the array digests, set the AddrLib slice count to three
and iterate slices before mips. The test binds all slices in one Texture2DArray
view and selects the slice in the translated pixel shader.

The cube test uses six 64×64 faces with seven mip levels and 24,576 bytes per
face. The red and green channels encode texel coordinates. The blue channel
varies by face and mip. The pixel shader projects an interpolated 3D direction
with the RDNA2 cube instructions and point-samples a `TextureCube` view.
Readback compares all 32,766 texels with the linear fixture before and after
the CPU edit. To reproduce the address digest, use the same RGBA8 AddrLib
parameters as the array test with a 64×64 base, six slices and seven levels.
[AMD cube sampling](https://gpuopen.com/learn/fetching-from-cubes-and-octahedrons/)

The streaming test reserves five 64 KiB pages for a 256×256 RGBA8 texture. Mip 1
occupies the first page. Coin Run displays that coarse mip while it maps the
first and last pages of mip 0. Once the two interior pages arrive, it displays
the detailed mip. It then evicts all four detail pages and reloads a recolored
image at the same address. The game selects a resident mip through its shader
resource view at each stage. The test checks page residency, compares GPU
readback with the linear fixture, and checks the XMem and upload trace.
The fixture uses the same AddrLib parameters as the rectangular RGBA8 cases
with a 256×256 base, one slice and two levels.

Native decoding covers layout 261, single-sampled 2D textures and 2D arrays
with no resource flags. The base texture must consist of complete tiles. Each
tile covers 32×32 pixels in RGBA8 UNORM or 128×64 pixels in BC1 UNORM. Multilevel textures
also require both base dimensions to be powers of two. TuxBox pads the larger
mips to whole tiles and packs the smallest mips into one 4 KiB tile per slice.
The mirror accepts up to 4,096 subresources and requires a layout smaller
than 4 GiB. Other BC formats still use XG. [AMD GFX10 mip placement](https://github.com/GPUOpen-Drivers/pal/blob/c5e800072a32f68b6ccc4422936d96167c6e0728/src/core/imported/addrlib/src/gfx10/gfx10addrlib.cpp#L3830-L3989)
