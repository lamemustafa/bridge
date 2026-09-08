import importlib.util
import json
from pathlib import Path
import tempfile
import unittest

spec = importlib.util.spec_from_file_location("capture", Path(__file__).with_name("collect-macos-test-crashes.py"))
capture = importlib.util.module_from_spec(spec)
spec.loader.exec_module(capture)


class CaptureTests(unittest.TestCase):
    def report(self):
        return json.dumps({"bug_type": "309"}) + "\n" + json.dumps({
            "procName": "bridge_lib-abcdef", "procPath": "/Users/synthetic/private-binary",
            "environment": {"TOKEN": "must-not-escape"},
            "exception": {"type": "EXC_BREAKPOINT", "signal": "SIGTRAP", "message": "must-not-escape"},
            "faultingThread": 0,
            "threads": [{"triggered": True, "threadState": {"register": "must-not-escape"},
                         "frames": [{"imageIndex": 0, "symbol": "worker_exit /Users/synthetic/source.rs",
                                     "imageOffset": 42, "symbolLocation": 1}]}],
            "usedImages": [{"path": "/Users/synthetic/libexample.dylib"}],
        })

    def test_retains_stack_without_raw_process_data(self):
        result = capture.minimize(self.report())
        encoded = json.dumps(result)
        self.assertNotIn("must-not-escape", encoded)
        self.assertNotIn("/Users/", encoded)
        self.assertEqual(result["threads"][0]["frames"][0]["image"], "libexample.dylib")
        self.assertEqual(result["exception"]["signal"], "SIGTRAP")

    def test_distinguishes_missing_from_invalid_fresh_report(self):
        with tempfile.TemporaryDirectory() as root:
            directory = Path(root)
            self.assertEqual(capture.collect([directory], 0)["status"], "no_fresh_reports")
            (directory / "bridge_lib-test.ips").write_text("invalid")
            self.assertEqual(capture.collect([directory], 0)["status"], "capture_error")

    def test_excludes_old_reports_and_bounds_frames(self):
        with tempfile.TemporaryDirectory() as root:
            directory = Path(root)
            path = directory / "bridge_lib-test.ips"
            path.write_text(self.report())
            self.assertEqual(capture.collect([directory], path.stat().st_mtime + 1)["status"], "no_fresh_reports")
            body = json.loads(self.report().split("\n", 1)[1])
            body["threads"][0]["frames"] *= 70
            result = capture.minimize(json.dumps({"bug_type": "309"}) + "\n" + json.dumps(body))
            self.assertTrue(result["threads"][0]["frames_truncated"])
            self.assertEqual(len(result["threads"][0]["frames"]), 64)


if __name__ == "__main__":
    unittest.main()
