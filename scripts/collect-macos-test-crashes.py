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


def label(value):
    if not isinstance(value, str):
        return None
    value = re.sub(r"(?:[A-Za-z]:[\\/]|/)[^\s<>]+", "<path>", value)
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
    result = {
        "process": process,
        "exception": {key: label(report.get("exception", {}).get(key)) for key in ("type", "signal")},
        "faulting_thread": offset(report.get("faultingThread")),
        "threads_truncated": len(threads) > 64,
        "threads": [],
    }
    for index, thread in enumerate(threads[:64]):
        frames = thread.get("frames", [])
        output = {"index": index, "triggered": thread.get("triggered") is True,
                  "frames_truncated": len(frames) > 64, "frames": []}
        for frame in frames[:64]:
            image_index = frame.get("imageIndex")
            image = images[image_index] if isinstance(image_index, int) and 0 <= image_index < len(images) else {}
            output["frames"].append({
                "image": label(Path(image.get("name") or image.get("path") or "unknown").name),
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


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--since-file", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--wait-seconds", type=int, choices=range(31), default=0)
    args = parser.parse_args()
    directories = [Path.home() / "Library/Logs/DiagnosticReports", Path("/Library/Logs/DiagnosticReports")]
    since = args.since_file.stat().st_mtime
    deadline = time.monotonic() + args.wait_seconds
    while True:
        result = collect(directories, since)
        if result["status"] != "no_fresh_reports" or time.monotonic() >= deadline:
            break
        time.sleep(1)
    args.output.write_text(json.dumps(result, indent=2) + "\n")
    print(json.dumps({"status": result["status"], "reports": len(result["reports"]), "errors": result["errors"]}))
    return 1 if result["status"] == "capture_error" else 0


if __name__ == "__main__":
    raise SystemExit(main())
