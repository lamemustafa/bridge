import hashlib
import importlib.util
from pathlib import Path
import tempfile
import unittest

spec = importlib.util.spec_from_file_location("inventory_windows_perl", Path(__file__).with_name("inventory-windows-perl.py"))
module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)


class InventoryTests(unittest.TestCase):
    def test_reads_actual_source_bytes_and_relative_paths(self):
        source = Path(__file__).read_bytes()
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / "perl/bin").mkdir(parents=True)
            (root / "perl/bin/source.txt").write_bytes(source)
            result = module.inventory(root)
        self.assertEqual(result["total_files"], 1)
        self.assertEqual(result["total_bytes"], len(source))
        self.assertEqual(result["files"], [{"path": "perl/bin/source.txt", "bytes": len(source),
                                           "sha256": hashlib.sha256(source).hexdigest()}])

    def test_missing_or_empty_root_cannot_report_zero_files(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            with self.assertRaises(FileNotFoundError):
                module.inventory(root / "missing")
            with self.assertRaises(ValueError):
                module.inventory(root)

    def test_symlink_outside_distribution_is_rejected(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / "linked").symlink_to(Path(__file__).resolve())
            with self.assertRaises(ValueError):
                module.inventory(root)


if __name__ == "__main__":
    unittest.main()
