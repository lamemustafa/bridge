import hashlib
import importlib.util
import json
import os
from pathlib import Path
import sys
import tempfile
import unittest
from unittest.mock import patch


def module(name, filename):
    spec = importlib.util.spec_from_file_location(name, Path(__file__).with_name(filename))
    result = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(result)
    return result


retain = module("retain", "retain-macos-test-binaries.py")
capture = module("capture", "collect-macos-test-crashes.py")
# Adversarial identity metadata and opaque executable bytes, not live evidence.
UUID = "00000000-0000-0000-0000-000000000001"
EXPECTED = (UUID, "arm64")


class BinaryEvidenceTests(unittest.TestCase):
    def setUp(self):
        self.directory = tempfile.TemporaryDirectory()
        self.addCleanup(self.directory.cleanup)
        self.root = Path(self.directory.name)
        self.target = self.root / "target"
        self.target.mkdir()
        self.output = self.root / "output"
        self.output.mkdir()
        self.binary = self.target / "bridge_lib-cafe"
        self.binary.write_bytes(b"synthetic executable bytes")
        self.report = {"process": self.binary.name, "process_images": [
            {"uuid": UUID, "arch": "arm64", "image_index": 0}]}

    def collect(self, reports=None):
        return retain.collect({"status": "captured", "reports": reports or [self.report]},
                              self.target, self.output)

    def test_exact_identity_copy_digest_and_deduplication(self):
        with patch.object(retain, "binary_identities", return_value=[EXPECTED]):
            result = self.collect([self.report, self.report])
        self.assertEqual(result["status"], "linked")
        self.assertEqual(len(result["binaries"]), 1)
        binary = result["binaries"][0]
        self.assertEqual(binary["sha256"], hashlib.sha256(self.binary.read_bytes()).hexdigest())
        self.assertEqual((self.output / binary["file"]).read_bytes(), self.binary.read_bytes())
        self.assertNotIn(str(self.root), json.dumps(result))
        self.assertIn("debug_0", result["debug_info"])

    def test_mismatch_or_tool_error_never_leaves_a_qualified_copy(self):
        cases = [([], "image_identity_mismatch"),
                 ([EXPECTED, EXPECTED], "image_identity_mismatch"),
                 (retain.EvidenceError("image_tool_failed"), "image_tool_failed")]
        for value, error in cases:
            with self.subTest(error=error), patch.object(retain, "binary_identities") as identities:
                if isinstance(value, Exception):
                    identities.side_effect = value
                else:
                    identities.return_value = value
                result = self.collect()
            self.assertEqual(result["errors"], [error])
            self.assertEqual(list(self.output.iterdir()), [])

    def test_staged_identity_is_verified(self):
        with patch.object(retain, "binary_identities", side_effect=[[EXPECTED], []]):
            result = self.collect()
        self.assertEqual(result["errors"], ["staged_image_identity_mismatch"])
        self.assertEqual(list(self.output.iterdir()), [])

    def test_symlink_missing_and_byte_limit_are_errors(self):
        self.binary.unlink()
        self.binary.symlink_to(self.root / "missing")
        self.assertEqual(self.collect()["errors"], ["binary_read_or_copy_failed"])
        self.binary.unlink()
        self.assertEqual(self.collect()["errors"], ["binary_read_or_copy_failed"])
        self.binary.write_bytes(b"too large")
        with patch.object(retain, "MAX_BYTES", 1):
            self.assertEqual(self.collect()["errors"], ["binary_byte_limit"])

    @unittest.skipUnless(hasattr(os, "mkfifo"), "macOS/POSIX file boundary")
    def test_fifo_is_rejected_without_waiting_for_a_writer(self):
        self.binary.unlink()
        os.mkfifo(self.binary)
        self.assertEqual(self.collect()["errors"], ["not_regular_file"])

    def test_tool_stdout_and_stderr_are_bounded_while_reading(self):
        for descriptor in [1, 2]:
            with self.subTest(descriptor=descriptor), self.assertRaisesRegex(
                    retain.EvidenceError, "^image_tool_output_limit$"):
                retain.bounded_tool([sys.executable, "-c", f"import os; os.write({descriptor}, b'x'*1000000)"])
        with self.assertRaisesRegex(retain.EvidenceError, "^image_tool_failed$"):
            retain.bounded_tool([sys.executable, "-c", "raise SystemExit(7)"])
        with self.assertRaisesRegex(retain.EvidenceError, "^image_tool_timeout$"):
            retain.bounded_tool([sys.executable, "-c", "import time; time.sleep(10)"], timeout=0.05)

    def test_ambiguous_missing_or_unsafe_report_identity_is_refused(self):
        for mutation, error in [
            ({"process_images": []}, "process_image_ambiguous_or_missing"),
            ({"process_images": self.report["process_images"] * 2}, "process_image_ambiguous_or_missing"),
            ({"process": "../bridge_lib-cafe"}, "invalid_process_name"),
            ({"process_images": [{"uuid": UUID, "arch": "private/path"}]}, "process_image_identity_unavailable"),
        ]:
            with self.subTest(error=error):
                self.assertEqual(self.collect([{**self.report, **mutation}])["errors"], [error])

    def test_no_report_is_unavailable_and_truncation_is_incomplete(self):
        empty = retain.collect({"status": "no_fresh_reports", "reports": []}, self.target, self.output)
        self.assertEqual((empty["status"], empty["errors"]), ("unavailable", []))
        with patch.object(retain, "binary_identities", return_value=[EXPECTED]):
            bounded = self.collect([self.report] * 9)
        self.assertEqual(bounded["status"], "partial")
        self.assertTrue(bounded["reports_truncated"])

    def test_captured_format_maps_frame_identity_and_rejects_boolean_index(self):
        fixture = Path(__file__).parent / "testdata/macos-sigtrap-control.ips"
        metadata, body = fixture.read_text().split("\n", 1)
        report = json.loads(body)
        report["usedImages"][0].update(uuid=UUID.upper(), arch="arm64")
        report["threads"][0]["frames"][0]["imageIndex"] = True
        result = capture.minimize(metadata + "\n" + json.dumps(report))
        self.assertEqual(result["process_images"][0]["uuid"], UUID)
        self.assertIsNone(result["threads"][0]["frames"][0]["image_index"])
        self.assertEqual(result["threads"][0]["frames"][3]["uuid"], UUID)
        self.assertEqual(result["threads"][0]["frames"][3]["image_index"], 0)
        self.assertEqual(capture.image_identity({"uuid": "private/path", "arch": []}),
                         {"uuid": None, "arch": None})


if __name__ == "__main__":
    unittest.main()
