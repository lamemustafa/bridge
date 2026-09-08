"""Bind the transported cache bytes to this experiment's source and toolchain."""
import hashlib
import json
import os
import subprocess


def identity():
    return {
        "source_sha": subprocess.check_output(["git", "rev-parse", "HEAD"], text=True).strip(),
        "run_id": os.environ["GITHUB_RUN_ID"],
        "run_attempt": os.environ["GITHUB_RUN_ATTEMPT"],
        "cache_key": os.environ["BRIDGE_COMPILER_CACHE_KEY"],
        "rustc": subprocess.check_output(["rustc", "--version", "--verbose"], text=True),
        "runner": {key: os.environ.get(key) for key in ("RUNNER_OS", "RUNNER_ARCH", "ImageOS", "ImageVersion")},
    }


def inventory(root):
    assert root.is_dir() and not root.is_symlink(), "cache root must be a regular directory"
    files = {}
    for path in sorted(root.rglob("*")):
        assert not path.is_symlink(), "cache payload must not contain symlinks"
        assert path.is_dir() or path.is_file(), "unexpected cache entry type"
        if path.is_file():
            with path.open("rb") as stream:
                digest = hashlib.file_digest(stream, "sha256").hexdigest()
            files[path.relative_to(root).as_posix()] = {"sha256": digest, "bytes": path.stat().st_size}
    assert files, "compiler cache is empty"
    return files


def seal(transfer, output):
    files = inventory(transfer / "cache")
    result = {"identity": identity(), "files": files,
              "file_count": len(files), "payload_bytes": sum(item["bytes"] for item in files.values())}
    data = json.dumps(result, indent=2) + "\n"
    (transfer / "payload.json").write_text(data)
    (output / "payload-producer.json").write_text(data)


def verify(transfer, output):
    manifest = transfer / "payload.json"
    assert manifest.is_file() and not manifest.is_symlink(), "cache manifest must be a regular file"
    result = json.loads(manifest.read_text())
    assert result["identity"] == identity(), "cache provenance or runner image differs"
    files = inventory(transfer / "cache")
    assert result["files"] == files, "transported cache bytes differ"
    assert result["file_count"] == len(files)
    assert result["payload_bytes"] == sum(item["bytes"] for item in files.values())
    (output / "payload-consumer.json").write_text(json.dumps(result, indent=2) + "\n")
