"""Generate two original RDNA2 fixtures in the subset of DXBC TomBox reads.

The payload is synthetic: these are inputs to TomBox's extractor, not retail
compiler output. No Xbox SDK, game files or pretranslated HLSL are used.
"""
import struct
import zlib


def words(*values):
    return struct.pack(f"<{len(values)}I", *values)


def align4(data):
    return data + bytes(-len(data) % 4)


def signature(name, system_value, read_mask):
    # One ISG1/OSG1 entry, register 0, float4. The string follows the entry.
    return (words(1, 8, 0, 40, 0, system_value, 3, 0)
            + bytes([15, read_mask, 0, 0]) + words(0) + name.encode() + b"\0")


def shader(stage, instructions, inputs, outputs):
    code = words(16, 0, 0, 0, *instructions)
    fssm = bytearray(0xD0)
    fssm[:4] = b"FSSM"
    struct.pack_into("<QQ", fssm, 0xC0, len(fssm), len(code))
    xdat = b"XDAT" + words(1) + zlib.compress(fssm + code)
    body = align4(xdat) + b"NOBC"
    dxil = (words((stage << 16) | 0x60, 6 + len(body) // 4)
            + b"DXIL" + words(0x100, 16 + len(body) - 4, 4) + body)

    chunks = [(b"ISG1", inputs), (b"OSG1", outputs), (b"DXIL", dxil)]
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
    ], signature("POSITION", 0, 15), signature("SV_Position", 1, 0)))
    emit("triangle_ps", shader(0, [
        0x7E000280,              # v_mov_b32 v0, 0
        0x7E0202F2,              # v_mov_b32 v1, 1.0
        0xF800080F, 0x01000100,  # exp mrt0 v0, v1, v0, v1 done (green)
        end,
    ], signature("SV_Position", 1, 0), signature("SV_Target", 64, 0)))
