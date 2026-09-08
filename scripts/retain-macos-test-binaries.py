"""Retain only test executables whose image identity matches fresh minimized IPS.

No build is invoked. UUID/architecture linkage identifies the executable image,
not the individual nextest test or missing source line tables.
"""

import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import selectors
import stat
import subprocess
import tempfile
import time


MAX_BYTES = 256 * 1024 * 1024
MAX_REPORTS = 8
PROCESS = re.compile(r"bridge_lib(?:-[0-9a-f]+)?\Z")
UUID = r"[0-9a-fA-F]{8}(?:-[0-9a-fA-F]{4}){3}-[0-9a-fA-F]{12}"
IDENTITY = re.compile(rf"UUID: ({UUID}) \((arm64|arm64e|x86_64|x86_64h|i386)\) ")


class EvidenceError(Exception):
    pass


def bounded_tool(command, timeout=10):
    try:
        process = subprocess.Popen(command, stdout=subprocess.PIPE, stderr=subprocess.STDOUT)
    except OSError:
        raise EvidenceError("image_tool_failed") from None
    deadline = time.monotonic() + timeout
    output = bytearray()
    try:
        with selectors.DefaultSelector() as selector:
            selector.register(process.stdout, selectors.EVENT_READ)
            while True:
                remaining = deadline - time.monotonic()
                if remaining <= 0 or not selector.select(remaining):
                    raise EvidenceError("image_tool_timeout")
                chunk = os.read(process.stdout.fileno(), 4096)
                if not chunk:
                    break
                if len(output) + len(chunk) > 16 * 1024:
                    raise EvidenceError("image_tool_output_limit")
                output.extend(chunk)
        if process.wait(timeout=max(0, deadline - time.monotonic())) != 0:
            raise EvidenceError("image_tool_failed")
        return output.decode("utf-8")
    except subprocess.TimeoutExpired:
        raise EvidenceError("image_tool_timeout") from None
    except (OSError, UnicodeError):
        raise EvidenceError("image_tool_failed") from None
    finally:
        if process.poll() is None:
            process.kill()
        process.wait()
        process.stdout.close()


def binary_identities(path):
    output = bounded_tool(["dwarfdump", "--uuid", str(path)])
    identities = [match.groups() for line in output.splitlines()
                  if (match := IDENTITY.match(line))]
    if not identities or len(identities) > 8:
        raise EvidenceError("image_identity_unavailable")
    return [(identifier.lower(), arch) for identifier, arch in identities]


def bounded_digest(source, limit):
    digest = hashlib.sha256()
    size = 0
    while chunk := source.read(1024 * 1024):
        size += len(chunk)
        if size > limit:
            raise EvidenceError("binary_changed_during_capture")
        digest.update(chunk)
    return digest.hexdigest()


def retain(source, output, expected, budget):
    temporary = None
    try:
        # Open the exact report basename under the caller's known target root.
        # No glob, symlink traversal, post-test cargo command or rebuild.
        with os.fdopen(os.open(source, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK), "rb") as original:
            initial = os.fstat(original.fileno())
            if not stat.S_ISREG(initial.st_mode):
                raise EvidenceError("not_regular_file")
            if initial.st_size <= 0 or initial.st_size > budget:
                raise EvidenceError("binary_byte_limit")
            if binary_identities(source).count(expected) != 1:
                raise EvidenceError("image_identity_mismatch")
            with tempfile.NamedTemporaryFile(dir=output, delete=False) as staged:
                temporary = Path(staged.name)
                digest = hashlib.sha256()
                size = 0
                while chunk := original.read(1024 * 1024):
                    size += len(chunk)
                    if size > budget:
                        raise EvidenceError("binary_byte_limit")
                    digest.update(chunk)
                    staged.write(chunk)
            original.seek(0)
            current_digest = bounded_digest(original, initial.st_size)
            if size != initial.st_size or digest.hexdigest() != current_digest:
                raise EvidenceError("binary_changed_during_capture")
        with temporary.open("rb") as staged:
            if bounded_digest(staged, initial.st_size) != current_digest:
                raise EvidenceError("staged_digest_mismatch")
        if binary_identities(temporary).count(expected) != 1:
            raise EvidenceError("staged_image_identity_mismatch")
        name = f"{expected[0]}-{expected[1]}-{source.name}"
        temporary.chmod(0o700)
        temporary.replace(output / name)
        temporary = None
        return {"file": name, "sha256": current_digest, "bytes": size,
                "uuid": expected[0], "arch": expected[1]}
    except OSError:
        raise EvidenceError("binary_read_or_copy_failed") from None
    finally:
        if temporary is not None:
            temporary.unlink(missing_ok=True)


def collect(snapshot, target, output):
    reports = snapshot.get("reports", [])
    if not isinstance(reports, list):
        raise EvidenceError("invalid_reports")
    result = {"status": "unavailable", "binaries": [], "errors": [],
              "report_count": len(reports), "reports_truncated": len(reports) > MAX_REPORTS,
              "debug_info": "line_tables_unavailable_ci_profile_debug_0"}
    if snapshot.get("status") == "capture_error" or snapshot.get("truncated"):
        result["errors"].append("crash_capture_incomplete")
    if len(reports) > MAX_REPORTS:
        result["errors"].append("report_limit")
    seen = set()
    remaining = MAX_BYTES
    for report in reports[:MAX_REPORTS]:
        try:
            process = report.get("process")
            images = report.get("process_images", [])
            if not isinstance(process, str) or not PROCESS.fullmatch(process):
                raise EvidenceError("invalid_process_name")
            if len(images) != 1 or report.get("process_images_truncated"):
                raise EvidenceError("process_image_ambiguous_or_missing")
            identifier, arch = images[0].get("uuid"), images[0].get("arch")
            if not isinstance(identifier, str) or not re.fullmatch(UUID, identifier) or \
                    arch not in ("arm64", "arm64e", "x86_64", "x86_64h", "i386"):
                raise EvidenceError("process_image_identity_unavailable")
            expected = (identifier.lower(), arch)
            if expected in seen:
                continue
            binary = retain(target / process, output, expected, remaining)
            binary["process"] = process
            result["binaries"].append(binary)
            remaining -= binary["bytes"]
            seen.add(expected)
        except (EvidenceError, AttributeError, TypeError, IndexError) as error:
            result["errors"].append(str(error) if isinstance(error, EvidenceError) else "invalid_report")
    if result["errors"]:
        result["status"] = "partial" if result["binaries"] else "error"
    elif result["binaries"]:
        result["status"] = "linked"
    return result


def provenance():
    checkout = subprocess.run(["git", "rev-parse", "HEAD"], capture_output=True,
                              text=True, check=True, timeout=10).stdout.strip()
    result = {"checkout_sha": checkout if re.fullmatch(r"[0-9a-f]{40}", checkout) else None}
    for name in ("GITHUB_SHA", "PR_HEAD_SHA", "GITHUB_RUN_ID", "GITHUB_RUN_ATTEMPT", "GITHUB_JOB"):
        value = os.environ.get(name, "")
        result[name.lower()] = value if re.fullmatch(r"[A-Za-z0-9_-]{1,100}", value) else None
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--crash-json", type=Path, required=True)
    parser.add_argument("--target", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    args.output.mkdir(parents=True, exist_ok=False)
    try:
        with args.crash_json.open("rb") as source:
            data = source.read(16 * 1024 * 1024 + 1)
        if len(data) > 16 * 1024 * 1024:
            raise EvidenceError("crash_json_byte_limit")
        result = collect(json.loads(data), args.target.resolve(), args.output)
    except (EvidenceError, OSError, ValueError, TypeError, AttributeError, KeyError, subprocess.SubprocessError) as error:
        result = {"status": "error", "binaries": [], "errors": [
            str(error) if isinstance(error, EvidenceError) else "input_or_provenance_failed"]}
    result.setdefault("debug_info", "line_tables_unavailable_ci_profile_debug_0")
    try:
        result["provenance"] = provenance()
    except (OSError, subprocess.SubprocessError):
        result["errors"].append("provenance_failed")
        result["status"] = "error"
    (args.output / "manifest.json").write_text(json.dumps(result, indent=2) + "\n")
    print(json.dumps({"status": result["status"], "binaries": len(result["binaries"]),
                      "errors": result["errors"]}))
    return 1 if result["errors"] else 0


if __name__ == "__main__":
    raise SystemExit(main())
