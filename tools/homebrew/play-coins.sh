#!/usr/bin/env bash
set -eu
if [[ ${1:-} == --gamechat-test || ${1:-} == --bvh-sort-test || ${1:-} == --fsr-postpass-test || ${1:-} == --fsr-prepass-test || ${1:-} == --fsr-sharpen-test ]]; then
    if [[ $# != 2 || ! -f $2 ]]; then
        echo "Usage: $0 $1 path-to-retail-library.dll" >&2
        exit 2
    fi
    library_path=$(realpath -- "$2")
fi
if [[ ${1:-} == --scarlett-http-test || ${1:-} == --scarlett-user-test ]]; then
    [[ $# == 3 && -d $2 && -f $3/bin/x64/xgameruntime.dll ]] || {
        echo "Usage: $0 $1 PACKAGE GDK_WINDOWS_DIRECTORY" >&2; exit 2;
    }
    gdk_windows=$(realpath -- "$3")
    library_dir=$(realpath -- "$2")
elif [[ ${1:-} == --runtime-import-test ]]; then
    [[ $# -ge 2 && $# -le 3 && -d $2 ]] || { echo "Usage: $0 $1 PACKAGE [X64_UCRT_REDIST]" >&2; exit 2; }
    library_dir=$(realpath -- "$2")
    winrt_exe=
    if [[ -n ${TUXBOX_WINRT_FIXTURE:-} ]]; then
        winrt_exe=$(realpath -e -- "$TUXBOX_WINRT_FIXTURE")
    fi
    ucrt_dir=
    if [[ $# == 3 ]]; then
        ucrt_dir=$(realpath -- "$3")
        [[ -f "$ucrt_dir/ucrtbase.dll" ]] || { echo "Missing $ucrt_dir/ucrtbase.dll" >&2; exit 2; }
    fi
elif [[ ${1:-} == --scarlett-libraries-test ]]; then
    [[ $# == 2 && -d $2 ]] || { echo "Usage: $0 $1 DIRECTORY" >&2; exit 2; }
    library_dir=$(realpath -- "$2")
fi
cd "$(dirname "$0")/../.."
: "${PROTON:?Set PROTON to the absolute path of the Proton Experimental proton executable}"
build="$PWD/target/x86_64-pc-windows-msvc/release"
required=(coin_window.exe)
case ${1:-} in
    --runtime-import-test)
        tools/homebrew/build-clear.sh "$1"
        required=(runtime_import_probe.exe runtime_cpp_fixture.exe)
        ;;
    --gamechat-test) tools/homebrew/build-clear.sh ;;
    --scarlett-import-test)
        [[ $# == 1 ]] || exit 2
        tools/homebrew/build-clear.sh "$1"
        required=(scarlett_import_probe.exe)
        ;;
    --scarlett-http-test|--scarlett-user-test)
        tools/homebrew/build-clear.sh "$1" "$gdk_windows"
        required=(scarlett_startup_probe.exe)
        ;;
    --rdna2-test|--command-list-test|--device-diagnostics-test|--scarlett-libraries-test)
        [[ $1 == --scarlett-libraries-test || $# == 1 ]] || exit 2
        # Run these coverage checks against the current game, fixture DLLs and translator.
        tools/homebrew/build-clear.sh
        ;;
esac
if [[ ${1:-} == --fidelityfx-test || ${1:-} == --fidelityfx-context ]]; then
    required+=(fidelityfx_probe.exe d3d12_xs.dll xmem.dll)
fi
if [[ ${1:-} == --scarlett-libraries-test ]]; then
    required+=(fidelityfx_probe.exe xmem.dll d3d12_xs.dll)
fi
if [[ ${1:-} == --bvh-sort-test ]]; then
    required+=(bvh_sort_probe.exe bvh_commands.dll)
elif [[ ${1:-} == --fsr-prepass-test || ${1:-} == --fsr-sharpen-test ]]; then
    required+=(fsr_prepass_probe.exe d3d12_xs.dll)
elif [[ ${1:-} == --fsr-postpass-test ]]; then
    required+=(fsr_postpass_probe.exe d3d12_xs.dll)
fi
if [[ ${1:-} == --compute-barrier-test || ${1:-} == --thread-dispatch-test ]]; then
    required+=(bvh_commands.dll)
fi
if [[ ${1:-} == --indirect-test ]]; then
    required+=(scarlett_commands_{legacy,260402,261000}.dll)
fi
if [[ ${1:-} == --gameinput-test ]]; then
    required+=(gameinput-xinput/{GameInput.dll,gameinput_probe.exe,gameinput_callbacks.exe,gameinput_history.exe,coin_window.exe})
fi
for artifact in "${required[@]}"; do
    if [[ ! -f "$build/$artifact" ]]; then
        tools/homebrew/build-clear.sh
        break
    fi
done
export STEAM_COMPAT_DATA_PATH="${STEAM_COMPAT_DATA_PATH:-$build/coin-prefix}"
export STEAM_COMPAT_CLIENT_INSTALL_PATH="${STEAM_COMPAT_CLIENT_INSTALL_PATH:-$HOME/.steam/root}"
mkdir -p "$STEAM_COMPAT_DATA_PATH"
repo=$PWD
cd "$build"
export TOMBOX_LOG="${TOMBOX_LOG:-coin-window.log}"
# The Windows GUI executable has no console of its own. Capture its scripted
# checks through cmd so Linux callers see pixel mismatches and the exit status.
case "${1:-}" in
    --runtime-import-test)
        scratch=$(mktemp -d "$build/runtime-check.XXXXXXXX")
        trap 'rm -rf -- "$scratch"' EXIT
        cp runtime_import_probe.exe runtime_cpp_fixture.exe "$library_dir"/{msvcp140,vcruntime140,vcruntime140_1}.dll "$scratch/"
        (cd "$scratch" && WINEDLLOVERRIDES="${WINEDLLOVERRIDES:+$WINEDLLOVERRIDES;}ucrtbase=b;msvcp140,vcruntime140,vcruntime140_1=n,b" \
            timeout 120s "$PROTON" run cmd.exe /d /c 'runtime_import_probe.exe coin-runtime 17 > report.tsv 2> errors.txt && runtime_cpp_fixture.exe > cpp.tsv 2>> errors.txt') || {
            [[ ! -f "$scratch/errors.txt" ]] || cat "$scratch/errors.txt" >&2
            exit 1
        }
        if [[ -s "$scratch/errors.txt" ]]; then cat "$scratch/errors.txt" >&2; fi
        [[ -s "$scratch/report.tsv" && -s "$scratch/cpp.tsv" ]]
        # A pair may pass in both executables. Keep the explicit check in that case.
        LC_ALL=C awk -F '\t' '{sub(/\r$/, ""); if (NF != 3) exit 1; if (!seen[$1 FS $2]++) print}' \
            "$scratch/report.tsv" "$scratch/cpp.tsv" > "$scratch/checked.tsv"
        if [[ -n $winrt_exe ]]; then
            cp -- "$winrt_exe" "$scratch/runtime_winrt_fixture.exe"
            cp -- "$library_dir/vccorlib140.dll" "$scratch/"
            (cd "$scratch" && WINEDLLOVERRIDES="${WINEDLLOVERRIDES:+$WINEDLLOVERRIDES;}ucrtbase=b;vccorlib140,msvcp140,vcruntime140,vcruntime140_1=n,b" \
                timeout 45s "$PROTON" run cmd.exe /d /c 'runtime_winrt_fixture.exe > winrt.log 2>&1') || {
                cat "$scratch/winrt.log" >&2; exit 1;
            }
            [[ -s "$scratch/winrt.tsv" ]]
        fi
        if [[ -n $ucrt_dir ]]; then
            cp -- "$ucrt_dir/ucrtbase.dll" "$scratch/"
            timeout 45s "$repo/tuxbox" run --native-ucrt "$scratch/runtime_import_probe.exe" --native-ucrt || {
                cat "$scratch/native-errors.txt" >&2; exit 1;
            }
            [[ -s "$scratch/native.tsv" ]]
            tr -d '\r' < "$scratch/native.tsv" > "$scratch/native-checked.tsv"
        fi
        if [[ -n $winrt_exe ]]; then mv "$scratch/winrt.tsv" "$build/runtime-winrt-checks.tsv"; fi
        if [[ -n $ucrt_dir ]]; then mv "$scratch/native-checked.tsv" "$build/runtime-native-ucrt-checks.tsv"; fi
        mv "$scratch/checked.tsv" "$build/runtime-import-checks.tsv"
        echo "Verified runtime imports written to $build/runtime-import-checks.tsv"
        exit 0
        ;;
    --scarlett-import-test)
        scratch=$(mktemp -d "$build/import-check.XXXXXXXX")
        trap 'rm -rf -- "$scratch"' EXIT
        cp scarlett_import_probe.exe "$scratch/"
        (cd "$scratch" && "$PROTON" run cmd.exe /d /c 'scarlett_import_probe.exe coin-import 17 > checked.tsv 2> errors.txt') || {
            cat "$scratch/errors.txt" >&2
            exit 1
        }
        tr -d '\r' < "$scratch/checked.tsv" > "$scratch/report.tsv"
        mv "$scratch/report.tsv" "$build/scarlett-import-checks.tsv"
        echo "Verified import report written to $build/scarlett-import-checks.tsv"
        exit 0
        ;;
    --gamechat-test)
        # Reproduce app-local loading by basename with the caller's selected provider.
        scratch=$(mktemp -d gamechat-check.XXXXXXXX)
        trap 'rm -f -- "$scratch"/{coin_window.exe,GameChat2.dll,d3d12_xs.dll,xmem.dll,chat.log,coin-window.log}; rmdir -- "$scratch"' EXIT
        cp -- coin_window.exe d3d12_xs.dll xmem.dll "$scratch/"
        cp -- "$library_path" "$scratch/GameChat2.dll"
        status=0
        (cd "$scratch" && WINEDLLOVERRIDES="GameChat2=n;${WINEDLLOVERRIDES:-}" "$PROTON" run cmd.exe /d /c "coin_window.exe --gamechat-test > chat.log 2>&1") || status=$?
        cat "$scratch/chat.log"
        exit "$status"
        ;;
    --scarlett-user-test)
        exec python3 "$repo/tools/homebrew/scarlett_libraries.py" "$library_dir" --user-runtime "$gdk_windows/bin/x64/xgameruntime.dll"
        ;;
    --scarlett-http-test)
        exec python3 "$repo/tools/homebrew/scarlett_libraries.py" "$library_dir" --http-runtime "$gdk_windows/bin/x64/xgameruntime.dll"
        ;;
    --scarlett-libraries-test)
        exec python3 "$repo/tools/homebrew/scarlett_libraries.py" "$library_dir"
        ;;
    --bvh-sort-test|--fsr-postpass-test|--fsr-prepass-test|--fsr-sharpen-test)
        export TOMBOX_DXC_DIR="${TOMBOX_DXC_DIR:-Z:$PWD}"
        if [[ $1 == --bvh-sort-test ]]; then
            export TOMBOX_BVH_LIBRARY="Z:$library_path"
            probe=bvh_sort_probe.exe
        else
            export TOMBOX_FSR_LIBRARY="Z:$library_path"
            case $1 in
                --fsr-prepass-test) probe=fsr_prepass_probe.exe ;;
                --fsr-sharpen-test) probe="fsr_prepass_probe.exe --sharpen" ;;
                --fsr-postpass-test) probe=fsr_postpass_probe.exe ;;
            esac
        fi
        result=$(mktemp retail-probe.XXXXXXXX)
        trap 'rm -f -- "$result"' EXIT
        status=0
        "$PROTON" run cmd.exe /d /c "$probe > $result 2>&1" || status=$?
        cat "$result"
        exit "$status"
        ;;
    --rdna2-test)
        scratch=$(mktemp -d rdna2-check.XXXXXXXX)
        trap 'rm -f -- "$scratch"/{cases.bin,suite.bin,results.bin,game.log,memory.log,memory.exe,memory-mnemonics.txt,graphics-gs.dxbc,graphics-ps.dxbc,coverage.tsv}; rmdir -- "$scratch"' EXIT
        export TOMBOX_COIN_PROBE_DIR="$PWD/$scratch" PYTHONDONTWRITEBYTECODE=1
        export TOMBOX_DXC_DIR="${TOMBOX_DXC_DIR:-Z:$PWD}"
        python3 "$repo/tools/homebrew/rdna2_fixture.py" --memory "$scratch"
        cargo test --manifest-path "$repo/Cargo.toml" --locked --release -p tuxbox-xshader \
            --test valu_conformance -- --ignored --exact coin_run_valu_conformance --nocapture
        for mode in --test --3d-test --array-test --cube-test --thread-dispatch-test; do
            "$repo/tools/homebrew/play-coins.sh" "$mode"
        done
        mv -- "$scratch/coverage.tsv" coin-rdna2-coverage.tsv
        echo "Verified shader coverage written to $PWD/coin-rdna2-coverage.tsv"
        exit 0
        ;;
    --fidelityfx-test|--fidelityfx-context)
        [[ $# == 1 ]] || exit 2
        TOMBOX_LOG=fidelityfx-context.log python3 "$repo/tools/homebrew/fidelityfx_fixture.py" "$1"
        exit $?
        ;;
    --command-list-test|--device-diagnostics-test|--test|--thread-dispatch-test|--compute-barrier-test|--pso-fallback-test|--indirect-test|--scarlett-copy-test|--uav-test|--discard-test|--placed-test|--present-test|--tiled-test|--tiled-reference|--mip-reference|--bc1-reference|--rect-test|--array-test|--cube-test|--state-test|--stream-test|--3d-test|--controller-test|--gameinput-test|--audio-test)
        if [[ $# == 1 ]]; then
            scratch=$(mktemp -d coin-check.XXXXXXXX)
            trap 'rm -f -- "$scratch/result.txt" "$scratch/barriers.log" "$scratch/"*.bin; rmdir -- "$scratch"' EXIT
            if [[ $1 == --audio-test ]]; then
                dbus-run-session -- python3 "$repo/tools/homebrew/audio_fixture.py" "$scratch/result.txt"
                exit 0
            fi
            if [[ $1 == --gameinput-test ]]; then
                WINEDLLOVERRIDES="${WINEDLLOVERRIDES:+$WINEDLLOVERRIDES;}gameinput=n" \
                    python3 "$repo/tools/homebrew/controller_fixture.py" "$scratch/result.txt" "$1"
                exit 0
            fi
            if [[ $1 == --controller-test ]]; then
                python3 "$repo/tools/homebrew/controller_fixture.py" "$scratch/result.txt"
                exit 0
            fi
            run_check() {
                local status=0
                "$PROTON" run cmd.exe /d /c "coin_window.exe $1 > $scratch/result.txt 2>&1" || status=$?
                if [[ -f $scratch/result.txt ]]; then cat "$scratch/result.txt"; fi
                return "$status"
            }
            if [[ $1 == --command-list-test ]]; then
                TOMBOX_LOG="$scratch/barriers.log" run_check "$1"
                python3 - "$scratch/barriers.log" <<'PYTHON'
from pathlib import Path
import re
import sys
trace = Path(sys.argv[1]).read_text(errors="replace")
expected = set(range(27, 32)) | set(range(56, 149))
expected -= {59, 60, 61, 67, 68, 80, 83, 85, 146}
entries = re.findall(r"cmdlist.hi slot=(\d+) arg1=(0x[0-9a-f]+) caller=coin_window\.exe\+0x[1-9a-f][0-9a-f]* -> 0", trace, re.I)
if sorted((int(s), int(arg, 16)) for s, arg in entries) != sorted((s, 0xc000+s) for s in expected):
    sys.exit("Missing or unexpected CommandList diagnostic")
if not re.search(r"cmdlist.hi slot=148 arg1=0xf148 caller=scarlett_commands_261000\.dll\+0x[1-9a-f][0-9a-f]* -> 0", trace, re.I):
    sys.exit("Missing high-slot diagnostic from the fixture DLL")
if trace.count("cmdlist.counter slot=90 recorded 32-bit zero write") != 1 or trace.count("cmdlist.counter slot=90 unsupported counter range, value or flags") != 2:
    sys.exit("BVH counter did not take the expected translated and unsupported paths")
print(f"Verified {len(expected)} command-list diagnostics and the DLL caller at slot 148.")
PYTHON
                exit 0
            fi
            if [[ $1 == --device-diagnostics-test ]]; then
                TOMBOX_LOG="$scratch/barriers.log" run_check "$1"
                python3 - "$scratch/barriers.log" <<'PYTHON'
from pathlib import Path
import re
import sys
trace = Path(sys.argv[1]).read_text(errors="replace")
slots = {int(s) for s in re.findall(r"dev_ext slot=(\d+) caller=coin_window\.exe\+0x[0-9a-f]+ -> E_NOTIMPL", trace, re.I)}
if slots != set(range(91, 160)):
    sys.exit(f"Device diagnostic trace mismatch: {sorted(slots ^ set(range(91, 160)))}")
if not re.search(r"dev_ext slot=159 caller=scarlett_commands_261000\.dll\+0x[1-9a-f][0-9a-f]* -> E_NOTIMPL", trace, re.I):
    sys.exit("Missing Device slot 159 trace from the fixture DLL")
print("Device slots 91–159 returned E_NOTIMPL and recorded the calling module and RVA.")
PYTHON
                exit 0
            fi
            if [[ $1 == --compute-barrier-test ]]; then
                # GPU scheduling can happen to serialize both passes even through a no-op.
                TOMBOX_LOG="$scratch/barriers.log" run_check "$1"
                if ! grep -q ' BarrierX slot=146 global compute UAV barrier recorded' "$scratch/barriers.log"; then
                    echo 'Missing translated compute UAV barrier' >&2
                    exit 1
                fi
                exit 0
            fi
            if [[ $1 == --discard-test || $1 == --indirect-test ]]; then
                # Discards and PIX annotations can produce identical pixels through a no-op.
                TOMBOX_LOG="$scratch/barriers.log" run_check "$1"
                if ! grep -q ' cmdlist.shim ' "$scratch/barriers.log"; then
                    echo 'Missing command-list shim trace' >&2
                    exit 1
                fi
                slots=55
                if [[ $1 == --indirect-test ]]; then slots='59|60|61'; fi
                if grep -Eq " cmdlist.hi slot=($slots) " "$scratch/barriers.log"; then
                    echo "$1 reached an unmapped command-list slot" >&2
                    exit 1
                fi
                exit 0
            fi
            if [[ $1 == --stream-test ]]; then
                python3 "$repo/tools/homebrew/rectangle_fixture.py" "$scratch" --stream
                TOMBOX_TRACE_UPLOAD_BARRIERS=1 TOMBOX_LOG="$scratch/barriers.log" run_check "$1 $scratch/stream.bin"
                # Compare XMem mapping events and the subresources entering COPY_DEST.
                diff -u - <(awk '
                    $3 == "XMemMapPhysicalPages" && $4 == "UNMAP" {print "unmap", $6; next}
                    $3 == "XMemMapPhysicalPages" {print "map", $5}
                    $3 == "upload.barrier" && $5 ~ /->0x400/ {print "upload", $4}
                ' "$scratch/barriers.log" | tr -d '\r') <<'EVENTS'
map pages=1
upload sub=1
map pages=1
map pages=1
map pages=2
upload sub=0
unmap pages=4
map pages=4
upload sub=0
EVENTS
                echo 'Verified that the detailed mip uploaded only after all four pages were mapped, on both loads.'
                exit 0
            fi
            if [[ $1 == --state-test ]]; then
                TOMBOX_TRACE_UPLOAD_BARRIERS=1 TOMBOX_LOG="$scratch/barriers.log" run_check "$1"
                # Assert the actual barrier arguments independently of pixel correctness.
                diff -u - <(awk '$3 == "upload.barrier" {print $4, $5}' "$scratch/barriers.log" | tr -d '\r') <<'BARRIERS'
sub=0 0x80->0x400
sub=1 0x80->0x400
sub=0 0x400->0x80
sub=1 0x400->0x80
sub=1 0x800->0x400
sub=1 0x400->0x800
BARRIERS
                echo 'Verified initial upload barriers and the COPY_SOURCE round trip on mip 1.'
                exit 0
            fi
            if [[ $1 == --rect-test || $1 == --array-test ]]; then
                shapes=(rgba-wide rgba-tall bc1-wide bc1-tall)
                options=()
                if [[ $1 == --array-test ]]; then
                    shapes=(rgba-tall bc1-wide)
                    options+=(--arrays)
                fi
                python3 "$repo/tools/homebrew/rectangle_fixture.py" "$scratch" "${options[@]}"
                for shape in "${shapes[@]}"; do
                    run_check "${1%-test}-$shape $scratch/$shape.bin"
                done
                exit 0
            fi
            args=$1
            if [[ $1 == --cube-test ]]; then
                python3 "$repo/tools/homebrew/rectangle_fixture.py" "$scratch" --cube
                args="$1 $scratch/cube.bin"
            elif [[ $1 == *-reference ]]; then
                options=()
                case "$1" in
                    --mip-reference) options+=(--mips) ;;
                    --bc1-reference) options+=(--bc1) ;;
                esac
                python3 "$repo/tools/homebrew/scarlett_reference.py" "$scratch/reference.bin" "${options[@]}"
                args="$1 $scratch/reference.bin"
            fi
            run_check "$args"
            exit 0
        fi
        ;;
esac
exec "$PROTON" run ./coin_window.exe "$@"
