"""CPU reference for the two simultaneous exports of PS09 pixel shader 3447."""
import hashlib
import math
import struct

from triangle_shaders import shader, signature


BINDINGS_SHA256 = "838b984f7a4130dc71cda18f24ef884a5c060e63d7b83d0b5189d0832c19fdf3"


def envelope(words, config, bindings):
    if hashlib.sha256(bindings).hexdigest() != BINDINGS_SHA256:
        raise ValueError("unexpected surface shader binding metadata")
    inputs = signature(("SV_Position", 1, 0), ("COLOR", 0, 15), ("TEXCOORD", 0, 15), ("NORMAL", 0, 15))
    outputs = signature(("SV_Target", 64, 0), ("SV_Target", 64, 0, 1))
    return shader(0, words, inputs, outputs)


def half_rtz(value):
    nearest, = struct.unpack("<e", struct.pack("<e", value))
    if abs(nearest) <= abs(value):
        return nearest
    bits, = struct.unpack("<H", struct.pack("<e", nearest))
    return struct.unpack("<e", struct.pack("<H", bits-1))[0]


def grid_reference():
    inputs, first, second = bytearray(), [], []
    for y in range(16):
        for x in range(16):
            # Non-zero normals span every octant and both halves of the folded
            # octahedron. Each material ID reaches the flat input as raw bits.
            nx, ny = (2*x - 15) / 16, (2*y - 15) / 16
            nz = (3 + (x+y) % 8) / 8 * (-1 if (x+y) % 2 else 1)
            color = ((1 + (x+y) % 7)/8, (x % 8)/8, (y % 8)/8, ((2*x+y) % 8)/8)
            material, gloss = (x+3*y) % 27, ((x+2*y) % 8)/8
            inputs.extend(struct.pack("<8fI3f", nx, ny, nz, *color, 0, material, gloss, 0, 0))
            norm = abs(nx) + abs(ny) + abs(nz)
            u, v = nx / norm, ny / norm
            if nz < 0:
                u, v = math.copysign(1-abs(v), u), math.copysign(1-abs(u), v)
            first.extend(color)
            second.extend(((1+u)/2, (1+v)/2, gloss, material/26))
    expected = [255 * min(1, max(0, half_rtz(value))) for value in first + second]
    return inputs, expected


def validate_grid(actual, expected):
    if len(actual) != len(expected):
        raise ValueError(f"surface shader returned {len(actual)} bytes, expected {len(expected)}")
    for i, (code, reference) in enumerate(zip(actual, expected)):
        if abs(code-reference) > 0.6:
            raise ValueError(f"surface shader, MRT {i // 1024}, pixel {(i % 1024) // 4}, channel {i % 4}: "
                             f"got {code}, reference {reference:.6f}")

def probe_mrt(cache_pixel_shader, archive, scratch, run):
    index, pso = 3447, 8924
    digest = "51f5bffb0e02f70b69897abe118cd3fae36e903cb08c621de91cba7bff4c56b9"
    blob, attrs, pixels = (scratch / f"mrt.{ext}" for ext in ("dxbc", "bin", "rgba"))
    blob.write_bytes(cache_pixel_shader(archive, pso, index, digest, special_envelope=envelope))
    data, reference = grid_reference()
    attrs.write_bytes(data)
    run("clear_pixel.exe", "--cache-mrt-grid", blob, attrs, pixels)
    validate_grid(pixels.read_bytes(), reference)
    return {"graphics_pso": pso, "shader_index": index, "record_sha256": digest, "bindings_sha256": BINDINGS_SHA256,
            "targets": 2, "matched_pixels": 512}
