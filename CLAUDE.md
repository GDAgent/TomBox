# TuxBox — Xbox Series X|S Compatibility Runtime

## Mission

Build a general Xbox Series X|S compatibility layer for x86-64 PCs. Linux with Proton is the primary development host. Native Windows uses the same Windows-format shim DLLs.

Start with reproducible workloads. Coin Run and the checked-in fixtures exercise graphics, input, audio and import behavior without a retail dump. Use public Microsoft GDK samples to compare documented usage with the relevant runtime path. Record the source revision, build target, host, and observed result. Keep per-title integration findings in their own case studies. [GDK sample audit](docs/GDK_SAMPLE_AUDIT.md) · [Homebrew guide](tools/homebrew/README.md)

---

# 1. Non-negotiable engineering principles

## Think critically; do not implement blindly

Before making a significant implementation decision:

- Establish what the target binary and available evidence actually show.
- Read relevant source code, specifications, and existing implementations.
- Identify assumptions, unknowns, dependencies, and architectural consequences.
- Consider more than one technically plausible approach.
- Choose the approach best supported by evidence.
- Define how the implementation will be tested.
- Record uncertainty rather than disguising it.

Do not invent undocumented structures, API signatures, calling conventions, memory layouts, GPU packet semantics, or function behavior.

Do not infer that two similarly named interfaces are ABI-compatible.

Do not assume that a Windows API, standard Direct3D 12 interface, or ordinary PE loader behaves identically to its Xbox counterpart.

When evidence contradicts the current design, revise the design. Do not defend an earlier assumption merely because code has already been written.

## Evidence over confidence

Distinguish these states everywhere:

1. Observed in the supplied build.
2. Supported by authoritative documentation.
3. Inferred from multiple independent observations.
4. Hypothesized but unverified.
5. Implemented.
6. Compiles.
7. Unit tested.
8. Integration tested.
9. Verified against the actual target.
10. Known to work in a repeatable end-to-end test.

Never describe a hypothesis as a fact or an implementation as verified merely because it compiles.

## No fake progress

Prohibited:

- Empty stubs reported as completed implementations.
- Functions that return success without doing the advertised work.
- Fake compatibility percentages.
- Hardcoded outputs that imitate successful runtime behavior.
- Tests that only check whether the program builds.
- Catch-all exception handlers that silently hide failures.
- Homebrew or mock output presented as retail evidence.
- Documentation claiming support without corresponding tests.
- Disabling validation merely to make a test pass.
- Silently ignoring unsupported executable features.

Unsupported functionality must fail explicitly, with actionable diagnostics.

A small, correct implementation is better than a large collection of placeholders.

## Preserve the user's work

At the start:

- Inspect the entire working directory and repository state.
- Identify existing source, branches, uncommitted changes, build artifacts, and project conventions.
- Do not overwrite, delete, reset, or reorganize existing user work without a clear need.
- Do not run destructive Git commands.
- Keep the original downloaded archive unchanged.
- Keep game data outside source control unless the user explicitly directs otherwise.

---

# 2. Inspecting a supplied build

Use an authorized, already-decrypted build supplied for the task. Preserve the original input, record its provenance and checksum, and keep derived output separate. Inventory executable files, modules, assets and manifests. Check archive paths and sizes before extracting. Discover the executable and dependency names from the package itself. Keep retail inputs out of source control. The native tests and homebrew workflow have no retail download prerequisite.

---

# 3. Research the actual Xbox Series X|S execution environment

Research the platform from primary sources wherever available: Microsoft documentation, public GDK documentation, PE/COFF specifications, Direct3D documentation, compiler and ABI references, established open-source implementations, and carefully attributed reverse-engineering research.

Do not assume that public documentation describes every console-specific behavior.

## A. Executable format and loader behavior

Investigate the actual executable files found in the dump.

Determine:

- Container and executable formats.
- PE/COFF characteristics and Xbox-specific extensions, if present.
- Machine type and instruction-set requirements.
- Image base and entry point.
- Section layout, alignment, and protections.
- Imports and exports.
- Relocation types and processing requirements.
- Delay imports and dynamically loaded modules.
- TLS and thread initialization.
- Exception/unwind metadata.
- Runtime initialization and termination.
- Calling conventions and ABI requirements.
- Module dependency and loading order.
- Address-space assumptions.
- Required memory mappings and protections.
- Any Xbox-specific loader metadata.

Build a parser that reports raw facts and validates structural invariants. Preserve unknown fields and bytes rather than discarding them.

Compare the parser's output with independent tools or parsers where possible.

Do not assume that a conventional Windows PE loader can load the target. Equally, do not assume a custom loader is necessary until the differences are demonstrated.

Evaluate, with evidence, the feasibility of:

- Native loading where compatible.
- A custom loader.
- Static or dynamic relinking.
- Import shims and compatibility libraries.
- Runtime translation.
- A hybrid model.

The architecture may differ between executable modules. Avoid forcing one mechanism onto every component.

## B. Xbox APIs and runtime services

Identify the actual imported and dynamically resolved interfaces used by the target.

Research relevant areas, including:

- Xbox/GDK runtime interfaces.
- XGameRuntime.
- XTaskQueue and asynchronous work.
- XAsync patterns.
- User and profile interfaces.
- Controller and input interfaces.
- Storage and save-data interfaces.
- Filesystem and package path behavior.
- Memory allocation and virtual memory.
- Threads, synchronization, and timing.
- Networking and service dependencies.
- Audio.
- Achievements and platform features.
- Application lifecycle and system configuration.

For every discovered API, record:

- Exact symbol or ordinal/NID, as applicable.
- Module providing it.
- Signature and calling convention, if known.
- Evidence for its semantics.
- Whether it is required during startup, gameplay, or shutdown.
- Whether it has a documented PC equivalent.
- Whether a compatibility implementation is feasible.
- Implementation and test status.
- Confidence and unresolved questions.

Separate documented Windows behavior, documented GDK behavior, Xbox-only behavior, and unknown proprietary behavior.

Do not implement speculative signatures. Resolve them through evidence, controlled experiments, or leave them explicitly unsupported.

## C. CPU, memory, and operating-system behavior

Investigate the target's CPU instruction requirements and platform assumptions.

Study:

- x86-64 instruction usage and CPU feature requirements.
- ABI and register/stack conventions.
- Thread-local state.
- Exception handling.
- Virtual memory and page protections.
- Allocation alignment and granularity.
- Synchronization primitives.
- Timers and clock behavior.
- Thread priorities and scheduling assumptions.
- Memory-mapped files and storage behavior.
- Unified memory assumptions and GPU-visible allocations.

Do not assume console memory capacities or layouts can be reproduced by simply allocating the same number of bytes on a PC.

Build targeted experiments for uncertain behavior.

## D. Graphics and GPU architecture

Xbox Series X|S graphics are based on AMD RDNA 2, but do not treat the GPU as an ordinary PC RDNA 2 device without investigating console-specific differences.

Determine what the sample or title under test uses.

Investigate:

- Direct3D 12 and Xbox-specific graphics interfaces.
- Command submission and command-list semantics.
- GPU resource creation and residency.
- Descriptor heaps and descriptor interpretation.
- Root signatures and resource bindings.
- Resource state transitions and barriers.
- Fences and synchronization.
- Tiled resources and sparse/reserved resources.
- Texture formats, tiling, swizzling, and metadata.
- Render targets and depth/stencil behavior.
- GPU virtual addressing.
- Shader formats and bytecode.
- Shader stages, wave behavior, and hardware assumptions.
- Compute workloads.
- Pipeline creation and caching.
- Presentation and swap-chain behavior.
- Console-specific extensions or packets, if demonstrated by evidence.

Determine whether shaders are standard DXBC/DXIL, use Xbox-specific representations, or contain other transformations or metadata.

Evaluate native Direct3D 12, Vulkan, and hybrid backends only after mapping the actual command, resource, and shader requirements.

A shader translator alone is not a graphics compatibility layer. Resource semantics, command processing, synchronization, memory behavior, and presentation must also be addressed.

Keep the graphics backend modular and independently testable.

## E. Audio, input, storage, and other subsystems

Inspect actual dependencies and determine what the game requires for:

- Audio device initialization and mixing.
- Controller input and device discovery.
- Keyboard/mouse mapping where appropriate.
- File access and virtual paths.
- Asset streaming and asynchronous I/O.
- Save data and persistence.
- Networking and platform services.
- Windowing, display, and presentation.
- Logging, crash reporting, and diagnostics.

Do not assume every subsystem is needed to reach the first boot milestone. Prioritize by observed startup dependencies.

---

# 4. Study AnyPS5 as an engineering reference

Official repository:

https://github.com/boykopovar/AnyPS5

Study the real source code and documentation, not just the README.

Inspect its:

- Architecture.
- Build system.
- Executable parser and relinker.
- Import resolution.
- Replacement system libraries.
- Shader compiler and graphics translation.
- Host integration.
- Diagnostics.
- Test infrastructure.
- Coding conventions.
- Technical debt.

Understand how it separates executable transformation, compatibility libraries, graphics translation, and host functionality.

Extract transferable engineering ideas, not assumptions about PlayStation internals.

Xbox and PlayStation are different platforms. Do not copy PS5 structures, NIDs, ABI assumptions, GPU command semantics, or system-library implementations into this project.

Respect upstream licenses and attribution requirements. Reuse code only when its license permits it and the relevant obligations are followed.

---

# 5. Per-title analysis

Identify the build and host before recording a title result.

Determine:

- Main executable and module list.
- Engine version, if recoverable.
- Executable architecture and metadata.
- Startup sequence.
- Static imports and dynamic library loads.
- Required runtime interfaces.
- Graphics backend and shader formats.
- Asset formats and containers.
- Audio formats and runtime requirements.
- Configuration and command-line assumptions.
- Filesystem paths and content lookup behavior.
- Threads and asynchronous initialization.
- Platform service dependencies.
- Any game-specific assumptions that differ from a PC build.

If the target is Unreal Engine-based, identify the relevant engine generation and console-specific runtime behavior. Do not assume an ordinary Windows Unreal build is equivalent.

Build a dependency graph that distinguishes:

1. Required to load the executable.
2. Required to enter the game's initialization.
3. Required to create the first window/device.
4. Required to render the first frame.
5. Required to reach the main menu.
6. Required for gameplay.
7. Optional platform functionality.

Prioritize the shortest technically valid path to an actual first frame, without hardcoding or faking missing behavior.

Keep game-specific work separate from reusable platform functionality. Label any workaround with the build and behavior it applies to.

---

# 6. Architecture and implementation

Do not decide the architecture before the initial binary and dependency analysis. Produce an evidence-based architecture proposal, then implement the smallest meaningful proof of concept.

Evaluate the following components. Their final boundaries and implementation strategy must follow the research.

## A. Binary analysis toolkit

Build reusable tools for:

- File identification.
- PE/COFF parsing.
- Xbox-specific metadata inspection.
- Section and directory inspection.
- Import/export enumeration.
- Relocation analysis.
- Exception and TLS inspection.
- Dependency graph generation.
- Structural validation.
- Disassembly and instruction-feature reporting where appropriate.

The toolkit must be safe against malformed input and must not execute files during analysis.

## B. Loader and relinker

If justified by the target, implement a loader/relinker responsible for:

- Validating executable structures.
- Mapping sections correctly.
- Applying supported relocations.
- Resolving imports.
- Loading dependent modules.
- Establishing required runtime state.
- Applying necessary, well-understood ABI adaptations.
- Reporting unsupported features precisely.

Use explicit feature support tables. Reject unsupported relocation types or loader features rather than silently mishandling them.

Never patch instructions or metadata without understanding the consequences and writing tests.

## C. Runtime compatibility layer

Build modular subsystems based on the actual dependency graph:

- Memory and virtual memory.
- Threads and synchronization.
- Timing.
- Filesystem and storage.
- Asynchronous task queues.
- Input and controllers.
- Audio.
- Networking, where appropriate.
- Application lifecycle.
- Platform services.
- Graphics integration.

Preserve observable API semantics as closely as practical. Avoid a collection of unrelated one-off stubs.

Maintain an API compatibility matrix with exact signatures, evidence, semantics, implementation state, tests, confidence, and limitations.

## D. Graphics backend

Keep graphics translation independent from executable loading.

Investigate and implement the required layers:

- Device and adapter abstraction.
- Resource and memory model.
- Descriptor translation.
- Command processing.
- Barriers and synchronization.
- Shader parsing and translation.
- Pipeline creation and caching.
- Presentation.

Choose native D3D12, Vulkan, or a hybrid based on demonstrated requirements and host support. Document the trade-offs.

Build small test programs for each supported graphics feature before integrating the game.

## E. Host platform integration

Prioritize x86-64 Linux with Proton. Run the native analysis tests and verify runtime changes with Coin Run under Proton. Record native Windows and Proton results separately.

Implement proper host integration for:

- Process and thread management.
- Virtual memory.
- Windowing and presentation.
- Controller input.
- Keyboard and mouse.
- Audio devices.
- Filesystem access.
- Diagnostics and crash reporting.

Keep host-specific code behind explicit interfaces so both Proton and native Windows can exercise the shared runtime.

---

# 7. Development workflow and autonomy

You are expected to work autonomously.

Do not stop after producing a report or roadmap. After reconnaissance, begin implementing the first validated subsystem and continue making useful progress.

Do not ask the user to choose every next step. Make ordinary engineering decisions independently and explain consequential decisions in the project log.

When blocked:

1. State the precise blocker.
2. Gather evidence to understand it.
3. Test whether the blocker is fundamental or merely an implementation gap.
4. Investigate alternative approaches.
5. Build a minimal reproduction or isolated test.
6. Document what the test proves.
7. Continue with independent work while the blocker remains unresolved.

Use available shell tools, compilers, debuggers, disassemblers, static analyzers, and test frameworks. Inspect available tools before adding dependencies.

Do not spend time building a polished UI before the executable analysis and runtime foundations are real. A small CLI and useful diagnostics are sufficient initially.

## Decision records

For every major architectural choice, record:

- Problem being solved.
- Evidence gathered.
- Options considered.
- Chosen approach.
- Rejected alternatives and why.
- Risks and unknowns.
- Tests that validate the decision.

Revisit decisions when new evidence appears.

## Critical-thinking checklist

Before each significant implementation, ask:

- What exactly does the evidence establish?
- What remains unknown?
- What alternative explanation could fit the observations?
- Is the ABI correct?
- Are memory ownership and lifetimes correct?
- Are concurrency and synchronization semantics correct?
- Does this match the target platform?
- Can the behavior be tested independently?
- What happens on malformed input or unsupported behavior?
- Which assumptions depend on a particular title or build?

---

# 8. Testing, diagnostics, and correctness

Establish:

- Unit tests.
- Parser and malformed-input tests.
- Regression tests.
- Integration tests.
- Fuzzing for binary parsers and other untrusted inputs.
- Sanitizer builds where supported.
- Runtime assertions and structured diagnostics.
- Performance benchmarks.
- Repeatable target-specific test procedures.

Build small synthetic test binaries and graphics programs where useful. They are test fixtures, not substitutes for the real game.

Test relevant behavior, including:

- Executable parsing and validation.
- Section mapping.
- Relocation handling.
- Import resolution.
- Module loading.
- Memory protection.
- TLS and thread initialization.
- Exceptions and unwind behavior.
- Thread creation and synchronization.
- File and asynchronous I/O.
- Graphics resource creation.
- Descriptor behavior.
- Shader translation.
- Command submission.
- GPU synchronization.
- Input events.
- Audio initialization.

Every discovered bug should become a regression test where practical.

A test must verify behavior, not merely that a function returns success.

Maintain separate statuses for implemented, compiled, unit-tested, integration-tested, and verified against the actual target.

Capture logs and enough environment information to reproduce failures.

---

# 9. Project structure and documentation

First inspect the existing repository and adapt to its conventions. Do not impose a new structure if one already exists and is sound.

Maintain these documents, creating them if needed:

`README.md`
- Project purpose.
- Current verified capabilities.
- Build instructions.
- Supported host platforms.
- Current limitations.

`docs/ARCHITECTURE.md`
- Execution model.
- Subsystem boundaries.
- Data flow.
- Major design decisions.

`docs/XBOX_RESEARCH.md`
- Platform research.
- Binary structures.
- ABI findings.
- Runtime and graphics interfaces.
- Evidence and references.

`docs/FORTNITE_ANALYSIS.md`
- Source URL and archive checksum.
- Dump inventory.
- Executables and modules.
- Dependencies.
- Startup sequence.
- Target-specific findings.

`docs/COMPATIBILITY.md`
- API compatibility matrix.
- Feature status.
- Test evidence.
- Known limitations.

`docs/TECHNICAL_DEBT.md`
- Incomplete functionality.
- Unverified assumptions.
- Known bugs.
- Architectural risks.

`docs/DEVELOPMENT_LOG.md`
- Chronological work log.
- Changes made.
- Commands and tests run.
- Results.
- Blockers and next actions.

Keep documentation synchronized with the implementation. Do not inflate support claims.

---

# 10. Security, scope, and data integrity

This project is for authorized interoperability research and compatibility engineering.

Work with binaries and materials the user is legally authorized to analyze.

Do not implement mechanisms to compromise consoles, obtain unauthorized access to protected services, extract cryptographic secrets, or defeat platform security and access controls.

Do not connect test titles to production services, impersonate authenticated players, or attempt to bypass account, entitlement, or anti-cheat systems.

Keep user-provided game data separate from source control. Do not redistribute copyrighted game executables or assets.

Treat downloaded archives and binaries as untrusted input. Inspect and parse them safely. Do not execute unknown binaries during initial analysis. Use an appropriately isolated environment for any later execution experiments, with no unnecessary network access.

Prioritize memory safety, bounds checking, safe parsing, and reproducible analysis.

---

# 11. Performance and hardware

Design for real PC hardware.

Investigate:

- CPU instruction requirements.
- Host CPU feature detection.
- Threading and scheduling.
- Memory allocation and mapping overhead.
- GPU resource and synchronization overhead.
- Shader compilation latency.
- Pipeline caching.
- Asset loading and streaming.
- Storage throughput.
- Frame pacing and presentation.

Profile before optimizing. Do not prematurely optimize unverified code.

Keep Series X and Series S characteristics in mind, but do not hardcode one console's memory capacity, GPU assumptions, or performance profile throughout the architecture.

The host PC's hardware is not identical to either console. Compatibility must be established through semantics and behavior, not superficial hardware similarity.

---

# 12. Milestones and definition of success

Track milestones independently. Never mark one complete without evidence.

1. Input obtained and checksum recorded.
2. Dump safely extracted and inventoried.
3. Main executable and modules identified.
4. Executable structures parsed and validated.
5. Dependencies and imports mapped.
6. Xbox-specific loader requirements understood.
7. A defensible execution architecture selected.
8. A minimal loader/relinker proof of concept demonstrated, if appropriate.
9. Required runtime foundation initialized.
10. Essential system interfaces implemented and tested.
11. Graphics device and required features initialized.
12. The title executable reaches meaningful initialization.
13. Actual game renders a first frame.
14. Main menu is reached.
15. Input works.
16. Gameplay is reached.
17. Audio works.
18. Stable repeatable gameplay is demonstrated.
19. Additional Xbox Series X|S titles are evaluated.

A milestone must have a test log, captured output, reproducible steps, or other suitable evidence.

Do not claim a game is playable because it displays a splash screen or menu.

---

# 13. Contributor starting point

Run `./tuxbox test` on Linux. For runtime work, select the smallest homebrew check that exercises the changed subsystem, then run it under Proton. A public sample comparison should identify the specific API behavior that matched or failed. Record native Windows and Proton results separately. Retail regressions use the named build and the steps in its case study.
