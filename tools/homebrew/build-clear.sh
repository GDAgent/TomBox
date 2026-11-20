#!/usr/bin/env bash
set -eu
cd "$(dirname "$0")/../.."
build=target/x86_64-pc-windows-msvc/release
sdk=${XWIN_SDK_ROOT:-${XDG_CACHE_HOME:-$HOME/.cache}/cargo-xwin/xwin}
RUSTFLAGS="-C target-feature=+crt-static ${RUSTFLAGS:-}" cargo xwin build --release --locked \
    --target x86_64-pc-windows-msvc -p d3d12_xs
python3 tools/homebrew/triangle_shaders.py > "$build/triangle_shaders.h"
clang-cl --target=x86_64-pc-windows-msvc -fuse-ld=lld /nologo /MT /std:c++17 /I"$build" \
    -imsvc "$sdk/crt/include" -imsvc "$sdk/sdk/include/ucrt" \
    -imsvc "$sdk/sdk/include/shared" -imsvc "$sdk/sdk/include/um" -imsvc "$sdk/sdk/include/winrt" \
    /Fo"$build/" /Fe"$build/clear_pixel.exe" tools/homebrew/clear_pixel.cpp \
    "$build/d3d12_xs.dll.lib" /link /libpath:"$sdk/crt/lib/x86_64" \
    /libpath:"$sdk/sdk/lib/um/x86_64" /libpath:"$sdk/sdk/lib/ucrt/x86_64" kernel32.lib
