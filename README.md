# TuxBox

TuxBox (formerly TomBox) is an experimental Xbox Series X|S compatibility runtime for x86-64 PCs. **Linux with Proton is the primary development platform.** The same compatibility DLLs also run on Windows. Game code runs on the host CPU. TuxBox translates Xbox Direct3D 12 calls and precompiled RDNA2 shaders to PC Direct3D 12. On Linux, Proton’s vkd3d-proton backend submits that work to Vulkan.

## Compatibility today

| Area | Verified result |
|---|---|
| Graphics | Coin Run draws 2D and 3D scenes under Proton. GPU readback checks geometry, textures, depth and selected retail compute shaders. |
| Input and audio | Linux virtual gamepads verify GameInput polling, history, hotplug and rumble. Stereo loopback measures simultaneous XAudio2 PCM voices and panning. |
| Retail titles | One recorded Windows build of Fortnite reached the lobby with rendering defects and silent audio. Gameplay and retail startup on Proton remain unverified. [Retail case study](docs/FORTNITE_ANALYSIS.md) |

Microsoft’s public GDK samples guide platform testing. The audit compares specific calls, distinguishes desktop from console builds, and records reproducible passes and failures. [GDK sample audit](docs/GDK_SAMPLE_AUDIT.md)

[![Verified imports](docs/img/badge-imports.svg)](docs/COMPATIBILITY.md#scarlett-dll-audit) [![Direct3D 12.x methods](docs/img/badge-d3d12x.svg)](docs/img/progress.svg) [![RDNA2 instructions](docs/img/badge-instructions.svg)](docs/img/progress.svg)

[![Progress map for observed imports, Direct3D methods and shader instructions](docs/img/progress.svg)](docs/img/progress.svg)

**Reading the map.** Each import square represents one DLL and symbol across the recorded title and 16 Scarlett libraries. Green requires a behavior check for that pair. An observed import or resolved export with unverified behavior is amber. Missing exports are grey. Hover for the callers and evidence. Direct3D green counts implemented or forwarded slots. RDNA2 green requires successful translation of every scanned retail shader containing the instruction. [Compatibility matrix](docs/COMPATIBILITY.md) · [Known limitations](docs/TECHNICAL_DEBT.md)

## Try Coin Run on Linux

Coin Run uses generated assets and needs no game dump. On x86-64 Linux, use Cargo and Python 3 to run the native parser and shader tests. To play Coin Run, install Proton Experimental in Steam and use a Vulkan-capable GPU, the Rust `x86_64-pc-windows-msvc` target, cargo-xwin, clang-cl and lld-link.

```sh
./tuxbox test
tools/homebrew/build-clear.sh
./tuxbox homebrew --3d-test
./tuxbox homebrew --3d
```

The launcher searches Steam’s registered libraries for Proton Experimental. Set `PROTON` to the path of the Proton executable for a custom installation. Collect both coins with WASD, the D-pad or the left stick. R or Menu restarts the board. Q or Escape quits. The test guide covers GPU readback, GameInput and audio loopback. [Coin Run test guide](tools/homebrew/README.md)

Use `./tuxbox run /path/to/game.exe` for an executable already prepared with its required DLLs and assets. The launcher sets the working directory to the executable’s directory and forwards its arguments and exit status. [Launch and prefix settings](docs/RUNNING.md)

## Build on Windows

The runtime targets Windows 10/11 x64 with a Direct3D 12 GPU, Microsoft Gaming Services, Rust and the Windows SDK. The title guide covers the static CRT build, DLL placement, drive mapping, configuration and logs. [Windows build and launch guide](docs/RUNNING.md)

## Research and scope

The development log records retail test results. The Linux research tools work with the included fixtures. [Development log](docs/DEVELOPMENT_LOG.md) · [Linux research tools](tools/homebrew/README.md#linux-research-tools) · [Architecture](docs/ARCHITECTURE.md) · [Design decisions](docs/DECISIONS.md) · [Xbox platform research](docs/XBOX_RESEARCH.md) · [Fortnite analysis](docs/FORTNITE_ANALYSIS.md) · [Progress data and generator](tools/progress/)

TuxBox is independent interoperability research for authorised, already-decrypted binaries. Bring your own game files. Package decryption, key extraction, console-security bypasses and anti-cheat changes are outside the project’s scope. Shims using the in-process network guard default to offline. The launcher itself can also run ordinary programs that use no shim. Project testing uses local substitutes or a private server. The local user and store provide no Xbox Live account or entitlements. Xbox is a Microsoft trademark. TuxBox is unaffiliated with Microsoft and game publishers.
