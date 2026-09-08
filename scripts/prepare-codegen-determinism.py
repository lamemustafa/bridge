"""Apply one attributed upstream fix to a checksum-verified disposable crate."""
import hashlib
import json
import os
from pathlib import Path, PurePosixPath
import subprocess
import tarfile
import tomllib


def prepare(root, output):
    manifest = root / "src-tauri/Cargo.toml"
    lock = tomllib.loads((root / "src-tauri/Cargo.lock").read_text())
    locked = next(p for p in lock["package"] if p["name"] == "tauri-codegen")
    assert locked["version"] == "2.6.3"
    metadata = json.loads(subprocess.check_output([
        "cargo", "metadata", "--locked", "--offline", "--format-version", "1",
        "--manifest-path", str(manifest)]))
    package = next(p for p in metadata["packages"] if p["name"] == "tauri-codegen")
    source = Path(package["manifest_path"]).parent
    archive = source.parents[2] / "cache" / source.parent.name / (source.name + ".crate")
    assert hashlib.sha256(archive.read_bytes()).hexdigest() == locked["checksum"]
    copied = Path(os.environ["RUNNER_TEMP"]) / "bridge-deterministic-tauri-codegen"
    copied.mkdir(exist_ok=False)
    originals = {}
    with tarfile.open(archive) as members:
        for member in members:
            path = PurePosixPath(member.name)
            assert not path.is_absolute() and ".." not in path.parts and path.parts[0] == source.name
            assert member.isdir() or member.isfile()
            if member.isdir():
                continue
            relative = Path(*path.parts[1:])
            target = copied / relative
            target.parent.mkdir(parents=True, exist_ok=True)
            data = members.extractfile(member).read()
            target.write_bytes(data)
            originals[relative.as_posix()] = hashlib.sha256(data).hexdigest()
    patch = root / "scripts/tauri-codegen-determinism.patch"
    subprocess.run(["git", "apply", "--check", str(patch)], cwd=copied, check=True)
    subprocess.run(["git", "apply", str(patch)], cwd=copied, check=True)
    changed = {name: hashlib.sha256((copied / name).read_bytes()).hexdigest()
               for name, digest in originals.items()
               if hashlib.sha256((copied / name).read_bytes()).hexdigest() != digest}
    assert set(changed) == {"src/embedded_assets.rs"}
    (output / "codegen-source-change.json").write_text(json.dumps({
        "crate": "tauri-codegen", "version": locked["version"],
        "archive_sha256": locked["checksum"], "original_files": originals,
        "changed_files": changed,
        "patch_sha256": hashlib.sha256(patch.read_bytes()).hexdigest(),
        "upstream_commit": "29c87c3d3f5bbcf5a7ae9de01af7e6bb738c1d01",
    }, indent=2) + "\n")
    return copied, locked
