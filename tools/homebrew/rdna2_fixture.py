"""Run the Coin Run shader verifier and its companion D3D12 memory probes."""
import argparse
import json
import os
from pathlib import Path
import re
import shutil
import struct
import subprocess

from triangle_shaders import compute_root, shader, signature

ROOT = Path(__file__).resolve().parents[2]


def exact(stream, size):
    value = stream.read(size)
    if len(value) != size:
        raise ValueError("truncated RDNA2 fixture")
    return value


def package_cases(directory):
    # gfx1030: load one four-word input from u0[group_id.x], seed VCC and the
    # accumulator, then run the case's original ISA in a one-thread workgroup.
    prefix = [
        0xF4080302, 0xFA000000, 0xBF8CC07F,     # load UAV descriptor into s[12:15]
        0x7E0E0205,                            # v7 = group ID x
        0xE0382000, 0x80030007, 0xBF8C3F70,    # load v[0:3]
        0x7D8A0680, 0x7E060302,                # VCC = v3 != 0; v3 = v2
    ]
    suffix = [
        0xBE88037E, 0xBEFE0381,                # save EXEC to s8; reactivate for readback
        0x7E08026A, 0x7E0A0208,                # v4 = VCC; v5 = saved EXEC
        0x7E0C02FF, 0x52444E41,                # v6 = completion marker
        0xE0782010, 0x80030307, 0xBF810000,    # store v[3:6] at byte offset 16; end
    ]
    with (directory / "cases.bin").open("rb") as source, (directory / "suite.bin").open("wb") as suite:
        count, = struct.unpack("<I", exact(source, 4))
        suite.write(struct.pack("<I", count))
        for _ in range(count):
            words, groups = struct.unpack("<II", exact(source, 8))
            if not 0 < words <= 65536 or not 0 < groups <= 65535:
                raise ValueError("invalid shader size or dispatch dimensions")
            body = struct.unpack(f"<{words}I", exact(source, words * 4))
            if body[-1] != 0xBF810000:
                raise ValueError("probe missing s_endpgm")
            xbox = shader(5, [*prefix, *body[:-1], *suffix], signature(), signature(), compute_root, (1, 1, 1))
            suite.write(struct.pack("<II", len(xbox), groups))
            suite.write(xbox)
            # Second half of each 32-byte UAV record is reserved for the shader's outputs.
            inputs = exact(source, groups * 16)
            rows = bytearray(b"\xa5" * (groups * 32))
            for i in range(groups):
                rows[i * 32:i * 32 + 16] = inputs[i * 16:(i + 1) * 16]
            suite.write(rows)
        if source.read(1):
            raise ValueError("trailing RDNA2 fixture data")


def run_windows(directory, command, log_name, env):
    log = directory / log_name
    result = subprocess.run([env["PROTON"], "run", "cmd.exe", "/d", "/c",
                             f"{command} > {directory.name}/{log_name} 2>&1"],
                            cwd=directory.parent, env=env)
    if log.exists():
        print(log.read_text(errors="replace"), end="", flush=True)
    result.check_returncode()


def memory_suite(directory, env):
    env["RUSTFLAGS"] = "-C target-feature=+crt-static " + env.get("RUSTFLAGS", "")
    build = subprocess.run(["cargo", "xwin", "test", "--locked", "--release", "--target", "x86_64-pc-windows-msvc",
                            "-p", "tombox-xshader", "--test", "mem_conformance", "--no-run", "--message-format=json-render-diagnostics"],
                           cwd=ROOT, env=env, stdout=subprocess.PIPE, text=True, check=True)
    exe, = [m["executable"] for m in map(json.loads, build.stdout.splitlines())
            if m.get("reason") == "compiler-artifact" and m["target"]["name"] == "mem_conformance" and m.get("executable")]
    shutil.copyfile(exe, directory / "memory.exe")
    env.pop("TOMBOX_MEM_ONLY", None)
    env["TOMBOX_MEM_MNEMONICS"] = f"{directory.name}/memory-mnemonics.txt"
    run_windows(directory, f"{directory.name}/memory.exe --nocapture", "memory.log", env)


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--memory", action="store_true")
    parser.add_argument("directory", type=Path)
    args = parser.parse_args()
    scratch = args.directory.resolve()
    # Only this generated basename is substituted into the cmd.exe command line.
    if not re.fullmatch(r"rdna2-check\.[a-zA-Z0-9]+", scratch.name):
        parser.error("expected the scratch directory created by play-coins.sh")
    environment = dict(os.environ)
    if args.memory:
        memory_suite(scratch, environment)
    else:
        package_cases(scratch)
        run_windows(scratch, f"coin_window.exe --rdna2-test {scratch.name}", "game.log", environment)
