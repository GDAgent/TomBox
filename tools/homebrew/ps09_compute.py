"""Inspect PS09 compute PSOs and check retail dispatches against guarded UAV references."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import struct
import subprocess
import sys

from ps09_cache import PS09Cache, open_cache
from ps09_classify import classifier_case
from triangle_shaders import shader, signature, words


# PSO, shader index, logical UAV binding ID, SHA-256 of the whole code record.
CLEAR_CASES = (
    (438, 1235, 24, "65387be3fc98ecbdcb37d3afe13a115ea2016c6cd022d2dce8fcfd9682fdde9c"),
    (521, 1318, 48, "23ed6ba267178cea7eec24d3b54d67c7046575b13d8faa7b3d980d75ca031c30"),
)


def compute_pso(cache, pso):
    """Read one indexed compute PSO, including its config, bindings and code."""
    if not isinstance(cache, PS09Cache):
        cache = PS09Cache(cache)
    if not 0 <= pso < cache.compute:
        raise ValueError(f"invalid PS09 compute PSO index {pso}")
    entry = cache.compute_table + pso * 24
    pso_hash, config_id, _, index, binding_id = struct.unpack_from("<Q4I", cache.data, entry)
    code = cache.code(index)
    config = cache.metadata(config_id)
    bindings = cache.metadata(binding_id)
    if len(config) != 28:
        raise ValueError("unrecognized PS09 compute configuration")
    registers = struct.unpack("<7I", config)
    xy, z = registers[3:5]
    threads = [xy & 0xffff, xy >> 16, z & 0xffff]
    if not all(threads) or threads[0] * threads[1] * threads[2] > 1024:
        raise ValueError(f"invalid PS09 threadgroup dimensions {threads}")
    if (len(bindings) < 56 or (len(bindings) - 24) % 32
            or struct.unpack_from("<II", bindings) != (0x403, len(bindings) - 20)):
        raise ValueError("unrecognized PS09 compute binding metadata")

    descriptors = []
    fields = list(struct.iter_unpack("<8I", bindings[24:]))
    if fields[-1][0] != 0x43:
        raise ValueError("missing PS09 binding metadata terminator")
    for tag, kind, offset, slot, payload, flags, _, _ in fields[:-1]:
        if tag == 0x41:
            descriptors.append({"kind": kind, "offset_dwords": offset, "logical_slot": slot,
                                "payload": payload, "flags": flags})
        elif tag != 0x42:
            raise ValueError(f"unrecognized PS09 binding tag {tag:#x}")
    return {"pso": pso, "pso_hash": f"{pso_hash:016x}", "shader": index,
            "threads": threads, "config": registers, "binding_metadata": binding_id,
            "bindings": descriptors, "code_sha256": hashlib.sha256(code).hexdigest()}, code


def _clear_root(info, logical_slot):
    # These two resource maps expose one descriptor at dword 0 of the second
    # user table (s5). Their code fetches s[8:11] or s[12:15] at that offset,
    # uses s6 as group X, converts the descriptor to byte addressing and stores
    # whole dwords. Bind its physical table element as u0 on the test device.
    if len(info["bindings"]) != 1:
        raise ValueError("expected a single PS09 descriptor")
    desc, = info["bindings"]
    if ((desc["kind"], desc["offset_dwords"], desc["logical_slot"], desc["flags"])
            != (0x303, 0, logical_slot, 0x10504) or info["threads"] != [32, 1, 1]):
        raise ValueError("unexpected PS09 single-UAV layout")
    # Table parameters reserve s4 and s5. The first table is unused by both programs.
    return words(1, 2, 24, 0, 0, 0,
                 0, 0, 48, 0, 0, 56,
                 1, 64, 1, 84,
                 0, 1, 0, 0, 0, 1, 1, 0, 0, 0)

def _mixed_root(info):
    # A single s4 table mixes seven UAVs and one CBV in 32-byte slots.
    layout = [(d["offset_dwords"], d["kind"], d["logical_slot"], d["flags"]) for d in info["bindings"]]
    expected = [(8*i, 0x303, slot, 0x404) for i, slot in enumerate((11, 9, 8, 5, 7, 6, 3))]
    if layout != expected + [(56, 0x302, 0, 0x404)] or info["threads"] != [1024, 1, 1]:
        raise ValueError("unexpected PS09 command shader resource layout")
    ranges = []
    for offset, kind, slot, _ in layout:
        ranges.extend((2 if kind == 0x302 else 1, 1, slot, 0, offset // 8))
    return words(1, 1, 24, 0, 0, 0, 0, 0, 36, len(layout), 44, *ranges)


def _filter_root(info):
    layout = [(d["offset_dwords"], d["kind"], d["logical_slot"], d["flags"]) for d in info["bindings"]]
    expected = [(992, 0x300, 60, 0x408), (1424, 0x302, 5, 0x404),
                (0, 0x303, 48, 0x10504), (8, 0x303, 43, 0x10508),
                (16, 0x303, 44, 0x10508), (24, 0x303, 38, 0x10508)]
    if layout != expected or info["threads"] != [8, 8, 1]:
        raise ValueError("unexpected PS09 filter shader resource layout")
    ranges = []
    for offset, kind, slot, _ in layout:
        ranges.extend(({0x300: 0, 0x302: 2, 0x303: 1}[kind], 1, slot, 0, offset // 8))
    return words(1, 2, 24, 0, 0, 0, 0, 0, 48, 0, 0, 56, 2, 64, 4, 104, *ranges)


def _filter_reference(packed, flat, direct):
    # The shader's fixed depth stencil, in texels. With discontinuous
    # depths, only the central tap survives the relative-depth test.
    taps = ([(0, 0), (-3, -3), (1, -3), (3, -3), (-3, 0),
             (3, 0), (-3, 3), (-1, 3), (3, 3)] if flat else [(0, 0)])
    scale = 15 if packed else 255

    def half(value):
        return struct.unpack("<e", struct.pack("<e", value))[0]

    def pixel(x, y):
        total = 0.0
        for dx, dy in taps:
            u, v = x + dx, y + dy
            if direct and not (0 <= u < 8 and 0 <= v < 8):
                opacity = 1.0
            else:
                u, v = u % 8, v % 8
                sample = (u + v) % 16 if packed else 17 + 3*u + 2*v
                opacity = half(sample / scale)
            total = half(total + opacity)
        # The guest accumulates in binary16 and multiplies by a binary16 reciprocal.
        return int(scale * half(total * half(1 / len(taps))) + 0.5)

    ref = [0x95959595] * 1024
    for y in range(8):
        for x in range(4 if packed else 8):
            ref[32*y + x] = pixel(2*x, y) | (pixel(2*x + 1, y) << 4) if packed else pixel(x, y)
    return struct.pack("<1024I", *ref)


def compute_envelope(info, code, root):
    if len(code) % 4 or code[24:36] != bytes.fromhex("000080d50001fe03c00a0000"):
        raise ValueError("unrecognized PS09 program entry")
    program = struct.unpack_from(f"<{(len(code) - 36) // 4}I", code, 36)
    return shader(5, program, signature(), signature(), root, tuple(info["threads"]))


def probe_compute(repo, archive, scratch):
    """Return evidence only after build, translation, dispatch and full readback."""
    target = repo / "target/x86_64-pc-windows-msvc/release"
    env = dict(os.environ)
    env["RUSTFLAGS"] = "-C target-feature=+crt-static " + env.get("RUSTFLAGS", "")
    env.setdefault("TOMBOX_DXC_DIR", "Z:" + str(target).replace("/", "\\"))
    subprocess.run(["cargo", "xwin", "build", "--locked", "--release", "--target",
                    "x86_64-pc-windows-msvc", "-p", "tuxbox-xshader", "--example", "ps09_compute"],
                   cwd=repo, env=env, check=True, stdout=sys.stderr)
    exe = target / "examples/ps09_compute.exe"

    def guest(path):
        return "Z:" + str(path.resolve()).replace("/", "\\")

    def dispatch(blob, readback, *args):
        subprocess.run([repo / "tuxbox", "run", exe, *args, guest(blob), guest(readback)],
                       cwd=repo, check=True, env=env, timeout=120, stdout=sys.stderr)
        return readback.read_bytes()

    results = []
    with open_cache(archive) as cache:
        for pso, index, slot, expected_hash in CLEAR_CASES:
            info, code = compute_pso(cache, pso)
            if info["shader"] != index or info["code_sha256"] != expected_hash:
                raise ValueError(f"PS09 compute PSO {pso} differs from the reference")
            blob, readback = scratch / f"cs-{index}.dxbc", scratch / f"cs-{index}.out"
            blob.write_bytes(compute_envelope(info, code, _clear_root(info, slot)))
            expected = [0xfeed0000 | i for i in range(128)]
            if index == 1235:
                # Global X <= 7 clears dword X.
                expected[:8] = [0] * 8
            else:
                # Global X < 2 writes {0,1,1} at byte offset 32*X.
                for start in [0, 8]:
                    expected[start:start + 3] = [0, 1, 1]
            if dispatch(blob, readback) != struct.pack("<128I", *expected):
                raise ValueError(f"PS09 shader {index} failed guarded UAV readback")
            results.append({"compute_pso": pso, "shader_index": index, "record_sha256": expected_hash,
                            "threads": info["threads"], "groups": 3, "matched_dwords": len(expected)})
        info, code = compute_pso(cache, 205)
        if info["shader"] != 1009 or info["code_sha256"] != "6740efc2c360080896dfa8a980eeff9cd6dd867422229efa6a47dbe6263d3ac0":
            raise ValueError("unexpected PS09 compute PSO 205")
        blob = scratch / "cs-1009.dxbc"
        blob.write_bytes(compute_envelope(info, code, _mixed_root(info)))
        # u11 receives the two source elements expanded from binary16 to float.
        copied = struct.pack("<8f", 1, -2, 3, -4, 5, -6, 7, -8) + bytes([0x55]) * 480
        for predicate in (False, True):
            commands = bytearray([0x99]) * 512
            if predicate:
                # Command 2 replicates the low 16 bits of u5[0].x into u6[3].
                struct.pack_into("<4I", commands, 48, *([0x5678] * 4))
            mode = "--mixed" if predicate else "--mixed-zero"
            if dispatch(blob, scratch / f"1009-{predicate}.out", mode) != copied + commands:
                raise ValueError(f"PS09 shader 1009 failed guarded UAV readback ({mode})")
        results.append({"compute_pso": 205, "shader_index": 1009, "record_sha256": info["code_sha256"],
                        "threads": info["threads"], "groups": 1, "dispatches": 2, "matched_dwords": 512,
                        "checked_predicate": "v_cmpx_ne_i16"})
        for pso, index, checksum in [
            (1230, 2027, "9f037bafd86b107a0caa3ad005545319995ffd2f9ba7cdb76ca650c9cfde40c9"),
            (1231, 2028, "ed5e9120ef7d1eece114bd05ab405aba4195dc28fad257fa66193d5d32e8568f"),
        ]:
            info, code = compute_pso(cache, pso)
            if info["shader"] != index or info["code_sha256"] != checksum:
                raise ValueError(f"unexpected PS09 compute PSO {pso}")
            blob = scratch / f"cs-{index}.dxbc"
            blob.write_bytes(compute_envelope(info, code, _filter_root(info)))
            packed = index == 2028
            for flat, direct in [(False, False), (True, False), (True, True)]:
                mode = "--filter" + ("-packed" if packed else "") + ("-flat" if flat else "") + ("-direct" if direct else "")
                observed = dispatch(blob, scratch / f"{index}{mode}.out", mode)
                if observed != _filter_reference(packed, flat, direct):
                    raise ValueError(f"PS09 shader {index} failed guarded texture readback ({mode})")
            results.append({"compute_pso": pso, "shader_index": index, "record_sha256": checksum,
                            "threads": info["threads"], "groups": 1, "dispatches": 3, "matched_dwords": 3072,
                            "checked_predicates": ["v_cmp_nle_f16", "v_cmpx_nle_f16"]})
        info, code = compute_pso(cache, 439)
        root, expected = classifier_case(info)
        blob = scratch / "cs-1236.dxbc"
        blob.write_bytes(compute_envelope(info, code, root))
        if dispatch(blob, scratch / "1236-classify.out", "--classify") != expected:
            raise ValueError("PS09 shader 1236 failed guarded material bin readback")
        results.append({"compute_pso": 439, "shader_index": 1236, "record_sha256": info["code_sha256"],
                        "threads": info["threads"], "groups": 1, "dispatches": 5,
                        "matched_dwords": len(expected) // 4, "checked_predicate": "v_cmp_ne_i64"})
    return results


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("archive", type=Path)
    parser.add_argument("pso", type=int, nargs="+", help="compute PSO indices to inspect")
    args = parser.parse_args()
    with open_cache(args.archive) as cache:
        print(json.dumps([compute_pso(cache, index)[0] for index in args.pso], indent=2))
