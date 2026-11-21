#!/usr/bin/env bash
set -eu
cd "$(dirname "$0")/../.."
build=target/x86_64-pc-windows-msvc/release
sdk=${XWIN_SDK_ROOT:-${XDG_CACHE_HOME:-$HOME/.cache}/cargo-xwin/xwin}
compile_flags=()
build_pe() {
    local output=$1 source=$2
    shift 2
    clang-cl "${compile_flags[@]}" --target=x86_64-pc-windows-msvc -fuse-ld=lld /nologo /MT /EHsc /std:c++17 /I"$build" \
        -imsvc "$sdk/crt/include" -imsvc "$sdk/sdk/include/ucrt" \
        -imsvc "$sdk/sdk/include/shared" -imsvc "$sdk/sdk/include/um" -imsvc "$sdk/sdk/include/winrt" \
        /Fo"$build/" /Fe"$output" "$source" \
        /link /libpath:"$sdk/crt/lib/x86_64" \
        /libpath:"$sdk/sdk/lib/um/x86_64" /libpath:"$sdk/sdk/lib/ucrt/x86_64" kernel32.lib "$@"
}
if [[ ${1:-} == --package-read-test ]]; then
    mkdir -p "$build"
    build_pe "$build/package_read_probe.exe" tools/homebrew/package_read_probe.cpp
    exit 0
fi
if [[ ${1:-} == --winrt-oom-fixture ]]; then
    [[ $# == 3 ]] || { echo "Usage: $0 $1 MSVC_TOOLS_DIRECTORY WINDOWS_WINMD" >&2; exit 2; }
    msvc=$(realpath -e -- "$2")
    winmd=$(realpath -e -- "$3")
    mkdir -p "$build"
    # MSVC emits the C++/CX catch metadata for converting bad_alloc. Its
    # metadata reader needs .NET Framework in the selected Wine build prefix.
    export WINEDEBUG="${WINEDEBUG:--all}"
    wine "$msvc/bin/Hostx64/x64/cl.exe" /nologo /MD /EHsc /ZW /ZW:nostdlib /d1ZWbad_alloc \
        "/FU$(winepath -w "$msvc/lib/x86/store/references/platform.winmd")" "/FU$(winepath -w "$winmd")" \
        "/I$(winepath -w "$msvc/include")" "/I$(winepath -w "$sdk/sdk/include/ucrt")" \
        "/I$(winepath -w "$sdk/sdk/include/shared")" "/I$(winepath -w "$sdk/sdk/include/um")" \
        /c "$(winepath -w "$PWD/tools/homebrew/runtime_winrt_fixture.cpp")" \
        "/Fo$(winepath -w "$PWD/$build/runtime_winrt_fixture.obj")"
    lld-link /subsystem:console /out:"$build/runtime_winrt_fixture.exe" "$build/runtime_winrt_fixture.obj" \
        /libpath:"$sdk/crt/lib/x86_64" /libpath:"$sdk/sdk/lib/um/x86_64" \
        /libpath:"$sdk/sdk/lib/ucrt/x86_64" kernel32.lib vccorlib.lib runtimeobject.lib
    exit 0
fi
if [[ ${1:-} == --runtime-import-test ]]; then
    mkdir -p "$build"
    build_pe "$build/runtime_import_probe.exe" tools/homebrew/runtime_import_probe.cpp user32.lib
    stl_flags=(--target=x86_64-pc-windows-msvc -fms-runtime-lib=dll -fexceptions -fcxx-exceptions -std=c++17
        -isystem "$sdk/crt/include" -isystem "$sdk/sdk/include/ucrt"
        -isystem "$sdk/sdk/include/shared" -isystem "$sdk/sdk/include/um")
    clang++ "${stl_flags[@]}" -S -emit-llvm tools/homebrew/runtime_cpp_fixture.cpp -o "$build/runtime_cpp.ll"
    python3 tools/homebrew/trace_runtime_calls.py "$build/runtime_cpp.ll" "$build/runtime_calls.ll"
    clang++ --target=x86_64-pc-windows-msvc -c "$build/runtime_calls.ll" -o "$build/runtime_calls.obj"
    clang++ "${stl_flags[@]}" -c tools/homebrew/runtime_cpp_report.cpp -o "$build/runtime_cpp_report.obj"
    lld-link /out:"$build/runtime_cpp_fixture.exe" "$build"/{runtime_calls,runtime_cpp_report}.obj \
        /libpath:"$sdk/crt/lib/x86_64" /libpath:"$sdk/sdk/lib/um/x86_64" \
        /libpath:"$sdk/sdk/lib/ucrt/x86_64" msvcprt.lib kernel32.lib
    exit 0
fi
if [[ ${1:-} == --scarlett-import-test ]]; then
    mkdir -p "$build"
    build_pe "$build/scarlett_import_probe.exe" tools/homebrew/scarlett_import_probe.cpp ole32.lib
    exit 0
fi
if [[ ${1:-} == --scarlett-http-test || ${1:-} == --scarlett-user-test ]]; then
    mkdir -p "$build"
    compile_flags=("-I$2/include")
    build_pe "$build/scarlett_startup_probe.exe" tools/homebrew/scarlett_startup_probe.cpp \
        "$2/lib/x64/xgameruntime.lib" ole32.lib advapi32.lib
    exit 0
fi
export RUSTFLAGS="-C target-feature=+crt-static ${RUSTFLAGS:-}"
cargo xwin build --release --locked \
    --target x86_64-pc-windows-msvc -p d3d12_xs
cargo xwin build --release --locked --target x86_64-pc-windows-msvc -p xmem --lib --examples
cp "$build/examples/coin_run.exe" "$build/"
python3 tools/homebrew/triangle_shaders.py > "$build/triangle_shaders.h"
build_pe "$build/bvh_sort_probe.exe" tools/homebrew/bvh_sort_probe.cpp "$build/d3d12_xs.dll.lib" d3d12.lib
build_pe "$build/fsr_prepass_probe.exe" tools/homebrew/fsr_prepass_probe.cpp "$build/d3d12_xs.dll.lib" d3d12.lib
build_pe "$build/fsr_postpass_probe.exe" tools/homebrew/fsr_postpass_probe.cpp "$build/d3d12_xs.dll.lib" d3d12.lib
build_pe "$build/fidelityfx_probe.exe" tools/homebrew/fidelityfx_probe.cpp "$build/d3d12_xs.dll.lib"
build_pe "$build/ps09_depth_probe.exe" tools/homebrew/ps09_depth_probe.cpp "$build/d3d12_xs.dll.lib" d3d12.lib
build_pe "$build/ps09_raster_probe.exe" tools/homebrew/ps09_raster_probe.cpp "$build/d3d12_xs.dll.lib" d3d12.lib
build_pe "$build/clear_pixel.exe" tools/homebrew/clear_pixel.cpp "$build/d3d12_xs.dll.lib"
build_pe "$build/coin_window.exe" tools/homebrew/coin_window.cpp "$build/d3d12_xs.dll.lib" "$build/examples/coin_rules.lib" "$build/xmem.dll.lib" \
    /subsystem:windows /entry:mainCRTStartup user32.lib ole32.lib xinput.lib ntdll.lib advapi32.lib userenv.lib ws2_32.lib bcrypt.lib

build_pe "$build/bvh_commands.dll" tools/homebrew/bvh_commands.cpp /dll

# Compile the same indirect draw under each observed caller ABI.
for edition in legacy 260402 261000; do
    compile_flags=()
    if [[ $edition != legacy ]]; then compile_flags+=("/DFIXTURE_GXDK=\"$edition\""); fi
    if [[ $edition == 260402 ]]; then compile_flags+=(/DFIXTURE_PIX_TIMING); fi
    build_pe "$build/scarlett_commands_$edition.dll" tools/homebrew/scarlett_commands.cpp /dll
done
compile_flags=()

# Keep the optional GameInput runtime separate from the host GameInput DLL.
adapter="$build/gameinput-xinput"
src=crates/compat/gameinput
mkdir -p "$adapter"
build_pe "$adapter/GameInput.dll" "$src/gameinput.cpp" /dll xinput.lib /def:"$src/GameInput.def"
# The DLL exports a name too, but the recorded console consumer imports ordinal 1.
sed 's/@1$/@1 NONAME/' "$src/GameInput.def" > "$adapter/ordinal.def"
lld-link /lib /machine:x64 /def:"$adapter/ordinal.def" /out:"$adapter/ordinal.lib"
build_pe "$adapter/gameinput_probe.exe" tools/homebrew/gameinput_probe.cpp "$adapter/ordinal.lib"
build_pe "$adapter/gameinput_callbacks.exe" tools/homebrew/gameinput_callbacks.cpp "$adapter/ordinal.lib"
build_pe "$adapter/gameinput_history.exe" tools/homebrew/gameinput_history.cpp "$adapter/ordinal.lib"
cp "$build/coin_window.exe" "$adapter/"
