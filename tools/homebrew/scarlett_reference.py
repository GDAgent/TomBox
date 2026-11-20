"""Build the layout-261 GPU test input from two pinned DirectXTex test images."""
import argparse
import hashlib
from pathlib import Path
from urllib.request import urlopen

BASE = "https://raw.githubusercontent.com/walbourn/directxtextest/b185b8757b86b09257ab4f7701ba921248c8ba6e/"


def fetch(path, sha256):
    with urlopen(BASE + path, timeout=20) as response:
        data = response.read()
    if hashlib.sha256(data).hexdigest() != sha256:
        raise ValueError(f"unexpected SHA-256 for {path}")
    return data


def linear_mips(rgba, side):
    # Accumulate unrounded channel sums. Each output is the box average of
    # the original pixels, so rounding at one mip cannot affect the next.
    sums = list(rgba)
    area = 1
    yield rgba
    while side > 1:
        reduced = []
        for y in range(0, side, 2):
            for x in range(0, side, 2):
                i = (y * side + x) * 4
                j = i + side * 4
                reduced.extend(sums[i+c] + sums[i+4+c] + sums[j+c] + sums[j+4+c] for c in range(4))
        sums = reduced
        side //= 2
        area *= 4
        yield bytes((value + area // 2) // area for value in sums)


def write_bc1(output):
    tiled = fetch("xbox/Media/scarlettwindowslogoBC1.dds",
                  "e935009a1b67d00adee7e20b958ea33cacc86090b5719fbd9f96592129d45b1f")[164:]
    linear = bytearray()
    # Gather rows for this pinned surface. Each 32x16 tile holds 512 BC1 blocks.
    # Verify the result against texconv's independent compression of the linear
    # image before handing it to the ordinary PC texture upload.
    for mip, base in enumerate((12288, 4096, 2048, 1536, 1280, 1024, 768, 512, 256)):
        side = max(1, 64 >> mip)
        for y in range(side):
            for x in range(side):
                tile = (y // 16) * ((side + 31) // 32) + x // 32
                bits = (x & 1, y & 1, (y >> 1) & 1, (x >> 1) & 1, (x >> 2) & 1,
                        (y >> 2) & 1, (x >> 3) & 1, (y >> 3) & 1, (x >> 4) & 1)
                block = sum(bit << pos for pos, bit in enumerate(bits))
                start = base + tile * 4096 + block * 8
                linear.extend(tiled[start:start + 8])
    if hashlib.sha256(linear).hexdigest() != "06ea20308112037035c4381e247f55887bb2de20d40fda688400f16023596958":
        raise ValueError("BC1 blocks disagree with the linear texconv reference")
    output.write_bytes(tiled + linear)


def main(output, mips):
    tiled = fetch("xbox/Media/scarlettwindowslogo.dds",
                  "61010af98479660d765ae2159349ee26f5d2dc613c9a6549fef7d83d7c17f195")
    linear = fetch("media/windowslogo_X8R8G8B8.dds",
                   "2af48ed449019363e3ef1a77d8fb6fc882f8d0aaf7420eaa83148b3e0d690dc8")
    # These offsets describe only the two hash-pinned files. The Xbox DDS
    # has a 164-byte header and its full-resolution mip follows the other mips.
    top_mip = tiled[164 + 90112:]
    bgrx = linear[128:]
    if len(top_mip) != 256 * 256 * 4 or len(bgrx) != len(top_mip):
        raise ValueError("unexpected mip size")
    rgba = bytearray(len(bgrx))
    rgba[0::4], rgba[1::4], rgba[2::4] = bgrx[2::4], bgrx[1::4], bgrx[0::4]
    rgba[3::4] = bytes([255]) * (256 * 256)
    if mips:
        output.write_bytes(tiled[164:] + b"".join(linear_mips(rgba, 256)))
    else:
        output.write_bytes(top_mip + rgba)


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("output", type=Path)
    modes = parser.add_mutually_exclusive_group()
    modes.add_argument("--bc1", action="store_true", help="prepare the Scarlett and linear BC1 mip chains")
    modes.add_argument("--mips", action="store_true", help="include the full tiled chain and all linear mip references")
    args = parser.parse_args()
    if args.bc1:
        write_bc1(args.output)
    else:
        main(args.output, args.mips)
