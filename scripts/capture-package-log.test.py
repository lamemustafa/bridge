# SPDX-License-Identifier: Apache-2.0
import importlib.util
import io
import json
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

script = Path(__file__).with_name("capture-package-log.py").resolve()
spec = importlib.util.spec_from_file_location("capture_package", script)
capture = importlib.util.module_from_spec(spec)
spec.loader.exec_module(capture)


class CaptureTests(unittest.TestCase):
    def test_retains_small_child_error_without_modification(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            data = b"creating disk image\nchild stderr: diagnostic control\n"
            self.assertEqual(capture.capture(io.BytesIO(data), root), data)
            self.assertEqual((root / "build.log").read_bytes(), data)
            self.assertFalse(json.loads((root / "capture.json").read_text())["prefix_truncated"])

    def test_large_output_is_drained_and_final_error_retained(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            data = b"x" * (capture.PREFIX_BYTES * 2) + b"\nfinal error\n"
            source = io.BytesIO(data)
            tail = capture.capture(source, root)
            self.assertEqual(source.tell(), len(data))
            self.assertEqual((root / "build.log").stat().st_size, capture.PREFIX_BYTES)
            self.assertEqual(len(tail), capture.TAIL_BYTES)
            self.assertTrue(tail.endswith(b"final error\n"))
            self.assertEqual(json.loads((root / "capture.json").read_text())["total_bytes"], len(data))

    def test_pipefail_preserves_success_and_child_failure(self):
        for code in [0, 37]:
            with self.subTest(code=code), tempfile.TemporaryDirectory() as directory:
                result = subprocess.run([
                    "bash", "-o", "pipefail", "-c",
                    '"$1" -c "import sys; print(\'child stderr control\', file=sys.stderr); sys.exit($3)" 2>&1 | "$1" "$2"',
                    "package-control", sys.executable, str(script), str(code),
                ], cwd=directory, capture_output=True, check=False)
                self.assertEqual(result.returncode, code)
                self.assertIn(b"child stderr control", result.stdout)
                self.assertIn(b"child stderr control", (Path(directory) / "package-diagnostics/build.log").read_bytes())


if __name__ == "__main__":
    unittest.main()
