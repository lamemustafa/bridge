"""Cold native correctness control for the installed Perl directory reduction."""
import argparse
from contextlib import contextmanager
import gzip
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import re
import subprocess
import tempfile
import time
import tomllib

ROOT = Path(__file__).resolve().parents[1]
PREFIXES = ("c/include", "c/libexec", "c/x86_64-w64-mingw32")
TEST = "db::encrypted::tests::sqlcipher_encrypts_contents_and_rejects_the_wrong_key"
CARGO = ["rustup", "run", "1.96.0", "cargo"]


def load(name, filename):
    spec = importlib.util.spec_from_file_location(name, ROOT / "scripts" / filename)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


inventory = load("perl_inventory", "inventory-windows-perl.py")
capture = load("bounded_capture", "capture-package-log.py")


def digest(data):
    return hashlib.sha256(data).hexdigest()


def tree_digest(rows):
    return digest(json.dumps(rows, sort_keys=True, separators=(",", ":")).encode())


@contextmanager
def distribution_variant(root, reduced):
    """Move only the three inventoried directories, restoring even on failure."""
    for prefix in PREFIXES:
        path = root / prefix
        inventory.reject_link(path)
        if not path.is_dir():
            raise ValueError("Expected full-distribution control directory missing")
    moved = []
    backup = None
    try:
        if reduced:
            # A sibling stays on the same volume: renames preserve bytes/metadata.
            backup = Path(tempfile.mkdtemp(prefix="bridge-perl-cold-", dir=root.parent))
            for prefix in PREFIXES:
                source, destination = root / prefix, backup / prefix
                destination.parent.mkdir(parents=True, exist_ok=True)
                source.rename(destination)
                moved.append((source, destination))
        yield
    finally:
        errors = []
        for source, destination in reversed(moved):
            try:
                if os.path.lexists(source):
                    raise ValueError("Unexpected path appeared in a moved directory; refusing overwrite")
                destination.rename(source)
            except (OSError, ValueError) as error:
                errors.append(error)
        if errors:
            raise ExceptionGroup("Perl restoration incomplete; original bytes remain in the sibling backup", errors)
        if backup is not None:
            (backup / "c").rmdir()
            backup.rmdir()


def execute(command, output, name):
    started = time.monotonic()
    with subprocess.Popen(command, stdout=subprocess.PIPE, stderr=subprocess.STDOUT) as process:
        capture.capture(process.stdout, output / name)
        status = process.wait()
    if status:
        raise subprocess.CalledProcessError(status, command)
    return round(time.monotonic() - started, 6)


def write(path, value):
    path.write_text(json.dumps(value, indent=2) + "\n")


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--case", choices=("full", "reduced"), required=True)
    args = parser.parse_args()
    if os.name != "nt":
        raise ValueError("Windows MSVC experiment only")
    target = ROOT / "src-tauri/target"
    if Path(os.environ["CARGO_TARGET_DIR"]).resolve() != target.resolve():
        raise ValueError("Experiment requires the explicit source-local Cargo target")
    if os.environ.get("CARGO_BUILD_TARGET"):
        raise ValueError("Experiment requires the default Windows MSVC target")
    if os.environ.get("PERL_CACHE_HIT") != "true":
        raise ValueError("Comparison requires the exact installed-distribution cache")
    for variable in ("RUSTC_WRAPPER", "RUSTC_WORKSPACE_WRAPPER", "OPENSSL_NO_VENDOR"):
        assert not os.environ.get(variable), f"Unexpected experiment input: {variable}"
    root = Path("C:/Strawberry")
    assert Path(os.environ["OPENSSL_SRC_PERL"]).resolve() == (root / "perl/bin/perl.exe").resolve()
    output = ROOT / "perl-cold-measurements" / args.case
    output.mkdir(parents=True, exist_ok=False)
    manifest = ROOT / "src-tauri/Cargo.toml"
    lock = ROOT / "src-tauri/Cargo.lock"
    inputs = {str(path.relative_to(ROOT)): digest(path.read_bytes()) for path in
              (manifest, lock, ROOT / "src-tauri/src/db/encrypted.rs")}
    packages = {p["name"]: p["version"] for p in tomllib.loads(lock.read_text())["package"]
                if p["name"] in ("openssl-src", "openssl-sys", "libsqlite3-sys")}
    assert packages["openssl-src"] == "300.6.1+3.6.3"
    write(output / "source.json", {
        "head": os.environ["GITHUB_SHA"], "run_id": int(os.environ["GITHUB_RUN_ID"]),
        "run_attempt": int(os.environ["GITHUB_RUN_ATTEMPT"]), "case": args.case,
        "image": os.environ["ImageVersion"], "image_os": os.environ["ImageOS"],
        "perl_cache_hit": os.environ["PERL_CACHE_HIT"],
        "input_hashes": inputs, "native_packages": packages,
        "excluded_prefixes": list(PREFIXES) if args.case == "reduced" else [],
        "scope": "Cold native correctness only; elapsed times are diagnostics, not a speed comparison.",
    })
    original = inventory.inventory(root)
    original_identity = inventory.perl_identity(root)
    (output / "original-inventory.json.gz").write_bytes(gzip.compress(json.dumps(original).encode(), mtime=0))
    try:
        with distribution_variant(root, args.case == "reduced"):
            active = inventory.inventory(root)
            expected = [row for row in original["files"] if args.case == "full" or
                        not row["path"].startswith(tuple(p + "/" for p in PREFIXES))]
            assert active["files"] == expected, "Unexpected change outside selected exclusions"
            assert inventory.perl_identity(root) == original_identity
            write(output / "active-tree.json", {
                "original_tree_sha256": tree_digest(original["files"]),
                "active_tree_sha256": tree_digest(active["files"]),
                "original_files": original["total_files"], "active_files": active["total_files"],
                "original_bytes": original["total_bytes"], "active_bytes": active["total_bytes"],
                "perl_identity": original_identity, "only_selected_exclusions": True,
            })
            print(json.dumps({"case": args.case, "phase": "tree_verified", "files": active["total_files"]}), flush=True)
            execute(CARGO + ["fetch", "--locked", "--manifest-path", str(manifest)], output, "fetch.log")
            execute(CARGO + ["clean", "--manifest-path", str(manifest), "--release", "-p", "openssl-src",
                             "-p", "openssl-sys", "-p", "libsqlite3-sys"], output, "clean.log")
            timings = target / "cargo-timings"
            previous = set(timings.glob("cargo-timing-*.html"))
            elapsed = execute(CARGO + ["test", "--locked", "--manifest-path", str(manifest), "--release", "--lib",
                                       TEST, "--timings", "--", "--exact", "--nocapture"], output, "test.log")
            reports = set(timings.glob("cargo-timing-*.html")) - previous
            assert len(reports) == 1, "Missing or ambiguous native timing report"
            report = reports.pop().read_text()
            (output / "cargo-timing.html").write_text(report)
            match = re.search(r"const UNIT_DATA = (\[.*?\]);", report, re.S)
            assert match, "Compiler timing units missing"
            units = json.loads(match.group(1))
            native = [u for u in units if u["name"] in ("openssl-sys", "libsqlite3-sys") and u["mode"] == "run-custom-build"]
            assert {u["name"] for u in native} == {"openssl-sys", "libsqlite3-sys"}
            assert all(u["duration"] > 0 for u in native), "Native builds were not fresh"
            log = (output / "test.log/build-tail.log").read_text(errors="replace")
            assert f"test {TEST} ... ok" in log
            assert "test result: ok. 1 passed; 0 failed; 0 ignored" in log
            write(output / "result.json", {"test": TEST, "passed": True, "command_seconds": elapsed,
                                            "fresh_native_units": native})
            print(json.dumps({"case": args.case, "phase": "cold_cipher_test_passed"}), flush=True)
    finally:
        restored = inventory.inventory(root)
        unchanged = all(digest((ROOT / name).read_bytes()) == expected for name, expected in inputs.items())
        write(output / "restoration.json", {"full_tree_restored": restored["files"] == original["files"],
                                             "source_inputs_unchanged": unchanged,
                                             "tree_sha256": tree_digest(restored["files"])})
        assert restored["files"] == original["files"] and unchanged, "Experiment input restoration failed"


if __name__ == "__main__":
    main()
