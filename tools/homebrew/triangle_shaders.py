"""Generate original RDNA2 fixtures in the subset of DXBC TomBox reads.

The payload is synthetic: these are inputs to TomBox's extractor, not retail
compiler output. No Xbox SDK, game files or pretranslated HLSL are used.
"""
import struct
import zlib


def words(*values):
    return struct.pack(f"<{len(values)}I", *values)


def align4(data):
    return data + bytes(-len(data) % 4)


def signature(*elements):
    # Each 32-byte ISG1/OSG1 entry describes a float4 in the next register.
    entries = bytearray()
    names = bytearray()
    for register, (name, system_value, read_mask) in enumerate(elements):
        offset = 8 + 32 * len(elements) + len(names)
        entries.extend(words(0, offset, 0, system_value, 3, register))
        entries.extend(bytes([15, read_mask, 0, 0]) + words(0))
        names.extend(name.encode() + b"\0")
    return words(len(elements), 8) + entries + names


def shader(stage, instructions, inputs, outputs, root=None):
    code = words(16, 0, 0, 0, *instructions)
    fssm = bytearray(0xD0)
    fssm[:4] = b"FSSM"
    struct.pack_into("<QQ", fssm, 0xC0, len(fssm), len(code))
    xdat = b"XDAT" + words(1) + zlib.compress(fssm + code)
    body = align4(xdat) + b"NOBC"
    dxil = (words((stage << 16) | 0x60, 6 + len(body) // 4)
            + b"DXIL" + words(0x100, 16 + len(body) - 4, 4) + body)

    chunks = [(b"ISG1", inputs), (b"OSG1", outputs), (b"DXIL", dxil)]
    if root is not None:
        chunks.append((b"RTS0", root))
    start = 32 + 4 * len(chunks)
    offsets = []
    payload = bytearray()
    for tag, data in chunks:
        offsets.append(start + len(payload))
        payload.extend(tag + words(len(data)) + align4(data))
    return b"DXBC" + bytes(16) + words(1, start + len(payload), len(chunks), *offsets) + payload


def emit(name, data):
    print(f"static constexpr unsigned char {name}[] = {{")
    for offset in range(0, len(data), 16):
        print("    " + ", ".join(f"0x{b:02x}" for b in data[offset:offset+16]) + ",")
    print("};")


if __name__ == "__main__":
    end = 0xBF810000  # s_endpgm
    emit("triangle_vs", shader(1, [
        0xF80008CF, 0x07060504,  # exp pos0 v4, v5, v6, v7 done
        end,
    ], signature(("POSITION", 0, 15)), signature(("SV_Position", 1, 0))))
    emit("triangle_ps", shader(0, [
        0x7E000280,              # v_mov_b32 v0, 0
        0x7E0202F2,              # v_mov_b32 v1, 1.0
        0xF800080F, 0x01000100,  # exp mrt0 v0, v1, v0, v1 done (green)
        end,
    ], signature(("SV_Position", 1, 0)), signature(("SV_Target", 64, 0))))

    # One compute-visible UAV table at u0; group ID X follows its pointer in s5.
    compute_root = words(1, 1, 24, 0, 0, 0, 0, 0, 36, 1, 44, 1, 1, 0, 0, 0)
    emit("fallback_cs", shader(5, [
        0xF4080302, 0xFA000000,         # s_load_dwordx4 s[12:15], s[4:5], 0
        0xBF8CC07F,                     # s_waitcnt lgkmcnt(0)
        0x7E020205,                     # v_mov_b32 v1, s5
        0x7E0402FF, 0x57AC71A9,         # v_mov_b32 v2, result
        0xE0702000, 0x80030201,         # buffer_store_dword v2, v1, s[12:15], 0 idxen
        end,
    ], signature(), signature(), compute_root))

    # Pass the second vertex attribute (v8..v11) to the pixel stage as TEXCOORD0.
    emit("board_vs", shader(1, [
        0xF80000CF, 0x07060504,  # exp pos0 v4, v5, v6, v7
        0xF8000A0F, 0x0B0A0908,  # exp param0 v8, v9, v10, v11 done
        end,
    ], signature(("POSITION", 0, 15), ("TEXCOORD", 0, 15)),
       signature(("SV_Position", 1, 0), ("TEXCOORD", 0, 0))))
    # Fixed tilted view and perspective projection of camera-relative world positions.
    # gfx1030 encodings assembled with llvm-mc. UVs pass through to the atlas sampler.
    emit("scene_vs", shader(1, [
        0x10000AFF, 0x3F19999A,               # v0 = 0.6 * height
        0xD54B0001, 0x0401FF06, 0xBF4CCCCD,   # v1 = v0 - 0.8 * z
        0x10000AFF, 0xBF4CCCCD,               # v0 = -0.8 * height
        0xD54B0002, 0x0401FF06, 0xBF19999A,   # v2 = v0 - 0.6 * z
        0x060604FF, 0x40C00000,               # w = 6 + v2
        0x100808FF, 0x40100000,               # clip x = 2.25 * x
        0x100A02FF, 0x40400000,               # clip y = 3 * v1
        0x060C06F1,                           # clip z = w - 0.5
        0xF80000CF, 0x03060504,               # exp pos0 v4, v5, v6, v3
        0xF8000A0F, 0x0B0A0908,               # exp param0 v8..v11 done
        end,
    ], signature(("POSITION", 0, 15), ("TEXCOORD", 0, 15)),
       signature(("SV_Position", 1, 0), ("TEXCOORD", 0, 0))))
    # RTS0 v1.0: two pixel-visible descriptor tables (t0 in s8, s0 in s9).
    root = (words(1, 2, 24, 0, 0, 1) + words(0, 5, 48, 0, 5, 56)
            + words(1, 64, 1, 84) + words(0, 1, 0, 0, 0, 3, 1, 0, 0, 0))
    # Assemble a cube address from an interpolated XYZ direction in v4:v6.
    # Encodings from llvm-mc -triple=amdgcn-amd-amdhsa -mcpu=gfx1030.
    cube_address = [
        0xD545000C, 0x041A0B04,  # v_cubesc_f32 v12, v4, v5, v6
        0xD546000D, 0x041A0B04,  # v_cubetc_f32 v13, v4, v5, v6
        0xD544000E, 0x041A0B04,  # v_cubeid_f32 v14, v4, v5, v6
        0xD547000F, 0x041A0B04,  # v_cubema_f32 v15, v4, v5, v6
        0xD5AA010F, 0x0201010F,  # v_rcp_f32 v15, |v15|
        0xD54B0004, 0x03FE1F0C, 0x3FC00000,  # v_fma_f32 v4, v12, v15, 1.5
        0xD54B0005, 0x03FE1F0D, 0x3FC00000,  # v_fma_f32 v5, v13, v15, 1.5
        0x7E0C030E,              # v_mov_b32 v6, v14
    ]
    for name, dim, channels, address in (("board_ps", 1, 2, []), ("array_ps", 5, 3, []),
                                         ("cube_ps", 3, 3, cube_address), ("scene_ps", 1, 3, [])):
        interp = []
        for channel in range(channels):
            dest = (channel + 4) << 18
            interp.extend([0xC8000000 | dest | channel << 8,       # v_interp_p1_f32 (i)
                           0xC8010001 | dest | channel << 8])      # v_interp_p2_f32 (j)
        # The scene carries face brightness in TEXCOORD.z (v6). Shade RGB and preserve alpha.
        lighting = [0x10101106, 0x10121306, 0x10141506] if name == "scene_ps" else []
        emit(name, shader(0, [
            *interp,
            *address,
            0xBE8C0309,                # s_mov_b32 s12, s9 (sampler table pointer)
            0xF40C0004, 0xFA000000,    # s_load_dwordx8 s[0:7], s[8:9], 0
            0xF4080406, 0xFA000000,    # s_load_dwordx4 s[16:19], s[12:13], 0
            0xBF8C007F,                # s_waitcnt lgkmcnt(0)
            0xF0800F00 | dim << 3, 0x00800804,    # image_sample v[8:11], vaddr=v4, s[0:7], s[16:19]
            0xBF8C0F70,                # s_waitcnt vmcnt(0)
            *lighting,
            0xF800080F, 0x0B0A0908,    # exp mrt0 v8, v9, v10, v11 done
            end,
        ], signature(("SV_Position", 1, 0), ("TEXCOORD", 0, (1 << channels) - 1)),
           signature(("SV_Target", 64, 0)), root))
