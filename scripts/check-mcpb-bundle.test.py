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
    def test_schema_receipts_require_matching_write_completion(self):
        response = b'{"jsonrpc":"2.0","id":3,"result":{}}\n'
        digest = smoke.hashlib.sha256(response).hexdigest()
        prepared = {"record_type": "response_prepared", "tool": "voucher_schema",
                    "receipt_id": "test-receipt", "bytes_prepared": len(response),
                    "response_sha256": digest}
        completed = {"record_type": "stdio_write_completed", "receipt_id": "test-receipt",
                     "bytes_written": len(response), "response_sha256": digest}
        def encoded(records):
            return b"\n".join(json.dumps(record).encode() for record in records) + b"\n"
        self.assertEqual(smoke.validate_schema_receipts(encoded([prepared, completed]), response), 2)
        with self.assertRaisesRegex(smoke.SmokeError, "schema_egress_completion_missing"):
            smoke.validate_schema_receipts(encoded([prepared]), response)
        for key, value in [("receipt_id", "other"), ("response_sha256", "wrong"),
                           ("bytes_written", 1), ("record_type", "response_prepared")]:
            with self.subTest(key=key), self.assertRaisesRegex(smoke.SmokeError, "schema_egress_completion_invalid"):
                smoke.validate_schema_receipts(encoded([prepared, dict(completed, **{key: value})]), response)

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
        self.assertEqual(resolved["BRIDGE_AGENT_ENABLE_WRITES"], "true")
        for key in ("BRIDGE_TALLY_HOST", "BRIDGE_TALLY_PORT"):
            broken = json.loads(template.read_text(encoding="utf-8"))
            broken["server"]["mcp_config"]["env"][key] = "${user_config.redaction}"
            with self.subTest(key=key), self.assertRaisesRegex(
                    smoke.SmokeError, "endpoint_environment_mapping_mismatch"):
                smoke.resolve_environment(broken)

    def test_initialize_server_version_must_match_the_archived_manifest(self):
        manifest = {"version": "0.2.0"}
        response = {"result": {"serverInfo": {"version": "0.2.0"}}}
        self.assertEqual(smoke.validate_server_version(response, manifest), "0.2.0")
        with self.assertRaisesRegex(smoke.SmokeError, "server_version_mismatch"):
            smoke.validate_server_version(
                {"result": {"serverInfo": {"version": "0.2.1"}}}, manifest)

    def test_read_only_user_setting_still_resolves_to_false(self):
        template = Path(__file__).resolve().parents[1] / "packaging/mcpb/manifest.json"
        manifest = json.loads(template.read_text(encoding="utf-8"))
        manifest["user_config"]["enable_writes"]["default"] = False
        self.assertEqual(smoke.resolve_environment(manifest)["BRIDGE_AGENT_ENABLE_WRITES"], "false")

    def test_write_setting_requires_a_real_boolean_default(self):
        template = Path(__file__).resolve().parents[1] / "packaging/mcpb/manifest.json"
        for default in ("true", "false", 0, 1, None):
            manifest = json.loads(template.read_text(encoding="utf-8"))
            manifest["user_config"]["enable_writes"]["default"] = default
            with self.subTest(default=default), self.assertRaisesRegex(
                    smoke.SmokeError, "writes_default_must_be_boolean"):
                smoke.resolve_environment(manifest)

    def test_write_mapping_cannot_bypass_user_setting(self):
        template = Path(__file__).resolve().parents[1] / "packaging/mcpb/manifest.json"
        for mapping in ("true", "false", "${user_config.redaction}"):
            manifest = json.loads(template.read_text(encoding="utf-8"))
            manifest["server"]["mcp_config"]["env"]["BRIDGE_AGENT_ENABLE_WRITES"] = mapping
            with self.subTest(mapping=mapping), self.assertRaisesRegex(
                    smoke.SmokeError, "writes_environment_mapping_mismatch"):
                smoke.resolve_environment(manifest)

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
