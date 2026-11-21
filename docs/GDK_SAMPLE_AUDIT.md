# GDK sample compatibility audit

Reviewed Microsoft’s public catalog at revision `b1e77b6ac10f95b07296025fda171ca2ddc42dbe` on 2026-11-21. Microsoft lists Console and PC build targets. For each comparison below, the TuxBox result identifies what actually ran. A PC build under Proton exercises host APIs. A Scarlett build also exercises the console imports, calling conventions and shader translation it uses. Microsoft’s sample repository documents the desktop GDK and the GDK with Xbox Extensions (GDKX) build requirements. [Pinned sample list][catalog] · [Microsoft build requirements][requirements]

## Programs run from Microsoft source

Both programs used the unmodified source from the pinned revision, cross-compiled on x86-64 Linux with clang-cl, the xwin Windows SDK, a static CRT and OneCore API-set imports. Both ran as desktop x64 executables under Proton Experimental `experimental-11.0-20261027b`. Neither executable imports a TuxBox shim.

| Sample | Observed result | What it establishes |
|---|---|---|
| [CMakeXboxConsoleApp][hello] | The desktop build of `Main.cpp` exited 0 and printed `Hello, world from Xbox` without the CPU-support error. | The launcher and Windows console/CRT path work for this program. The upstream console target and its GDKX link configuration remain untested. |
| [xbcompress][compress] | Both `-z` (MSZIP) and default (LZMS) exited 1 on a 65,536-byte input. Proton reported an unimplemented `Cabinet.dll.30`. Neither produced compressed output. | A desktop host-provider gap on this Proton build. Decompression, native Windows, and the console build were not exercised in this run. |

### Reproduce the desktop comparison on Linux

Set `SAMPLES` to a checkout of the pinned Microsoft repository and `PROTON` to the Proton Experimental executable. From the TuxBox root, use the same cached SDK as Coin Run. The temporary directory holds the executables and test data.

```bash
work=$(mktemp -d)
sdk=${XWIN_SDK_ROOT:-${XDG_CACHE_HOME:-$HOME/.cache}/cargo-xwin/xwin}
flags=(--target=x86_64-pc-windows-msvc -fuse-ld=lld /nologo /MT /EHsc /D_CONSOLE
       -imsvc "$sdk/crt/include" -imsvc "$sdk/sdk/include/ucrt"
       -imsvc "$sdk/sdk/include/shared" -imsvc "$sdk/sdk/include/um")
libs=(/libpath:"$sdk/crt/lib/x86_64" /libpath:"$sdk/sdk/lib/ucrt/x86_64"
      /libpath:"$sdk/sdk/lib/um/x86_64" onecore_apiset.lib)
clang-cl "${flags[@]}" /Fo"$work/" /Fe"$work/hello.exe" \
  "$SAMPLES/Samples/Tools/CMakeXboxConsoleApp/Main.cpp" /link /subsystem:console "${libs[@]}"
clang-cl "${flags[@]}" /DUNICODE /D_UNICODE /D_WIN32_WINNT=0x0A00 /Fo"$work/" \
  /Fe"$work/xbcompress.exe" "$SAMPLES/Samples/Tools/xbcompress/compresstool.cpp" \
  /link /subsystem:console,10.0 /nodefaultlib:kernel32.lib /nodefaultlib:onecore.lib "${libs[@]}"
python3 - "$work" <<'PYDATA'
from pathlib import Path
import sys
for name in ("mszip", "lzms"):
    (Path(sys.argv[1]) / f"{name}.bin").write_bytes(bytes(range(256)) * 256)
PYDATA
./tuxbox run "$work/hello.exe"
./tuxbox run "$work/xbcompress.exe" -z mszip.bin
./tuxbox run "$work/xbcompress.exe" lzms.bin
```

If the terminal hides Windows stdout, run `hello.exe > hello.log 2>&1` through Proton’s `cmd.exe /d /c` from the executable directory with the same Steam client and prefix settings. If a newer provider passes compression, expand its output with `xbcompress -u` in a separate directory and compare the bytes with the input before recording a round-trip pass.

## API behavior compared with homebrew

| Microsoft sample and catalog targets | TuxBox evidence on Linux with Proton | Remaining work |
|---|---|---|
| [SimpleTriangle][triangle] · Console | Microsoft binds a vertex buffer and draws three vertices. Coin Run’s companion `clear_pixel.exe --triangle` translates Xbox shaders, draws three vertices and checks the resulting pixels and background. | Run the Scarlett sample itself. Microsoft lists SimpleTriangleDesktop separately as PC. |
| [AMD_FSR][fsr] · Console and PC | Tests dispatch 15 of 29 kernels from the audited FidelityFX DLL and compare GPU output with references. | Verify context creation and a complete upscaled frame. Individual kernels cover part of the sample workload. |
| [GameInputSequential][input] · Console and PC | The sample obtains a current reading and advances through device history. The v0 adapter exercises that sequence, end-of-history, hotplug and recovery with virtual gamepads. | Run the upstream sample against its GameInput version and test physical devices. |
| [SimplePlaySound][sound] · Console | The sample submits source buffers and includes PCM, ADPCM, xWMA and XMA2 paths. Coin Run verifies PCM, panning and mixing by Linux stereo loopback. | Xbox compressed formats, including XMA2 with APU memory, and playback from a retail title. |
| [AsynchronousProgramming][async] · Console and PC · [UserManagement][user] · Console | The package startup probe uses a desktop Gaming Runtime under Proton, manually dispatches a completion and checks the result of a silent default-user request. | Provider-based custom async operations, multi-user/device association, and the console service ABI. The retail local-user/store adapters are build-specific. |
| [SimpleDirectStorageCombo][storage] · Console and PC | The shim services a reconstructed file-to-memory request. A regression checks positioned plain and zlib reads, exact output and the untouched destination tail. | Queue cancellation, status/error propagation, multiple queues, GPU destinations and compression formats beyond the tested zlib path. |

The catalog also covers Live, Handheld, Unity and xCloud. This audit establishes no sample execution for those categories. Save/load persistence, authenticated services, suspend/resume and streaming-client behavior remain unverified here. The highest-value next graphics comparison is a Scarlett build of SimpleTriangle with known vertex data and a captured frame. It would exercise the sample’s real loader, pipeline and presentation together. [Catalog][catalog] · [Homebrew test commands](../tools/homebrew/README.md) · [Retail evidence](COMPATIBILITY.md)

[catalog]: https://github.com/microsoft/Xbox-GDK-Samples/blob/b1e77b6ac10f95b07296025fda171ca2ddc42dbe/gdk-samples-list.md
[requirements]: https://github.com/microsoft/Xbox-GDK-Samples/blob/b1e77b6ac10f95b07296025fda171ca2ddc42dbe/README.md
[hello]: https://github.com/microsoft/Xbox-GDK-Samples/blob/b1e77b6ac10f95b07296025fda171ca2ddc42dbe/Samples/Tools/CMakeXboxConsoleApp/Main.cpp
[compress]: https://github.com/microsoft/Xbox-GDK-Samples/blob/b1e77b6ac10f95b07296025fda171ca2ddc42dbe/Samples/Tools/xbcompress/CMakeLists.txt
[triangle]: https://github.com/microsoft/Xbox-GDK-Samples/blob/b1e77b6ac10f95b07296025fda171ca2ddc42dbe/Samples/IntroGraphics/SimpleTriangle/SimpleTriangle.cpp
[fsr]: https://github.com/microsoft/Xbox-GDK-Samples/tree/b1e77b6ac10f95b07296025fda171ca2ddc42dbe/Samples/Graphics/AMD_FSR
[input]: https://github.com/microsoft/Xbox-GDK-Samples/blob/b1e77b6ac10f95b07296025fda171ca2ddc42dbe/Samples/System/GameInputSequential/GameInputSequential.cpp
[sound]: https://github.com/microsoft/Xbox-GDK-Samples/blob/b1e77b6ac10f95b07296025fda171ca2ddc42dbe/Samples/Audio/SimplePlaySound/SimplePlaySound.cpp
[async]: https://github.com/microsoft/Xbox-GDK-Samples/blob/b1e77b6ac10f95b07296025fda171ca2ddc42dbe/Samples/System/AsynchronousProgramming/AsynchronousProgramming.cpp
[user]: https://github.com/microsoft/Xbox-GDK-Samples/blob/b1e77b6ac10f95b07296025fda171ca2ddc42dbe/Samples/System/UserManagement/UserManagement.cpp
[storage]: https://github.com/microsoft/Xbox-GDK-Samples/blob/b1e77b6ac10f95b07296025fda171ca2ddc42dbe/Samples/System/SimpleDirectStorageCombo/SimpleDirectStorageCombo.cpp
