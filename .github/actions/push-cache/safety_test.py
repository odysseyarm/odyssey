import gzip
import io
from pathlib import Path
import subprocess
import tarfile
import tempfile
import unittest
import zipfile

import safety


class SafetyTest(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.private = self.root / "private"
        crate = self.private / "crates/odyssey-cv"
        (crate / "src").mkdir(parents=True)
        (crate / "Cargo.toml").write_text('[package]\nname = "odyssey-cv"\n')
        self.source = b"fn private_tracker() { let secret = 73; }\n"
        (crate / "src/lib.rs").write_bytes(self.source)
        self.fingerprints = safety.private_sources([self.private], [self.private])

    def scan(self, data):
        payload = self.root / "unrelated-name.bin"
        payload.write_bytes(data)
        return safety.scan_file(payload, self.fingerprints)

    def test_renamed_private_source_and_eskf_rejected(self):
        self.assertTrue(self.scan(self.source))
        eskf = self.private / "third_party/eskf/src/lib.rs"
        eskf.parent.mkdir(parents=True)
        eskf.write_bytes(b"fn private_filter() {}\n")
        fingerprints = safety.private_sources([], [self.private])
        self.assertTrue(safety.hash_stream(io.BytesIO(eskf.read_bytes()), fingerprints, ()))

    def test_gzip_and_zstd_private_source_rejected(self):
        self.assertTrue(self.scan(gzip.compress(self.source)))
        compressed = subprocess.check_output(["zstd", "-q", "-c"], input=self.source)
        self.assertTrue(self.scan(compressed))

    def test_nested_archives_and_traversal_name_rejected_without_extraction(self):
        inner = io.BytesIO()
        with zipfile.ZipFile(inner, "w") as archive:
            archive.writestr("../../outside/renamed.rs", self.source)
        outer = io.BytesIO()
        with tarfile.open(fileobj=outer, mode="w:gz") as archive:
            data = inner.getvalue()
            info = tarfile.TarInfo("nested.zip")
            info.size = len(data)
            archive.addfile(info, io.BytesIO(data))
        self.assertTrue(self.scan(outer.getvalue()))
        self.assertFalse((self.root.parent / "outside/renamed.rs").exists())

    def test_corrupt_archive_and_excessive_nesting_fail_closed(self):
        with self.assertRaises((EOFError, OSError)):
            self.scan(b"\x1f\x8btruncated")
        with self.assertRaises(ValueError):
            self.scan(b"7z\xbc\xaf\x27\x1cunsupported")
        data = b"ordinary payload"
        for _ in range(safety.MAX_ARCHIVE_DEPTH):
            data = gzip.compress(data)
        with self.assertRaises(ValueError):
            self.scan(data)

    def test_public_archive_fixtures_do_not_hide_later_private_members(self):
        payload = self.root / "fixture.zip"
        payload.write_bytes(b"PK\x03\x04deliberately invalid test data")
        with self.assertRaises(zipfile.BadZipFile):
            safety.scan_file(payload, self.fingerprints)
        self.assertFalse(safety.scan_file(payload, self.fingerprints, public_graph=True))
        archive_bytes = io.BytesIO()
        with zipfile.ZipFile(archive_bytes, "w") as archive:
            archive.writestr("bad-zip.fixture", payload.read_bytes())
            archive.writestr("private.rs", self.source)
        payload.write_bytes(archive_bytes.getvalue())
        self.assertTrue(safety.scan_file(payload, self.fingerprints, public_graph=True))

    def test_compiled_code_empty_marker_and_public_stub_allowed(self):
        self.assertFalse(self.scan(b"\x7fELF compiled private implementation"))
        self.assertFalse(self.scan(b""))
        stub = self.root / "stub"
        (stub / "src").mkdir(parents=True)
        (stub / "Cargo.toml").write_text('[package]\nname = "odyssey-cv"\n')
        (stub / "src/lib.rs").write_bytes(b"")
        self.assertEqual(safety.private_sources([stub], [stub]), set())

    def test_forbidden_source_reference_crossing_read_boundary_rejected(self):
        forbidden = b"/nix/store/private-source"
        payload = io.BytesIO(b"x" * (1024 * 1024 - 3) + forbidden + b"suffix")
        self.assertTrue(safety.hash_stream(payload, set(), (forbidden,)))


if __name__ == "__main__":
    unittest.main()
