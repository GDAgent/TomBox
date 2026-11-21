"""Regression fixtures for table boundaries which cannot be inferred from physical size."""
import io
import tempfile
import unittest
import zlib
from pathlib import Path

from package_assets import BOUNDARY, DESCRIPTOR, GIB4, HEADER, audit_assets, inspect_store


def store(virtual_end=GIB4 + 4096):
    # Two spans, 256 bytes each. The final virtual offset needs more than 32 bits.
    data = bytearray(768)
    HEADER.pack_into(data, 0, b"RDS5", 3, virtual_end & 0xffffffff, 0)
    BOUNDARY.pack_into(data, 16, 1, 0)
    BOUNDARY.pack_into(data, 24, 2 | (1 << 30) | (7 << 26), 0)
    BOUNDARY.pack_into(data, 32, 3 | ((virtual_end // 4096 % 4) << 30), virtual_end // 16384)
    HEADER.pack_into(data, 256, b"ETPF", 0, 1, 0)
    DESCRIPTOR.pack_into(data, 272, 0, 98 << 3, 2, 0)
    return data


class PackageAssets(unittest.TestCase):
    def test_64bit_virtual_extent_and_leading_codec(self):
        data = store()
        size, codecs, formats, sampled = inspect_store(io.BytesIO(data), len(data))
        self.assertEqual((size, codecs, formats, sampled), (GIB4 + 4096, {0: 1, 7: 1}, {98: 1}, False))

    def test_descriptor_table_must_fit_first_chunk(self):
        data = store()
        HEADER.pack_into(data, 256, b"ETPF", 0, 11, 0)
        with self.assertRaisesRegex(ValueError, "table extent"):
            inspect_store(io.BytesIO(data), len(data))

    def test_zlib_requires_end_of_stream_and_declared_length(self):
        data = store(8192)
        DESCRIPTOR.pack_into(data, 272, 0, 98 << 3, 1, 0)
        BOUNDARY.pack_into(data, 24, 2 | (1 << 30) | (8 << 26), 0)
        packed = zlib.compress(bytes(4096))
        data[512:512+len(packed)] = packed
        self.assertTrue(inspect_store(io.BytesIO(data), len(data))[3])
        data[512:512+len(packed)] = bytes(len(packed))
        with self.assertRaises(zlib.error):
            inspect_store(io.BytesIO(data), len(data))

    def test_unknown_archive_does_not_hide_valid_files(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            (root / "bad.rpf").write_bytes(b"abcd")
            (root / "ok.ttpf").write_bytes(store())
            result = audit_assets(root, sorted(root.iterdir()))
            self.assertEqual(result["verified"], {"ttpf": 1})
            self.assertIn("bad.rpf", result["inspection_errors"])


if __name__ == "__main__":
    unittest.main()
