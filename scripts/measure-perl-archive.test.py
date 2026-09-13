"""Archive/parser and disposable-directory controls; no Windows timing claims."""
import hashlib
import importlib.util
import io
from pathlib import Path, PureWindowsPath
import tarfile
import tempfile
import unittest

spec = importlib.util.spec_from_file_location("perl_archive", Path(__file__).with_name("measure-perl-archive.py"))
module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)
SOURCE = Path(__file__).read_bytes()
EXPECTED = [{"path": "control.py", "bytes": len(SOURCE), "sha256": hashlib.sha256(SOURCE).hexdigest()}]


def archive(name="C:/Strawberry/control.py", content=SOURCE, link=False):
    stream = io.BytesIO()
    with tarfile.open(fileobj=stream, mode="w") as tar:
        member = tarfile.TarInfo(name)
        member.size = len(content)
        if link:
            member.type, member.linkname, member.size = tarfile.SYMTYPE, "outside", 0
        tar.addfile(member, None if link else io.BytesIO(content))
    stream.seek(0)
    return stream


class ArchiveTests(unittest.TestCase):
    def test_windows_manifest_has_no_translated_line_ending(self):
        with tempfile.TemporaryDirectory() as temp:
            path = Path(temp) / "manifest.txt"
            module.write_archive_manifest(path, PureWindowsPath("C:/Strawberry"))
            self.assertEqual(path.read_bytes(), b"C:/Strawberry")

    def test_parses_actual_source_bytes(self):
        self.assertEqual(module.verify_tar(archive(), EXPECTED, "C:/Strawberry")["verified_files"], 1)

    def test_rejects_wrong_hash_and_omitted_source(self):
        for rows in ([{**EXPECTED[0], "sha256": "0" * 64}], EXPECTED + [{**EXPECTED[0], "path": "missing"}]):
            with self.assertRaises(ValueError):
                module.verify_tar(archive(), rows, "C:/Strawberry")

    def test_rejects_escape_and_link(self):
        for stream in (archive("C:/outside/control.py"), archive("C:/Strawberry/../control.py"), archive(link=True)):
            with self.assertRaises(ValueError):
                module.verify_tar(stream, EXPECTED, "C:/Strawberry")

    def test_rejects_nonzero_padding_inside_and_beyond_reader_buffer(self):
        raw = archive().getvalue()
        first_end = 512 + ((len(SOURCE) + 511) // 512) * 512
        for offset in (first_end + 512, len(raw) + 512):
            altered = bytearray(raw)
            if len(altered) < offset + 17:
                altered.extend(bytes(offset + 17 - len(altered)))
            altered[offset:offset + 17] = b"nonzero-padding!!"
            with self.assertRaises(ValueError):
                module.verify_tar(io.BytesIO(altered), EXPECTED, "C:/Strawberry")

    def test_rejects_missing_end_blocks(self):
        raw = archive().getvalue()
        first_end = 512 + ((len(SOURCE) + 511) // 512) * 512
        for length in (first_end, first_end + 512):
            with self.assertRaises(ValueError):
                module.verify_tar(io.BytesIO(raw[:length]), EXPECTED, "C:/Strawberry")

    def test_body_failure_cleans_partial_directories_and_restores_original(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp) / "distribution"
            root.mkdir()
            (root / "control").write_bytes(SOURCE)
            with self.assertRaisesRegex(RuntimeError, "control failure"):
                with module.parked_installation(root):
                    with module.extraction_destination(root):
                        (root / "empty/child").mkdir(parents=True)
                        raise RuntimeError("control failure")
            self.assertEqual((root / "control").read_bytes(), SOURCE)
            self.assertEqual(list(Path(temp).glob("bridge-perl-archive-*")), [])

    def test_replaced_extraction_directory_is_not_deleted(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp) / "extracted"
            moved = Path(temp) / "moved"
            with self.assertRaisesRegex(ValueError, "destination was replaced"):
                with module.extraction_destination(root):
                    (root / "control").write_bytes(SOURCE)
                    root.rename(moved)
                    root.mkdir()
                    (root / "replacement").write_bytes(b"unexpected")
            self.assertEqual((root / "replacement").read_bytes(), b"unexpected")
            self.assertEqual((moved / "control").read_bytes(), SOURCE)

    def test_collision_never_overwrites_replacement_or_original(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp) / "distribution"
            root.mkdir()
            (root / "control").write_bytes(SOURCE)
            with self.assertRaisesRegex(ValueError, "Refusing to overwrite"):
                with module.parked_installation(root):
                    root.mkdir()
                    (root / "replacement").write_bytes(b"unexpected")
            self.assertEqual((root / "replacement").read_bytes(), b"unexpected")
            backups = list(Path(temp).glob("bridge-perl-archive-*/original/control"))
            self.assertEqual(len(backups), 1)
            self.assertEqual(backups[0].read_bytes(), SOURCE)


if __name__ == "__main__":
    unittest.main()
