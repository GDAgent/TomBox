"""Compare Coin Run's VCC/EXEC bits directly with an AMD RDNA2 GPU via ROCr."""
import argparse
import csv
import os
import random
import re
import struct
import subprocess
import sys
import tempfile
from pathlib import Path
from typing import NamedTuple

from rdna2_fixture import ROOT, package_cases

# Both encodings, independent FP32 and FP16/64 input modes, and output-only denormals.
MODES = ((32, 0), (32, 15), (64, 1), (64, 4), (64, 10))


class Case(NamedTuple):
    name: str
    mode: int
    words: list[int]
    asm: str
    x: str
    y: str
    rows: bytes


def opcode_names():
    # LLVM is an independent authority for valid compact opcode slots.
    for opcode in range(256):
        word = 0x7c000500 | opcode << 17
        data = " ".join(map(hex, word.to_bytes(4, "little")))
        decoded = subprocess.run(["llvm-mc", "-triple=amdgcn", "-mcpu=gfx1030", "-disassemble"],
                                 input=data, capture_output=True, text=True, check=True)
        match = re.match(r"\s*(v_cmp\w+)_e32\b", decoded.stdout)
        if match:
            yield opcode, match[1]


def operands(name):
    kind = name.split("_")[-1]
    width = int(kind[1:])
    sign = 1 << (width - 1)
    if kind.startswith("f"):
        unit = 1 << {16: 10, 32: 23, 64: 52}[width]
        inf = sign - unit
        # Include both signs of zero, subnormals, normal boundaries, infinity,
        # signaling NaN (low payload bit only) and quiet NaN.
        values = [0, 1, unit-1, unit, sign//2-unit, inf-1, inf, inf+1, inf+unit//2]
        values += [v | sign for v in values]
    else:
        values = [0, 1, 2, 0x81, sign-1, sign, sign+1, 2*sign-2, 2*sign-1]
    right = [0, *(1 << i for i in range(10)), 1023] if "_class_" in name else values
    pairs = [(a, b) for a in values for b in right]
    rng = random.Random(190)
    pairs.extend((rng.getrandbits(width), rng.getrandbits(width)) for _ in range(48))
    data = bytearray()
    for i, (a, b) in enumerate(pairs):
        if width == 64:
            data.extend(struct.pack("<QQ", a, b))
        else:
            data.extend(struct.pack("<4I", a, b, 0, i & 1))
    return data


def compare_case(opcode, name, encoding, mode):
    wide = name.endswith("64")
    classify = "_class_" in name
    cmpx = name.startswith("v_cmpx_")
    words = [0xbfa50000 | mode]  # s_denorm_mode
    if wide and not classify:
        # Coin Run used row.w to seed VCC. Reload it as the high half of v[2:3].
        words.extend([0xe030200c, 0x80030307, 0xbf8c3f70])
    src1 = 2 if wide else 1
    arg0, arg1 = "%3", "%4"
    if encoding == 32:
        words.append(0x7c000100 | opcode << 17 | src1 << 9)
    else:
        mod0, mod1 = 0, 0
        if name.rsplit("_", 1)[-1].startswith("f"):
            mod0 = 1 << 8  # absolute src0
            if classify:
                mod1, arg0 = 1 << 29, "-|%3|"
            else:
                mod1, arg0, arg1 = 2 << 29, "|%3|", "-%4"
        dst = 126 if cmpx else 106
        words.extend([0xd4000000 | opcode << 16 | mod0 | dst,
                      0x02000000 | mod1 | (256 + src1) << 9 | 256])
    words.append(0xbf810000)
    inst = f"{name}_e{encoding} " + ("" if cmpx else "vcc_lo, ") + f"{arg0}, {arg1}"
    x = "as_ulong(r.xy)" if wide else "r.x"
    y = ("r.z" if classify else "as_ulong(r.zw)") if wide else "r.y"
    return Case(name, mode, words, inst, x, y, operands(name))


def sdwa_cases():
    selects = ("BYTE_0", "BYTE_1", "BYTE_2", "BYTE_3", "WORD_0", "WORD_1", "DWORD")
    modifiers = (("%s", 0), ("-%s", 0x10), ("|%s|", 0x20), ("-|%s|", 0x30))
    for opcode, name in [(0xc9, "v_cmp_lt_f16"), (0xda, "v_cmpx_eq_f16"),
                         (0x01, "v_cmp_lt_f32"), (0x12, "v_cmpx_eq_f32"),
                         (0x8f, "v_cmp_class_f16"), (0x98, "v_cmpx_class_f32"),
                         (0xca, "v_cmp_eq_f16")]:
        rows = operands(name)
        rng = random.Random(170)
        for i in range(256):
            lhs = rng.getrandbits(32)
            # Mix equal operands, independent full registers, half 1.0 and zero.
            if i % 4 == 0:
                rhs = lhs
            elif i % 4 == 1:
                rhs = rng.getrandbits(32)
            else:
                rhs = 0x3c00 if i % 4 == 2 else 0
            rows.extend(struct.pack("<4I", lhs, rhs, 0, i & 1))
        for sel0 in range(7):
            variants = []
            if name != "v_cmp_eq_f16":
                sel1 = (sel0 + 3) % len(selects)
                for mod0, mod1 in [(0, 0), (1, 2), (3, 1), (2, 3)]:
                    if "_class_" in name:
                        mod1 = 0  # src1 is the integer class mask
                    text0, bits0 = modifiers[mod0]
                    text1, bits1 = modifiers[mod1]
                    variants.append((text0 % "%3", text1 % "%4", bits0 << 16 | bits1 << 24, sel1))
            if name in ("v_cmp_eq_f16", "v_cmp_lt_f32"):
                # SDWA S0=1 selects the scalar source encoding; 242 is the inline 1.0.
                variants.append(("1.0", "%4", 1 << 23 | 242, 6))
            for arg0, arg1, fields, sel1 in variants:
                dest = "" if name.startswith("v_cmpx_") else "vcc_lo, "
                asm = f"{name}_sdwa {dest}{arg0}, {arg1} src0_sel:{selects[sel0]} src1_sel:{selects[sel1]}"
                # Guest SDWAB fields follow RDNA2 ISA Table 89. Clang separately
                # assembles the symbolic instruction in the native ROCr kernel.
                words = [0xbfa5000f, 0x7c0002f9 | opcode << 17,
                         fields | sel0 << 16 | sel1 << 24, 0xbf810000]
                yield Case(name, 15, words, asm, "r.x", "r.y", rows)


def build_inputs(scratch, cases):
    total = sum(len(case.rows) // 16 for case in cases)
    state = bytearray()
    source = ["__kernel void compare(__global uint *io) {"]
    with (scratch / "cases.bin").open("wb") as suite:
        suite.write(struct.pack("<I", len(cases)))
        for _, mode, words, inst, x, y, rows in cases:
            start, count = len(state)//16, len(rows)//16
            suite.write(struct.pack("<4I", len(words), count, 1, 0))
            suite.write(struct.pack(f"<{len(words)}I", *words))
            suite.write(rows)
            # Exactly one active lane. Save and restore its EXEC around the
            # compare so a false CMPX cannot suppress the oracle's own store.
            source.extend([
                f'  __asm__ volatile("s_denorm_mode {mode}");',
                f'  for (uint i = {start}; i < {start+count}; ++i) {{',
                '    uint4 r = ((__global uint4*)io)[i]; uint vcc, exec, saved;',
                '    __asm__ volatile("s_mov_b32 %2,exec_lo\\n v_readfirstlane_b32 vcc_lo,%5\\n '
                + inst + '\\n s_mov_b32 %0,vcc_lo\\n s_mov_b32 %1,exec_lo\\n s_mov_b32 exec_lo,%2"',
                f'      : "=&s"(vcc), "=&s"(exec), "=&s"(saved) : "v"({x}), "v"({y}), "v"((uint)(r.w!=0)) : "vcc");',
                f'    io[{4*total}+i] = (vcc&1) | ((exec&1)<<1);', '  }'])
            state.extend(rows)
    (scratch / "compare.cl").write_text("\n".join([*source, "}"]))
    (scratch / "state.bin").write_bytes(state + bytes(4 * total))
    package_cases(scratch)
    return total


def probe(scratch, gpu, rocm, sdwa=False):
    if sdwa:
        cases = list(sdwa_cases())
    else:
        cases = [compare_case(op, name, width, mode) for op, name in opcode_names() for width, mode in MODES]
        if len(cases) != 190 * len(MODES):
            raise ValueError("LLVM did not identify all 190 gfx1030 VOPC instructions")
    total = build_inputs(scratch, cases)
    path = Path(__file__).resolve().parent
    subprocess.run(["c++", "-O2", "-std=c++17", "-I", rocm / "include", path / "native_vopc/dispatch.cpp",
                    "-L", rocm / "lib", f"-Wl,-rpath-link,{rocm / 'lib'}", "-lhsa-runtime64", "-o", "dispatch"], cwd=scratch, check=True)
    subprocess.run(["clang", "-target", "amdgcn-amd-amdhsa", f"-mcpu={gpu}", "-nogpulib",
                    "-O2", "-fno-unroll-loops", "-x", "cl", "-c", "compare.cl", "-o", "compare.o"],
                   cwd=scratch, check=True)
    subprocess.run(["ld.lld", "-shared", "compare.o", "-o", "compare.co"], cwd=scratch, check=True)
    env = dict(os.environ)
    env["LD_LIBRARY_PATH"] = os.pathsep.join(filter(None, (str(rocm / "lib"), env.get("LD_LIBRARY_PATH"))))
    subprocess.run(["./dispatch", gpu, "compare.co", "state.bin", "native.bin"],
                   cwd=scratch, env=env, timeout=120, check=True)
    subprocess.run([ROOT / "tuxbox", "run", scratch.parent / "coin_window.exe", "--rdna2-test", scratch.name],
                   cwd=ROOT, check=True, timeout=600, stdout=sys.stderr)
    native = (scratch / "native.bin").read_bytes()[16*total:]
    guest = (scratch / "results.bin").read_bytes()
    if len(native) != 4*total or len(guest) != 16*total:
        raise ValueError("incomplete native or translated VOPC readback")
    counts = {}
    pos = 0
    for case in cases:
        name, mode, rows = case.name, case.mode, case.rows
        for i, row in enumerate(struct.iter_unpack("<4I", rows)):
            _, vcc, active, marker = struct.unpack_from("<4I", guest, pos*16)
            expected, = struct.unpack_from("<I", native, pos*4)
            actual = ((active & 1) << 1) | (vcc & 1)
            if marker != 0x52444e41 or actual != expected:
                raise ValueError(f"{case.asm} mode {mode}, vector {i} {row}: "
                                 f"native EXEC/VCC {expected:02b}, translated {actual:02b}, marker {marker:#x}")
            pos += 1
        counts[name] = counts.get(name, 0) + len(rows)//16
    return counts


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--gpu", required=True, help="native ROCr GPU name, for example gfx1036")
    parser.add_argument("--sdwa", action="store_true", help="check floating SDWA modifiers, subwords and inline constants")
    parser.add_argument("--rocm", type=Path, default=Path("/opt/rocm"), help="ROCr headers and library prefix")
    args = parser.parse_args()
    if not re.fullmatch(r"gfx103[0-6]", args.gpu):
        parser.error("expected an RDNA2 gfx103x device")
    try:
        subprocess.run([ROOT / "tools/homebrew/build-clear.sh"], cwd=ROOT, check=True, stdout=sys.stderr)
        build = ROOT / "target/x86_64-pc-windows-msvc/release"
        with tempfile.TemporaryDirectory(dir=build, prefix="native-vopc-") as tmp:
            passed = probe(Path(tmp), args.gpu, args.rocm.resolve(), sdwa=args.sdwa)
        out = csv.writer(sys.stdout, delimiter="\t", lineterminator="\n")
        out.writerow(("mnemonic", "native_gpu", "matched_vectors"))
        out.writerows((name, args.gpu, count) for name, count in sorted(passed.items()))
    except (OSError, ValueError, subprocess.SubprocessError) as error:
        parser.exit(1, f"Native VOPC check failed: {error}\n")
