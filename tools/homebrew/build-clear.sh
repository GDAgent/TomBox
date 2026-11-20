#!/usr/bin/env bash
set -eu
cd "$(dirname "$0")/../.."
build=target/x86_64-pc-windows-msvc/release
sdk=${XWIN_SDK_ROOT:-${XDG_CACHE_HOME:-$HOME/.cache}/cargo-xwin/xwin}
export RUSTFLAGS="-C target-feature=+crt-static ${RUSTFLAGS:-}"
cargo xwin build --release --locked \
    --target x86_64-pc-windows-msvc -p d3d12_xs
cargo xwin build --release --locked --target x86_64-pc-windows-msvc -p xmem --lib --examples
cp "$build/examples/coin_run.exe" "$build/"
python3 tools/homebrew/triangle_shaders.py > "$build/triangle_shaders.h"
build_pe() {
    local output=$1 source=$2
    shift 2
    clang-cl --target=x86_64-pc-windows-msvc -fuse-ld=lld /nologo /MT /EHsc /std:c++17 /I"$build" \
        -imsvc "$sdk/crt/include" -imsvc "$sdk/sdk/include/ucrt" \
        -imsvc "$sdk/sdk/include/shared" -imsvc "$sdk/sdk/include/um" -imsvc "$sdk/sdk/include/winrt" \
        /Fo"$build/" /Fe"$output" "$source" \
        /link /libpath:"$sdk/crt/lib/x86_64" \
        /libpath:"$sdk/sdk/lib/um/x86_64" /libpath:"$sdk/sdk/lib/ucrt/x86_64" kernel32.lib "$@"
}
build_pe "$build/clear_pixel.exe" tools/homebrew/clear_pixel.cpp "$build/d3d12_xs.dll.lib"
build_pe "$build/coin_window.exe" tools/homebrew/coin_window.cpp "$build/d3d12_xs.dll.lib" "$build/examples/coin_rules.lib" "$build/xmem.dll.lib" \
    /subsystem:windows /entry:mainCRTStartup user32.lib ole32.lib xinput.lib ntdll.lib advapi32.lib userenv.lib ws2_32.lib bcrypt.lib

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
