#!/usr/bin/env bash
set -eu
cd "$(dirname "$0")/../.."
: "${PROTON:?Set PROTON to the absolute path of the Proton Experimental proton executable}"
build="$PWD/target/x86_64-pc-windows-msvc/release"
required=(coin_window.exe)
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
    --test|--placed-test|--present-test|--tiled-test|--tiled-reference|--mip-reference|--bc1-reference|--rect-test|--array-test|--cube-test|--state-test|--stream-test|--3d-test|--controller-test|--gameinput-test|--audio-test)
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
