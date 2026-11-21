"""Verify the PS09 two-word visibility rasterizers with a guarded 2x2 image."""
import struct

from ps09_compute import compute_pso, compute_envelope
from triangle_shaders import words

CASES = {
    1430: (2227, "096c18c7bda1783c5c2480a347b26cc0808a1ba69117b4ef44379323dbf3ca9a"),
    1431: (2228, "ac7612a5f261684f6dea6305fe502607c9142740bf118f902f230bad75c4dee2"),
}


def root(info):
    fields = ("kind", "offset_dwords", "logical_slot", "payload", "flags")
    layout = [tuple(binding[k] for k in fields) for binding in info["bindings"]]
    expected = [(0x300, 104, 13, 2048, 0x404), (0x300, 120, 73, 1, 0x404),
                (0x300, 144, 72, 176, 0x404), (0x300, 168, 71, 336, 0x404),
                (0x300, 232, 74, 4, 0x408), (0x303, 0, 10, 4, 0x10508),
                (0x302, 8, 15, 0, 0x10504), (0x302, 16, 0, 0, 0x10504),
                (2, 0, 500, 0, 0x20604)]
    if (CASES.get(info["pso"]) != (info["shader"], info["code_sha256"])
            or info["threads"] != [64, 1, 1] or layout != expected):
        raise ValueError("unrecognized PS09 raster shader or resource bindings")
    ranges = []
    for kind, offset, slot, _, _ in layout[:-1]:
        ranges.extend(({0x300: 0, 0x303: 1, 0x302: 2}[kind], 1, slot, 0, offset // 8))
    return words(1, 3, 24, 0, 0, 0,
                 0, 0, 60, 0, 0, 68, 1, 0, 76,
                 5, 88, 3, 188, 500, 0, 5, *ranges)


def reference_bytes():
    """Native GFX1036 readbacks for the segment, close depths, and empty material."""
    winner = 0x7fff800000000001
    guards = (0x0123456789abcdef, 0xfedcba9876543210)
    return b"".join(struct.pack("<4Q", a, b, *guards)
                    for a, b in ((winner, winner), (winner, winner + 1), (0, 0)))


def probe_raster(cache, scratch, run):
    """Dispatch three cases per shader and validate every byte against native GFX1036."""
    expected = reference_bytes()
    results = []
    for pso, (index, digest) in CASES.items():
        info, code = compute_pso(cache, pso)
        blob, output = scratch / f"raster-{index}.dxbc", scratch / f"raster-{index}.rg32"
        blob.write_bytes(compute_envelope(info, code, root(info)))
        output.unlink(missing_ok=True)
        run("ps09_raster_probe.exe", "--visibility-raster", blob, output)
        if output.read_bytes() != expected:
            raise ValueError(f"PS09 raster shader {index} failed guarded 64-bit visibility readback")
        results.append({"compute_pso": pso, "shader_index": index, "record_sha256": digest,
                        "threads": info["threads"], "groups": 1, "dispatches": 3, "matched_dwords": 24,
                        "checked_predicate": "v_cmpx_lt_u64"})
    return results
