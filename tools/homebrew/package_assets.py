"""Read-only checks for the RPF9 and RDS5/ETPF variants in the Scarlett audit."""
import mmap
import re
import struct
import subprocess
import sys
import tempfile
import zlib
from collections import Counter
from pathlib import Path

HEADER = struct.Struct("<4sIII")
BOUNDARY = struct.Struct("<II")
DESCRIPTOR = struct.Struct("<IIII")
GIB4 = 1 << 32


def read_exact(source, size):
    data = source.read(size)
    if len(data) != size:
        raise ValueError("truncated record")
    return data


def records(source, layout, count):
    while count:
        batch = min(count, 4096)
        yield from layout.iter_unpack(read_exact(source, batch * layout.size))
        count -= batch


def boundary(lo, hi):
    physical = (lo & 0x03ffffff) * 256
    virtual = ((hi & 0xfffff) * 4 + (lo >> 30)) * 4096
    return physical, virtual, (lo >> 26) & 15


def inspect_store(source, size):
    magic, count, short_size, _ = HEADER.unpack(read_exact(source, HEADER.size))
    table_end = HEADER.size + count * BOUNDARY.size
    if magic != b"RDS5" or count < 2 or table_end > size:
        raise ValueError("invalid RDS5 boundary table")
    rows = records(source, BOUNDARY, count)
    physical, virtual, codec = boundary(*next(rows))
    start = physical
    if not table_end <= start < size or virtual or codec:
        raise ValueError("expected a raw first span starting at virtual offset zero")

    codecs = Counter()
    first_span = None
    zlib_span = None
    for row in rows:
        end, vend, next_codec = boundary(*row)
        if not physical < end <= size or vend <= virtual:
            raise ValueError("invalid physical or virtual chunk extent")
        span = end - physical, vend - virtual
        if first_span is None:
            first_span = min(span)
        # Bound the optional sample in both address spaces.
        if codec == 8 and zlib_span is None and max(span) <= 1024 * 1024:
            zlib_span = physical, *span
        codecs[codec] += 1
        physical, virtual, codec = end, vend, next_codec
    if physical != size or (virtual & 0xffffffff) != short_size:
        raise ValueError("final boundary disagrees with the store size")

    source.seek(start)
    magic, _, textures, _ = HEADER.unpack(read_exact(source, HEADER.size))
    if magic != b"ETPF" or HEADER.size + textures * (DESCRIPTOR.size + 8) > first_span:
        raise ValueError("invalid ETPF descriptor or hash table extent")
    formats = Counter()
    for _, flags, page, _ in records(source, DESCRIPTOR, textures):
        if (page & 0xfffff) * 4096 >= virtual:
            raise ValueError("texture allocation starts outside the store")
        formats[(flags >> 3) & 0x7f] += 1

    if zlib_span:
        pos, compressed_size, expected_size = zlib_span
        source.seek(pos)
        stream = zlib.decompressobj()
        decoded = stream.decompress(read_exact(source, compressed_size), expected_size + 1)
        if not stream.eof or len(decoded) != expected_size:
            raise ValueError("zlib sample disagrees with its chunk extent")
    return virtual, codecs, formats, zlib_span is not None


def locate_shader_cache(data):
    # Validate the checksum of the complete first cache block.
    for match in re.finditer(rb"\x78[\x01\x5e\x9c\xda]", data):
        pos = match.start()
        try:
            prefix = zlib.decompressobj().decompress(data[pos:pos + 128], 4)
            if prefix != b"PS\x00\x09":
                continue
            block, _ = cache_block(data, pos)
        except (ValueError, zlib.error):
            continue
        if len(block) == 65536:
            return pos, block
    return None


def cache_block(data, pos):
    compressed = data[pos:pos + 65536 + 128]
    stream = zlib.decompressobj()
    block = stream.decompress(compressed, 65537)
    if not stream.eof or not 0 < len(block) <= 65536:
        raise ValueError("incomplete or oversized PS09 cache block")
    used = len(compressed) - len(stream.unused_data)
    return block, (pos + used + 15) & ~15


def shader_header(source):
    with mmap.mmap(source.fileno(), 0, access=mmap.ACCESS_READ) as data:
        located = locate_shader_cache(data)
        if located:
            return dict(zip(("graphics_psos", "compute_psos", "metadata_records", "shader_binaries"),
                            struct.unpack_from("<IIII", located[1], 0x50)))


def audit_assets(root, paths):
    sizes = Counter()
    counts = Counter()
    recognized = Counter()
    errors = {}
    formats = Counter()
    codecs = Counter()
    headers = []
    large_stores = samples = 0
    for path in paths:
        kind = path.suffix.lower()[1:]
        size = path.stat().st_size
        sizes[kind] += size
        counts[kind] += 1
        try:
            with path.open("rb") as source:
                if kind == "rpf":
                    if read_exact(source, 4) != b"9FPR":
                        raise ValueError("expected RPF9 signature")
                    if path.name.casefold() == "shaders.rpf":
                        header = shader_header(source)
                        if header is not None:
                            headers.append(header)
                else:
                    virtual, tags, ids, sampled = inspect_store(source, size)
                    formats.update(ids)
                    codecs.update(tags)
                    large_stores += virtual > GIB4
                    samples += sampled
            recognized[kind] += 1
        except (ValueError, OSError, zlib.error) as error:
            errors[path.relative_to(root).as_posix()] = str(error)
    return {"files": dict(counts), "physical_bytes": dict(sizes), "verified": dict(recognized),
            "inspection_errors": errors, "ttpf_virtual_over_4gib": large_stores,
            "chunk_codec_counts": dict(sorted(codecs.items())), "texture_format_counts": dict(sorted(formats.items())),
            "stores_with_zlib_sample": samples, "ps09_headers": headers}


def probe_reads(repo, root, paths):
    def proton_path(path):
        return "Z:" + str(path.resolve()).replace("/", "\\")

    subprocess.run([repo / "tools/homebrew/build-clear.sh", "--package-read-test"], check=True,
                   stdout=sys.stderr)
    exe = repo / "target/x86_64-pc-windows-msvc/release/package_read_probe.exe"
    files, ranges, crossings = Counter(), Counter(), Counter()
    with tempfile.TemporaryDirectory() as tmp:
        manifest, result = Path(tmp) / "ranges.tsv", Path(tmp) / "passed.txt"
        with manifest.open("w", encoding="utf-8", newline="\n") as out:
            for path in paths:
                kind = path.suffix.lower()[1:]
                size = path.stat().st_size
                length = min(size, 64)
                offsets = {0, size - length}
                if size >= GIB4 + 32:
                    offsets.add(GIB4 - 32)
                    crossings[kind] += 1
                relative = path.relative_to(root).as_posix()
                if any(c in relative for c in "\t\r\n"):
                    raise ValueError(f"path cannot be represented in range manifest: {relative!r}")
                with path.open("rb") as source:
                    for offset in sorted(offsets):
                        source.seek(offset)
                        expected = read_exact(source, length).hex()
                        out.write(f"{size}\t{offset}\t{expected}\t{relative}\n")
                        ranges[kind] += 1
                files[kind] += 1
        subprocess.run([repo / "tuxbox", "run", exe, proton_path(root), proton_path(manifest),
                        proton_path(result)], check=True, stdout=sys.stderr, timeout=180)
        if not result.is_file() or result.read_text().strip() != str(sum(ranges.values())):
            raise ValueError("Proton did not confirm all requested archive reads")
    return {"files": dict(files), "ranges": dict(ranges), "crossings_4gib": dict(crossings)}
