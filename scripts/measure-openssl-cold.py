"""Windows-only fresh OpenSSL A/B/A comparison using Bridge's existing cipher test."""
import hashlib
import importlib.util
import json
import os
from pathlib import Path, PurePosixPath
import re
import subprocess
import tarfile
import time
import tomllib


ROOT = Path.cwd()
OUTPUT = ROOT / "openssl-cold-measurements"
MANIFEST = ROOT / "src-tauri/Cargo.toml"
LOCK = ROOT / "src-tauri/Cargo.lock"
TEST = "db::encrypted::tests::sqlcipher_encrypts_contents_and_rejects_the_wrong_key"
VERSION = "300.6.1+3.6.3"
spec = importlib.util.spec_from_file_location("capture", ROOT / "scripts/capture-package-log.py")
capture = importlib.util.module_from_spec(spec)
spec.loader.exec_module(capture)


def digest(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def execute(command, name):
    started = time.monotonic()
    with subprocess.Popen(command, stdout=subprocess.PIPE, stderr=subprocess.STDOUT) as process:
        capture.capture(process.stdout, OUTPUT / name)
        result = process.wait()
    if result:
        raise subprocess.CalledProcessError(result, command)
    return time.monotonic() - started


def metadata():
    return json.loads(subprocess.check_output([
        "cargo", "metadata", "--offline", "--format-version", "1", "--manifest-path", str(MANIFEST)]))


def run():
    assert os.name == "nt", "this experiment is scoped to Windows MSVC"
    assert not os.environ.get("RUSTC_WRAPPER"), "compiler caching must not confound the comparison"
    OUTPUT.mkdir(exist_ok=False)
    originals = {path: path.read_bytes() for path in (MANIFEST, LOCK)}
    copied = Path(os.environ["RUNNER_TEMP"]) / "bridge-openssl-src-control"
    assert not copied.exists(), "a new, isolated source copy is required"
    rows = []
    try:
        execute(["cargo", "fetch", "--locked", "--manifest-path", str(MANIFEST)], "fetch.log")
        before = metadata()
        packages = [p for p in before["packages"] if p["name"] == "openssl-src"]
        assert len(packages) == 1 and packages[0]["version"] == VERSION
        source = Path(packages[0]["manifest_path"]).parent
        locked = next(p for p in tomllib.loads(originals[LOCK].decode())["package"] if p["name"] == "openssl-src")
        archive_path = source.parents[2] / "cache" / source.parent.name / (source.name + ".crate")
        assert digest(archive_path) == locked["checksum"], "registry archive does not match Cargo.lock"
        checksums = {"package": locked["checksum"], "files": {}}
        copied.mkdir()
        # Extract the checksum-verified crate rather than trusting a mutable
        # registry source tree. Preserve all upstream license/notice files.
        with tarfile.open(archive_path) as archive:
            for member in archive:
                path = PurePosixPath(member.name)
                assert not path.is_absolute() and ".." not in path.parts and path.parts[0] == source.name
                assert member.isfile() or member.isdir(), "unexpected archive entry type"
                if member.isdir():
                    continue
                relative = Path(*path.parts[1:])
                target = copied / relative
                target.parent.mkdir(parents=True, exist_ok=True)
                data = archive.extractfile(member).read()
                target.write_bytes(data)
                checksums["files"][relative.as_posix()] = hashlib.sha256(data).hexdigest()
        for relative, expected in checksums["files"].items():
            assert digest(copied / relative) == expected, f"copy checksum mismatch: {relative}"
        (OUTPUT / "crate-checksums.json").write_text(json.dumps(checksums, indent=2) + "\n")
        assert b"[patch.crates-io]" not in originals[MANIFEST]
        MANIFEST.write_bytes(originals[MANIFEST] + (
            '\n[patch.crates-io]\nopenssl-src = { path = ' + json.dumps(copied.as_posix()) + ' }\n').encode())
        after = metadata()
        before_packages = {(p["name"], p["version"]) for p in before["packages"]}
        assert before_packages == {(p["name"], p["version"]) for p in after["packages"]}, "dependency versions changed"
        patched_lock = tomllib.loads(LOCK.read_text())
        expected_lock = tomllib.loads(originals[LOCK].decode())
        for package in expected_lock["package"]:
            if package["name"] == "openssl-src":
                package.pop("source")
                package.pop("checksum")
        assert patched_lock == expected_lock, "unexpected dependency resolution change"
        unit = copied / "src/lib.rs"
        baseline = unit.read_bytes()
        needle = b'.arg("no-tests")'
        assert baseline.count(needle) == 1 and b'"no-makedepend"' not in baseline
        candidate = baseline.replace(needle, needle + b'\n            .arg("no-makedepend")')
        (OUTPUT / "source-change.json").write_text(json.dumps({
            "crate_version": VERSION, "package_checksum": checksums["package"],
            "source_verified_files": len(checksums["files"]),
            "baseline_src_sha256": hashlib.sha256(baseline).hexdigest(),
            "candidate_src_sha256": hashlib.sha256(candidate).hexdigest(),
            "only_changed_file": "src/lib.rs", "only_added_argument": "no-makedepend",
            "checkout_sha": subprocess.check_output(["git", "rev-parse", "HEAD"], text=True).strip(),
            "run_id": os.environ.get("GITHUB_RUN_ID"), "run_attempt": os.environ.get("GITHUB_RUN_ATTEMPT"),
        }, indent=2) + "\n")
        for name, contents in [("1-original", baseline), ("2-no-makedepend", candidate), ("3-original", baseline)]:
            unit.write_bytes(contents)
            execute(["cargo", "clean", "--manifest-path", str(MANIFEST), "--release", "-p", "openssl-src", "-p", "openssl-sys"], name + "-clean.log")
            timings = ROOT / "src-tauri/target/cargo-timings"
            previous = set(timings.glob("cargo-timing-*.html"))
            seconds = execute(["cargo", "test", "--locked", "--manifest-path", str(MANIFEST),
                               "--release", "--lib", TEST, "--timings", "--", "--exact", "--nocapture"], name + "-test.log")
            reports = set(timings.glob("cargo-timing-*.html")) - previous
            assert len(reports) == 1, "missing or ambiguous compiler timings"
            report = reports.pop().read_text()
            (OUTPUT / (name + ".html")).write_text(report)
            match = re.search(r"const UNIT_DATA = (\[.*?\]);", report, re.S)
            assert match, "compiler timing data unavailable"
            units = json.loads(match.group(1))
            log = (OUTPUT / (name + "-test.log") / "build-tail.log").read_text(errors="replace")
            assert f"test {TEST} ... ok" in log and "test result: ok. 1 passed; 0 failed; 0 ignored" in log, "exact existing cipher test did not pass"
            rows.append({"sample": name, "command_seconds": seconds, "test": TEST, "test_passed": True,
                         "openssl_src_sha256": digest(unit), "openssl_units": [u for u in units if u["name"] in ("openssl-src", "openssl-sys")],
                         "other_compiled_units": [u for u in units if u["name"] not in ("openssl-src", "openssl-sys") and u["duration"] > 0]})
            (OUTPUT / "results.json").write_text(json.dumps(rows, indent=2) + "\n")
            print(json.dumps({"sample": name, "command_seconds": seconds, "test_passed": True}), flush=True)
        unit.write_bytes(baseline)
    finally:
        for path, contents in originals.items():
            path.write_bytes(contents)
        restored = all(path.read_bytes() == contents for path, contents in originals.items())
        (OUTPUT / "restoration.json").write_text(json.dumps({"application_inputs_restored": restored}) + "\n")
        assert restored


if __name__ == "__main__":
    run()
