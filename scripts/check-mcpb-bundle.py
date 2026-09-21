#!/usr/bin/env python3
# SPDX-License-Identifier: Apache-2.0
"""Verify and launch a trusted, locally built MCPB archive without contacting Tally."""
import argparse
import platform
import hashlib
import json
import os
from pathlib import Path, PurePosixPath
import stat
import subprocess
import sys
import tempfile
import threading
import zipfile

RESOURCES = ("LICENSE", "NOTICE", "THIRD_PARTY_LICENSES.txt", "THIRD_PARTY_LICENSES_RUST.txt")
DEFAULT_TOOLS = {
    "tally_status", "list_companies", "voucher_schema", "validate_masters", "outstandings",
    "ledger_masters", "ledger_movement", "trial_balance", "vouchers", "voucher_presence", "read_evidence", "egress_log", "verify_import",
}
# The bundle always enables file preparation and bank-statement parsing; they
# write nothing to Tally. Posting is the one tool behind the user's switch, and
# it is off by default until bridge#574 and bridge#575 are fixed.
IMPORT_TOOLS = {"build_import_xml", "parse_bank_statement"}
POSTING_TOOLS = {"post_import"}


def expected_tools(environment):
    """The tools/list a bundle launched with `environment` must advertise."""
    tools = set(DEFAULT_TOOLS)
    if environment.get("BRIDGE_AGENT_ENABLE_IMPORT") == "true" or environment.get("BRIDGE_AGENT_ENABLE_WRITES") == "true":
        tools |= IMPORT_TOOLS
    if environment.get("BRIDGE_AGENT_ENABLE_WRITES") == "true":
        tools |= POSTING_TOOLS
    return tools
# The bundled PDFium: beside the binary, where parse_bank_statement loads it,
# with the licence notice scripts/fetch-pdfium.py generated at the root. Both are
# checked against packaging/pdfium/pdfium.lock.json, not merely present.
PDFIUM_NOTICE = "THIRD_PARTY_LICENSES_PDFIUM.txt"
PDFIUM_LIBRARIES = {"darwin": "libpdfium.dylib", "win32": "pdfium.dll"}
PDFIUM_PLATFORMS = {("darwin", "arm64"): "macos-arm64", ("win32", "amd64"): "windows-x64"}
ARCHIVE_MEMBERS = len(RESOURCES) + 4  # manifest, binary, PDFium library, PDFium notice
STATEMENT_FIXTURE = Path("src-tauri/crates/bridge-bank-statement/tests/fixtures/hdfc-synthetic.pdf")
STATEMENT_PASSWORD = "synthetic-user-4321"
MAX_BUNDLE_BYTES = 128 * 1024 * 1024
MAX_OUTPUT_BYTES = 512 * 1024


class SmokeError(Exception):
    """A fixed diagnostic code; never include tool payloads in diagnostics."""


def require(condition, code):
    if not condition:
        raise SmokeError(code)


def member_path(name):
    path = PurePosixPath(name)
    require(bool(name) and not path.is_absolute() and path.as_posix() == name
            and not any(part in (".", "..") for part in path.parts)
            and not any(character in name for character in ("\\", ":", "\0")),
            "invalid_archive_member_path")
    return path


def unpack_bundle(archive, destination, repository):
    require(archive.stat().st_size <= MAX_BUNDLE_BYTES, "archive_too_large")
    with zipfile.ZipFile(archive) as bundle:
        members = bundle.infolist()
        require(len(members) == ARCHIVE_MEMBERS, "unexpected_archive_members")
        names = [item.filename for item in members]
        require(len(set(names)) == len(names), "duplicate_archive_member")
        require(sum(item.file_size for item in members) <= MAX_BUNDLE_BYTES,
                "unpacked_archive_too_large")
        for item in members:
            member_path(item.orig_filename)
            mode = item.external_attr >> 16
            require(not item.is_dir() and stat.S_IFMT(mode) in (0, stat.S_IFREG),
                    "archive_member_is_not_regular_file")
        manifest = json.loads(bundle.read("manifest.json"))
        entry = manifest["server"]["entry_point"]
        require(isinstance(entry, str), "invalid_entry_point")
        member_path(entry)
        require(entry.startswith("bin/"), "invalid_entry_point")
        library = str(PurePosixPath(entry).parent / PDFIUM_LIBRARIES.get(sys.platform, ""))
        require(set(names) == set(RESOURCES) | {"manifest.json", entry, library, PDFIUM_NOTICE},
                "unexpected_archive_members")
        config = manifest["server"]["mcp_config"]
        require(manifest["server"]["type"] == "binary"
                and config["command"] == "${__dirname}/" + entry
                and not config.get("args") and not config.get("platform_overrides"),
                "launch_command_does_not_match_binary")
        require(manifest["compatibility"]["platforms"] == [sys.platform],
                "archive_platform_mismatch")
        for resource in RESOURCES:
            require(bundle.read(resource) == (repository / resource).read_bytes(),
                    "legal_resource_bytes_differ")
        pin = pdfium_pin(repository)
        for member, prefix in ((library, "library"), (PDFIUM_NOTICE, "notice")):
            data = bundle.read(member)
            require(len(data) == pin[prefix + "_bytes"]
                    and hashlib.sha256(data).hexdigest() == pin[prefix + "_sha256"],
                    "pdfium_" + prefix + "_not_pinned")
        for item in members:
            target = destination.joinpath(*member_path(item.filename).parts)
            require(target.resolve().is_relative_to(destination.resolve()),
                    "archive_member_escapes_destination")
            target.parent.mkdir(parents=True, exist_ok=True)
            target.write_bytes(bundle.read(item))
        binary = destination.joinpath(*member_path(entry).parts)
        if os.name != "nt":
            mode = bundle.getinfo(entry).external_attr >> 16
            require(mode & stat.S_IXUSR, "binary_is_not_executable")
            binary.chmod(mode & 0o777)
    return manifest, binary


def pdfium_pin(repository, host=None):
    key = host or (sys.platform, platform.machine().lower())
    lock = json.loads((repository / "packaging" / "pdfium" / "pdfium.lock.json").read_text(encoding="utf-8"))
    name = PDFIUM_PLATFORMS.get(key)
    require(name in lock["platforms"], "pdfium_platform_not_pinned")
    return lock["platforms"][name]


def statement_smoke(command, base_environment, temporary, repository):
    """Parse the synthetic statement through the unpacked bundle, so the
    bundled PDFium is proven to load from beside the binary."""
    work = Path(temporary) / "statement"
    work.mkdir()
    statement = work / "statement.pdf"
    statement.write_bytes((repository / STATEMENT_FIXTURE).read_bytes())
    password_file = work / "statement.password"
    descriptor = os.open(password_file, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
    with os.fdopen(descriptor, "w", encoding="utf-8") as handle:
        handle.write(STATEMENT_PASSWORD + "\n")
    # The default bundle environment, posting off: parsing must not need it.
    environment = dict(base_environment, BRIDGE_AGENT_DATA_DIR=str(work / "data"))
    arguments = {
        "statement_path": str(statement), "password_file": str(password_file), "bank": "hdfc",
        "account_label": "Synthetic CA xx4321", "opening_balance": "1,000.00",
        "closing_balance": "1,02,200.00", "total_debits": "8,800.00",
        "total_credits": "1,10,000.00", "bank_ledger": "Synthetic Bank Ledger",
        "suspense_ledger": "Suspense",
    }
    requests = [
        {"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {
            "protocolVersion": "2025-06-18", "capabilities": {},
            "clientInfo": {"name": "bridge-mcpb-smoke", "version": "1.0.0"}}},
        {"jsonrpc": "2.0", "method": "notifications/initialized"},
        {"jsonrpc": "2.0", "id": 2, "method": "tools/call", "params": {
            "name": "parse_bank_statement", "arguments": arguments}},
    ]
    payload = b"".join(json.dumps(request).encode() + b"\n" for request in requests)
    output, diagnostics = run_bounded([command], payload, environment, timeout=60)
    require(STATEMENT_PASSWORD.encode() not in output + diagnostics, "statement_password_echoed")
    replies = [json.loads(line) for line in output.splitlines()]
    require([reply.get("id") for reply in replies] == [1, 2], "statement_response_ids")
    call = replies[1].get("result", {})
    result = call.get("structuredContent", {}).get("result", {})
    require(call.get("isError") is False and result.get("statement_rows") == 6
            and result.get("vouchers") == 6 and isinstance(result.get("proposals_id"), str),
            "statement_parse_failed:" + str(result.get("error", {}).get("code", "no_result")))
    return result["vouchers"]


def run_bounded(command, payload, environment, timeout=15):
    outputs = {}
    exceeded = threading.Event()
    with subprocess.Popen(command, stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                          stderr=subprocess.PIPE, env=environment) as process:
        def collect(name, stream):
            outputs[name] = stream.read(MAX_OUTPUT_BYTES + 1)
            if len(outputs[name]) > MAX_OUTPUT_BYTES:
                exceeded.set()
                process.kill()

        readers = [threading.Thread(target=collect, args=(name, stream), daemon=True)
                   for name, stream in (("stdout", process.stdout), ("stderr", process.stderr))]
        for reader in readers:
            reader.start()
        try:
            process.stdin.write(payload)
            process.stdin.close()
            process.wait(timeout=timeout)
        except subprocess.TimeoutExpired:
            process.kill()
            process.wait()
            raise SmokeError("server_timeout") from None
        finally:
            for reader in readers:
                reader.join(timeout=1)
        require(not exceeded.is_set(), "server_output_limit")
        require(process.returncode == 0, f"server_exit_failed:{process.returncode}")
        return outputs["stdout"], outputs["stderr"]


def resolve_environment(manifest):
    mappings = manifest["server"]["mcp_config"]["env"]
    require(set(mappings) == {"BRIDGE_TALLY_HOST", "BRIDGE_TALLY_PORT", "BRIDGE_AGENT_REDACTION",
                              "BRIDGE_AGENT_ENABLE_IMPORT", "BRIDGE_AGENT_ENABLE_WRITES"},
            "unexpected_environment_mapping")
    require(mappings["BRIDGE_AGENT_ENABLE_IMPORT"] == "true", "import_environment_mapping_mismatch")
    writes = manifest["user_config"].get("enable_writes", {})
    require(writes.get("type") == "boolean" and isinstance(writes.get("default"), bool),
            "writes_default_must_be_boolean")
    # Posting stays off by default until bridge#574 and bridge#575 are fixed.
    require(writes["default"] is False, "posting_must_default_off")
    require(mappings["BRIDGE_AGENT_ENABLE_WRITES"] == "${user_config.enable_writes}",
            "writes_environment_mapping_mismatch")
    # Supply isolated client settings through the manifest itself. Overwriting
    # the resulting environment would hide broken host/port substitutions.
    values = {name: option["default"] for name, option in manifest["user_config"].items()}
    values.update(host="127.0.0.1", port=9)
    resolved = {}
    for key, value in mappings.items():
        for name, setting in values.items():
            value = value.replace("${user_config." + name + "}",
                                  json.dumps(setting) if isinstance(setting, bool) else str(setting))
        require("${" not in value, "unresolved_environment_mapping")
        resolved[key] = value
    require(resolved["BRIDGE_TALLY_HOST"] == values["host"]
            and resolved["BRIDGE_TALLY_PORT"] == str(values["port"]),
            "endpoint_environment_mapping_mismatch")
    return resolved


def validate_schema_receipts(raw, response):
    records = [json.loads(line) for line in raw.splitlines()]
    require(len(records) == 2, "schema_egress_completion_missing")
    prepared, completed = records
    digest = hashlib.sha256(response).hexdigest()
    require(prepared.get("record_type") == "response_prepared"
            and prepared.get("tool") == "voucher_schema"
            and isinstance(prepared.get("receipt_id"), str) and bool(prepared["receipt_id"])
            and prepared.get("bytes_prepared") == len(response)
            and prepared.get("response_sha256") == digest,
            "schema_egress_preparation_invalid")
    require(completed.get("record_type") == "stdio_write_completed"
            and completed.get("receipt_id") == prepared["receipt_id"]
            and completed.get("bytes_written") == len(response)
            and completed.get("response_sha256") == digest,
            "schema_egress_completion_invalid")
    return len(records)


def validate_server_version(reply, manifest):
    version = manifest.get("version")
    require(isinstance(version, str)
            and reply.get("result", {}).get("serverInfo", {}).get("version") == version,
            "server_version_mismatch")
    return version


def smoke(archive, repository):
    with tempfile.TemporaryDirectory(prefix="bridge-mcpb-smoke-") as temporary:
        destination = Path(temporary) / "bundle"
        manifest, binary = unpack_bundle(archive, destination, repository)
        environment = {key: value for key, value in os.environ.items()
                       if not key.startswith(("BRIDGE_AGENT_", "BRIDGE_TALLY_", "BRIDGE_PDFIUM_"))}
        environment.update(resolve_environment(manifest))
        environment["BRIDGE_AGENT_DATA_DIR"] = str(Path(temporary) / "data")
        requests = [
            {"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {
                "protocolVersion": "2025-06-18", "capabilities": {},
                "clientInfo": {"name": "bridge-mcpb-smoke", "version": "1.0.0"}}},
            {"jsonrpc": "2.0", "method": "notifications/initialized"},
            {"jsonrpc": "2.0", "id": 2, "method": "tools/list"},
            {"jsonrpc": "2.0", "id": 3, "method": "tools/call", "params": {
                "name": "voucher_schema", "arguments": {}}},
        ]
        payload = b"".join(json.dumps(request).encode() + b"\n" for request in requests)
        command = manifest["server"]["mcp_config"]["command"].replace("${__dirname}", str(destination))
        output, diagnostics = run_bounded([command], payload, environment)
        replies = [json.loads(line) for line in output.splitlines()]
        require([reply.get("id") for reply in replies] == [1, 2, 3], "unexpected_response_ids")
        require(all(reply.get("jsonrpc") == "2.0" and "result" in reply for reply in replies),
                "invalid_jsonrpc_response")
        require(replies[0]["result"]["protocolVersion"] == "2025-06-18", "protocol_mismatch")
        server_version = validate_server_version(replies[0], manifest)
        names = [tool["name"] for tool in replies[1]["result"]["tools"]]
        expected = expected_tools(environment)
        require(len(names) == len(expected) and set(names) == expected, "default_tools_mismatch")
        require("post_import" not in names and IMPORT_TOOLS <= set(names),
                "default_bundle_must_prepare_but_not_post")
        # The user's opt-in adds posting and nothing else; verify_import stays
        # available either way for safe recovery of saved batches.
        enabled_environment = dict(environment, BRIDGE_AGENT_ENABLE_WRITES="true")
        catalogue_payload = b"".join(json.dumps(request).encode() + b"\n" for request in requests[:-1])
        enabled_output, enabled_diagnostics = run_bounded([command], catalogue_payload, enabled_environment)
        enabled_replies = [json.loads(line) for line in enabled_output.splitlines()]
        require([reply.get("id") for reply in enabled_replies] == [1, 2], "opt_in_response_ids")
        enabled_names = [tool["name"] for tool in enabled_replies[1]["result"]["tools"]]
        require(len(enabled_names) == len(expected_tools(enabled_environment))
                and set(enabled_names) == expected_tools(enabled_environment)
                and set(enabled_names) - set(names) == POSTING_TOOLS,
                "opt_in_tools_mismatch")
        schema = replies[2]["result"]
        require(schema.get("isError") is False
                and schema["structuredContent"]["result"]["schema"]["type"] == "object"
                and json.loads(schema["content"][0]["text"]) == schema["structuredContent"],
                "voucher_schema_response_invalid")
        receipts = validate_schema_receipts(
            (Path(temporary) / "data" / "agent-egress.jsonl").read_bytes(),
            output.splitlines(keepends=True)[2],
        )
        statement_vouchers = statement_smoke(command, environment, temporary, repository)
        return {
            "archive_sha256": hashlib.sha256(archive.read_bytes()).hexdigest(),
            "binary_sha256": hashlib.sha256(binary.read_bytes()).hexdigest(),
            "platform": sys.platform, "archive_entries": ARCHIVE_MEMBERS,
            "legal_resources": len(RESOURCES) + 1,
            "pdfium_library_sha256": pdfium_pin(repository)["library_sha256"],
            "statement_vouchers": statement_vouchers,
            "server_version": server_version,
            "response_ids": [reply["id"] for reply in replies], "response_bytes": len(output),
            "default_tool_count": len(names), "opt_in_tool_count": len(enabled_names),
            "opt_in_stderr_bytes": len(enabled_diagnostics), "egress_receipts": receipts,
            "stderr_bytes": len(diagnostics), "stderr_sha256": hashlib.sha256(diagnostics).hexdigest(),
        }


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("archive", type=Path)
    parser.add_argument("--output", type=Path, help="Write the payload-free verification metadata")
    args = parser.parse_args()
    try:
        result = smoke(args.archive, Path(__file__).resolve().parents[1])
        report = json.dumps(result, indent=2) + "\n"
        if args.output:
            args.output.write_text(report, encoding="utf-8")
        print(report, end="")
    except (SmokeError, OSError, ValueError, KeyError, TypeError, zipfile.BadZipFile) as error:
        code = str(error) if isinstance(error, SmokeError) else type(error).__name__
        print("MCPB smoke failed: " + code, file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
