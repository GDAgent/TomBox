"""Reproduce the segment visibility reference by dispatching original PS09 code on RDNA2."""
import argparse
import os
from pathlib import Path
import re
import struct
import subprocess
import tempfile

from ps09_cache import open_cache
from ps09_compute import compute_pso
from ps09_raster import CASES, reference_bytes, root


def kernel_source(record, size):
    code = record[36:36 + size]
    if len(code) != size or code[-4:] != bytes.fromhex("000081bf"):
        raise ValueError("missing measured PS09 kernel boundary")
    raw = ",".join(hex(w) for w, in struct.iter_unpack("<I", code))
    # Group X and all five inline words are zero. s4 and s5 point at the
    # validated descriptor tables. The compiler reserves the original register
    # footprint and keeps initial input operands outside the clobber set.
    entry = [r"s_mov_b32 s4, %0", r"s_mov_b32 s5, %1",
             *[f"s_mov_b32 s{n}, 0" for n in range(6, 12)],
             r"v_mov_b32 v0, %2", f".long {raw}"]
    # Reserved ROCr ABI registers are outside the input/clobber constraint.
    # The guest itself exits with S_ENDPGM before control returns to the wrapper.
    registers = [f'"s{n}"' for n in range(4, 72) if n not in (32, 33)]
    registers += [f'"v{n}"' for n in range(40)]
    assembly = r"\n".join(entry)
    return ('__kernel void probe(uint table0, uint table1) {\n'
            '    uint lane = __builtin_amdgcn_workitem_id_x();\n'
            f'    __asm__ volatile("{assembly}" :: "s"(table0), "s"(table1), "v"(lane) '
            f': {",".join(registers)});\n}}\n')


def reproduce(archive, gpu, rocm, scratch):
    host = Path(__file__).parent.resolve() / "native_vopc/raster.cpp"
    env = dict(os.environ)
    env["LD_LIBRARY_PATH"] = os.pathsep.join(filter(None, (str(rocm / "lib"), env.get("LD_LIBRARY_PATH"))))

    def run(*args):
        subprocess.run(args, cwd=scratch, env=env, check=True, timeout=60)

    run("c++", "-std=c++17", "-O2", "-I", rocm / "include", host, "-L", rocm / "lib",
        f"-Wl,-rpath-link,{rocm / 'lib'}", "-lhsa-runtime64", "-o", "raster")
    with open_cache(archive) as cache:
        for pso, (index, _) in CASES.items():
            info, code = compute_pso(cache, pso)
            root(info)  # validate the full record SHA and its original bindings before dispatch
            (scratch / "probe.cl").write_text(kernel_source(code, {2227: 2992, 2228: 2944}[index]))
            run("clang", "-target", "amdgcn-amd-amdhsa", f"-mcpu={gpu}", "-mwavefrontsize64", "-nogpulib",
                "-O2", "-x", "cl", "-c", "probe.cl", "-o", "probe.o")
            run("ld.lld", "-shared", "probe.o", "-o", "probe.co")
            rows = []
            for mode in range(3):
                run("./raster", gpu, "probe.co", str(mode), "readback.bin")
                rows.append((scratch / "readback.bin").read_bytes())
            if b"".join(rows) != reference_bytes():
                raise ValueError(f"GPU reference mismatch for shader {index}: {[r.hex() for r in rows]}")
            print(f"shader {index}: all 12 native RG32 pixels match ({gpu})")


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("archive", type=Path, help="local shaders.rpf")
    parser.add_argument("--gpu", required=True, help="native ROCr GPU name, for example gfx1036")
    parser.add_argument("--rocm", type=Path, default=Path("/opt/rocm"))
    args = parser.parse_args()
    if not re.fullmatch(r"gfx103[0-6]", args.gpu):
        parser.error("expected an RDNA2 gfx103x device")
    try:
        with tempfile.TemporaryDirectory(prefix="ps09-raster-") as scratch:
            reproduce(args.archive.resolve(), args.gpu, args.rocm.resolve(), Path(scratch))
    except (OSError, ValueError, subprocess.SubprocessError) as error:
        parser.exit(1, f"Native PS09 raster check failed: {error}\n")
