# Compatibility Matrix

Derived from the **actual imports** of `FortniteClient-XSX-Shipping.exe` (67 modules /
670 functions) and the exports of the Xbox system DLLs in `EmbeddedXvd/Windows/System32/`.
Raw per-DLL data: `analysis/fortnite_exe.json`, `analysis/syslib/*.json`.

## Evidence levels (CLAUDE.md §1)
1 Observed · 2 Documented · 3 Inferred · 4 Hypothesized · 5 Implemented · 6 Compiles ·
7 Unit-tested · 8 Integration-tested · 9 Verified vs target · 10 End-to-end.
The dependency inventory comes from the recorded imports and exports. The module notes identify runtime checks. On the progress map, the green GameInput and XAudio2 squares count the individual factory exports exercised by Coin Run on Proton. The four other listed XAudio2 imports retain their unverified status.

## Module strategy matrix

Legend — Plan: **BindOS** (forward to the real Windows DLL), **Reuse** (ship/forward to a
PC redistributable of the same component), **Reimpl** (write our own), **Stub** (minimal
no-op/offline impl to pass startup), **Translate** (reimpl that maps onto a host API).

| Module | Used by EXE | Role | Plan | Difficulty | Notes |
|---|---|---|---|---|---|
| `api-ms-win-core-*` (≈36) | many | Win32 API-Sets (kernel/sync/file/mem/thread…) | BindOS | low* | *per-symbol behaviour check vs desktop |
| `api-ms-win-crt-*` (≈12) | many | UCRT | BindOS | low | desktop UCRT |
| `ext-ms-win-rtcore-ntuser-window/message` | 9 | Windowing + message loop | BindOS→`user32` | low-med | map Xbox rtcore ntuser to real user32 |
| `msvcp140` / `vcruntime140(_1)` | — | MSVC C++ runtime | Reuse | none | shipped with the game |
| `bcrypt`, `crypt32` | few | Crypto | BindOS | low | desktop equivalents |
| `ws2_32` | ord | Winsock | BindOS | low | standard |
| `winhttp` | few | HTTP | BindOS | low | desktop equivalent |
| `mfplat`, `mfreadwrite` | few | Media Foundation (movies) | BindOS | low-med | desktop MF; not needed for first frame |
| `d3d12_xs.dll` | 6 | **Xbox D3D12** | **Translate**→PC `d3d12.dll` | **HIGH** | std COM interfaces + Xbox ext (`D3D12XboxCreateDevice`, `DXGIXPresentArray`, `D3DFlushEntireCpuCache`, `D3D12ConfigureVirtualMemoryX`) |
| `xg_xs.dll` | 1 | RDNA2 tiling/swizzle/DCC math | Reimpl / bypass | **MED-HIGH** | `XGCreateTextureComputer`; self-contained math, no HW dep |
| `xmem.dll` | 10 | Memory allocator | Reimpl→VirtualAlloc2/D3D12 heaps | MED | `XMemAlloc/Virtual/PhysicalPages`; thin over ntdll |
| `GameInput.dll` | 1 | Input (IGameInput) | Reuse PC GameInput / opt-in XInput adapter | MED | Homebrew on Proton verifies `GameInputCreate` ordinal 1, v0 polling, forward and backward history, device and reading callbacks, and two-motor `SetRumbleState` output on Linux virtual pads. Physical controllers and retail titles remain unverified. |
| `XAudio2_9.dll` | 5 (ord #1,2,4,5,6) | Audio | Reuse PC XAudio2_9 | MED | Coin Run verifies `XAudio2Create` (#1) and two simultaneous PCM voices with stereo panning via `SetOutputMatrix`, measured by Linux loopback on Proton. Reverb (#2), effects (#4), spatial audio (#5–6) and retail playback remain unverified. |
| `AcpHal.dll` | 6 | Xbox audio-DSP HAL | Stub/software | LOW-MED | `ApuAlloc`,`AcpHalCreate`,…; no PC ACP, route to software mix |
| `XCRDAPI.dll` | 1 | **DirectStorage** | Reuse PC DirectStorage / Reimpl file-backed | MED | `DStorageGetFactory`; for `.ucas` IoStore streaming |
| `dxcompiler_xs.dll` | 1 | Runtime DXIL compiler (DXC) | Reuse PC `dxcompiler.dll` | LOW-MED | `DxcCreateInstance` |
| `xgameruntime.dll` | **dynamic** | GDK services dispatcher | Reimpl | MED-HIGH | `QueryApiImpl` dispatch; loader must intercept its LoadLibrary |
| `EOSSDK-XboxOneGDK-Shipping.dll` | 48 | Epic Online Services | Stub (offline) | LOW→HIGH | 48 fns, mostly RTC/voice + Platform init; stub to pass startup |
| `GameChat2.dll` | delay | GDK chat | Stub | LOW | delay-loaded |

\* "low*" = the API-set *contract* is standard, but Xbox GameOS implementations may diverge
behind identical names; each must be validated, not assumed (CLAUDE.md principle; assumption A3).

## Shortest path to a first frame (implied ordering)
1. **Loader** (map + DIR64 relocs + TLS + SEH + import resolution/redirection).
2. **BindOS** the ~52 standard modules; **Stub** EOS/GameChat/xgameruntime-nonessential.
3. **`xmem`** (memory) + GDK bootstrap enough for `FEngineLoop::PreInit`.
4. **`d3d12_xs`→PC D3D12 translation** + **`xg_xs`** + windowing for `RHIInit`/device.
5. First `DXGIXPresentArray`→ present.

## GDK service surface (XGameRuntime)
Not exposed as named imports; reached via `xgameruntime.dll!QueryApiImpl` after a dynamic
load. The concrete XTaskQueue/XUser/XGameSave/etc. functions the title pulls will be
enumerated by (a) tracing `QueryApiImpl` call sites in the EXE using the symbol DB, or
(b) runtime instrumentation once the loader runs. **TBD — not yet enumerated.**

## Toolkit capabilities (verified, not game APIs)
| Capability | Level | Evidence |
|---|---|---|
| PE32+ parse (headers/sections/dirs/imports/exports/reloc/TLS) | 7 | tests; cross-checked vs `dumpbin`; parsed the 150 MB Fortnite EXE |
| ZIP64 inventory + extraction safety vetting | 7 | tests; vetted the real 25.5 GB archive (safe) |

## Known limitations
- The tested Fortnite build reaches the lobby with rendering defects. Retail gameplay remains unverified. [Retail test results](DEVELOPMENT_LOG.md)
- GDK (`QueryApiImpl`) service list not yet enumerated.
- Divergence of Xbox `api-ms-win-*` from desktop not yet validated per symbol.

## Scarlett DLL audit

All 16 supplied DLLs are x64 PE images. Their ordinary import tables contain
1,337 entries (611 distinct DLL/symbol pairs). The files have no delay-import
entries. The supplied sibling DLLs and the built TomBox shims were checked by
export name or ordinal. A Windows probe resolved all 492 distinct host imports
across 61 modules with `LoadLibraryExA` and `GetProcAddress` under Proton
Experimental `experimental-11.0-20261027b`. This checks symbol availability.
The GPU results below come from dispatching the embedded shaders separately.

**One static import is missing.** The BVH builder asks for `xmem!XMemVirtualQuery`.
Its caller reads Xbox-specific memory information beyond the desktop query
fields. The upscaler also imports `XMemGetWorkingSetStatistics`. TomBox exports
it and returns `E_NOTIMPL`.

| Supplied DLL | Import entries | Exports | Translation evidence and open work |
|---|---:|---:|---|
| `amd_fidelityfx_loader_xbox.dll` | 22 | 5 | Names the supplied upscaler and a separate frame-generation provider. The latter DLL was absent from the supplied set. Provider selection happens at runtime. |
| `amd_fidelityfx_upscaler_xbox.dll` | 86 | 5 | 14/29 compute shaders compile and pass GPU readback, including the input prepass. Fifteen kernels and full-frame upscaling remain open. The long model-specific DLL name in this file is its own PE export name. |
| `XboxBvhBuilder.dll` | 57 | 12 | 16/52 compute shaders compile and pass GPU readback. The other 36 kernels, the virtual-memory query and the command-list gaps below remain open. |
| `Bink2Scarlett.dll` | 60 | 74 | The host resolves `XAudio2_9!#1` to `XAudio2Create`. Bink playback, including synchronized audio and video, needs a functional test. |
| `libHttpClient.GDK.dll` | 198 | 129 | WinHTTP and Crypt32 imports resolve. The binary also names `xgameruntime.dll` and `QueryApiImpl`. GDK service calls, HTTP and WebSocket behavior remain unverified. |
| `XCurl.dll` | 140 | 61 | WinHTTP and Crypt32 imports resolve. Strings identify `xgameruntime.dll`, `gameplatformservices.dll`, `QueryApiImpl` and a URL-security query. These dynamic paths need runtime testing. |
| `concrt140.dll` | 103 | 291 | Concurrency runtime. All static imports resolve. |
| `msvcp140.dll` | 187 | 1493 | C++ standard library. All static imports resolve. The binary also names `appmodel.dll`. |
| `msvcp140_1.dll` | 33 | 7 | C++ runtime extension. All static imports resolve. |
| `msvcp140_2.dll` | 96 | 44 | C++ runtime extension. All static imports resolve. |
| `msvcp140_atomic_wait.dll` | 64 | 32 | All static imports resolve. The binary also names `icu.dll`. |
| `msvcp140_codecvt_ids.dll` | 24 | 4 | Character-conversion facet IDs. All static imports resolve. |
| `vccorlib140.dll` | 114 | 256 | C++/CX runtime. Static WinRT imports resolve. The binary also names `gcombase.dll`. Activation behavior remains unverified. |
| `vcomp140.dll` | 78 | 113 | OpenMP runtime. All static imports resolve. The binary also names `vcomp120ui.dll`. |
| `vcruntime140.dll` | 41 | 71 | Compiler runtime. All static imports resolve. |
| `vcruntime140_1.dll` | 34 | 3 | Exception-handling runtime. All static imports resolve. |

Only the upscaler and BVH builder contain identifiable DXBC shader containers.
The ten Microsoft runtime DLLs report version 14.29.30157.0. Their static imports
are satisfied by the supplied runtimes and host APIs. DLL and function names in
strings identify candidates for dynamic lookup. Whether an application takes
those paths, uses a fallback or requires a particular GDK service depends on
the workload. [GPU readback procedures](../tools/homebrew/README.md#retail-fidelityfx-prepass)

### Scarlett command-list UAV clears

In the inspected FidelityFX upscaler (SHA-256 `faff02f294a7e9149ce079dffe3dedbcabe99b1f180ab14222aa8689e00a75fa`),
the clear job binds the descriptor heap and flushes pending barriers. Both branches
pass GPU and CPU descriptor handles, the resource, four clear components, a zero
rectangle count and a null rectangle pointer. The method names are inferred from
these calls and AMD’s public DX12 clear implementations. [AMD integer clear](https://github.com/GPUOpen-LibrariesAndSDKs/FidelityFX-SDK/blob/c6efa6bf7f2027b3ec94f28578bb5965eabb9e55/sdk/src/backends/dx12/ffx_dx12.cpp#L3421-L3426) · [AMD float clear](https://github.com/GPUOpen-LibrariesAndSDKs/FidelityFX-SDK/blob/d7531ae47d8b36a5d4025663e731a47a38be882f/sdk/src/backends/dx12/ffx_dx12.cpp#L1996-L2004)

| Xbox slot | Call RVA | Clear values at the call | PC method |
|---|---|---|---|
| 53 | `0x72ee` | Four color words copied into a temporary array | `ClearUnorderedAccessViewUint` (49) |
| 54 | `0x7296` | Pointer to the job’s original float color | `ClearUnorderedAccessViewFloat` (50) |

The `--uav-test` homebrew fixture runs independently of the DLL. It checks all
54 texels of each 9×6 surface after a full clear followed by a 5×3 rectangular
clear. The integer case preserves the RGBA byte values. The float case checks
conversion to RGBA8 UNORM. [Run the fixture](../tools/homebrew/README.md#graphical-coin-run)

### Scarlett resource discard

At RVA `0x6dcb` the same FidelityFX DLL takes a resource from its resource table,
flushes pending barriers and calls command-list slot 55 with the resource and a
null region. This matches AMD’s discard job. TomBox forwards the call to PC
`DiscardResource` (slot 51). [AMD discard job](https://github.com/GPUOpen-LibrariesAndSDKs/FidelityFX-SDK/blob/c6efa6bf7f2027b3ec94f28578bb5965eabb9e55/sdk/src/backends/dx12/ffx_dx12.cpp#L3431-L3444)

A dispatch-table regression test checks the selected host method. On Proton,
`--discard-test` exercises the observed null-region call and a synthetic
rectangle using the Windows SDK region layout. It overwrites discarded pixels
before readback and checks that surrounding pixels retain their color.
[Discard semantics](https://learn.microsoft.com/en-us/windows/win32/api/d3d12/nf-d3d12-id3d12graphicscommandlist-discardresource)

### Committed textures, copies and indirect commands across GXDK editions

Microsoft publishes a Scarlett upscaler DLL (SHA-256 `3ffe4c8646ea3ac5f15ef6df49a019311bb5ba0ae9d491e6cc957d2b62584880`) with the Xbox samples. Its `.xbld` section identifies April 2026 Update 2 (260402). The supplied upscaler above identifies October 2026 (261000). The following RVAs are relative to the image base. [Public sample binary](https://github.com/microsoft/Xbox-GDK-Samples/blob/b1e77b6ac10f95b07296025fda171ca2ddc42dbe/Kits/FidelityFX/signedbin/amd_fidelityfx_upscaler_xbox.dll)

| Interface and slot | April RVA | October RVA | Observed behavior and translation |
|---|---|---|---|
| Device 76 | `0x3883` | `0x33cc` | `CreateCommittedResourceX` supplies a resource description with ordinary fields through byte 51 and DCC options beginning at byte 52. TomBox preserves the logical resource and lets the PC driver choose compression and the layout for private GPU textures using Xbox layout `0x101`. The April binary also uses flags `0xc00005` for a render target with UAV access. |
| Command list 85 | `0x6dff` | `0x7147`, `0x7191` | Both texture-copy branches use the desktop location layout and a trailing zero. TomBox forwards coordinates, source box and locations to PC `CopyTextureRegion`. Other trailing flag values are unverified. |
| Command list 61 | `0x6a71` | `0x67ac` | `ExecuteIndirect` passes a command signature, maximum count of one, argument buffer and job offset, then a null count buffer and zero count-buffer offset. |
| Command list 59 | `0x2b9f`, `0x79b2` | `0x6b5a`, `0x6e52` | PIX packets carry an event kind, data pointer and byte size. The host receives the PIX v2 metadata tag. The earlier RHI uses this slot for `ExecuteIndirect`. |
| Command list 60 | `0x2b7b`, `0x7948` | — | The timing path provides an event kind without a packet. TomBox preserves host GPU event boundaries with a generic label. Xbox CPU/GPU event correlation remains open. |
| Device 67 | `0x5765` | `0x56d9` | The offline compute pipeline path requests `ID3D12PipelineState`. The October binary explicitly logs a fallback on failure and retries at slot 62 (RVA `0x5825`). TomBox clears the output and returns `E_NOTIMPL` so the caller can create an ordinary compute pipeline. Offline packet deserialization remains open. |

The GDK sample source provides the named calls and resource fields. Microsoft’s PIX headers describe how the packet and timing paths select begin, end and marker operations. TomBox uses the edition of the module issuing the call to distinguish slots 59–61. For unmarked modules and older unrecognized editions it retains the original slot-59 indirect path. Conflicting tags or an unreadable edition section select a diagnostic probe. Only the April timing path has a verified slot-60 call. [Resource creation](https://github.com/microsoft/Xbox-GDK-Samples/blob/b1e77b6ac10f95b07296025fda171ca2ddc42dbe/Kits/AMDTK/fidelityfx/src/backends/gdk/ffx_gdk.cpp#L1398-L1431) · [Indirect dispatch and copies](https://github.com/microsoft/Xbox-GDK-Samples/blob/b1e77b6ac10f95b07296025fda171ca2ddc42dbe/Kits/AMDTK/fidelityfx/src/backends/gdk/ffx_gdk.cpp#L2988-L3059) · [PIX event encoding](https://github.com/microsoft/PixEvents/blob/b0caa735f8510f4ff60c29ef1c88101620defc5e/include/PIXEvents.h#L406-L446) · [PIX desktop forwarding](https://github.com/microsoft/PixEvents/blob/b0caa735f8510f4ff60c29ef1c88101620defc5e/include/pix3_win.h#L452-L523)

The homebrew copy, indirect and compute-fallback tests use original shaders and synthetic inputs. The two tagged fixture DLLs contain just the sample edition string. The tests check GPU output under Proton, including the zero-count indirect path. [Run the tests](../tools/homebrew/README.md#graphical-coin-run)

The optional public-binary test loads the pinned Xbox loader and upscaler with Microsoft's x64 WinPixEventRuntime 1.0.240308001 supplying the PIX import. Four render-resolution queries pass under Proton. Creating an FSR 3.1.6 context exercises the private texture layout (`0x101`, assigned at April RVA `0x3732`) and reaches device slot 67. Its first offline compute description contains a root signature followed by four pointer/size pairs. The observed lengths in bytes are 28 for the packet, 20 for packet metadata, 7,296 for GPU instructions and 504 for shader metadata. This April provider returns a runtime error with a null context after deserialization fails. The default FSR 4 path separately requests a UAV texture with format `R9G9B9E5_SHAREDEXP`, which the tested host rejects. Upscaled output remains unverified. [Public sample setup](https://github.com/microsoft/Xbox-GDK-Samples/blob/b1e77b6ac10f95b07296025fda171ca2ddc42dbe/Samples/Graphics/AMD_FSR/readme.md) · [Offline pipeline components](https://github.com/microsoft/Xbox-GDK-Samples/blob/b1e77b6ac10f95b07296025fda171ca2ddc42dbe/Samples/Graphics/HlslCompile/AcoPipelineState.cpp#L54-L86)

### GXDK 250405 compute UAV barrier

The supplied BVH builder calls command-list slot 146 at RVAs `0xd7b4` and `0xd80a`. Both pass one global barrier group containing one barrier. The four 32-bit fields at RVA `0xcdf60` specify compute shading (`0x80`) before and after the barrier and unordered access (`0x10`) on both sides. TomBox recognizes that form for callers tagged GXDK 250405 and records a desktop UAV barrier with a null resource. This orders UAV accesses across resources on the command list and works with the base D3D12 interface. Other barrier groups and access masks are unverified. [Enhanced barrier layout](https://microsoft.github.io/DirectX-Specs/d3d/D3D12EnhancedBarriers.html#d3d12_global_barrier) · [Desktop UAV ordering](https://learn.microsoft.com/en-us/windows/win32/api/d3d12/ns-d3d12-d3d12_resource_uav_barrier)

The homebrew test records two dependent compute dispatches, checks the host barrier trace and reads back 32 incremented values. The tracker marks slot 146 partial because the translation handles this single global compute UAV barrier form. [Run the fixture](../tools/homebrew/README.md#graphical-coin-run)

### GXDK 250405 thread-count dispatch

At RVAs `0x14972` and `0x15215`, the BVH builder writes a five-word inline dispatch packet. When the packet buffer is full, it passes the same X, Y, Z and control word to command-list slot 80. The observed values `0x20` and `0x8020` match the RDNA2 thread-dimension and wave32 control bits. TomBox interprets these calls as thread counts and divides each axis by the bound shader’s group dimension before issuing desktop `Dispatch`. [AMD register definitions](https://github.com/torvalds/linux/blob/master/drivers/gpu/drm/amd/include/asic_reg/gc/gc_10_3_0_sh_mask.h) · [Desktop dispatch dimensions](https://learn.microsoft.com/en-us/windows/win32/api/d3d12/nf-d3d12-id3d12graphicscommandlist-dispatch)

The translation covers whole groups for a subset of straight-line shaders using ordinary register operands, buffer loads and stores, and lane-local arithmetic. Wave-sensitive instructions, unrecognized shaders and control words, partial groups and counts above the desktop limit are skipped with a diagnostic. The homebrew test verifies both observed control words, pipeline switching and the bounds of GPU writes. Sixteen BVH builder shaders have passed readback through ordinary group dispatch. The library’s own slot 80 dispatch sequence remains unverified. [Run the fixture](../tools/homebrew/README.md#graphical-coin-run)

### Open GPU synchronization calls

The supplied `XboxBvhBuilder.dll` (GXDK 250405, SHA-256 `99f803ac22c01ef329dddb1d40be67238d2a812d01e8d040b3cb5135bc23707e`) uses a table of four-byte counters. At RVAs `0xd679` and `0xd6de`, command-list slots 73 and 71 receive the same counter address. Slot 73 receives a comparison value and a 32-bit mask. Slot 71 receives an incremented value and either zero or `0x1000`. Slot 90 receives a counter address and zero at RVA `0xe836`. Microsoft's pipelined postprocessing sample demonstrates GPU-side waits and bottom-of-pipe writes with *64-bit* counters. The 32-bit flags and ordering between queues remain open. [Microsoft synchronization sample](https://github.com/microsoft/Xbox-GDK-Samples/blob/b1e77b6ac10f95b07296025fda171ca2ddc42dbe/Samples/Graphics/PipelinedPostprocess/PipelinedPostprocess.cpp#L278-L300)

The exported `BuildRaytracingAccelerationStructure` at RVA `0x21610` copies the 48-byte build description into a batch entry. At `0xd069` the builder copies the field at byte 40 into the job. That field matches `ScratchAccelerationStructureData`, a GPU virtual address. Slots 71, 73 and 90 address four-byte words within this scratch buffer. A desktop immediate write requires the destination buffer in `COPY_DEST`; translating it also requires knowing the backing resource and its state at that point in the command list. [Build description](https://learn.microsoft.com/en-us/windows/win32/api/d3d12/ns-d3d12-d3d12_build_raytracing_acceleration_structure_desc) · [Desktop immediate-write requirements](https://learn.microsoft.com/en-us/windows/win32/api/d3d12/nf-d3d12-id3d12graphicscommandlist2-writebufferimmediate)

### D3D12.x slot tracking

The map covers device slots 0–159 and command-list slots 0–148. Each interface keeps its original baseline and grows through the highest recorded slot, including the intervening positions. Grey covers both unidentified positions and observed calls that still need a translation. [Tracker data](../tools/progress/progress.json)

| Command-list slot | Caller and call RVA | Evidence | Status |
|---|---|---|---|
| 71, 73, 90 | BVH builder 250405 · `0xd6de`, `0xd679`, `0xe836` respectively | The three calls address counters in the build scratch buffer. | Unmapped |
| 80 | BVH builder 250405 · `0x149ba`, `0x1525d` | Thread-count dispatch translates whole groups for the verified shader subset. | Partial |
| 141 | FidelityFX 260402 · `0x68a5`, 261000 · `0x65f8` | Both callers pass the GPU address and size of the buffer they name `FFX_XBox_ComputeScratch`. The name is at RVA `0x67478` in April and `0x482b0` in October. | Unmapped |
| 146 | BVH builder 250405 · `0xd7b4`, `0xd80a` | The single global compute UAV barrier form passes GPU readback under Proton. | Partial |
| 148 | BVH builder 250405 · `0x114af`, `0x14afb` | Callers pass a byte size and 8 or 16. The second caller writes records through the returned pointer. | Unmapped |
