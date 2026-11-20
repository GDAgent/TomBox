"""Generate offline layout-261 textures and their linear GPU readback references."""
import argparse
import hashlib
import struct
from pathlib import Path

# Size, layout size, mip origins and SHA-256 of ALL element addresses (little-
# endian uint32, mip then row then column). Captured from AMD AddrLib GFX10
# Addr2ComputeSurfaceInfo / Addr2ComputeSurfaceAddrFromCoord, SW_4K_S, one slice.
CASES = {
    "rgba-wide": (256, 64, 98304, (32768, 16384, 8192, 4096, 2048, 1536, 1280, 1024, 768),
                  "b69032e6cae61a11fbacc601a98672c7acb5aa7fd0fbba87bf6462746286b794"),
    "rgba-tall": (64, 256, 94208, (28672, 12288, 4096, 2048, 1536, 1280, 1024, 768, 512),
                  "a7ce41ae1245b7d8316dcc0a7788382d0ea2c85088fe38bd572981837f46e883"),
    "bc1-wide": (256, 64, 16384, (8192, 4096, 2048, 1536, 1280, 1024, 768, 512, 256),
                 "39baf31969820984d2257563f3ec9526ae1eada5c8cf39779aa32b023a3cd52f"),
    "bc1-tall": (128, 256, 28672, (12288, 4096, 2048, 1536, 1280, 1024, 768, 512, 256),
                 "21e901a8eaa2c2c0dd0d001be0867d99e6c55a1511cabed64199d8f261b6daa7"),
}

# AddrLib address streams for three slices, in slice, mip, row, column order.
ARRAY_DIGESTS = {
    "rgba-tall": "bd8b4b5c32e512f4791bb5d37d2ab42e30f72742c3885a74929af228861ede5a",
    "bc1-wide": "2fed589b8197d0245539a84ed4f3efcb7ac223f03f4ff3eb96fcf19d34d833d7",
}

# Six square faces. AddrLib address order is face, mip, row, column.
CUBE_CASE = (64, 64, 24576, (8192, 4096, 2048, 1536, 1280, 1024, 768),
             "0761883bcad75def980122cabb3e6ab5a02a013a7ab1da18852f7293cfdedd5f")

# The 128x128 mip fits one 64 KiB page. Four more pages hold the 256x256 mip.
STREAM_CASE = (256, 256, 327680, (65536, 0),
               "7fae20312204cd4f3b93c78cc333bace0e8833074ef3a0db523d332e151b0984")


def block_offset(x, y, width, bc1):
    height = 16 if bc1 else 32
    tile = (y // height) * ((width + 31) // 32) + x // 32
    # Address bits within a 4 KiB tile, low to high, after the byte-in-element bits.
    order = "xyyxxyxyx" if bc1 else "xxyyyxyxyx"
    coords = {"x": x, "y": y}
    index = 0
    for bit, axis in enumerate(order):
        index |= (coords[axis] & 1) << bit
        coords[axis] >>= 1
    return tile * 4096 + index * (8 if bc1 else 4)


def endpoints(code):
    rgb = tuple(255 if code & bit else 0 for bit in (4, 2, 1))
    return (rgb[0] >> 3) << 11 | (rgb[1] >> 2) << 5 | rgb[2] >> 3, bytes((*rgb, 255))


def write_case(name, output, case, slices=1):
    width, height, stride, origins, digest = case
    bc1 = name.startswith("bc1")
    element = 4 if bc1 else 1
    tiled = bytearray([0xcd]) * (stride * slices)  # Keep padding visibly different from the pattern.
    linear = bytearray()
    addresses = hashlib.sha256()
    for sub in range(slices * len(origins)):
        layer, mip = divmod(sub, len(origins))
        origin = layer * stride + origins[mip]
        w, h = max(1, width >> mip), max(1, height >> mip)
        pixels = bytearray(w * h * 4)
        cols = (w + element - 1) // element
        for y in range((h + element - 1) // element):
            for x in range(cols):
                addr = origin + block_offset(x, y, cols, bc1)
                addresses.update(struct.pack("<I", addr))
                if bc1:
                    code = (x + 3*y + mip + 2*layer) & 7
                    a, color_a = endpoints(code)
                    b, color_b = endpoints(code ^ 7)
                    selectors = 0
                    for v in range(4):
                        for u in range(4):
                            selected = (u + v//2 + x + mip) & 1
                            selectors |= selected << (2 * (4*v + u))
                            px, py = 4*x + u, 4*y + v
                            if px < w and py < h:
                                i = 4 * (py*w + px)
                                pixels[i:i+4] = color_b if selected else color_a
                    tiled[addr:addr+8] = struct.pack("<HHI", a, b, selectors)
                else:
                    color = bytes((x, y, (17*mip + (x ^ y) + 73*layer) & 255, 255))
                    tiled[addr:addr+4] = color
                    i = 4 * (y*w + x)
                    pixels[i:i+4] = color
        linear.extend(pixels)
    if addresses.hexdigest() != digest:
        raise ValueError(f"{name}: element addresses disagree with AMD AddrLib")
    output.write_bytes(tiled + linear)


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("directory", type=Path)
    modes = parser.add_mutually_exclusive_group()
    modes.add_argument("--arrays", action="store_true")
    modes.add_argument("--cube", action="store_true")
    modes.add_argument("--stream", action="store_true")
    args = parser.parse_args()
    if args.stream:
        write_case("rgba-stream", args.directory / "stream.bin", STREAM_CASE)
    elif args.cube:
        write_case("rgba-cube", args.directory / "cube.bin", CUBE_CASE, 6)
    elif args.arrays:
        for name, digest in ARRAY_DIGESTS.items():
            write_case(name, args.directory / f"{name}.bin", (*CASES[name][:4], digest), 3)
    else:
        for name, case in CASES.items():
            write_case(name, args.directory / f"{name}.bin", case)
