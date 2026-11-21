# Architecture

## Execution model

TuxBox runs Xbox x86-64 instructions on the host CPU. The primary runtime host is Linux with Proton. The title and TuxBox shims share a 64-bit Windows-format process, so the same DLL interfaces also work on native Windows. The Direct3D 12.x shim emits desktop D3D12 calls. Proton provides the Windows APIs and vkd3d-proton translates D3D12 to Vulkan. The PE, archive, disassembly and shader analysis tools run natively on Linux.

The diagram sketches the subsystem boundaries. The tested Coin Run path uses Proton’s PE loader with app-local shim DLLs. [Linux test guide](../tools/homebrew/README.md)

```
          ┌─────────────────────────────────────────────────────────┐
          │                 x64 PE process (Proton or Windows)       │
          │                                                          │
          │   ┌───────────┐   maps & relocates   ┌───────────────┐   │
          │   │  loader   │─────────────────────▶│  game image   │   │
          │   │ /relinker │   resolves imports    │ (native x64)  │   │
          │   └─────┬─────┘        │              └──────┬────────┘   │
          │         │              ▼                     │ calls      │
          │         │     ┌──────────────────┐           ▼            │
          │         │     │ compat libraries │◀── imports resolved to │
          │         │     │ (xgameruntime,   │    our implementations │
          │         │     │  xtaskqueue, …)  │                        │
          │         │     └───────┬──────────┘                        │
          │         │             ▼                                   │
          │         │     ┌──────────────────┐   ┌─────────────────┐  │
          │         └────▶│ host services    │   │ graphics backend│  │
          │               │ (mem, threads,   │   │ D3D12.x → host  │  │
          │               │  fs, input, …)   │   │ D3D12 / Vulkan  │  │
          │               └──────────────────┘   └─────────────────┘  │
          └─────────────────────────────────────────────────────────┘
```

## Subsystem boundaries

| Subsystem | Responsibility | Status |
|---|---|---|
| `tuxbox-pe` | Safe PE/COFF parsing & fact reporting | **implemented, tested** |
| `pe-inspect` | CLI/JSON inspection, dependency classification | **implemented, tested** |
| PE loading | Map the executable and resolve its imports | Proton on Linux and the Windows loader on Windows load prepared titles and app-local shims |
| Compatibility libraries | Supply Xbox imports and bind host services | Partial implementations include XMem, Direct3D 12.x and GameInput |
| Host services | Memory, threading, filesystem, input and audio | Coin Run verifies selected operations, virtual controllers and audio loopback under Proton |
| Graphics | Translate Xbox commands, resources and shaders to desktop D3D12 | GPU readback covers homebrew scenes and selected retail compute shaders |
| Diagnostics | Record shim calls and report test results | Shared DLL logs, per-import behavior checks and GPU reference comparisons |

## Design rules

1. **Fail loud.** Unsupported relocations, imports, or features must produce an
   explicit diagnostic naming the exact symbol/feature — never a silent stub that
   returns success.
2. **Evidence-gated.** No subsystem is built ahead of demonstrated need from the
   dependency graph. Build the shortest valid path to a first frame.
3. **Host code behind interfaces.** All OS/GPU specifics sit behind traits so the
   core can be tested on Linux and the shared DLLs can run under Proton and Windows.
4. **Title-specific behavior.** The offline user and store adapters in `compat-common` depend on wrapper addresses from one recorded retail build. Installation requires the title’s explicit `local_user=1` configuration and matching PE timestamp. Multi-title GDK service support remains open.
5. **Independently testable.** Each subsystem ships with tests and small fixtures
   before game integration.

## Data flow to "first frame" (target path)

1. Parse & validate the main EXE (`tuxbox-pe`). ✔ tooling ready
2. Enumerate imports → dependency graph → classify required-at-startup modules.
3. Loader maps the image, applies relocations, builds an import thunk table bound to
   compat-library implementations; unresolved imports are reported, not faked.
4. Minimal compat libs bring the CRT + GDK init far enough to create a device.
5. Graphics backend creates a device/swapchain and presents the first rendered frame.

Coin Run exercises the full path on Proton. The Windows retail case has also reached presentation. Each additional title needs its own dependency inventory and first-frame result.
