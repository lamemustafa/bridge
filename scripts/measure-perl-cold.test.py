"""Filesystem failure controls; these do not qualify a reduced Perl runtime."""
import importlib.util
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

spec = importlib.util.spec_from_file_location("measure_perl_cold", Path(__file__).with_name("measure-perl-cold.py"))
module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)


class DistributionTests(unittest.TestCase):
    def setUp(self):
        self.directory = tempfile.TemporaryDirectory()
        self.addCleanup(self.directory.cleanup)
        self.root = Path(self.directory.name) / "distribution"
        for prefix in module.PREFIXES:
            directory = self.root / prefix
            directory.mkdir(parents=True)
            (directory / "control").write_bytes(Path(__file__).read_bytes())
        self.original = module.inventory.inventory(self.root)["files"]

    def assert_restored(self):
        self.assertEqual(module.inventory.inventory(self.root)["files"], self.original)
        self.assertEqual(list(self.root.parent.glob("bridge-perl-cold-*")), [])

    def test_full_control_is_unchanged(self):
        with module.distribution_variant(self.root, False):
            self.assert_restored()
        self.assert_restored()

    def test_reduction_restores_after_body_failure(self):
        with self.assertRaisesRegex(RuntimeError, "control failure"):
            with module.distribution_variant(self.root, True):
                self.assertTrue(all(not (self.root / p).exists() for p in module.PREFIXES))
                raise RuntimeError("control failure")
        self.assert_restored()

    def test_partial_move_failure_restores_prior_moves(self):
        rename = Path.rename
        def fail_second(path, target):
            if path == self.root / module.PREFIXES[1]:
                raise OSError("control move failure")
            return rename(path, target)
        with patch.object(Path, "rename", fail_second):
            with self.assertRaisesRegex(OSError, "control move failure"):
                with module.distribution_variant(self.root, True):
                    self.fail("Partial reduction must not reach the build")
        self.assert_restored()

    def test_collision_preserves_bytes_and_restores_other_directories(self):
        collision = self.root / module.PREFIXES[-1]
        with self.assertRaises(ExceptionGroup) as raised:
            with module.distribution_variant(self.root, True):
                collision.write_bytes(b"unexpected replacement")
        self.assertEqual(len(raised.exception.exceptions), 1)
        self.assertIsInstance(raised.exception.exceptions[0], ValueError)
        self.assertEqual(collision.read_bytes(), b"unexpected replacement")
        for prefix in module.PREFIXES[:-1]:
            self.assertEqual((self.root / prefix / "control").read_bytes(), Path(__file__).read_bytes())
        backups = list(self.root.parent.glob("bridge-perl-cold-*"))
        self.assertEqual(len(backups), 1)
        self.assertEqual((backups[0] / module.PREFIXES[-1] / "control").read_bytes(), Path(__file__).read_bytes())


if __name__ == "__main__":
    unittest.main()
