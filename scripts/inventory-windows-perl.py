#!/usr/bin/env python3
"""Read the installed pinned Perl tree; never alter its files or cache."""
import argparse
from collections import defaultdict
import gzip
import hashlib
import json
import os
from pathlib import Path
import re
import stat
import subprocess
import time

MAX_FILES = 50000
MAX_BYTES = 4 * 1024**3


def reject_link(path):
    info = path.lstat()
    if stat.S_ISLNK(info.st_mode) or getattr(info, "st_file_attributes", 0) & 0x400:
        raise ValueError("Perl inventory refuses symlinks and Windows reparse points")
    return info


def inventory(root):
    reject_link(root)
    if not root.is_dir():
        raise ValueError("Perl inventory root is not a directory")
    files = []
    groups = defaultdict(lambda: {"files": 0, "bytes": 0})
    total = 0

    def walk_error(error):
        raise error

    for directory, directories, names in os.walk(root, onerror=walk_error, followlinks=False):
        for name in directories:
            reject_link(Path(directory) / name)
        for name in names:
            path = Path(directory) / name
            before = reject_link(path)
            if not stat.S_ISREG(before.st_mode):
                raise ValueError("Perl inventory encountered a non-regular file")
            if len(files) >= MAX_FILES or total + before.st_size > MAX_BYTES:
                raise ValueError("Perl inventory exceeded its file/byte bound")
            digest = hashlib.sha256()
            read_bytes = 0
            with path.open("rb") as stream:
                for chunk in iter(lambda: stream.read(1024 * 1024), b""):
                    digest.update(chunk)
                    read_bytes += len(chunk)
            after = reject_link(path)
            if (before.st_size, before.st_mtime_ns, before.st_ino) != (
                after.st_size, after.st_mtime_ns, after.st_ino
            ) or read_bytes != before.st_size:
                raise ValueError("Perl inventory file changed during hashing")
            relative = path.relative_to(root).as_posix()
            parts = relative.split("/")
            group = "/".join(parts[:2]) if len(parts) > 2 else parts[0]
            files.append({"path": relative, "bytes": read_bytes, "sha256": digest.hexdigest()})
            groups[group]["files"] += 1
            groups[group]["bytes"] += read_bytes
            total += read_bytes
    if not files:
        raise ValueError("Perl inventory root is empty")
    return {"total_files": len(files), "total_bytes": total,
            "groups": dict(sorted(groups.items())), "files": sorted(files, key=lambda row: row["path"])}


def perl_identity(root):
    perl = root / "perl/bin/perl.exe"
    for path in (root, root / "perl", root / "perl/bin", perl):
        reject_link(path)
    code = ('print JSON::PP::encode_json({version=>"$^V", arch=>$Config{archname}, '
            'module=>$INC{"Locale/Maketext/Simple.pm"}})')
    result = subprocess.run([str(perl), "-MConfig", "-MJSON::PP", "-MLocale::Maketext::Simple", "-e", code],
                            capture_output=True, text=True, timeout=30, check=True)
    if len(result.stdout) > 4096 or result.stderr:
        raise ValueError("Unexpected Perl identity output")
    identity = json.loads(result.stdout)
    if identity["version"] != "v5.42.2":
        raise ValueError("Pinned Perl runtime version mismatch")
    module = Path(identity.pop("module")).resolve()
    identity["module_path"] = module.relative_to(root.resolve()).as_posix()
    return identity


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--root", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    if os.name != "nt":
        raise ValueError("Installed-runtime inventory requires Windows")
    if args.output.resolve().is_relative_to(args.root.resolve()):
        raise ValueError("Inventory output must be outside the distribution")
    commit = os.environ["GITHUB_SHA"]
    run_id = os.environ["GITHUB_RUN_ID"]
    if not re.fullmatch(r"[0-9a-f]{40}", commit) or not run_id.isdigit():
        raise ValueError("Missing source/run identity")
    started = time.monotonic()
    identity = perl_identity(args.root)
    result = inventory(args.root)
    module = identity["module_path"]
    if module not in {row["path"] for row in result["files"]}:
        raise ValueError("Validated Perl module is absent from the inventory")
    result.update({
        "schema_version": 1, "source_sha": commit, "run_id": int(run_id),
        "run_attempt": int(os.environ["GITHUB_RUN_ATTEMPT"]),
        "runner_image": os.environ["ImageVersion"], "runner_os": os.environ["ImageOS"],
        "configured_distribution_version": "5.42.2.1", "perl_identity": identity,
        "cache_key": "bridge-windows-strawberryperl-5.42.2.1-v1",
        "cache_hit": os.environ["PERL_CACHE_HIT"],
        "inventory_seconds": round(time.monotonic() - started, 6),
        "scope": "Installed distribution inventory and prerequisite identity only; no reduced cache or native-build qualification.",
    })
    if result["cache_hit"] not in ("true", "false", ""):
        raise ValueError("Unexpected cache restore output")
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_bytes(gzip.compress((json.dumps(result, indent=2) + "\n").encode(), mtime=0))
    print(json.dumps({key: result[key] for key in
                      ("source_sha", "run_id", "total_files", "total_bytes", "cache_hit", "perl_identity", "inventory_seconds")}))


if __name__ == "__main__":
    main()
