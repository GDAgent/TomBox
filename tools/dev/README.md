# Windows retail case-study tools

These scripts and debugger filters target the recorded Fortnite build. Use the root `tuxbox` launcher for other prepared executables and the [homebrew guide](../homebrew/README.md) for platform tests.

- `mdbg.rs`   — minimal Win32 debugger used to run the title (`rustc -O --edition 2021 mdbg.rs`).
                Captures OutputDebugString, first-chance exceptions, a module-resolved stack
                scan on crashes, `--exebp <rva>` breakpoints. Usage: `mdbg.exe <exe> [flags] [title args]`.
- `iter.sh`   — build + deploy `d3d12_xs.dll`, run the title under mdbg, print a summary
                (shader/PSO failures, frames presented, exit reason, last log lines).
- `allfx.sh`  — translate every `analysis/shaders/*.dxbc` fixture and compile the HLSL with fxc.
- `apply.pl`  — literal (non-regex) multi-block patcher: `perl apply.pl <file> <patch>` with
                `@@OLD / @@NEW / @@END` blocks; atomic, CRLF-safe.

Paths inside the scripts point at the original scratch locations; adjust `SB=` if needed.
