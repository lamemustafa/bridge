"""Disposable Windows archive round trips; never save a production cache."""
from contextlib import contextmanager
import gzip
import hashlib
import importlib.util
import json
import os
from pathlib import Path, PurePosixPath
import shutil
import stat
import subprocess
import tarfile
import tempfile

ROOT = Path(__file__).resolve().parents[1]
spec = importlib.util.spec_from_file_location("perl_cold", ROOT / "scripts/measure-perl-cold.py")
cold = importlib.util.module_from_spec(spec)
spec.loader.exec_module(cold)
SEQUENCE = ("full", "reduced", "full", "reduced", "reduced", "full")


def sha(path):
    digest = hashlib.sha256()
    with path.open("rb") as stream:
        for block in iter(lambda: stream.read(1024 * 1024), b""):
            digest.update(block)
    return digest.hexdigest()


@contextmanager
def parked_installation(root):
    """Preserve original bytes and refuse to overwrite an unexpected replacement."""
    cold.inventory.reject_link(root)
    backup = Path(tempfile.mkdtemp(prefix="bridge-perl-archive-", dir=root.parent))
    original = backup / "original"
    root.rename(original)
    try:
        yield
    finally:
        if os.path.lexists(root):
            raise ValueError("Refusing to overwrite replacement installation; original remains in backup")
        original.rename(root)
        backup.rmdir()


@contextmanager
def extraction_destination(root):
    root.mkdir(exist_ok=False)
    identity = cold.inventory.reject_link(root).st_ino
    try:
        yield
    finally:
        current = cold.inventory.reject_link(root)
        if not root.is_dir() or current.st_ino != identity:
            raise ValueError("Extraction destination was replaced; refusing cleanup")
        # Only this freshly created disposable destination is removed. Refuse
        # reparse points or unexpected filesystem objects before recursive cleanup.
        count = 0
        def fail(error):
            raise error
        for directory, directories, files in os.walk(root, onerror=fail, followlinks=False):
            for name in directories + files:
                info = cold.inventory.reject_link(Path(directory) / name)
                count += 1
                if count > 100000 or not (stat.S_ISDIR(info.st_mode) or stat.S_ISREG(info.st_mode)):
                    raise ValueError("Unexpected extraction cleanup entry")
        shutil.rmtree(root)


class BlockReader:
    """Track the physical stream position without hiding tar-reader lookahead."""
    def __init__(self, stream):
        self.stream, self.count, self.tail = stream, 0, b""

    def read(self, size):
        data = self.stream.read(size)
        self.count += len(data)
        self.tail = (self.tail + data)[-512:]
        return data


def verify_tar(stream, expected, prefix):
    expected = {row["path"]: row for row in expected}
    seen = set()
    members, last_end = 0, 0
    reader = BlockReader(stream)
    with tarfile.open(fileobj=reader, mode="r|", bufsize=512) as archive:
        for member in archive:
            members += 1
            last_end = member.offset_data + ((member.size + 511) // 512) * 512
            if members > 100000:
                raise ValueError("Archive member bound exceeded")
            name = member.name.replace("\\", "/").rstrip("/")
            if name == prefix and member.isdir():
                continue
            if not name.startswith(prefix + "/"):
                raise ValueError("Archive member escaped the installation prefix")
            relative = name[len(prefix) + 1:]
            if ".." in relative.split("/") or PurePosixPath(relative).is_absolute():
                raise ValueError("Unsafe archive member")
            if member.isdir():
                continue
            if not member.isfile() or relative not in expected or relative in seen:
                raise ValueError("Unexpected archive file or link")
            row = expected[relative]
            if member.size != row["bytes"]:
                raise ValueError("Archive file size mismatch")
            digest = hashlib.sha256()
            content = archive.extractfile(member)
            for block in iter(lambda: content.read(1024 * 1024), b""):
                digest.update(block)
            if digest.hexdigest() != row["sha256"]:
                raise ValueError("Archive file hash mismatch")
            seen.add(relative)
    if seen != set(expected):
        raise ValueError("Archive is missing expected files")
    # Block-sized reads leave no tarfile lookahead after the first zero block.
    # Require both end blocks, then inspect every remaining padding byte.
    if reader.count != last_end + 512 or reader.tail != bytes(512):
        raise ValueError("Missing or ambiguous first archive end block")
    padding = 0
    for block in iter(lambda: stream.read(1024 * 1024), b""):
        padding += len(block)
        if padding > 1024 * 1024 or any(block):
            raise ValueError("Unexpected archive padding")
    if padding < 512 or padding % 512:
        raise ValueError("Missing or incomplete second archive end block")
    return {"verified_files": len(seen), "members": members}


def verify_archive(archive, expected, zstd, prefix):
    process = subprocess.Popen([str(zstd), "-q", "-d", "-c", str(archive)],
                               stdout=subprocess.PIPE, stderr=subprocess.DEVNULL)
    try:
        with process.stdout:
            result = verify_tar(process.stdout, expected, prefix)
        status = process.wait(timeout=30)
        if status:
            raise subprocess.CalledProcessError(status, process.args)
        return result
    finally:
        if process.poll() is None:
            process.kill()
            process.wait()


def main():
    if os.name != "nt" or os.environ.get("PERL_CACHE_HIT") != "true":
        raise ValueError("Windows and the exact installed-Perl cache are required")
    root = Path("C:/Strawberry")
    tar = Path("C:/Program Files/Git/usr/bin/tar.exe")
    located = shutil.which("zstd")
    if located is None:
        raise ValueError("Pinned runner zstd is missing")
    zstd = Path(located)
    tools = {}
    for name, path in (("tar", tar), ("zstd", zstd)):
        if path.resolve().is_relative_to(root.resolve()):
            raise ValueError("Archive tools must remain available while the installation is parked")
        cold.inventory.reject_link(path)
        result = subprocess.run([str(path), "--version"], capture_output=True, check=True, timeout=30)
        if len(result.stdout) + len(result.stderr) > 8192:
            raise ValueError("Unexpected archive tool version output")
        tools[name] = {"path": str(path), "sha256": sha(path), "version": result.stdout.decode(errors="replace")}
    if "GNU tar" not in tools["tar"]["version"]:
        raise ValueError("Comparison requires the existing GNU tar path")
    output = ROOT / "perl-archive-measurements"
    output.mkdir(exist_ok=False)
    original = cold.inventory.inventory(root)
    identity = cold.inventory.perl_identity(root)
    sources = {str(p.relative_to(ROOT)).replace("\\", "/"): sha(p) for p in
               (Path(__file__), ROOT / "scripts/measure-perl-cold.py", ROOT / "scripts/inventory-windows-perl.py",
                ROOT / "scripts/capture-package-log.py")}
    cold.write(output / "source.json", {
        "head": os.environ["GITHUB_SHA"], "run_id": int(os.environ["GITHUB_RUN_ID"]),
        "run_attempt": int(os.environ["GITHUB_RUN_ATTEMPT"]), "image": os.environ["ImageVersion"],
        "image_os": os.environ["ImageOS"], "tools": tools, "source_hashes": sources,
        "original_tree_sha256": cold.tree_digest(original["files"]), "perl_identity": identity,
        "sequence": list(SEQUENCE), "scope": "Local archive extraction with warmed filesystem caches; no network, full CI speedup or production adoption claim.",
    })
    (output / "original-inventory.json.gz").write_bytes(gzip.compress(json.dumps(original).encode(), mtime=0))
    archives = {}
    try:
        # Archives remain local to the disposable runner; only metadata/logs ship.
        with tempfile.TemporaryDirectory(prefix="bridge-perl-archives-", dir=root.parent) as temporary:
            workspace = Path(temporary)
            for case in ("full", "reduced"):
                expected = [r for r in original["files"] if case == "full" or
                            not r["path"].startswith(tuple(p + "/" for p in cold.PREFIXES))]
                archive = workspace / f"{case}.tzst"
                manifest = workspace / f"{case}-manifest.txt"
                manifest.write_text(root.as_posix() + "\n", encoding="utf-8")
                with cold.distribution_variant(root, case == "reduced"):
                    assert cold.inventory.inventory(root)["files"] == expected
                    seconds = cold.execute([str(tar), "--posix", "-cf", archive.as_posix(), "--exclude", archive.as_posix(),
                                            "-P", "-C", ROOT.as_posix(), "--files-from", manifest.as_posix(),
                                            "--force-local", "--use-compress-program", "zstd -T0"], output, f"pack-{case}")
                assert cold.inventory.inventory(root)["files"] == original["files"]
                if archive.stat().st_size > 2 * 1024**3:
                    raise ValueError("Archive size bound exceeded")
                verified = verify_archive(archive, expected, zstd, root.as_posix())
                archives[case] = {"path": archive, "expected": expected, "sha256": sha(archive)}
                cold.write(output / f"archive-{case}.json", {
                    "case": case, "archive_bytes": archive.stat().st_size, "archive_sha256": archives[case]["sha256"],
                    "packing_seconds": seconds, "input_tree_sha256": cold.tree_digest(expected), **verified,
                })
            samples = []
            with parked_installation(root):
                for index, case in enumerate(SEQUENCE, 1):
                    selected = archives[case]
                    if sha(selected["path"]) != selected["sha256"]:
                        raise ValueError("Archive changed between trials")
                    with extraction_destination(root):
                        seconds = cold.execute([str(tar), "-xf", selected["path"].as_posix(), "-P", "-C", ROOT.as_posix(),
                                                "--force-local", "--use-compress-program", "zstd -d"], output, f"extract-{index}-{case}")
                        observed = cold.inventory.inventory(root)
                        assert observed["files"] == selected["expected"], "Extracted bytes differ from the archive input"
                        assert cold.inventory.perl_identity(root) == identity
                        samples.append({"index": index, "case": case, "extraction_seconds": seconds,
                                        "archive_sha256": selected["sha256"], "tree_sha256": cold.tree_digest(observed["files"]),
                                        "files": observed["total_files"], "bytes": observed["total_bytes"], "identity_verified": True})
                        cold.write(output / "samples.json", samples)
                        print(json.dumps(samples[-1]), flush=True)
            assert cold.inventory.inventory(root)["files"] == original["files"]
            cold.write(output / "result.json", {"passed": True, "round_trips": len(samples), "production_cache_saved": False})
    finally:
        restored = cold.inventory.inventory(root)
        unchanged = all(sha(ROOT / name) == value for name, value in sources.items())
        tools_same = all(sha(Path(row["path"])) == row["sha256"] for row in tools.values())
        cold.write(output / "restoration.json", {"full_tree_restored": restored["files"] == original["files"],
                                                 "source_inputs_unchanged": unchanged, "tools_unchanged": tools_same,
                                                 "tree_sha256": cold.tree_digest(restored["files"])})
        assert restored["files"] == original["files"] and unchanged and tools_same


if __name__ == "__main__":
    main()
