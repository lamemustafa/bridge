use std::env;
use std::io::Read;
use std::process;

use bridge_lib::dsc::{run_probe_child_from_args, run_probe_isolated};

fn main() {
    let status = std::thread::Builder::new()
        .name("dsc-probe-large-stack".to_string())
        .stack_size(64 * 1024 * 1024)
        .spawn(run)
        .expect("failed to spawn probe thread")
        .join()
        .unwrap_or(1);
    process::exit(status);
}

fn run() -> i32 {
    let mut detect_only = false;
    let mut force_load = false;
    let mut probe_child = false;
    let mut token_type = "explicit".to_string();
    let mut library: Option<String> = None;
    let mut pin_stdin = false;

    let mut args = env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--" => {}
            "--detect-only" => detect_only = true,
            "--force-load" => force_load = true,
            "--probe-child" | "--dsc-probe-child" => probe_child = true,
            "--token-type" => {
                token_type = args.next().unwrap_or_else(|| "explicit".to_string());
            }
            "--library" => library = args.next(),
            "--pin-stdin" => pin_stdin = true,
            "--help" | "-h" => {
                println!(
                    "Usage: cargo run --manifest-path src-tauri/Cargo.toml --bin dsc_probe -- [--detect-only] [--force-load] [--library PATH] [--pin-stdin]"
                );
                return 0;
            }
            unknown => {
                eprintln!("Unknown argument: {unknown}");
                return 2;
            }
        }
    }

    if probe_child {
        let mut child_args = vec![
            "--dsc-probe-child".to_string(),
            "--token-type".to_string(),
            token_type,
        ];
        if let Some(library) = library {
            child_args.push("--library".to_string());
            child_args.push(library);
        }
        if detect_only {
            child_args.push("--detect-only".to_string());
        }
        return run_probe_child_from_args(child_args).unwrap_or(2);
    }

    let pins = if detect_only {
        None
    } else if pin_stdin {
        let mut pin = String::new();
        if let Err(error) = std::io::stdin().take(4096).read_to_string(&mut pin) {
            eprintln!("Failed to read PIN from standard input: {error}");
            return 2;
        }
        while pin.ends_with('\r') || pin.ends_with('\n') {
            pin.pop();
        }
        if pin.is_empty() {
            eprintln!("PIN from standard input must not be empty");
            return 2;
        }
        Some(vec![pin])
    } else {
        eprintln!("Certificate extraction requires --pin-stdin");
        return 2;
    };

    match run_probe_isolated(detect_only, library, pins, force_load) {
        Ok(report) => {
            println!(
                "{}",
                serde_json::to_string_pretty(&report).expect("report should serialize")
            );
            0
        }
        Err(error) => {
            eprintln!("{error}");
            1
        }
    }
}
