"""Branch-only executable context experiment; restore application inputs on exit."""
import hashlib
import importlib.util
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
main = root / "src-tauri/src/main.rs"
build_script = root / "src-tauri/build.rs"
lockfile = root / "src-tauri/Cargo.lock"
asset = root / "public/compiler-cache-control.txt"
assert not asset.exists()
originals = {path: path.read_bytes() for path in (manifest, library, config, main, build_script, lockfile)}
env = os.environ.copy()
env["CARGO_INCREMENTAL"] = "0"
env["SCCACHE_DIR"] = str(Path(env["RUNNER_TEMP"]) / "bridge-compiler-cache")
env["SCCACHE_CACHE_SIZE"] = "2G"
assert not Path(env["SCCACHE_DIR"]).exists(), "cache must start empty"
rows = []
source_value = 1
spec = importlib.util.spec_from_file_location("capture", root / "scripts/capture-package-log.py")
capture = importlib.util.module_from_spec(spec)
spec.loader.exec_module(capture)


def execute(command, name, environment=env):
    with subprocess.Popen(command, env=environment, stdout=subprocess.PIPE,
                          stderr=subprocess.STDOUT) as process:
        capture.capture(process.stdout, output / name)
        code = process.wait()
        if code:
            raise subprocess.CalledProcessError(code, command)


def build(name, cached, require_correct=True):
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
    execute(["node", "node_modules/@tauri-apps/cli/tauri.js", "build", "--no-bundle", "--", "--locked", "--timings"],
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
    witness = json.loads(subprocess.check_output([str(root / ("src-tauri/target/release/bridge" + suffix)), "--compiler-cache-witness"], env=env))
    expected = {"title": json.loads(config.read_text())["app"]["windows"][0]["title"],
                "asset_sha256": hashlib.sha256(asset.read_bytes()).hexdigest(), "source_value": source_value}
    row = {"sample": name, "cached": cached, "command_seconds": seconds,
           "inputs": {str(path.relative_to(root)): hashlib.sha256(path.read_bytes()).hexdigest()
                      for path in (manifest, library, config, main, build_script, lockfile, asset)},
           "bridge_units": [unit for unit in units if unit["name"] == "bridge"],
           "other_compiled_units": [unit for unit in units if unit["name"] != "bridge" and unit["duration"] > 0],
           "observed": witness, "expected": expected, "correct": witness == expected}
    rows.append(row)
    (output / "results.json").write_text(json.dumps(rows, indent=2) + "\n")
    print(json.dumps({key: row[key] for key in ("sample", "command_seconds", "correct")}), flush=True)
    if require_correct:
        assert witness == expected, "context returned stale configuration or asset bytes"


try:
    execute(["rustc", "--version", "--verbose"], "rustc-version.txt")
    execute(["sccache", "--version"], "sccache-version.txt")
    # All samples use the same rlib prerequisite; the experiment isolates caching.
    old = b'crate-type = ["staticlib", "cdylib", "rlib"]'
    assert originals[manifest].count(old) == 1
    manifest.write_bytes(originals[manifest].replace(old, b'crate-type = ["rlib"]'))
    old_run = b'pub fn run() {'
    old_context = b'.run(tauri::generate_context!())'
    assert originals[library].count(old_run) == originals[library].count(old_context) == 1
    library.write_bytes(originals[library].replace(
        old_run, b'pub fn run(make_context: fn() -> tauri::Context<tauri::Wry>) {'
    ).replace(old_context, b'.run(make_context())') + b'\n\npub fn compiler_cache_source_value() -> u8 { 1 }\n')
    old_entry = b'bridge_lib::run()'
    assert originals[main].count(old_entry) == 1
    # Context construction remains at the original point inside library startup.
    # The same compiled factory serves normal startup and read-only CLI evidence.
    entry = originals[main].replace(old_entry, b'bridge_lib::run(compiler_cache_context)')
    assert entry.count(b'fn main() {') == 1
    entry = entry.replace(b'fn main() {', b'''fn main() {
    if std::env::args().nth(1).as_deref() == Some("--compiler-cache-witness") {
        use sha2::{Digest, Sha256};
        let context = compiler_cache_context();
        let bytes = context.assets().get(&"/compiler-cache-control.txt".into()).expect("control asset missing");
        println!("{}", serde_json::json!({
            "title": context.config().app.windows[0].title,
            "asset_sha256": Sha256::digest(bytes.as_ref()).iter().map(|byte| format!("{byte:02x}")).collect::<String>(),
            "source_value": bridge_lib::compiler_cache_source_value()
        }));
        return;
    }
''')
    main.write_bytes(entry + b'''

fn compiler_cache_context() -> tauri::Context<tauri::Wry> {
    tauri::generate_context!()
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
    # Retain a rejected cached result, then build the exact changed input without
    # a wrapper. The unwrapped control must pass before interpreting the cache.
    build("4-config-change-cached", True, require_correct=False)
    build("5-config-change-uncached", False)
    asset.write_bytes(b"compiler-cache-asset-changed\n")
    build("6-asset-change-cached", True, require_correct=False)
    build("7-asset-change-uncached", False)
    # A real Rust edit, then one-at-a-time reversions exercise ordinary source
    # invalidation and reuse of earlier contexts rather than only fresh inputs.
    source_value = 2
    library.write_bytes(library.read_bytes().replace(b'compiler_cache_source_value() -> u8 { 1 }', b'compiler_cache_source_value() -> u8 { 2 }'))
    build("8-source-change-cached", True)
    asset.write_bytes(b"compiler-cache-asset-original\n")
    build("9-asset-restored-cached", True)
    config.write_bytes(originals[config])
    build("10-config-restored-cached", True)
    source_value = 1
    library.write_bytes(library.read_bytes().replace(b'compiler_cache_source_value() -> u8 { 2 }', b'compiler_cache_source_value() -> u8 { 1 }'))
    build("11-source-restored-cached", True)
    summary = {"controls_complete": True, "candidate_correct": all(row["correct"] for row in rows),
               "source_sha": subprocess.check_output(["git", "rev-parse", "HEAD"], text=True).strip(),
               "run_id": env.get("GITHUB_RUN_ID"), "run_attempt": env.get("GITHUB_RUN_ATTEMPT"),
               "rejected_samples": [row["sample"] for row in rows if not row["correct"]]}
    (output / "decision.json").write_text(json.dumps(summary, indent=2) + "\n")
    assert summary["candidate_correct"], "compiler-cache candidate rejected; uncached controls completed"
finally:
    for path, contents in originals.items():
        path.write_bytes(contents)
    asset.unlink(missing_ok=True)
    restored = all(path.read_bytes() == contents for path, contents in originals.items())
    (output / "restoration.json").write_text(json.dumps({"application_inputs_restored": restored}) + "\n")
    assert restored
