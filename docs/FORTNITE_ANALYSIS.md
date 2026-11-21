# Fortnite Xbox Series X|S — Target Analysis

Target: **Fortnite Xbox Series X|S, version 2.0.1461.7919**. All facts below are
**observed from the supplied build** unless noted.

## Source archive (verified)

| Field | Value |
|---|---|
| Source URL | https://ia801000.us.archive.org/10/items/fortnite-xbox-series-xs/436609B6.FortniteClient_2.0.1461.7919_neutral__9ncxwbgmmv7m8_xs.zip |
| Size | 27,385,950,541 bytes (25.51 GB) — downloaded size matches server exactly |
| Container | ZIP64, `application/zip`, 834 entries (770 files, 64 dirs, 0 symlinks) |
| **SHA-256 (archive)** | `433E7B3B65C0C51A4F20001721F7CC9FDA9AE97A0022A7F7B5AA94914BEFCA95` |
| Safety | `zip-inspect` verdict: **safe** (no traversal/absolute/symlink/bomb findings) |

## Archive structure

Two top-level trees (**not** an encrypted XVC — loose decrypted files, premise confirmed):

- **`EmbeddedXvd/`** (607 entries) — the Xbox GameOS system image. `Windows/System32/`
  holds 276 system DLLs/EXEs (~223 MB): the game's import targets, incl. `d3d12_x.dll`,
  `d3d12_xs.dll`, `xgameruntime.dll`, `xmem.dll`, `xg_xs.dll`, `GameInput.dll`,
  `XAudio2_9.dll`, `AcpHal.dll`, `XCRDAPI.dll`, `dxcompiler_x/xs.dll`, `dxbc2dxil_xs.dll`,
  `xbthunk.dll`, `GameConfigHelper.dll`, plus ordinary Windows DLLs (combase, crypt32, …).
- **`Mount/`** (223 entries) — the game package: `MicrosoftGame.config`, `appxmanifest.xml`,
  `package.manifest`, `FortniteGame/Binaries/XSX/` (the code), `FortniteGame/Content/Paks/`
  (24 GB of `.pak`/`.ucas`/`.utoc` IoStore assets), `Content/Movies/`, and `Symbols/`.

What we extracted locally (code only, assets skipped): `input/extracted/` — Binaries +
System32 DLLs/EXEs + manifests + symbols = ~390 MB.

## MicrosoftGame.config (authoritative)

- Executable: `FortniteGame\Binaries\XSX\FortniteClient-XSX-Shipping.exe`, Id `AppFortnite`,
  **`TargetDeviceFamily="Scarlett"`** (Xbox Series).
- Identity `436609B6.FortniteClient`, Publisher Epic Games, Version `2.0.1461.7919`.
- `TitleId 0FF4B5BD`, `StoreId BT5P2X999VH2`, **`RequiresXboxLive=true`**,
  PersistentLocalStorage 1024 MB.

## Main executable

| Field | Value |
|---|---|
| Path | `Mount/FortniteGame/Binaries/XSX/FortniteClient-XSX-Shipping.exe` |
| SHA-256 | `0B3D2E5968B0913EB01F52612793BFCC1984FF9764BD15D1C5AABF4830FCC3A0` |
| Size | 157,572,608 bytes (150 MiB) |
| Format | **PE32+ / AMD64**, EXECUTABLE_IMAGE \| LARGE_ADDRESS_AWARE |
| Subsystem | 2 = WINDOWS_GUI |
| Linker | 14.24 (MSVC, ~VS 2019 16.4) |
| Image base | `0x140000000`; Entry point RVA `0x0229a298` (in `.text`) |
| SizeOfImage | `0x099b7000` (~161 MB) |
| DLL characteristics | HIGH_ENTROPY_VA \| DYNAMIC_BASE (ASLR) \| NX_COMPAT \| TERMINAL_SERVER_AWARE |
| Relocations | **2,045,522** base relocations (all DIR64) → image is relocatable |
| TLS | TLS directory present (callbacks array; 0 resolved by static walk) |
| Build timestamp | `0x5f9b76c4` (≈ Oct 2020) |

**Sections (8):** `.text` (code, ~99 MB), `.rdata`, `.data`, `.pdata` (SEH unwind),
`.msvcjmc` (MSVC Just-My-Code), `_RDATA`, **`.xbld`** (Xbox-specific, MEM_DISCARDABLE,
0x45d bytes — loader metadata, to be examined), `.reloc`.

## Engine & symbols

- **Unreal Engine** (Fortnite). `Mount/Symbols/FortniteClient-XSX-Shipping-Symbols.bin`
  is UE's console symbol DB: `[magic 0x076b85ee][size][zlib]`, **decompresses to 118 MB**
  of demangled C++ symbol names.
- Startup chain visible in symbols: `WinMain`/`GuardedMain` → `FEngineLoop` (PreInit/Init)
  → `RHIInit` → **`FD3D12DynamicRHI`** → device creation. `FScarlett*`/`ScarlettRHI` absent
  as class names → the Series path lives inside `FD3D12DynamicRHI` behind platform `#if`s.
  This symbol DB enables labelling EXE functions by name for startup tracing.

## Imports — the dependency surface (67 modules / 670 functions)

**~52 of 67 modules are standard Windows API-Sets / UCRT** (`api-ms-win-core-*`,
`api-ms-win-crt-*`, `ext-ms-win-rtcore-ntuser-*`) plus `bcrypt`, `crypt32`, `ws2_32`,
`msvcp140`, `vcruntime140`, `mfplat`/`mfreadwrite`, `winhttp` — all of which desktop
Windows implements. Candidate for binding to the real OS (verify per symbol).

**Xbox-only / special modules actually imported by the EXE** (the reimplementation list):

| Module | Fns used | Role (from exports) | PC equivalent exists? |
|---|---|---|---|
| `d3d12_xs.dll` | 6 | Xbox D3D12 (std COM + Xbox ext: `D3D12XboxCreateDevice`, `DXGIXPresentArray`, `D3DFlushEntireCpuCache`) | PC `d3d12.dll` (translate) |
| `xmem.dll` | 10 | Memory (`XMemAlloc`, `XMemVirtualAlloc`, `XMemAllocatePhysicalPages`, …) over ntdll | VirtualAlloc2 + D3D12 heaps |
| `xg_xs.dll` | 1 (`XGCreateTextureComputer`) | RDNA2 texture tiling/swizzle/DCC math (self-contained) | none (reimplement/bypass) |
| `GameInput.dll` | 1 (`GameInputCreate`) | Input (IGameInput COM) | **PC GameInput redist** |
| `XAudio2_9.dll` | 5 (ordinals #1–#6) | Audio | **PC XAudio2_9** |
| `AcpHal.dll` | 6 (`ApuAlloc`, `AcpHalCreate`, …) | Xbox audio-DSP (ACP/APU) HAL | none (stub/software) |
| `XCRDAPI.dll` | 1 (`DStorageGetFactory`) | **DirectStorage** for `.ucas` streaming | **PC DirectStorage** |
| `dxcompiler_xs.dll` | 1 (`DxcCreateInstance`) | Runtime DXIL compiler (DXC) | **PC `dxcompiler.dll`** |
| `EOSSDK-XboxOneGDK-Shipping.dll` | 48 | Epic Online Services (bundled w/ game) | stub for offline first-frame |
| `GameChat2.dll` | delay-load | GDK game chat (bundled) | stub |

The game loads `xgameruntime.dll` dynamically. The console DLL has five named exports:
`InitializeApiImpl`, `InitializeApiImplEx`, `QueryApiImpl`, `UninitializeApiImpl` and
`XErrorReport`. The title's statically linked GDK wrappers request service interfaces
through `QueryApiImpl`. TuxBox's observed Windows run uses Microsoft's PC runtime,
with local user and store implementations for this build. [PE export inventory](../analysis/syslib/xgameruntime.dll.json) · [Windows findings](DEVELOPMENT_LOG.md#2026-11-17-late--local-xbox-user-and-store-login-reaches-the-backend)

### Linux research with Xodus

Xodus's Wine fork pins a specific revision of its replacement DLL. At Wine revision
`eab6973`, the DLL lists all five console export names and dispatches requests for
threading, user and store interfaces. In that revision, `XTaskQueueCreate` and
`XAsyncBegin` return `E_NOTIMPL`. A successful initialization alone would therefore
say little about this title's ability to complete an async operation.
[Wine dependency](https://github.com/xodus-gaming/wine/tree/eab69739f15180b96797645ccade44c1c4414980/dlls/xgameruntime) · [Export list](https://github.com/xodus-gaming/xgameruntime/blob/64aebcabb8c66121eae25d3bf0ace4b582ebb0da/xgameruntime.spec) · [Dispatcher](https://github.com/xodus-gaming/xgameruntime/blob/64aebcabb8c66121eae25d3bf0ace4b582ebb0da/main.c) · [Threading implementation](https://github.com/xodus-gaming/xgameruntime/blob/64aebcabb8c66121eae25d3bf0ace4b582ebb0da/xthreading.c)

The standalone runtime at `642e41c` dispatches only the threading class and includes
async and queue tests. Its `VerifySimpleAsyncCall` test is a candidate first Wine
experiment because TuxBox's local user implementation relies on the title's own async
wrappers to deliver results. Passing that test would establish a PC-runtime baseline.
The interface IDs and method order requested by the console build still need verification.
[Standalone dispatcher](https://github.com/xodus-gaming/xgameruntime/blob/642e41c7617310c5d1b39d4e525b01ecae261f9e/src/main.cpp) · [Upstream threading tests](https://github.com/xodus-gaming/xgameruntime/blob/642e41c7617310c5d1b39d4e525b01ecae261f9e/tests/System/test_xthreading.cpp)

## Shaders (risk downgraded)

Shaders are in the **DirectX bytecode family (DXBC/DXIL)**, not an alien format — evidenced
by `dxcompiler_x/xs.dll` (DXC, HLSL→DXIL) and **`dxbc2dxil_xs.dll`** (DXBC→DXIL converter)
in the system image, and the EXE importing `DxcCreateInstance`. PC D3D12 consumes DXIL.
*Still to confirm:* the exact shader container inside the `.ucas`/`.pak` assets.

## Dependency-graph tiers (CLAUDE.md §5) — current best mapping

1. **Load the EXE:** PE loader + relocations (DIR64) + TLS + SEH; satisfy/redirect the 67
   import modules. Bind the ~52 standard modules to Windows; stub/implement the Xbox-only.
2. **Enter init (`FEngineLoop::PreInit`):** CRT + `xmem` + GDK bootstrap (dynamic
   `xgameruntime` `QueryApiImpl`) + config (`GameConfigHelper`).
3. **First device (`RHIInit`/`FD3D12DynamicRHI`):** `d3d12_xs` `D3D12XboxCreateDevice` →
   translate to PC `d3d12.dll` device; `xg_xs` texture layout; windowing via
   `ext-ms-win-rtcore-ntuser-*`.
4. **First frame:** command lists, resource creation, `DXGIXPresentArray` → PC present.
5. **Main menu:** asset streaming (`DStorageGetFactory`, IoStore `.ucas`/`.utoc`), DXC
   shader compile, UI.
6. **Gameplay / 7. Optional:** EOS, chat, Xbox Live services (`RequiresXboxLive` — will
   not connect to production services; assess whether first-frame can proceed offline).

## Open questions
1. Decode `-Symbols.bin` address↔name mapping to label EXE RVAs.
2. Inspect `.xbld` section contents (Xbox loader metadata).
3. Shader container format inside IoStore assets.
4. Which `api-ms-win-*` behaviours diverge from desktop (per-symbol).
5. Whether `FEngineLoop` can reach a device offline given `RequiresXboxLive`.

## Shader format — DEFINITIVE finding (2026-11-17): Xbox-native "XDAT", DXIL stripped

The single largest rendering risk is now resolved with hard evidence. The RHI creates
graphics pipelines via **Xbox device vtable slot 60** (compute = slot 55), which we
forward to PC `ID3D12Device::CreateGraphicsPipelineState` (slot 10). Every forward
fails with `E_OUTOFMEMORY` (0x8007000E), with NO D3D12 debug-layer validation message
— i.e. the runtime/driver rejects the shader bytecode itself, not the PSO desc.

Dumping a real VS + PS at runtime and parsing the container (`analysis/`):

* Container is a standard **DXBC wrapper**, 7 chunks: `SFI0 ISG1 OSG1 PSV0 RTS0 HASH DXIL`.
  Input/output signatures, pipeline-state validation and an **embedded root signature
  (RTS0)** are all present and well-formed. Shader model reported = **6.0**, type = VS/PS.
* The `DXIL` chunk's `DxilBitcodeHeader` reports `BitcodeOffset=1580, BitcodeSize=4`, and
  those 4 bytes are **`NOBC`** ("NO BitCode") — the portable LLVM DXIL bitcode has been
  **stripped at build time**. The magic `42 43 C0 DE` ("BC\xC0\xDE") appears **nowhere**
  in the container.
* In its place, immediately after the `DxilProgramHeader`, is an Xbox-proprietary blob
  beginning **`XDAT`** (version 3) with a table of offset/size sub-blobs (e.g. 72/904,
  976/587). This is the console's precompiled native shader payload.
* No AMD `s_endpgm` (`0xBF810000`) terminator is visible in the obvious encoding, so the
  `XDAT` payload is either an Xbox shader IR consumed by the GDK UMD (`xgs12_10.dll`) at
  PSO-create time, or packed/relocated native RDNA2 microcode — not final raw ISA in the
  clear.

**Consequence.** This build does not carry portable shader bytecode. The PC D3D12 runtime
cannot build any graphics/compute PSO from `XDAT`. The game therefore boots fully, opens
its "Gaming UI" window, starts the render + RHI threads, but renders **black** because no
pipeline can be created. Reaching a real first frame requires an **`XDAT` → DXIL/SPIR-V
shader translator** — reverse-engineering the Xbox shader container (and, if it is native
ISA, an RDNA2 recompiler). This is now the dominant remaining subsystem and the gating
item for milestones 13+ (first frame, menu, gameplay).

Everything *below* the shader is proven working: loader, Xbox PE, physical memory,
DirectStorage (incl. zlib), 27 GB content load, D3D12.x device/command-list/descriptor/
resource translation, swap-chain/present, and the game's full init up to UI.

### Update: the native ISA is recoverable (zlib), not encrypted

Further reversing the `XDAT` payload (via the GDK UMD `xgs12_10.dll`, which embeds 53
reference `XDAT` shaders, and by parsing a dumped VS): the large `XDAT` sub-blob is
**zlib-compressed** (starts `78 9c`). Inflating it yields an **`FSSM`** container
(magic `FSSM`, version 0x0101, entry name "Main", `deadbeef` signature placeholders,
section-offset tables, and resource-binding records). The `FSSM` body is **native RDNA2
microcode**, confirmed by textbook markers at 4-byte alignment:

* `s_waitcnt`  `0xBF8C....`  (e.g. FSSM+0x2e8, +0x360, +0x39c, +0x400)
* `s_endpgm`   `0xBF810000`  (FSSM+0x454 — end of the program)
* `s_code_end` `0xBF9F0000`  (padding after the program)
* `s_cbranch`  `0xBFA0....`  (control flow)

So the full chain for every shader is:

```
DXBC container
  └─ DXIL chunk  (bitcode stripped to "NOBC")
       └─ XDAT v3  (offset/size section table)
            └─ zlib (78 9c)
                 └─ FSSM v0x0101
                      ├─ resource-binding / register-map metadata
                      └─ RDNA2 native ISA  (gfx10xx, Series X|S)
```

**Implication for the translator.** Decompression is trivial (we already link
`miniz_oxide`). The real work is: (1) parse DXBC→XDAT→FSSM and extract the ISA + bindings
(tractable); (2) a **RDNA2 disassembler** (documented ISA); (3) a **RDNA2 → DXIL/SPIR-V
recompiler** (the large subsystem — CFG recovery, SSA, structurization). The outer DXBC
`ISG1`/`OSG1`/`PSV0`/`RTS0` chunks already give the I/O signatures, pipeline-validation
info and root signature, which the recompiler needs. Build order: extractor → disassembler
→ recompiler (simplest shaders first).

## Server-side code in the client executable (2026-11-17, static, from the symbol DB)

Question: is server gameplay code present in `FortniteClient-XSX-Shipping.exe`? Method:
function sizes from the symbol database (1,245,949 symbols), spot-checked by disassembly.
Nothing was executed.

**Present with full-sized bodies**
- Game mode: `AFortGameModeAthena::StartNewSafeZonePhase` 7,492 bytes, `InitGame` 4,560,
  `SpawnItemDeliverySupplyDrop` 4,002, `StartAircraftPhase` 2,837, `HandleMatchHasStarted`
  2,727, `CheckWinConditionLastManStanding` 2,113 (847 symbols in the class).
- Dedicated-server session: `AFortGameSessionDedicatedAthena::OnPlaylistAssetUpdateComplete`
  6,578, `AFortGameSessionDedicated::HandleMatchHasEnded` 3,933.
- Engine login/net path: `UWorld::NotifyControlMessage` 6,995, `UNetConnection::ReceivedPacket`
  3,620, `AGameModeBase::PreLogin/Login/PostLogin`, `UWorld::SpawnPlayActor`,
  `UIpNetDriver::InitListen` 333, `AOnlineBeaconHost::InitHost` 415.

**Stripped or stubbed**
- `UWorld::Listen`: no symbol at all. `AGameModeBase::ProcessServerTravel`: 3 bytes.
- Player server RPCs: of 372 `A*::Server*_Implementation` symbols, 155 are <= 16 bytes.
  `ServerExecuteInventoryItem`, `ServerEditBuildingActor`, `ServerRepairBuildingActor`,
  `ServerPlayEmoteItem`, `ServerAttemptAircraftJump` all share one address whose body is
  `ret` (identical-code folding of empty functions). `ServerCreateBuildingActor` is 94
  bytes, `ServerReadyToStartMatch` 10.
- `UFortKismetLibrary::PickLootDrops`: no native body found under that name.

Reading (inference): the pattern of a client target built without server code — class
scaffolding and game-mode logic that is not behind the server-code guard survive, the
listen entry point and most client-to-server RPC bodies do not. Not verified: whether the
surviving game-mode functions are complete internally (guarded blocks inside a function
would not show up in its symbol), and nothing here has been run.
