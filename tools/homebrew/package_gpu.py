"""GPU reference checks for specific, hash-pinned Scarlett 1.0.2.1 assets."""
import argparse
import csv
import hashlib
import math
import struct
import subprocess
import sys
import tempfile
import zlib
from pathlib import Path

from package_assets import BOUNDARY, DESCRIPTOR, HEADER, boundary, read_exact, records
from ps09_cache import open_cache
from ps09_compute import probe_compute
from ps09_depth import probe_depth, probe_facing
from ps09_mrt import probe_mrt
from ps09_raster import probe_raster
from ps09_brdf import grid_reference as brdf_grid, validate_grid as validate_brdf
from triangle_shaders import shader, signature

TEXTURE_STORE = "data/ttdb/graphics/ttdb_graphics.ttpf"
# Row-major RGBA references produced independently with 6Stars deswizzle_64kb_ttpf.
TEXTURES = {
    234: ("e2df212446b0b0d2bc8d233547a476e2a231bd8293affa1883243988274330cb",
          "9b8ff88ee37b24e7a2bf10da2f4243992bea141080e7dd7a7cbce1ca45a691a2"),
    765: ("9d91dc2d4acf2c38e3e636a3bc9edb9a88a208aaec9694ff63c17e5cd0be4eb5",
          "a67315f69b8517dfca328eaf6185c4f8f557417c8f774cc063dda7d1e3a21ee6"),
}
GRAPHICS_PSO = 10682
PIXEL_SHADER = 4553
SHADER_HASH = "f58c513842d162762d134c920c7b61f6927ad4f43a171ecd2855d221df67ff5e"


def texture_payload(path, index):
    # These two descriptors each occupy one complete zlib chunk.
    with path.open("rb") as f:
        magic, count, _, _ = HEADER.unpack(read_exact(f, 16))
        if magic != b"RDS5" or not 2 <= count <= (path.stat().st_size - 16) // 8:
            raise ValueError("invalid RDS5 boundary table")
        first = boundary(*BOUNDARY.unpack(read_exact(f, 8)))
        f.seek(first[0])
        magic, _, textures, _ = HEADER.unpack(read_exact(f, 16))
        if magic != b"ETPF" or index >= textures:
            raise ValueError("missing ETPF texture descriptor")
        f.seek(first[0] + 16 + 16 * index)
        dims, flags, page, _ = DESCRIPTOR.unpack(read_exact(f, 16))
        if dims != (255 | (255 << 14)) or (flags >> 3) & 127 != 28 or (flags >> 24) & 15:
            raise ValueError("expected single-mip 256x256 R8G8B8A8_UNORM with tile mode 0")
        start = (page & 0xfffff) * 4096
        f.seek(24)
        physical, virtual, codec = first
        for row in records(f, BOUNDARY, count - 1):
            end, vend, next_codec = boundary(*row)
            if virtual == start and vend - start == 262144 and codec == 8:
                if not 0 < end - physical <= 262144 + 1024:
                    raise ValueError("invalid compressed texture length")
                f.seek(physical)
                stream = zlib.decompressobj()
                tiled = stream.decompress(read_exact(f, end - physical), 262145)
                if stream.eof and len(tiled) == 262144:
                    return tiled
                raise ValueError("invalid texture zlib payload")
            physical, virtual, codec = end, vend, next_codec
    raise ValueError(f"texture descriptor {index} no longer occupies one zlib chunk")


def cache_pixel_shader(path, pso, index, digest, attributes=(), special_envelope=None):
    with open_cache(path) as cache:
        if not 0 <= pso < cache.graphics:
            raise ValueError(f"missing graphics PSO {pso}")
        entry = 128 + pso * 112
        if struct.unpack_from("<I", cache.data, entry + 56)[0] != index:
            raise ValueError("graphics PSO no longer refers to the expected shader")
        if special_envelope is not None:
            config = cache.metadata(struct.unpack_from("<I", cache.data, entry + 8)[0])
            bindings = cache.metadata(struct.unpack_from("<I", cache.data, entry + 60)[0])
        record = cache.code(index)
    if hashlib.sha256(record).hexdigest() != digest:
        raise ValueError("retail shader record differs from the reference")
    if len(record) < 40 or len(record) % 4 or record[24:36] != bytes.fromhex("000080d50001fe03c00a0000"):
        raise ValueError("unrecognized PS09 graphics program entry")
    # Preserve the complete instruction stream and padding after the pinned identity.
    words = struct.unpack_from(f"<{(len(record) - 36) // 4}I", record, 36)
    if special_envelope is not None:
        return special_envelope(words, config, bindings)
    return shader(0, words, signature(("SV_Position", 1, 0), *attributes), signature(("SV_Target", 64, 0)))


GRID_CASES = (
    (24690, 20532, "19a352802a268dcedf61158c22b80094d6ab26fb163a27b8df21e80e5e386e21"),
    (8964, 3482, "5953486e3b0561b3cf3ae52ba54c74b9dbdb66606ef474d882d47687f9b4487c"),
    (11910, 4915, "4c54f91763d23b6a3aeb42b3e65d171861b3aec56eee0becb48be831478f8ab8"),
    (11902, 4904, "157f93374d9ef001e1d91ea9dd61fea24a495024af132c9bec5b87952d4dfe2c"),
    (11903, 4906, "ede19603b1c9e19d4eba5abaabe0d95cd9af03ba79f001c1dbdd0088e092b296"),
    (11907, 4910, "b5f98fcd3007f937a40a36616577db1d65824b0eee1f5ada110e5ae4a29c66a5"),
    (11906, 4909, "5787fd89815a0fb84fdde7cc95c3b5b0239cc3c081481131c20e399ad7bbb10f"),
)


def saturate(x):
    return max(0, min(1, x))


def stripe_alpha(x, y):
    # Coverage of two softened edges. Width and opacity are vertex attributes.
    u, v = x/16, y/16
    z = (0.25, 0.5, 0.75, 1.25)[(x+y) % 4]
    opacity = 1 + x % 5
    attrs = (u, v, z, 0.125, 0.125, 1.25, 1.5, opacity)
    dx = 2 * abs(u - 0.125) / 1.5
    dy = 2 * abs(v - 0.125) / 1.25
    width = 0.6 * saturate(1.25 * z)
    # The final opacity is clamped to zero once z reaches 0.9.
    alpha = 0
    if z < 0.9:
        inner = saturate((dy - 1 + width) / width)
        outer = 1 - saturate((dy - 0.8*z) / (1 - 0.8*z))
        alpha = (0.5 * inner * outer * (1 - saturate((dx - 0.75)*4))
                 * saturate((0.9-z)*10) * saturate((opacity - 0.5)*2/9))
    half, = struct.unpack("<e", struct.pack("<e", alpha))
    return attrs, bytes((255, 0, 0, round(255 * half)))


def grid_reference(index):
    if index in (4904, 4906, 4909, 4910):
        return brdf_grid(index)
    clear = bytes((255, 0, 255, 255))
    inputs, rgba = bytearray(), bytearray()
    discarded = 0
    for y in range(16):
        for x in range(16):
            if index == 20532:
                u, v, intensity = x / 8, y / 8, (1 + (x + y) % 4) / 4
                attrs = (u, v, intensity, 0, 0, 0, 0, 0)
                keep = max(u, v) <= 1
                radial = (1 - math.hypot(2*u - 1, 2*v - 1)) * intensity
                half, = struct.unpack("<e", struct.pack("<e", radial))
                color = bytes((round(255 * max(0, min(1, half))), 0, 0, 0))
            elif index == 3482:
                u, v = (x + 0.5) / 16, (y + 0.5) / 16
                palette = (0, 0.125, 0.25, 0.75, 1)
                tint = (palette[x % 5], palette[y % 5], palette[(x+y) % 5], 1)
                attrs = (*tint, u, v, 0, 0)
                keep = 0.45 < math.hypot(u - 0.5, v - 0.5) < 0.5
                color = bytes(round(255 * value) for value in tint)
            elif index == 4915:
                attrs, color = stripe_alpha(x, y)
                keep = True
            else:
                raise ValueError(f"no pixel reference for shader {index}")
            inputs.extend(struct.pack("<8f", *attrs))
            rgba.extend(color if keep else clear)
            discarded += not keep
    return inputs, rgba, discarded


def probe_cache_grid(archive, scratch, run):
    results = []
    for pso, index, digest in GRID_CASES:
        blob, attrs, output = (scratch / f"grid-{index}.{ext}" for ext in ("dxbc", "bin", "rgba"))
        blob.write_bytes(cache_pixel_shader(archive, pso, index, digest, (("COLOR", 0, 15), ("TEXCOORD", 0, 15))))
        inputs, expected, discarded = grid_reference(index)
        attrs.write_bytes(inputs)
        run("clear_pixel.exe", "--cache-grid", blob, attrs, output)
        actual = output.read_bytes()
        precision = {}
        if index in (4904, 4906, 4909, 4910):
            precision["max_error_unorm8_steps"] = round(validate_brdf(index, actual), 4)
        elif actual != expected:
            first = next((i // 4 for i, (a, b) in enumerate(zip(actual, expected)) if a != b),
                         min(len(actual), len(expected)) // 4)
            raise ValueError(f"shader {index}, pixel {first}: got {actual[first*4:first*4+4].hex()}, "
                             f"expected {expected[first*4:first*4+4].hex()}")
        results.append({"graphics_pso": pso, "shader_index": index, "record_sha256": digest,
                        "matched_pixels": 256, "discarded_pixels": discarded, **precision})
    return results


def attach_compare_evidence(repo, archive, checked):
    # Attribute comparisons from the decoded retail code after each full
    # readback succeeds. Verify every referenced shader got a census row.
    subprocess.run(["cargo", "build", "--release", "--locked", "-p", "tuxbox-xshader",
                    "--example", "opstat"], cwd=repo, check=True, stdout=sys.stderr)
    by_index = {row["shader_index"]: row for row in checked}
    archive.stream.seek(0)
    result = subprocess.run([repo / "target/release/examples/opstat", "--ps09-vopc",
                             *map(str, by_index)], stdin=archive.stream, check=True,
                            capture_output=True, text=True)
    visited = set()
    for match in csv.DictReader(result.stdout.splitlines(), delimiter="\t"):
        index = int(match["shader_index"])
        visited.add(index)
        names = by_index[index].setdefault("vopc", [])
        if match["mnemonic"] != "-":
            names.append(match["mnemonic"])
    if by_index.keys() != visited:
        raise ValueError("incomplete PS09 readback instruction inventory")


def probe_package_gpu(repo, root):
    subprocess.run([repo / "tools/homebrew/build-clear.sh"], check=True, stdout=sys.stderr)
    build = repo / "target/x86_64-pc-windows-msvc/release"
    with tempfile.TemporaryDirectory(dir=build, prefix="package-gpu-") as tmp:
        scratch = Path(tmp)

        def run(exe, option, *paths):
            args = [p.relative_to(build).as_posix() for p in paths]
            subprocess.run([repo / "tuxbox", "run", build / exe, option, *args],
                           check=True, stdout=sys.stderr, timeout=120)

        textures = []
        for index, (source_hash, expected) in TEXTURES.items():
            tiled = texture_payload(root / TEXTURE_STORE, index)
            if hashlib.sha256(tiled).hexdigest() != source_hash:
                raise ValueError(f"retail texture {index} differs from the reference")
            src, pixels = scratch / "tiled.bin", scratch / f"{index}.rgba"
            src.write_bytes(tiled)
            run("coin_window.exe", "--ttpf-texture", src, pixels)
            if pixels.stat().st_size != 262144 or hashlib.sha256(pixels.read_bytes()).hexdigest() != expected:
                raise ValueError(f"GPU texture {index} differs from reference pixels")
            textures.append({"descriptor": index, "pixels": 65536, "rgba_sha256": expected})

        with open_cache(root / "shaders.rpf") as cache:
            blob, result = scratch / "cache-pixel.dxbc", scratch / "coverage.txt"
            blob.write_bytes(cache_pixel_shader(cache, GRAPHICS_PSO, PIXEL_SHADER, SHADER_HASH))
            run("clear_pixel.exe", "--cache-pixel", blob, result)
            if result.read_text().strip() != "256":
                raise ValueError("cache pixel shader readback did not confirm full triangle and background")
            grids = probe_cache_grid(cache, scratch, run)
            grids.append(probe_mrt(cache_pixel_shader, cache, scratch, run))
            grids.append(probe_depth(cache_pixel_shader, cache, scratch, run))
            grids.append(probe_facing(cache_pixel_shader, cache, scratch, run))
            compute = probe_compute(repo, cache, scratch)
            compute.extend(probe_raster(cache, scratch, run))
            triangle = {"graphics_pso": GRAPHICS_PSO, "shader_index": PIXEL_SHADER, "record_sha256": SHADER_HASH,
                        "matched_pixels": 256}
            attach_compare_evidence(repo, cache, [triangle, *grids, *compute])
    return {"ps09_cache_sha256": cache.sha256, "texture_store": TEXTURE_STORE, "textures": textures, "pixel_shader": triangle,
            "pixel_compare_shaders": grids, "compute_shaders": compute}


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description="Inventory named VOPC candidates in a retail PS09 cache")
    parser.add_argument("archive", type=Path, help="path to shaders.rpf")
    args = parser.parse_args()
    repo = Path(__file__).resolve().parents[2]
    try:
        subprocess.run(["cargo", "build", "--release", "--locked", "-p", "tuxbox-xshader",
                        "--example", "opstat"], cwd=repo, check=True)
        exe = repo / "target/release/examples/opstat"
        # Finish validating all compressed members before publishing any candidates.
        with open_cache(args.archive) as cache:
            cache.stream.seek(0)
            census = subprocess.run([exe, "--ps09-vopc"], stdin=cache.stream, check=True,
                                    stdout=subprocess.PIPE, text=True)
            print(f"# ps09_sha256 {cache.sha256}")
            sys.stdout.write(census.stdout)
    except (OSError, ValueError, zlib.error, subprocess.SubprocessError) as error:
        parser.exit(1, f"PS09 inventory failed: {error}\n")
