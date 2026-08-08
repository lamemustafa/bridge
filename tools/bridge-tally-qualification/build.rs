use std::process::Command;

fn main() {
    let rustc = std::env::var_os("RUSTC").expect("Cargo must provide RUSTC");
    let output = Command::new(rustc)
        .arg("--version")
        .output()
        .expect("rustc --version must run");
    assert!(output.status.success(), "rustc --version must succeed");
    let version = String::from_utf8(output.stdout).expect("rustc version must be UTF-8");
    println!(
        "cargo:rustc-env=BRIDGE_QUALIFICATION_RUSTC_VERSION={}",
        version.trim()
    );
    println!(
        "cargo:rustc-env=BRIDGE_QUALIFICATION_TARGET={}",
        std::env::var("TARGET").expect("Cargo must provide TARGET")
    );
    println!(
        "cargo:rustc-env=BRIDGE_QUALIFICATION_PROFILE={}",
        std::env::var("PROFILE").expect("Cargo must provide PROFILE")
    );
    if let Ok(commit) = std::env::var("GITHUB_SHA") {
        assert!(
            matches!(commit.len(), 40 | 64)
                && commit
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)),
            "GITHUB_SHA must be a lower-case Git object ID"
        );
        println!("cargo:rustc-env=BRIDGE_QUALIFICATION_COMMIT={commit}");
    }
    println!("cargo:rerun-if-env-changed=RUSTC");
    println!("cargo:rerun-if-env-changed=GITHUB_SHA");
}
