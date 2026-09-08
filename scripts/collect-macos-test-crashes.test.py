import copy
import importlib.util
import json
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

spec = importlib.util.spec_from_file_location("capture", Path(__file__).with_name("collect-macos-test-crashes.py"))
capture = importlib.util.module_from_spec(spec)
spec.loader.exec_module(capture)
FIXTURE = Path(__file__).parent / "testdata/macos-sigtrap-control.ips"


class CaptureTests(unittest.TestCase):
    def report(self, mutate=None):
        metadata, body = FIXTURE.read_text().split("\n", 1)
        report = json.loads(body)
        if mutate:
            mutate(report)
        return metadata + "\n" + json.dumps(report)

    def test_captured_control_retains_signal_and_stack(self):
        result = capture.minimize(self.report())
        self.assertEqual(result["exception"]["signal"], "SIGTRAP")
        self.assertEqual([frame["symbol"] for frame in result["threads"][0]["frames"]],
                         ["__pthread_kill", "pthread_kill", "raise", "main", "start"])

    def test_omits_raw_process_data_and_paths_with_spaces(self):
        for path in ("/Users/Synthetic User/private source.rs", r"C:\Users\Synthetic User\private source.rs"):
            with self.subTest(path=path):
                def mutate(report):
                    report["procPath"] = path
                    report["environment"] = {"TOKEN": "must-not-escape"}
                    report["exception"]["message"] = "must-not-escape"
                    thread = report["threads"][0]
                    thread["threadState"] = {"register": "must-not-escape"}
                    thread["frames"][0]["symbol"] = "worker_exit " + path
                result = capture.minimize(self.report(mutate))
                encoded = json.dumps(result)
                self.assertNotIn("must-not-escape", encoded)
                self.assertNotIn("Synthetic", encoded)
                self.assertNotIn("private source.rs", encoded)
                self.assertIsNone(result["threads"][0]["frames"][0]["symbol"])
                self.assertEqual(result["threads"][0]["frames"][0]["image"], "libsystem_kernel.dylib")

    def test_distinguishes_missing_from_invalid_fresh_report(self):
        with tempfile.TemporaryDirectory() as root:
            directory = Path(root)
            self.assertEqual(capture.collect([directory], 0)["status"], "no_fresh_reports")
            (directory / "bridge_lib-test.ips").write_text("invalid")
            self.assertEqual(capture.collect([directory], 0)["status"], "capture_error")

    def test_waits_for_partial_report_to_finish(self):
        with tempfile.TemporaryDirectory() as root:
            directory = Path(root)
            path = directory / "bridge_lib-cafe.ips"
            path.write_text(self.report()[:60])
            with patch.object(capture.time, "monotonic", side_effect=[0, 0, 2]), \
                    patch.object(capture.time, "sleep", side_effect=lambda _: path.write_text(self.report())):
                result = capture.wait_for_reports([directory], 0, 2)
            self.assertEqual(result["status"], "captured")
            self.assertEqual(len(result["reports"]), 1)

    def test_waits_for_later_reports_without_duplicating_earlier_report(self):
        with tempfile.TemporaryDirectory() as root:
            directory = Path(root)
            (directory / "bridge_lib-cafe-first.ips").write_text(self.report())
            later = directory / "bridge_lib-cafe-second.ips"
            with patch.object(capture.time, "monotonic", side_effect=[0, 0, 1, 2]), \
                    patch.object(capture.time, "sleep", side_effect=lambda _: later.write_text(self.report())) as sleep:
                result = capture.wait_for_reports([directory], 0, 2)
            self.assertEqual(result["status"], "captured")
            self.assertEqual(len(result["reports"]), 2)
            self.assertEqual(sleep.call_count, 2)

    def test_preserves_parse_error_at_deadline(self):
        with tempfile.TemporaryDirectory() as root:
            directory = Path(root)
            (directory / "bridge_lib-cafe.ips").write_text("invalid")
            with patch.object(capture.time, "monotonic", side_effect=[0, 0, 2]), \
                    patch.object(capture.time, "sleep") as sleep:
                result = capture.wait_for_reports([directory], 0, 2)
            self.assertEqual(result["status"], "capture_error")
            self.assertEqual(result["errors"], ["report_read_or_parse_failed"])
            sleep.assert_called_once_with(1)

    def test_excludes_old_reports_and_bounds_frames(self):
        with tempfile.TemporaryDirectory() as root:
            directory = Path(root)
            path = directory / "bridge_lib-cafe.ips"
            path.write_text(self.report())
            self.assertEqual(capture.collect([directory], path.stat().st_mtime + 1)["status"], "no_fresh_reports")
        def mutate(report):
            report["threads"][0]["frames"] *= 70
        result = capture.minimize(self.report(mutate))
        self.assertTrue(result["threads"][0]["frames_truncated"])
        self.assertEqual(len(result["threads"][0]["frames"]), 64)

    def test_retains_late_faulting_and_triggered_threads(self):
        def mutate(report):
            thread = report["threads"][0]
            report["threads"] = [copy.deepcopy(thread) for _ in range(70)]
            for thread in report["threads"]:
                thread["triggered"] = False
            report["faultingThread"] = 69
            report["threads"][68]["triggered"] = True
        result = capture.minimize(self.report(mutate))
        self.assertTrue(result["threads_truncated"])
        self.assertEqual(len(result["threads"]), 64)
        self.assertEqual([thread["index"] for thread in result["threads"][:2]], [69, 68])
        self.assertTrue(result["threads"][1]["triggered"])


if __name__ == "__main__":
    unittest.main()
