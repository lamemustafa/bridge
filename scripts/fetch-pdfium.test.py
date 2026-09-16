# SPDX-License-Identifier: Apache-2.0
"""fetch-pdfium.py refuses every byte it was not pinned to, and writes the
notice it was pinned to. Offline: archives are built here and passed with
--archive, and the lock is a temporary copy."""
import hashlib
import importlib.util
import io
import json
from pathlib import Path
import tarfile
import tempfile
import unittest

spec = importlib.util.spec_from_file_location("fetch_pdfium", Path(__file__).with_name("fetch-pdfium.py"))
fetch = importlib.util.module_from_spec(spec)
spec.loader.exec_module(fetch)


def archive_bytes(members):
    buffer = io.BytesIO()
    with tarfile.open(fileobj=buffer, mode="w:gz") as bundle:
        for name, data in members.items():
            info = tarfile.TarInfo(name)
            info.size = len(data)
            bundle.addfile(info, io.BytesIO(data))
    return buffer.getvalue()


class FetchPdfiumTests(unittest.TestCase):
    def setUp(self):
        self.directory = tempfile.TemporaryDirectory()
        self.root = Path(self.directory.name)
        self.members = {
            "LICENSE": b"top licence\r\n",
            "licenses/b.txt": b"b licence\n",
            "licenses/a.txt": b"a licence\n",
            "lib/libpdfium.dylib": b"library bytes",
            "include/fpdfview.h": b"not shipped",
        }
        self.archive = archive_bytes(self.members)
        (self.root / "pdfium.tgz").write_bytes(self.archive)
        lock = {"source": "https://example.invalid/pdfium", "release": "chromium/1",
                "platforms": {"test": {
                    "asset": "pdfium.tgz", "bytes": len(self.archive),
                    "sha256": hashlib.sha256(self.archive).hexdigest(),
                    "library": "lib/libpdfium.dylib"}}}
        pin = lock["platforms"]["test"]
        library = self.members["lib/libpdfium.dylib"]
        notice = fetch.notice_bytes(lock, pin, self.members)
        pin.update(library_bytes=len(library), library_sha256=hashlib.sha256(library).hexdigest(),
                   notice_bytes=len(notice), notice_sha256=hashlib.sha256(notice).hexdigest())
        self.lock = lock
        self.write_lock()
        self.original_lock = fetch.LOCK
        fetch.LOCK = self.root / "lock.json"

    def tearDown(self):
        fetch.LOCK = self.original_lock
        self.directory.cleanup()

    def write_lock(self):
        (self.root / "lock.json").write_text(json.dumps(self.lock))

    def run_fetch(self, archive=None):
        path = self.root / "given.tgz"
        path.write_bytes(archive if archive is not None else self.archive)
        return fetch.main(["--platform", "test", "--dest", str(self.root / "out"), "--archive", str(path)])

    def test_pinned_archive_writes_library_and_byte_exact_notice(self):
        self.assertEqual(self.run_fetch(), 0)
        out = self.root / "out"
        self.assertEqual(sorted(p.name for p in out.iterdir()), ["THIRD_PARTY_LICENSES_PDFIUM.txt", "libpdfium.dylib"])
        self.assertEqual((out / "libpdfium.dylib").read_bytes(), b"library bytes")
        notice = (out / "THIRD_PARTY_LICENSES_PDFIUM.txt").read_bytes()
        # licence bytes are reproduced unmodified (the CRLF survives), LICENSE first, then by name
        self.assertIn(b"===== LICENSE =====\ntop licence\r\n", notice)
        self.assertLess(notice.index(b"licenses/a.txt"), notice.index(b"licenses/b.txt"))
        self.assertNotIn(b"not shipped", notice)

    def test_an_unpinned_archive_is_refused_before_extraction(self):
        tampered = archive_bytes(dict(self.members, **{"lib/libpdfium.dylib": b"other library"}))
        with self.assertRaisesRegex(SystemExit, "refusing"):
            self.run_fetch(tampered)
        self.assertFalse((self.root / "out").exists())

    def test_a_library_that_differs_from_its_pin_is_refused(self):
        self.lock["platforms"]["test"]["library_sha256"] = "0" * 64
        self.write_lock()
        with self.assertRaisesRegex(SystemExit, "lib/libpdfium.dylib"):
            self.run_fetch()
        self.assertFalse((self.root / "out").exists())

    def test_a_notice_that_differs_from_its_pin_is_refused(self):
        self.lock["platforms"]["test"]["notice_sha256"] = "0" * 64
        self.write_lock()
        with self.assertRaisesRegex(SystemExit, "THIRD_PARTY_LICENSES_PDFIUM"):
            self.run_fetch()
        self.assertFalse((self.root / "out").exists())

    def test_the_committed_lock_pins_every_shipped_platform(self):
        lock = json.loads(self.original_lock.read_text(encoding="utf-8"))
        for platform in ("macos-arm64", "windows-x64"):
            pin = lock["platforms"][platform]
            for key in ("sha256", "library_sha256", "notice_sha256"):
                self.assertRegex(pin[key], "^[0-9a-f]{64}$", (platform, key))


if __name__ == "__main__":
    unittest.main()
