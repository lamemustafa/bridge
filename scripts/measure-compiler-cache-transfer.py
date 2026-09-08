"""Fresh-runner cache transfer and no-clean executable context experiment; restore application inputs on exit."""
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import time


phase = os.environ["CACHE_EXPERIMENT_PHASE"]
assert phase in ("writer", "reader")
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
added_asset = root / "public/compiler-cache-added.txt"
assert not asset.exists() and not added_asset.exists()
originals = {path: path.read_bytes() for path in (manifest, library, config, main, build_script, lockfile)}
env = os.environ.copy()
assert not env.get("RUSTC_WRAPPER") and not env.get("RUSTC_WORKSPACE_WRAPPER"), "unexpected inherited wrapper"
env["CARGO_INCREMENTAL"] = "0"
transfer = Path(env["RUNNER_TEMP"]) / "bridge-compiler-transfer"
env["SCCACHE_DIR"] = str(transfer / "cache")
env["SCCACHE_LOCAL_RW_MODE"] = "READ_WRITE" if phase == "writer" else "READ_ONLY"
env["SCCACHE_CACHE_SIZE"] = "2G"
assert not env.get("SCCACHE_GHA_ENABLED") and not env.get("SCCACHE_GHA_VERSION")
assert Path(env["SCCACHE_DIR"]).exists() == (phase == "reader"), "unexpected cache payload state"
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


def build(name, cached, clean=True):
    build_env = env.copy()
    if cached:
        build_env["RUSTC_WORKSPACE_WRAPPER"] = shutil.which("sccache")
        execute(["sccache", "--zero-stats"], name + "-zero.log")
    else:
        build_env.pop("RUSTC_WORKSPACE_WRAPPER", None)
    if clean:
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
                "asset_sha256": hashlib.sha256(asset.read_bytes()).hexdigest(), "source_value": source_value,
                "added_asset_sha256": hashlib.sha256(added_asset.read_bytes()).hexdigest() if added_asset.exists() else None}
    row = {"sample": name, "cached": cached, "command_seconds": seconds, "cleaned_bridge": clean,
           "inputs": {str(path.relative_to(root)): hashlib.sha256(path.read_bytes()).hexdigest() if path.exists() else None
                      for path in (manifest, library, config, main, build_script, lockfile, asset, added_asset)},
           "bridge_units": [unit for unit in units if unit["name"] == "bridge"],
           "other_compiled_units": [unit for unit in units if unit["name"] != "bridge" and unit["duration"] > 0],
           "observed": witness, "expected": expected, "correct": witness == expected}
    rows.append(row)
    (output / "results.json").write_text(json.dumps(rows, indent=2) + "\n")
    print(json.dumps({key: row[key] for key in ("sample", "command_seconds", "correct")}), flush=True)
    assert witness == expected, "context returned stale configuration or asset bytes"
    return stats


def transferred_bridge_hit(first, repeat, other_units):
    # Fresh runners may compile dependencies. The repeat follows an unwrapped
    # build and cleans only Bridge, isolating reuse from the read-only payload.
    return (first["cache_hits"]["counts"].get("Rust", 0) > 0
            and repeat["cache_hits"]["counts"].get("Rust", 0) == 1
            and repeat["compilations"] == 0 and not other_units)


try:
    execute(["rustc", "--version", "--verbose"], "rustc-version.txt")
    execute(["sccache", "--version"], "sccache-version.txt")
    payload_spec = importlib.util.spec_from_file_location("payload", root / "scripts/compiler-cache-payload.py")
    payload = importlib.util.module_from_spec(payload_spec)
    payload_spec.loader.exec_module(payload)
    if phase == "reader":
        payload.verify(transfer, output)
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
            "source_value": bridge_lib::compiler_cache_source_value(),
            "added_asset_sha256": context.assets().get(&"/compiler-cache-added.txt".into()).map(|bytes|
                Sha256::digest(bytes.as_ref()).iter().map(|byte| format!("{byte:02x}")).collect::<String>())
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
    if phase == "writer":
        build("1-unwrapped-control", False)
        fill = build("2-cache-fill", True)["stats"]
        assert not any(count for language, count in fill["cache_misses"]["counts"].items() if language != "Rust"), "native compilation reached the workspace cache"
        stats = build("3-local-repeat", True)["stats"]
        assert stats["cache_hits"]["counts"].get("Rust", 0) > 0 and stats["compilations"] == 0
        execute(["sccache", "--stop-server"], "stop-server.log")
        payload.seal(transfer, output)
        transport_hit = None
    else:
        first = build("1-remote-reuse", True)["stats"]
        build("2-unwrapped-control", False)
        repeat = build("3-remote-repeat", True)["stats"]
        transport_hit = transferred_bridge_hit(first, repeat, rows[-1]["other_compiled_units"])
        changed = json.loads(config.read_text())
        changed["app"]["windows"][0]["title"] = "Compiler Cache Changed Config"
        config.write_text(json.dumps(changed, indent=2) + "\n")
        build("4-config-edit", True, clean=False)
        asset.write_bytes(b"compiler-cache-asset-changed\n")
        build("5-asset-edit", True, clean=False)
        added_asset.write_bytes(b"compiler-cache-added-asset\n")
        build("6-asset-add", True, clean=False)
        added_asset.unlink()
        build("7-asset-remove", True, clean=False)
        source_value = 2
        library.write_bytes(library.read_bytes().replace(b'compiler_cache_source_value() -> u8 { 1 }', b'compiler_cache_source_value() -> u8 { 2 }'))
        build("8-source-edit", True, clean=False)
        asset.write_bytes(b"compiler-cache-asset-original\n")
        build("9-asset-restored", True, clean=False)
        config.write_bytes(originals[config])
        build("10-config-restored", True, clean=False)
        source_value = 1
        library.write_bytes(library.read_bytes().replace(b'compiler_cache_source_value() -> u8 { 2 }', b'compiler_cache_source_value() -> u8 { 1 }'))
        build("11-source-restored", True, clean=False)
        build("12-unchanged-no-clean", True, clean=False)
        execute(["sccache", "--stop-server"], "stop-server.log")
    summary = {"wrapper": "RUSTC_WORKSPACE_WRAPPER", "phase": phase, "transport_hit": transport_hit, "controls_complete": True, "candidate_correct": all(row["correct"] for row in rows),
               "source_sha": subprocess.check_output(["git", "rev-parse", "HEAD"], text=True).strip(),
               "run_id": env.get("GITHUB_RUN_ID"), "run_attempt": env.get("GITHUB_RUN_ATTEMPT"),
               "rejected_samples": [row["sample"] for row in rows if not row["correct"]]}
    (output / "decision.json").write_text(json.dumps(summary, indent=2) + "\n")
    assert summary["candidate_correct"]
    assert phase == "writer" or transport_hit, "fresh-runner compiler-cache reuse not established"
finally:
    for path, contents in originals.items():
        path.write_bytes(contents)
    asset.unlink(missing_ok=True)
    added_asset.unlink(missing_ok=True)
    restored = all(path.read_bytes() == contents for path, contents in originals.items())
    (output / "restoration.json").write_text(json.dumps({"application_inputs_restored": restored}) + "\n")
    assert restored
