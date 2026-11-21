"""Guarded reference for the 8x8 half-tile material classifier in compute PSO 439."""
import struct

from triangle_shaders import words


def classifier_case(info):
    """Return the original binding layout and expected bytes from all five dispatches."""
    digest = "38606fa3c502987603b34d169f0fb8472c3444aa48d248e32a6918c348b96938"
    expected = [(0x300, 144, 72, 176, 0x404), (0x300, 168, 71, 336, 0x404),
                (0x300, 0, 237, 4, 0x10508), (0x303, 8, 24, 4, 0x10504),
                (0x302, 16, 17, 0, 0x10504)]
    fields = ("kind", "offset_dwords", "logical_slot", "payload", "flags")
    layout = [tuple(row[key] for key in fields) for row in info["bindings"]]
    if (info["shader"] != 1236 or info["code_sha256"] != digest or layout != expected
            or info["threads"] != [8, 8, 1]):
        raise ValueError("unexpected PS09 material classifier or descriptor layout")
    ranges = []
    for kind, offset, register, _, _ in layout:
        ranges.extend(({0x300: 0, 0x302: 2, 0x303: 1}[kind], 1, register, 0, offset // 8))
    root = words(1, 2, 24, 0, 0, 0, 0, 0, 48, 0, 0, 56, 2, 64, 3, 104, *ranges)

    # Expected material bin per 4x8 half tile, left then right. An empty
    # half has no visibility IDs and does not allocate a list entry.
    bins = [
        (None, None),  # All RG32 IDs are zero.
        (2, 2),        # High word alone selects material 32, geometry flag 2.
        (1, 4),        # Different materials supply left flag 1 and right flag 4.
        (None, 2),     # Only lane 63 has a nonzero ID.
        (1 | 4, 1),    # Clip to 5x4. Combine both flags on the left.
    ]
    reference = bytearray()
    for halves in bins:
        counters = [8] * 8
        output = counters + [0x77777777] * 120
        for tag, material in zip((0, 4), halves):
            if material is not None:
                output[8 + 16 * material + counters[material]] = tag
                counters[material] += 1
        output[:8] = counters
        reference.extend(struct.pack("<128I", *output))
    return root, bytes(reference)
