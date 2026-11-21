"""GPU readback for the cache's tagged depth export, including null exports."""
import hashlib
import struct

from triangle_shaders import shader, signature, words

PSO = 12178
INDEX = 5192
CODE_HASH = "140155da37a0b5ddf64fbef4de2f0e82672c2dbdb72beb9211eb07d946fc077b"
BINDINGS_HASH = "4ccbe11654a14beffed2d8053f26c19fa1e1cc32445cb06aee80140dac7650df"


def envelope(program, config, bindings):
    if len(config) != 380 or struct.unpack_from("<I", config, 88)[0] != 0x8080:
        raise ValueError("unrecognized PS09 tagged-depth pixel input mask")
    if hashlib.sha256(bindings).hexdigest() != BINDINGS_HASH:
        raise ValueError("unrecognized PS09 depth pass binding metadata")
    # 0x41 records specify the s8 table's buffer descriptor at dword 168
    # (32-byte slot 21, stride 336), the s9 table's image at zero, and s10.
    # The desktop fixture binds these at t0, t1 and one root constant at b0.
    root = words(1, 3, 24, 0, 0, 1,
                 0, 5, 60, 0, 5, 68, 1, 5, 76,
                 1, 88, 1, 108, 0, 0, 1,
                 0, 1, 0, 0, 21, 0, 1, 1, 0, 0)
    return shader(0, program, signature(("SV_Position", 1, 0)), signature(("SV_Depth", 65, 0)), root, ps09_graphics=config)


def probe_depth(cache_pixel_shader, archive, scratch, run):
    blob, output = scratch / "tagged-depth.dxbc", scratch / "depth.f32"
    blob.write_bytes(cache_pixel_shader(archive, PSO, INDEX, CODE_HASH, special_envelope=envelope))
    run("ps09_depth_probe.exe", "--tagged-depth", blob, output)
    result = output.read_bytes()
    if len(result) != 16 * 16 * 4:
        raise ValueError("incomplete tagged-depth readback")
    written = 0
    for y in range(16):
        for x in range(16):
            # Pixel IDs address 336-byte records across both halves of the
            # packed 64-bit value. Only bit-zero set and material tag 83 pass.
            record = (x + 3*y) % 8 + 32 * (y % 2)
            keep = (x + 2*y) % 5 >= 3 and record % 3 != 0
            depth24 = (0x10000 + (x*359 + y*1087)*293) & 0xffffff
            expected = struct.pack("<f", depth24 * (2**-24 + 2**-47) if keep else 1.0)
            pos = 4 * (16*y + x)
            if result[pos:pos + 4] != expected:
                raise ValueError(f"tagged depth at ({x}, {y}) differs from the reference")
            written += keep
    return {"graphics_pso": PSO, "shader_index": INDEX, "record_sha256": CODE_HASH,
            "bindings_sha256": BINDINGS_HASH, "matched_pixels": 256, "discarded_pixels": 256 - written}


def facing_envelope(program, config, bindings):
    if (len(config) != 380 or struct.unpack_from("<I", config, 88)[0] != 0x1080
            or len(bindings) != 48 or struct.unpack_from("<I", bindings, 16)[0] != 0x43):
        raise ValueError("unexpected PS09 front-face input or binding metadata")
    inputs = bytearray(signature(("SV_IsFrontFace", 9, 1)))
    struct.pack_into("<I", inputs, 8 + 16, 1)  # one uint, mask x
    inputs[8 + 24] = 1
    return shader(0, program, inputs, signature(("SV_Target", 64, 0)), ps09_graphics=config)


def probe_facing(cache_pixel_shader, archive, scratch, run):
    pso, index = 14459, 8604
    digest = "e43dfd0444eaaf89ab6f909a25423c84aaa19c151dd3cb09c1ad4b1f5a2767dc"
    blob, output = scratch / "facing.dxbc", scratch / "facing.rgba"
    blob.write_bytes(cache_pixel_shader(archive, pso, index, digest, special_envelope=facing_envelope))
    run("clear_pixel.exe", "--cache-facing", blob, output)
    # The positive front-face input selects packed half 0x1c04, the negative
    # back-face input 0x2b87. Only R is nonzero in either RGBA8 export.
    def rgba(half):
        red, = struct.unpack("<e", struct.pack("<H", half))
        return bytes((round(255 * red), 0, 0, 0))
    expected = (rgba(0x1c04) * 8 + rgba(0x2b87) * 8) * 16
    if output.read_bytes() != expected:
        raise ValueError("front-facing/back-facing pixel export differs from the reference")
    return {"graphics_pso": pso, "shader_index": index, "record_sha256": digest,
            "matched_pixels": 256, "front_facing_pixels": 128, "back_facing_pixels": 128}
