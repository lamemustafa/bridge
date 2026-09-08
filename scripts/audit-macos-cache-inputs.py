"""Read selected compiler cache metadata; never build, clean, save or use credentials."""
import hashlib
import json
import os
from pathlib import Path
import subprocess

output = Path("cache-input-audit")
output.mkdir(exist_ok=True)
identity = {
    "source_sha": subprocess.check_output(["git", "rev-parse", "HEAD"], text=True).strip(),
    "run_id": os.environ["GITHUB_RUN_ID"],
    "run_attempt": os.environ["GITHUB_RUN_ATTEMPT"],
    "exact_cache_hit": os.environ.get("DEPENDENCY_CACHE_EXACT_HIT"),
    "image": {key: os.environ.get(key) for key in ("ImageOS", "ImageVersion")},
}
(output / "identity.json").write_text(json.dumps(identity, indent=2) + "\n")
assert identity["exact_cache_hit"] == "true", "expected existing packaging cache was not restored exactly"
allowed = {"MACOSX_DEPLOYMENT_TARGET", "SDKROOT", "DEVELOPER_DIR", "CC", "CFLAGS", "CXX", "CXXFLAGS", "AR"}
rows = []
for package in ("openssl-sys", "aws-lc-sys", "libsqlite3-sys", "objc2-exception-helper"):
    paths = sorted(Path("src-tauri/target/release/.fingerprint").glob(package + "-*/run-build-script-*.json"))
    assert paths, f"missing cached build inputs for {package}"
    for path in paths:
        assert path.is_file() and not path.is_symlink()
        raw = path.read_bytes()
        parsed = json.loads(raw)
        variables = []
        for entry in parsed["local"]:
            item = entry.get("RerunIfEnvChanged")
            if item and item["var"] in allowed:
                variables.append(item)
        rows.append({"package": package, "path": path.as_posix(),
                     "fingerprint_sha256": hashlib.sha256(raw).hexdigest(), "variables": variables})
(output / "recorded-inputs.json").write_text(json.dumps(rows, indent=2) + "\n")
print(json.dumps({"cached_native_fingerprints": len(rows), "packages": sorted({row["package"] for row in rows})}))
