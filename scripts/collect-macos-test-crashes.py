"""Retain bounded stack evidence from fresh Bridge test crash reports, not raw IPS.

Format: https://developer.apple.com/documentation/xcode/interpreting-the-json-format-of-a-crash-report
"""

import argparse
import json
from pathlib import Path
import re
import time


MAX_REPORTS = 8
MAX_BYTES = 8 * 1024 * 1024
PROCESS = re.compile(r"bridge_lib(?:-[0-9a-f]+)?\Z")
UUID = re.compile(r"[0-9a-fA-F]{8}(?:-[0-9a-fA-F]{4}){3}-[0-9a-fA-F]{12}\Z")
ARCHES = {"arm64", "arm64e", "x86_64", "x86_64h", "i386"}


def image_identity(image):
    identifier = image.get("uuid")
    arch = image.get("arch")
    return {
        "uuid": identifier.lower() if isinstance(identifier, str) and UUID.fullmatch(identifier) else None,
        "arch": arch if isinstance(arch, str) and arch in ARCHES else None,
    }


def label(value):
    if not isinstance(value, str) or any(char in value for char in "/\\") or any(ord(char) < 32 for char in value):
        return None
    return value[:512]


def offset(value):
    return value if type(value) is int and value >= 0 else None


def minimize(text):
    metadata, body = text.split("\n", 1)
    if str(json.loads(metadata).get("bug_type")) != "309":
        raise ValueError("not_crash_report")
    report = json.loads(body)
    process = report.get("procName", "")
    if not PROCESS.fullmatch(process):
        raise ValueError("not_bridge_test")
    images = report.get("usedImages", [])
    threads = report.get("threads", [])
    process_images = [{"image_index": index, **image_identity(entry)}
                      for index, entry in enumerate(images)
                      if Path(entry.get("name") or entry.get("path") or "unknown").name == process]
    result = {
        "process": process,
        "process_images": process_images[:2],
        "process_images_truncated": len(process_images) > 2,
        "exception": {key: label(report.get("exception", {}).get(key)) for key in ("type", "signal")},
        "faulting_thread": offset(report.get("faultingThread")),
        "threads_truncated": len(threads) > 64,
        "threads": [],
    }
    priority = [result["faulting_thread"]] + [index for index, thread in enumerate(threads) if thread.get("triggered") is True]
    indices = dict.fromkeys(index for index in priority + list(range(len(threads)))
                            if type(index) is int and 0 <= index < len(threads))
    for index in list(indices)[:64]:
        thread = threads[index]
        frames = thread.get("frames", [])
        output = {"index": index, "triggered": thread.get("triggered") is True,
                  "frames_truncated": len(frames) > 64, "frames": []}
        for frame in frames[:64]:
            image_index = frame.get("imageIndex")
            valid_index = type(image_index) is int and 0 <= image_index < len(images)
            image = images[image_index] if valid_index else {}
            output["frames"].append({
                "image": label(Path(image.get("name") or image.get("path") or "unknown").name),
                "image_index": image_index if valid_index else None,
                **image_identity(image),
                "symbol": label(frame.get("symbol")),
                "image_offset": offset(frame.get("imageOffset")),
                "symbol_offset": offset(frame.get("symbolLocation")),
            })
        result["threads"].append(output)
    return result


def collect(directories, since):
    result = {"status": "no_fresh_reports", "reports": [], "errors": [], "truncated": False}
    candidates = []
    for directory in directories:
        try:
            if not directory.exists():
                continue
            for path in directory.glob("bridge_lib*.ips"):
                if path.is_symlink():
                    result["errors"].append("symlink_report")
                elif path.stat().st_mtime >= since:
                    candidates.append(path)
        except OSError:
            result["errors"].append("directory_read_failed")
    result["truncated"] = len(candidates) > MAX_REPORTS
    for path in sorted(candidates)[:MAX_REPORTS]:
        try:
            with path.open("rb") as source:
                data = source.read(MAX_BYTES + 1)
            if len(data) > MAX_BYTES:
                result["errors"].append("report_too_large")
                continue
            result["reports"].append(minimize(data.decode("utf-8")))
        except (OSError, ValueError, TypeError, AttributeError, KeyError, IndexError):
            result["errors"].append("report_read_or_parse_failed")
    if result["errors"] or result["truncated"]:
        result["status"] = "capture_error"
    elif result["reports"]:
        result["status"] = "captured"
    return result


def wait_for_reports(directories, since, wait_seconds):
    deadline = time.monotonic() + wait_seconds
    last_error = None
    while True:
        # Refresh the bounded snapshot through the grace period: later IPS files
        # can arrive after the first complete report. Replacing avoids duplicates.
        result = collect(directories, since)
        if result["status"] == "capture_error":
            last_error = result
        if time.monotonic() >= deadline:
            return result if result["status"] == "captured" else last_error or result
        time.sleep(1)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--since-file", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--wait-seconds", type=int, choices=range(31), default=0)
    args = parser.parse_args()
    directories = [Path.home() / "Library/Logs/DiagnosticReports", Path("/Library/Logs/DiagnosticReports")]
    since = args.since_file.stat().st_mtime
    result = wait_for_reports(directories, since, args.wait_seconds)
    args.output.write_text(json.dumps(result, indent=2) + "\n")
    print(json.dumps({"status": result["status"], "reports": len(result["reports"]), "errors": result["errors"]}))
    return 1 if result["status"] == "capture_error" else 0


if __name__ == "__main__":
    raise SystemExit(main())
