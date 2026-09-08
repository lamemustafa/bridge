"""Branch-only compiler experiment. Restore every application input on exit."""
import hashlib
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import time


root = Path.cwd()
output = root / "compiler-cache-measurements"
output.mkdir(exist_ok=True)
manifest = root / "src-tauri/Cargo.toml"
library = root / "src-tauri/src/lib.rs"
config = root / "src-tauri/tauri.conf.json"
asset = root / "public/compiler-cache-control.txt"
probe = root / "src-tauri/src/bin/compiler_cache_probe.rs"
assert not asset.exists() and not probe.exists()
originals = {path: path.read_bytes() for path in (manifest, library, config)}
env = os.environ.copy()
env["CARGO_INCREMENTAL"] = "0"
env["SCCACHE_DIR"] = str(Path(env["RUNNER_TEMP"]) / "bridge-compiler-cache")
env["SCCACHE_CACHE_SIZE"] = "2G"
assert not Path(env["SCCACHE_DIR"]).exists(), "cache must start empty"
rows = []


def execute(command, name, environment=env):
    with (output / name).open("wb") as log:
        subprocess.run(command, env=environment, stdout=log,
                       stderr=subprocess.STDOUT, check=True)


def build(name, cached):
    build_env = env.copy()
    if cached:
        build_env["RUSTC_WRAPPER"] = shutil.which("sccache")
        execute(["sccache", "--zero-stats"], name + "-zero.log")
    else:
        build_env.pop("RUSTC_WRAPPER", None)
    execute(["cargo", "clean", "--manifest-path", str(manifest), "--release", "-p", "bridge"],
            name + "-clean.log", build_env)
    timings = root / "src-tauri/target/cargo-timings"
    previous = set(timings.glob("cargo-timing-*.html"))
    start = time.monotonic()
    execute(["node", "node_modules/@tauri-apps/cli/tauri.js", "build", "--no-bundle", "--", "--timings"],
            name + "-build.log", build_env)
    seconds = time.monotonic() - start
    reports = set(timings.glob("cargo-timing-*.html")) - previous
    assert len(reports) == 1, "expected one new Cargo timing report"
    report = reports.pop().read_text()
    (output / (name + ".html")).write_text(report)
    match = re.search(r"const UNIT_DATA = (\[.*?\]);", report, re.S)
    assert match, "missing compiler timing units"
    units = json.loads(match.group(1))
    stats = None
    if cached:
        stats = json.loads(subprocess.check_output(["sccache", "--show-stats", "--stats-format", "json"], env=env))
        (output / (name + "-stats.json")).write_text(json.dumps(stats, indent=2) + "\n")
    suffix = ".exe" if os.name == "nt" else ""
    witness = json.loads(subprocess.check_output([str(root / ("src-tauri/target/release/compiler_cache_probe" + suffix))], env=env))
    expected = {"title": json.loads(config.read_text())["app"]["windows"][0]["title"],
                "asset_sha256": hashlib.sha256(asset.read_bytes()).hexdigest()}
    row = {"sample": name, "cached": cached, "command_seconds": seconds,
           "inputs": {str(path.relative_to(root)): hashlib.sha256(path.read_bytes()).hexdigest()
                      for path in (manifest, library, config, asset, probe)},
           "bridge_units": [unit for unit in units if unit["name"] == "bridge"],
           "other_compiled_units": [unit for unit in units if unit["name"] != "bridge" and unit["duration"] > 0],
           "observed": witness, "expected": expected, "correct": witness == expected}
    rows.append(row)
    (output / "results.json").write_text(json.dumps(rows, indent=2) + "\n")
    print(json.dumps({key: row[key] for key in ("sample", "command_seconds", "correct")}), flush=True)
    assert witness == expected, "cached context returned stale configuration or asset bytes"


try:
    execute(["rustc", "--version", "--verbose"], "rustc-version.txt")
    execute(["sccache", "--version"], "sccache-version.txt")
    # All samples use the same rlib prerequisite; the experiment isolates caching.
    old = b'crate-type = ["staticlib", "cdylib", "rlib"]'
    assert originals[manifest].count(old) == 1
    manifest.write_bytes(originals[manifest].replace(old, b'crate-type = ["rlib"]'))
    old = b'.run(tauri::generate_context!())'
    assert originals[library].count(old) == 1
    library.write_bytes(originals[library].replace(old, b'.run(compiler_cache_context())') + b'''

// Experiment-only: both the application and witness consume this cached context.
pub fn compiler_cache_context() -> tauri::Context<tauri::Wry> {
    tauri::generate_context!()
}
''')
    probe.write_text('''use sha2::{Digest, Sha256};

fn main() {
    let context = bridge_lib::compiler_cache_context();
    let bytes = context.assets().get(&"/compiler-cache-control.txt".into()).expect("control asset missing");
    println!("{}", serde_json::json!({
        "title": context.config().app.windows[0].title,
        "asset_sha256": format!("{:x}", Sha256::digest(bytes.as_ref()))
    }));
}
''')
    asset.parent.mkdir(exist_ok=True)
    asset.write_bytes(b"compiler-cache-asset-original\n")
    # First sample also primes any missing dependency artifacts; report that cost separately.
    build("1-uncached", False)
    build("2-cache-fill", True)
    build("3-cache-hit", True)
    changed = json.loads(config.read_text())
    changed["app"]["windows"][0]["title"] = "Compiler Cache Changed Config"
    config.write_text(json.dumps(changed, indent=2) + "\n")
    build("4-config-change", True)
    asset.write_bytes(b"compiler-cache-asset-changed\n")
    build("5-asset-change", True)
finally:
    for path, contents in originals.items():
        path.write_bytes(contents)
    asset.unlink(missing_ok=True)
    probe.unlink(missing_ok=True)
    restored = all(path.read_bytes() == contents for path, contents in originals.items())
    (output / "restoration.json").write_text(json.dumps({"application_inputs_restored": restored}) + "\n")
    assert restored
