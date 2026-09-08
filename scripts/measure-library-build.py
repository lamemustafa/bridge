"""Branch-only same-runner compiler experiment; restore the manifest on exit."""
import hashlib
import json
from pathlib import Path
import re
import subprocess
import time

manifest = Path("src-tauri/Cargo.toml")
original = manifest.read_bytes()
old = b'crate-type = ["staticlib", "cdylib", "rlib"]'
new = b'crate-type = ["rlib"]'
if original.count(old) != 1:
    raise SystemExit("unexpected baseline library formats")
output = Path("library-build-measurements")
output.mkdir(exist_ok=True)
rows = []
try:
    for index, formats in enumerate(["original", "rlib", "original"], 1):
        manifest.write_bytes(original if formats == "original" else original.replace(old, new))
        with (output / f"{index}-{formats}-clean.log").open("wb") as log:
            subprocess.run(["cargo", "clean", "--manifest-path", str(manifest), "--release", "-p", "bridge"], stdout=log, stderr=subprocess.STDOUT, check=True)
        timing_dir = Path("src-tauri/target/cargo-timings")
        previous = set(timing_dir.glob("cargo-timing-*.html"))
        start = time.monotonic()
        with (output / f"{index}-{formats}-build.log").open("wb") as log:
            subprocess.run(["node", "node_modules/@tauri-apps/cli/tauri.js", "build", "--no-bundle", "--", "--timings"], stdout=log, stderr=subprocess.STDOUT, check=True, shell=False)
        elapsed = time.monotonic() - start
        reports = set(timing_dir.glob("cargo-timing-*.html")) - previous
        if len(reports) != 1:
            raise RuntimeError("expected one new compiler timing report")
        report = reports.pop()
        text = report.read_text()
        match = re.search(r"const UNIT_DATA = (\[.*?\]);", text, re.S)
        if not match:
            raise RuntimeError("missing compiler units")
        units = json.loads(match.group(1))
        selected = [unit for unit in units if unit["name"] == "bridge"]
        if not any(unit["target"] == "" and unit["duration"] > 0 for unit in selected):
            raise RuntimeError("Bridge did not compile")
        (output / f"{index}-{formats}.html").write_text(text)
        row = {"order": index, "formats": formats, "command_seconds": elapsed,
               "manifest_sha256": hashlib.sha256(manifest.read_bytes()).hexdigest(), "bridge_units": selected,
               "other_compiled_units": [unit for unit in units if unit["name"] != "bridge"]}
        rows.append(row)
        (output / "results.json").write_text(json.dumps(rows, indent=2) + "\n")
        print(json.dumps({"order": index, "formats": formats, "command_seconds": elapsed}), flush=True)
finally:
    manifest.write_bytes(original)
    if manifest.read_bytes() != original:
        raise RuntimeError("baseline manifest was not restored")
