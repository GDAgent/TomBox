"""Generate original RDNA2 fixtures in the subset of DXBC TuxBox reads.

The payload is synthetic: these are inputs to TuxBox's extractor, not retail
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


def shader(stage, instructions, inputs, outputs, root=None, threads=(0, 0, 0), psv=None):
    # The compute preamble with explicit dimensions is followed by a driver v_nop marker.
    marker = (0xD5800000, 0x03FE0000, 0) if any(threads) else ()
    code = words(16, *threads, *marker, *instructions)
    fssm = bytearray(0xD0)
    fssm[:4] = b"FSSM"
    struct.pack_into("<QQ", fssm, 0xC0, len(fssm), len(code))
    xdat = b"XDAT" + words(1) + zlib.compress(fssm + code)
    body = align4(xdat) + b"NOBC"
    dxil = (words((stage << 16) | 0x60, 6 + len(body) // 4)
            + b"DXIL" + words(0x100, 16 + len(body) - 4, 4) + body)

    chunks = [(b"ISG1", inputs), (b"OSG1", outputs), (b"DXIL", dxil)]
    if psv is not None:
        chunks.append((b"PSV0", psv))
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


# One compute-visible UAV table at u0; group ID X follows its pointer in s5.
compute_root = words(1, 1, 24, 0, 0, 0, 0, 0, 36, 1, 44, 1, 1, 0, 0, 0)


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

    emit("fallback_cs", shader(5, [
        0xF4080302, 0xFA000000,         # s_load_dwordx4 s[12:15], s[4:5], 0
        0xBF8CC07F,                     # s_waitcnt lgkmcnt(0)
        0x7E020205,                     # v_mov_b32 v1, s5
        0x7E0402FF, 0x57AC71A9,         # v_mov_b32 v2, result
        0xE0702000, 0x80030201,         # buffer_store_dword v2, v1, s[12:15], 0 idxen
        end,
    ], signature(), signature(), compute_root))

    # Inline u0 (two address words) followed by four b0 constants. The older
    # executable preamble encodes 1,0,0 while PSVRuntimeInfo2 specifies 32 threads.
    packed_root = words(1, 2, 24, 0, 0, 0x03000000, 4, 0, 48, 1, 0, 56, 0, 0, 0, 0, 4)
    psv = words(48, 0, 0, 0, 0, 0, 0, 5, 0, 0, 32, 1, 1, 0)
    emit("packed_root_cs", shader(5, [
        0x8703FF01, 255,                # s3 = low 8 bits of the high address word
        0xBE820300,                     # s2 = low word
        0xD7460000, 0x04010A04,         # v0 = (group X << 5) + local X
        0xF4080201, 0xFA000008,         # s[8:11] = inline constants
        0xBF8CC07F, 0x7DA80008,         # enable lanes where s8 > v0
        0xF4040001, 0xFA000000,         # s[0:1] = UAV address
        0xBF8CC07F, 0xBE811D95,         # stride = 32 bytes
        0xBE8203C1, 0xBE8303FF, 0x11014FAC,
        0xE0302004, 0x80000300,         # v3 = row[v0].word1
        0xBF8C3F70, 0x4A060609,         # v3 += s9
        0xE0702010, 0x80000300,         # row[v0].word4 = v3
        end,
    ], signature(), signature(), packed_root, threads=(1, 0, 0), psv=psv))

    # Keep a value in every lane before EXEC masks the tail. FI=1 must read
    # those saved values even when the neighbor branches around the exchange.
    emit("packed_quad_cs", shader(5, [
        0xBE820300, 0x8703FF01, 255, 0xF4040301, 0xFA000000,
        0xBF8CC07F, 0xBE8D1D95, 0xBE8E03C1, 0xBE8F03FF, 0x11014FAC,
        0xD7460002, 0x04010A04,         # v2 = group X * 32 + local X
        0xE0302004, 0x80030302, 0xBF8C3F70,
        0x7E080303,                     # v4 = v3 = input
        0x7DA804FF, 71, 0xBF880008,     # cmpx 71 > v2; skip to end if empty
        0x060806FA, 0xFF0CB103,         # v4 = v3[lane xor 1] + v3
        0x060A08FA, 0xFF0C4E04,         # v5 = v4[lane xor 2] + v4
        0xE0702010, 0x80030402, 0xE0702014, 0x80030502, end,
    ], signature(), signature(), packed_root, psv=psv))

    ballot_psv = words(48, 0, 0, 0, 0, 0, 0, 5, 0, 0, 64, 1, 1, 0)
    # A wave can retire after writing LDS while another reaches S_BARRIER.
    # Read the retiring wave's data in the surviving lanes of each workgroup.
    emit("retiring_wave_cs", shader(5, [
        0xBE820300, 0x8703FF01, 255, 0xF4040301, 0xFA000000,
        0xBF8CC07F, 0xBE8D1D95, 0xBE8E03C1, 0xBE8F03FF, 0x11014FAC,
        0xD7460002, 0x04010C04,         # v2 = group X * 64 + local X
        0xE0302004, 0x80030302, 0xBF8C3F70,
        0x34080082, 0xD8340000, 0x00000304, 0xBF8CC07F,  # LDS[local X] = input
        0x7DA80094, 0xBF880008,         # keep first 20 lanes, branch to ENDPGM
        0xBF8A0000, 0x3A0A08FF, 128,    # barrier, toggle wave bit in byte address
        0xD8D80000, 0x06000005, 0xBF8CC07F,
        0xE0702010, 0x80030602, end,
    ], signature(), signature(), packed_root, threads=(64, 1, 1), psv=ballot_psv))

    emit("packed_ballot_cs", shader(5, [
        0xBE820300, 0x8703FF01, 255,    # copy the packed root pointer low word first
        0xF4040301, 0xFA000000,         # s[12:13] = UAV address
        0xBF8CC07F, 0xBE8D1D95,         # descriptor stride = 32
        0xBE8E03C1, 0xBE8F03FF, 0x11014FAC,
        0x360200BF,                     # v1 = local X & 63
        0xD7460002, 0x04010C04,         # v2 = (group X << 6) + local X
        0xE0302004, 0x80030302,         # v3 = row[v2].word1
        0xBF8C3F70, 0x7D8802B9,         # vcc = lane index < 57
        0xBE88246A,                     # save EXEC in s[8:9], mask out lanes 57..63
        0xD485000A, 0x00020680,         # ballot v3 != 0 into s[10:11]
        0x3A0A020A, 0x3A0C020B,         # XOR each ballot word with the lane index
        0xE0702010, 0x80030502,         # row[v2].word4 = low XOR
        0xE0702014, 0x80030602,         # row[v2].word5 = high XOR
        0xBEFE0408, end,                # restore EXEC
    ], signature(), signature(), packed_root, threads=(64, 1, 1), psv=ballot_psv))

    for width in (32, 64):
        log_width = width.bit_length() - 1
        scan = [
            0xBE820300, 0x8703FF01, 255,  # s[2:3] = root argument pointer
            0xF4040301, 0xFA000000, 0xBF8CC07F,
            0xBE8D1D95, 0xBE8E03C1, 0xBE8F03FF, 0x11014FAC,  # 32-byte row stride
            0x7D840080 | (width - 1),    # compare local X with the last lane
            0xD7460002, 0x04010004 | ((128 + log_width) << 9),
            0xE0302004, 0x80030302, 0xBF8C3F70,  # v3 = row[v2].word1
        ]
        for bit in range(5):
            distance = 1 << bit
            swizzle = (32 - 2 * distance) | ((distance - 1) << 5)
            # Gather the last prefix of the preceding block and add it in the upper block.
            mask = sum(1 << lane for lane in range(32) if lane & distance)
            scan.extend([0xBE8603FF, mask])
            if width == 64:
                scan.extend([0xBE8703FF, mask])
            scan.extend([0xD8D40000 | swizzle, 0x04000003, 0xBF8CC07F,
                         0xD5010004, 0x001A0880, 0x4A060903])
        if width == 64:
            scan.extend([0xBE860380, 0xBE8703C1, 0xD7600008, 0x00013F03,
                         0xD5010004, 0x00181080, 0x4A060903])
        scan.extend([0xE0702010, 0x80030302, end])  # row[v2].word4 = inclusive sum
        emit(f"packed_scan{width}_cs", shader(5, scan, signature(), signature(), packed_root,
             psv=words(48, 0, 0, 0, 0, 0, 0, 5, 0, 0, width, 1, 1, 0)))

    increment_prefix = [
        0xF4080302, 0xFA000000,         # s_load_dwordx4 s[12:15], s[4:5], 0
        0xBF8CC07F,                     # s_waitcnt lgkmcnt(0)
        0x7E020205,                     # v_mov_b32 v1, s5 (group ID)
    ]
    increment_body = [
        0xE0302000, 0x80030201,         # buffer_load_dword v2, v1, s[12:15], 0 idxen
        0xBF8C3F70,                     # s_waitcnt vmcnt(0)
        0x4A040481,                     # v_add_nc_u32 v2, 1, v2
        0xE0702000, 0x80030201,         # buffer_store_dword v2, v1, s[12:15], 0 idxen
        end,
    ]
    emit("increment_cs", shader(5, increment_prefix + increment_body, signature(), signature(), compute_root))
    thread_index = [
        0x34020282,  # v_lshlrev_b32 v1, 2, v1
        0x4a020300,  # v_add_nc_u32 v1, v0, v1 (4 * group ID + local thread ID)
    ]
    emit("increment4_cs", shader(5, increment_prefix + thread_index + increment_body,
                                 signature(), signature(), compute_root, threads=(4, 1, 1)))

    # Pass the second vertex attribute (v8..v11) to the pixel stage as TEXCOORD0.
    emit("board_vs", shader(1, [
        0xF80000CF, 0x07060504,  # exp pos0 v4, v5, v6, v7
        0xF8000A0F, 0x0B0A0908,  # exp param0 v8, v9, v10, v11 done
        end,
    ], signature(("POSITION", 0, 15), ("TEXCOORD", 0, 15)),
       signature(("SV_Position", 1, 0), ("TEXCOORD", 0, 0))))
    # Geometry probe: emit a different triangle from the (offscreen) input.
    # Each 48-byte output record holds position, UV and a flat per-vertex tag.
    geometry = [0x7E100280]  # v8 = LDS output base 0
    for vertex, (x, y, u, v, tag) in enumerate([(-1, 1, 0, 0, .25), (3, 1, 2, 0, .5), (-1, -3, 0, 2, .75)]):
        for component, value in enumerate([x, y, 0, 1, u, v, (0, 1, .5)[vertex], 0, tag, 0, 0, 0]):
            literal, = struct.unpack("<I", struct.pack("<f", value))
            geometry.extend([0x7E1202FF, literal,  # v9 = data
                             0xD8340000 | (48 * vertex + 4 * component), 0x0908])  # ds_write_b32
        geometry.append(0xBF900022 if vertex < 2 else 0xBF900032)  # emit; final emit and cut
    geometry.extend([0xBF900003, end])
    # PSVRuntimeInfo1: triangle input, triangle-strip output, max 3 vertices.
    psv = words(28, 3, 5, 1, 0, 0, 0, (3 << 16) | 2, 0)
    gs_out = signature(("SV_Position", 1, 0), ("TEXCOORD", 0, 0), ("COLOR", 0, 0))
    emit("shader_probe_gs", shader(2, geometry, signature(("SV_Position", 1, 15)), gs_out, psv=psv))
    emit("shader_probe_ps", shader(0, [
        0xC8100000, 0xC8110001,  # interpolate u into v4
        0xC8140100, 0xC8150101,  # interpolate v into v5
        0xC81A0402,              # v_interp_mov_f32 v6, p0, attr1.x
        0x10100B04,              # v8 = u*v, nonlinear across a pixel quad
        0xD8D4801B, 0x08000008,  # ds_swizzle_b32 v8, v8: diagonally opposite pixel
        # Use an intermediate P1 value and custom I/J weights in alpha.
        0x7E1202FF, 0x3E800000, 0x7E1402F0,  # v9=.25, v10=.5
        0xC81C0209, 0x060E0EFF, 0x3E000000, 0xC81D020A,
        0xF800080F, 0x07060804,  # export and wait for the source VGPRs to be consumed
        0xBCFD0000, 0x7E0802FF, 0xBF9F0000, end,  # overwrite v4 with a padding-shaped literal
        0xBF9F0000, 0xBF9F0000,  # s_code_end is padding after termination
    ], signature(("SV_Position", 1, 0), ("TEXCOORD", 0, 7), ("COLOR", 0, 1)), signature(("SV_Target", 64, 0))))
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
