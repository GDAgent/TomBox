# Compatibility Matrix

Derived from the **actual imports** of `FortniteClient-XSX-Shipping.exe` (67 modules /
670 functions) and the exports of the Xbox system DLLs in `EmbeddedXvd/Windows/System32/`.
Raw per-DLL data: `analysis/fortnite_exe.json`, `analysis/syslib/*.json`.

## Evidence levels (CLAUDE.md §1)
1 Observed · 2 Documented · 3 Inferred · 4 Hypothesized · 5 Implemented · 6 Compiles ·
7 Unit-tested · 8 Integration-tested · 9 Verified vs target · 10 End-to-end.
Everything in the module matrix below is **level 1 (observed)** for the dependency facts;
no implementation exists yet.

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
| `XAudio2_9.dll` | 5 (ord #1,2,4,5,6) | Audio | Reuse PC XAudio2_9 | MED | ordinals match PC: `XAudio2Create`,`CreateAudioReverb`,`CreateFX`,`X3DAudioCalculate`,`X3DAudioInitialize` |
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
- No game API is implemented. No loader yet. No game runs.
- GDK (`QueryApiImpl`) service list not yet enumerated.
- Divergence of Xbox `api-ms-win-*` from desktop not yet validated per symbol.

## Additional x64 middleware observed in PE imports

These findings come from the import and export tables of six user-supplied
DLLs. Runtime behavior of the six binaries is untested. The synthetic
graphics example exercises the platform functions independently.

| DLL | Evidence from this build | TomBox coverage or open question |
|---|---|---|
| `amd_fidelityfx_loader_xbox.dll` | Exports five FidelityFX entry points including `ffxCreateContext` and `ffxDispatch`. Imports Windows library-loader APIs. | Static imports do not identify which provider will be loaded at runtime. |
| `amd_fidelityfx_upscaler_xbox.dll` | Imports `D3D12CreateRootSignatureDeserializer`, `XMemAlloc`, `XMemFree` and `XMemGetWorkingSetStatistics`. | A synthetic descriptor-table round trip exercises the deserializer. The statistics function returns `E_NOTIMPL`. Upscaling needs its own output test. |
| `Bink2Scarlett.dll` | Exports video frame operations. Imports `XMemAlloc`, `XMemFree` and ordinal 1 from `XAudio2_9.dll`. | The coin game exercises allocation and release. Audio and frame decoding remain unverified. |
| `libHttpClient.GDK.dll` | Exports HTTP and WebSocket operations and imports WinHTTP and Crypt32. | A local request with a known response would test the host HTTP path. |
| `XboxBvhBuilder.dll` | Imports `D3D12SerializeVersionedRootSignature` and `XMemVirtualQuery`. Exports ray-tracing acceleration structure operations. | TomBox forwards the serializer. `XMemVirtualQuery` is missing. The caller examines additional memory information beyond the desktop memory-query fields, so the Xbox output layout needs verification. |
| `XCurl.dll` | Exports curl easy/multi functions and imports WinHTTP, Crypt32 and Windows API sets. | HTTP behavior and availability of the imported API sets need checking on the target host. |
