"""Read the indexed PS09 cache once and bounds-check its directories."""
import hashlib
import mmap
import struct
import tempfile
from contextlib import contextmanager

from package_assets import cache_block, locate_shader_cache


def shader_cache_blocks(path):
    with path.open("rb") as f, mmap.mmap(f.fileno(), 0, access=mmap.ACCESS_READ) as data:
        located = locate_shader_cache(data)
        if not located:
            raise ValueError("missing checksummed PS09 header")
        pos, header = located
        graphics, compute, meta, shaders, meta_bytes, code_bytes = struct.unpack_from("<6I", header, 0x50)
        total = 128 + 112 * graphics + 24 * compute + 8 * (meta + shaders) + meta_bytes + code_bytes
        if total > 512 * 1024**2:
            raise ValueError("PS09 cache exceeds the probe size limit")
        for offset in range(0, total, 65536):
            block, pos = cache_block(data, pos)
            if len(block) != min(65536, total - offset):
                raise ValueError("unexpected PS09 cache block length")
            yield block


class PS09Cache:
    def __init__(self, data, stream=None, sha256=None):
        self.data = data
        self.stream = stream
        self.sha256 = sha256
        if len(data) < 128 or data[:4] != b"PS\0\x09":
            raise ValueError("invalid PS09 header")
        (self.graphics, self.compute, self.metas, self.shaders,
         self.meta_bytes, self.code_bytes) = struct.unpack_from("<6I", data, 0x50)
        self.compute_table = 128 + self.graphics * 112
        self.meta_table = self.compute_table + self.compute * 24
        self.shader_table = self.meta_table + self.metas * 8
        self.meta_base = self.shader_table + self.shaders * 8
        self.code_base = self.meta_base + self.meta_bytes
        if self.code_base + self.code_bytes != len(data):
            raise ValueError("PS09 cache length differs from declared section sizes")

    def _entry(self, index, table, count, base, extent):
        if not 0 <= index < count:
            raise ValueError(f"PS09 index {index} exceeds directory size {count}")
        offset, size = struct.unpack_from("<II", self.data, table + 8 * index)
        if offset + size > extent:
            raise ValueError(f"PS09 record {index} exceeds its section")
        return self.data[base + offset:base + offset + size]

    def metadata(self, index):
        return self._entry(index, self.meta_table, self.metas, self.meta_base, self.meta_bytes)

    def code(self, index):
        return self._entry(index, self.shader_table, self.shaders, self.code_base, self.code_bytes)


@contextmanager
def open_cache(source):
    if isinstance(source, PS09Cache):
        yield source
        return
    with tempfile.TemporaryFile() as store:
        digest = hashlib.sha256()
        for block in shader_cache_blocks(source):
            digest.update(block)
            store.write(block)
        store.flush()
        with mmap.mmap(store.fileno(), 0, access=mmap.ACCESS_READ) as data:
            yield PS09Cache(data, store, digest.hexdigest())
