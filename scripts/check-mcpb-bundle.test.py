# SPDX-License-Identifier: Apache-2.0
import importlib.util
import json
import os
from pathlib import Path
import sys
import tempfile
import unittest
import zipfile

spec = importlib.util.spec_from_file_location("mcpb_smoke", Path(__file__).with_name("check-mcpb-bundle.py"))
smoke = importlib.util.module_from_spec(spec)
spec.loader.exec_module(smoke)


class BundleSmokeTests(unittest.TestCase):
    def test_entry_point_cannot_escape_extraction_directory(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            archive = root / "invalid.mcpb"
            manifest = {"server": {"entry_point": "bin/../../outside"}}
            with zipfile.ZipFile(archive, "w") as bundle:
                bundle.writestr("manifest.json", json.dumps(manifest))
                for resource in smoke.RESOURCES:
                    bundle.writestr(resource, "packaging fixture")
                bundle.writestr("bin/bridge_mcp", "packaging fixture")
            with self.assertRaisesRegex(smoke.SmokeError, "invalid_archive_member_path"):
                smoke.unpack_bundle(archive, root / "unpacked", root)
            self.assertFalse((root / "outside").exists())
            self.assertFalse((root / "unpacked").exists())

    def test_unsafe_member_names_are_rejected(self):
        for name in ("../outside", "/outside", "bin/../../outside", "bin\\outside", "bin/file:stream", "bin/\0file"):
            with self.subTest(name=name), self.assertRaises(smoke.SmokeError):
                smoke.member_path(name)

    def test_endpoint_settings_are_resolved_through_manifest_mappings(self):
        template = Path(__file__).resolve().parents[1] / "packaging/mcpb/manifest.json"
        manifest = json.loads(template.read_text(encoding="utf-8"))
        resolved = smoke.resolve_environment(manifest)
        self.assertEqual(resolved["BRIDGE_TALLY_HOST"], "127.0.0.1")
        self.assertEqual(resolved["BRIDGE_TALLY_PORT"], "9")
        for key in ("BRIDGE_TALLY_HOST", "BRIDGE_TALLY_PORT"):
            broken = json.loads(template.read_text(encoding="utf-8"))
            broken["server"]["mcp_config"]["env"][key] = "${user_config.redaction}"
            with self.subTest(key=key), self.assertRaisesRegex(
                    smoke.SmokeError, "endpoint_environment_mapping_mismatch"):
                smoke.resolve_environment(broken)

    def test_server_output_is_bounded(self):
        command = [sys.executable, "-c", "import sys; sys.stdout.write('x' * 1048576); sys.stdout.flush()"]
        with self.assertRaisesRegex(smoke.SmokeError, "server_output_limit"):
            smoke.run_bounded(command, b"", os.environ.copy())

    def test_server_timeout_is_enforced(self):
        command = [sys.executable, "-c", "import time; time.sleep(10)"]
        with self.assertRaisesRegex(smoke.SmokeError, "server_timeout"):
            smoke.run_bounded(command, b"", os.environ.copy(), timeout=0.1)


if __name__ == "__main__":
    unittest.main()
