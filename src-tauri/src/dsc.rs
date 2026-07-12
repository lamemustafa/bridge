use std::env;
use std::error::Error;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use cryptoki::context::{CInitializeArgs, CInitializeFlags, Pkcs11};
use cryptoki::object::{Attribute, AttributeType, ObjectClass};
use cryptoki::session::UserType;
use cryptoki::types::AuthPin;
use pkcs11::types::{
    CKA_CLASS, CKA_ID, CKA_LABEL, CKA_TOKEN, CKA_VALUE, CKF_RW_SESSION, CKF_SERIAL_SESSION,
    CKO_CERTIFICATE, CKU_USER, CK_ATTRIBUTE, CK_BBOOL, CK_TRUE,
};
use serde::{Deserialize, Serialize};
use sha1::{Digest, Sha1};
use x509_parser::prelude::parse_x509_certificate;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProbeReport {
    pub platform: String,
    pub arch: String,
    pub workspace_root: String,
    pub bundled_library_root: String,
    pub physical_token_hint: Option<String>,
    pub force_load: bool,
    pub detect_only: bool,
    pub attempts: Vec<ProbeAttempt>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProbeAttempt {
    pub token_type: String,
    pub library_path: String,
    pub library_exists: bool,
    pub loaded: bool,
    pub initialized: bool,
    pub slot_count: usize,
    pub token_info: Option<String>,
    pub login_success: bool,
    pub certificate_count: Option<usize>,
    pub certificates: Vec<CertificateSummary>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CertificateSummary {
    pub label: String,
    pub id: Option<String>,
    pub common_name: Option<String>,
    pub organization: Option<String>,
    pub issuer_name: Option<String>,
    pub serial_number: Option<String>,
    pub valid_from: Option<String>,
    pub valid_to: Option<String>,
    pub fingerprint: Option<String>,
    pub parse_error: Option<String>,
}

#[derive(Debug, Clone)]
struct TokenConfig {
    token_type: String,
    library_path: PathBuf,
    pins: Vec<String>,
}

pub fn run_probe_isolated(
    detect_only: bool,
    explicit_library: Option<String>,
    explicit_pins: Option<Vec<String>>,
    force_load: bool,
) -> Result<ProbeReport, Box<dyn Error>> {
    let workspace_root = workspace_root()?;
    let library_root = workspace_root.join("assets").join("lib").join("dsc");
    let physical_token_present = physical_token_hint().is_some();
    let configs = token_configs(&library_root, explicit_library, explicit_pins);

    let attempts = configs
        .iter()
        .map(|config| {
            if !config.library_path.exists() {
                return skipped_attempt(config, "PKCS#11 library does not exist");
            }

            if !physical_token_present && !force_load {
                return skipped_attempt(
                    config,
                    "no physical token detected; skipped native library load",
                );
            }

            run_child_attempt(config, detect_only)
        })
        .collect();

    Ok(ProbeReport {
        platform: env::consts::OS.to_string(),
        arch: env::consts::ARCH.to_string(),
        // Avoid exposing user-specific absolute paths and hardware identifiers to the webview.
        workspace_root: ".".to_string(),
        bundled_library_root: "assets/lib/dsc".to_string(),
        physical_token_hint: physical_token_present
            .then(|| "Physical DSC token detected".to_string()),
        force_load,
        detect_only,
        attempts,
    })
}

pub fn run_single_attempt(
    token_type: String,
    library_path: String,
    pins: Vec<String>,
    detect_only: bool,
) -> ProbeAttempt {
    let config = TokenConfig {
        token_type,
        library_path: PathBuf::from(library_path),
        pins,
    };

    probe_config(&config, detect_only, true, true)
}

pub fn run_probe_child_from_args<I>(args: I) -> Option<i32>
where
    I: IntoIterator<Item = String>,
{
    let mut detect_only = false;
    let mut token_type = "explicit".to_string();
    let mut library: Option<String> = None;
    let mut is_child = false;

    let mut args = args.into_iter();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--probe-child" | "--dsc-probe-child" => is_child = true,
            "--detect-only" => detect_only = true,
            "--token-type" => {
                token_type = args.next().unwrap_or_else(|| "explicit".to_string());
            }
            "--library" => library = args.next(),
            _ => {}
        }
    }

    if !is_child {
        return None;
    }

    let Some(library) = library else {
        eprintln!("--dsc-probe-child requires --library");
        return Some(2);
    };

    let pins = if detect_only {
        Vec::new()
    } else {
        let mut input = String::new();
        if let Err(error) = std::io::stdin().take(8192).read_to_string(&mut input) {
            eprintln!("failed to read DSC probe input: {error}");
            return Some(2);
        }
        match serde_json::from_str::<Vec<String>>(&input) {
            Ok(pins) if pins.len() == 1 && !pins[0].is_empty() => pins,
            _ => {
                eprintln!("DSC probe requires exactly one PIN through standard input");
                return Some(2);
            }
        }
    };

    let attempt = run_single_attempt(token_type, library, pins, detect_only);
    match serde_json::to_string_pretty(&attempt) {
        Ok(json) => {
            println!("{json}");
            Some(0)
        }
        Err(error) => {
            eprintln!("failed to serialize DSC probe attempt: {error}");
            Some(1)
        }
    }
}

fn workspace_root() -> Result<PathBuf, Box<dyn Error>> {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    for candidate in manifest_dir.ancestors() {
        if candidate.join("assets").join("lib").join("dsc").exists() {
            return Ok(candidate.to_path_buf());
        }
    }

    // Source builds may intentionally rely on vendor-installed PKCS#11 drivers rather than
    // redistributing proprietary modules. Keep probing diagnostic instead of failing startup.
    Ok(manifest_dir
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or(manifest_dir))
}

fn token_configs(
    library_root: &Path,
    explicit_library: Option<String>,
    explicit_pins: Option<Vec<String>>,
) -> Vec<TokenConfig> {
    if let Some(library) = explicit_library {
        return vec![TokenConfig {
            token_type: "explicit".to_string(),
            library_path: PathBuf::from(library),
            pins: explicit_pins.unwrap_or_default(),
        }];
    }

    let mut configs = Vec::new();
    // A process-level override supports uncommon vendor installations without exposing native
    // library loading to the Tauri/webview command surface.
    if let Some(library) =
        env::var_os("BRIDGE_DSC_PKCS11_LIBRARY").filter(|value| !value.is_empty())
    {
        configs.push(TokenConfig {
            token_type: "configured".to_string(),
            library_path: PathBuf::from(library),
            pins: Vec::new(),
        });
    }

    configs.extend(match env::consts::OS {
        "macos" => vec![
            TokenConfig {
                token_type: "epass2003".to_string(),
                library_path: library_root.join("libcastle_v2.1.0.0.dylib"),
                pins: Vec::new(),
            },
            TokenConfig {
                token_type: "hyperscu".to_string(),
                library_path: library_root.join("libcastle_v2.1.0.0.dylib"),
                pins: Vec::new(),
            },
            TokenConfig {
                token_type: "watchdata".to_string(),
                library_path: library_root.join("libwdpkcs_Proxkey.dylib"),
                pins: Vec::new(),
            },
            token_config("epass2003", "/usr/local/lib/libcastle_v2.1.0.0.dylib"),
            token_config("epass2003", "/opt/homebrew/lib/libcastle_v2.1.0.0.dylib"),
            token_config("watchdata", "/usr/local/lib/libwdpkcs_Proxkey.dylib"),
            token_config("watchdata", "/opt/homebrew/lib/libwdpkcs_Proxkey.dylib"),
        ],
        "windows" => {
            let system_root = env::var_os("SystemRoot")
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from(r"C:\Windows"));
            let system32 = system_root.join("System32");
            vec![
                TokenConfig {
                    token_type: "watchdata".to_string(),
                    library_path: library_root.join("windows").join("SignatureP11.dll"),
                    pins: Vec::new(),
                },
                TokenConfig {
                    token_type: "epass2003".to_string(),
                    library_path: library_root.join("windows").join("eps2003csp11.dll"),
                    pins: Vec::new(),
                },
                TokenConfig {
                    token_type: "watchdata".to_string(),
                    library_path: system32.join("SignatureP11.dll"),
                    pins: Vec::new(),
                },
                TokenConfig {
                    token_type: "epass2003".to_string(),
                    library_path: system32.join("eps2003csp11.dll"),
                    pins: Vec::new(),
                },
            ]
        }
        _ => vec![
            TokenConfig {
                token_type: "watchdata".to_string(),
                library_path: library_root.join("libwdpkcs_Proxkey.so"),
                pins: Vec::new(),
            },
            TokenConfig {
                token_type: "epass2003".to_string(),
                library_path: library_root.join("libcastle.so"),
                pins: Vec::new(),
            },
            TokenConfig {
                token_type: "hyperscu".to_string(),
                library_path: library_root.join("libcastle.so"),
                pins: Vec::new(),
            },
            token_config("epass2003", "/usr/local/lib/libcastle.so"),
            token_config("epass2003", "/usr/lib/libcastle.so"),
            token_config("watchdata", "/usr/local/lib/libwdpkcs_Proxkey.so"),
            token_config("watchdata", "/usr/lib/libwdpkcs_Proxkey.so"),
        ],
    });

    configs.sort_by(|left, right| left.library_path.cmp(&right.library_path));
    configs.dedup_by(|left, right| left.library_path == right.library_path);

    if let Some(pins) = explicit_pins {
        for config in &mut configs {
            config.pins = pins.clone();
        }
    }

    configs
}

fn token_config(token_type: &str, library_path: impl Into<PathBuf>) -> TokenConfig {
    TokenConfig {
        token_type: token_type.to_string(),
        library_path: library_path.into(),
        pins: Vec::new(),
    }
}

fn skipped_attempt(config: &TokenConfig, error: &str) -> ProbeAttempt {
    ProbeAttempt {
        token_type: config.token_type.clone(),
        library_path: display_library_path(&config.library_path),
        library_exists: config.library_path.exists(),
        loaded: false,
        initialized: false,
        slot_count: 0,
        token_info: None,
        login_success: false,
        certificate_count: None,
        certificates: Vec::new(),
        error: Some(error.to_string()),
    }
}

fn run_child_attempt(config: &TokenConfig, detect_only: bool) -> ProbeAttempt {
    let exe = match probe_child_exe() {
        Ok(exe) => exe,
        Err(error) => {
            return skipped_attempt(
                config,
                &format!("could not resolve probe executable: {error}"),
            )
        }
    };

    let mut command = Command::new(exe);
    command
        .arg("--dsc-probe-child")
        .arg("--token-type")
        .arg(&config.token_type)
        .arg("--library")
        .arg(&config.library_path)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    if detect_only {
        command.arg("--detect-only");
    }

    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(error) => {
            return skipped_attempt(config, &format!("failed to run child process: {error}"))
        }
    };
    if !detect_only {
        let write_result = child
            .stdin
            .take()
            .ok_or_else(|| "child standard input was not available".to_string())
            .and_then(|mut stdin| {
                serde_json::to_writer(&mut stdin, &config.pins)
                    .map_err(|error| format!("failed to serialize probe input: {error}"))?;
                stdin
                    .flush()
                    .map_err(|error| format!("failed to write probe input: {error}"))
            });
        if let Err(error) = write_result {
            let _ = child.kill();
            let _ = child.wait();
            return skipped_attempt(config, &error);
        }
    }

    match child.wait_with_output() {
        Ok(output) if output.status.success() => {
            let stdout = String::from_utf8_lossy(&output.stdout);
            serde_json::from_str::<ProbeAttempt>(stdout.trim()).unwrap_or_else(|error| {
                skipped_attempt(
                    config,
                    &format!("child succeeded but output was not valid JSON: {error}"),
                )
            })
        }
        Ok(output) => {
            let mut attempt = skipped_attempt(config, "child process failed");
            attempt.error = Some(format!(
                "child process exited with status {:?}",
                output.status.code()
            ));
            attempt
        }
        Err(error) => skipped_attempt(
            config,
            &format!("failed to wait for child process: {error}"),
        ),
    }
}

fn probe_child_exe() -> Result<PathBuf, Box<dyn Error>> {
    let current_exe = env::current_exe()?;
    let current_stem = current_exe
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or_default();

    if current_stem == "dsc_probe" {
        return Ok(current_exe);
    }

    if current_exe.exists() {
        return Ok(current_exe);
    }

    let child_name = if cfg!(windows) {
        "dsc_probe.exe"
    } else {
        "dsc_probe"
    };
    let child_exe = current_exe.with_file_name(child_name);

    if child_exe.exists() {
        Ok(child_exe)
    } else {
        Err(format!(
            "expected probe child at {}; build it with `cargo build --manifest-path src-tauri/Cargo.toml --bin dsc_probe`",
            child_exe.display()
        )
        .into())
    }
}

fn probe_config(
    config: &TokenConfig,
    detect_only: bool,
    physical_token_present: bool,
    force_load: bool,
) -> ProbeAttempt {
    let mut attempt = ProbeAttempt {
        token_type: config.token_type.clone(),
        library_path: display_library_path(&config.library_path),
        library_exists: config.library_path.exists(),
        loaded: false,
        initialized: false,
        slot_count: 0,
        token_info: None,
        login_success: false,
        certificate_count: None,
        certificates: Vec::new(),
        error: None,
    };

    if !attempt.library_exists {
        attempt.error = Some("PKCS#11 library does not exist".to_string());
        return attempt;
    }

    if !physical_token_present && !force_load {
        attempt.error = Some("no physical token detected; skipped native library load".to_string());
        return attempt;
    }

    if let Some(low_level_attempt) = probe_config_low_level(config, detect_only) {
        return low_level_attempt;
    }

    let pkcs11 = match Pkcs11::new(&config.library_path) {
        Ok(pkcs11) => {
            attempt.loaded = true;
            pkcs11
        }
        Err(error) => {
            attempt.error = Some(format!("failed to load PKCS#11 library: {error}"));
            return attempt;
        }
    };

    if let Err(error) = pkcs11.initialize(CInitializeArgs::new(CInitializeFlags::OS_LOCKING_OK)) {
        attempt.error = Some(format!("failed to initialize PKCS#11 library: {error}"));
        return attempt;
    }
    attempt.initialized = true;

    let slots = match pkcs11.get_slots_with_token() {
        Ok(slots) => slots,
        Err(error) => {
            attempt.error = Some(format!("failed to get slots with token: {error}"));
            return attempt;
        }
    };
    attempt.slot_count = slots.len();

    let Some(slot) = slots.first().copied() else {
        attempt.error = Some("no token slots found".to_string());
        return attempt;
    };

    if let Ok(token_info) = pkcs11.get_token_info(slot) {
        attempt.token_info = Some(format!("{token_info:?}"));
    }

    if detect_only {
        return attempt;
    }

    if config.pins.is_empty() {
        attempt.error = Some("PIN is required to extract DSC certificates".to_string());
        return attempt;
    }

    let session = match pkcs11.open_rw_session(slot) {
        Ok(session) => session,
        Err(error) => {
            attempt.error = Some(format!("failed to open RW session: {error}"));
            return attempt;
        }
    };

    for pin in &config.pins {
        let auth_pin = AuthPin::new(pin.to_owned().into());
        match session.login(UserType::User, Some(&auth_pin)) {
            Ok(()) => {
                attempt.login_success = true;
                break;
            }
            Err(error) => {
                let message = error.to_string();
                if message.contains("CKR_PIN_LOCKED") || message.to_lowercase().contains("locked") {
                    attempt.error = Some(format!("token PIN appears locked: {message}"));
                    return attempt;
                }
            }
        }
    }

    if !attempt.login_success {
        attempt.error = Some("could not log in with provided PINs".to_string());
        return attempt;
    }

    match count_certificates(&session) {
        Ok(count) => attempt.certificate_count = Some(count),
        Err(error) => {
            attempt.error = Some(format!(
                "login succeeded, but certificate enumeration failed: {error}"
            ));
        }
    }

    let _ = session.logout();
    attempt
}

fn probe_config_low_level(config: &TokenConfig, detect_only: bool) -> Option<ProbeAttempt> {
    let mut attempt = ProbeAttempt {
        token_type: config.token_type.clone(),
        library_path: display_library_path(&config.library_path),
        library_exists: config.library_path.exists(),
        loaded: false,
        initialized: false,
        slot_count: 0,
        token_info: None,
        login_success: false,
        certificate_count: None,
        certificates: Vec::new(),
        error: None,
    };

    let mut ctx = match pkcs11::Ctx::new(&config.library_path) {
        Ok(ctx) => {
            attempt.loaded = true;
            ctx
        }
        Err(error) => {
            attempt.error = Some(format!("low-level PKCS#11 load failed: {error}"));
            return Some(attempt);
        }
    };

    if let Err(error) = ctx.initialize(None) {
        let message = error.to_string();
        if !message.contains("0x191") && !message.contains("CKR_CRYPTOKI_ALREADY_INITIALIZED") {
            attempt.error = Some(format!("low-level PKCS#11 initialize failed: {message}"));
            return Some(attempt);
        }
    }
    attempt.initialized = true;

    let slots = match ctx.get_slot_list(true) {
        Ok(slots) => slots,
        Err(error) => {
            attempt.error = Some(format!("low-level get_slot_list(true) failed: {error}"));
            let _ = ctx.finalize();
            return Some(attempt);
        }
    };
    let slots = if slots.is_empty() {
        match ctx.get_slot_list(false) {
            Ok(all_slots) => all_slots,
            Err(_) => slots,
        }
    } else {
        slots
    };
    attempt.slot_count = slots.len();

    let Some(slot) = slots.first().copied() else {
        attempt.error = Some("no token slots found".to_string());
        let _ = ctx.finalize();
        return Some(attempt);
    };

    if let Ok(token_info) = ctx.get_token_info(slot) {
        attempt.token_info = Some(format!("{token_info:?}"));
    }

    if detect_only {
        let _ = ctx.finalize();
        return Some(attempt);
    }

    if config.pins.is_empty() {
        attempt.error = Some("PIN is required to extract DSC certificates".to_string());
        let _ = ctx.finalize();
        return Some(attempt);
    }

    let session = match ctx.open_session(slot, CKF_SERIAL_SESSION | CKF_RW_SESSION, None, None) {
        Ok(session) => session,
        Err(error) => {
            attempt.error = Some(format!("low-level open_session failed: {error}"));
            let _ = ctx.finalize();
            return Some(attempt);
        }
    };

    for pin in &config.pins {
        match ctx.login(session, CKU_USER, Some(pin)) {
            Ok(()) => {
                attempt.login_success = true;
                break;
            }
            Err(error) => {
                let message = error.to_string();
                if message.contains("CKR_PIN_LOCKED") || message.to_lowercase().contains("locked") {
                    attempt.error = Some(format!("token PIN appears locked: {message}"));
                    let _ = ctx.close_session(session);
                    let _ = ctx.finalize();
                    return Some(attempt);
                }
            }
        }
    }

    if !attempt.login_success {
        attempt.error = Some("could not log in with provided PINs".to_string());
        let _ = ctx.close_session(session);
        let _ = ctx.finalize();
        return Some(attempt);
    }

    match read_certificates_low_level(&ctx, session) {
        Ok(certificates) => {
            attempt.certificate_count = Some(certificates.len());
            attempt.certificates = certificates;
        }
        Err(error) => {
            attempt.error = Some(format!(
                "login succeeded, but low-level certificate extraction failed: {error}"
            ));
        }
    }

    let _ = ctx.logout(session);
    let _ = ctx.close_session(session);
    let _ = ctx.finalize();
    Some(attempt)
}

fn read_certificates_low_level(
    ctx: &pkcs11::Ctx,
    session: pkcs11::types::CK_SESSION_HANDLE,
) -> Result<Vec<CertificateSummary>, Box<dyn Error>> {
    let class = CKO_CERTIFICATE;
    let token: CK_BBOOL = CK_TRUE;
    let template = vec![
        CK_ATTRIBUTE::new(CKA_CLASS).with_ck_ulong(&class),
        CK_ATTRIBUTE::new(CKA_TOKEN).with_bool(&token),
    ];

    ctx.find_objects_init(session, &template)?;
    let handles = ctx.find_objects(session, 25);
    let final_result = ctx.find_objects_final(session);
    let handles = handles?;
    final_result?;

    let mut certificates = Vec::with_capacity(handles.len());
    for handle in handles {
        certificates.push(read_certificate_low_level(ctx, session, handle)?);
    }

    Ok(certificates)
}

fn read_certificate_low_level(
    ctx: &pkcs11::Ctx,
    session: pkcs11::types::CK_SESSION_HANDLE,
    handle: pkcs11::types::CK_OBJECT_HANDLE,
) -> Result<CertificateSummary, Box<dyn Error>> {
    let mut attrs = vec![
        CK_ATTRIBUTE::new(CKA_VALUE),
        CK_ATTRIBUTE::new(CKA_LABEL),
        CK_ATTRIBUTE::new(CKA_ID),
    ];
    let _ = ctx.get_attribute_value(session, handle, &mut attrs)?;

    let mut value = vec![0_u8; attrs[0].ulValueLen as usize];
    let mut label = vec![0_u8; attrs[1].ulValueLen as usize];
    let mut id = vec![0_u8; attrs[2].ulValueLen as usize];
    attrs[0].set_bytes(&value);
    attrs[1].set_bytes(&label);
    attrs[2].set_bytes(&id);
    let _ = ctx.get_attribute_value(session, handle, &mut attrs)?;

    value = attrs[0].get_bytes().unwrap_or_default();
    label = attrs[1].get_bytes().unwrap_or_default();
    id = attrs[2].get_bytes().unwrap_or_default();

    let label = String::from_utf8_lossy(&label)
        .trim_end_matches('\0')
        .trim_end()
        .to_string();
    let id = if id.is_empty() {
        None
    } else {
        Some(hex_upper(&id))
    };

    let mut summary = CertificateSummary {
        label,
        id,
        common_name: None,
        organization: None,
        issuer_name: None,
        serial_number: None,
        valid_from: None,
        valid_to: None,
        fingerprint: None,
        parse_error: None,
    };

    if value.is_empty() {
        summary.parse_error = Some("certificate has no DER value".to_string());
        return Ok(summary);
    }

    if let Err(error) = enrich_certificate_native(&mut summary, &value) {
        summary.parse_error = Some(error.to_string());
    }

    Ok(summary)
}

fn count_certificates(session: &cryptoki::session::Session) -> Result<usize, Box<dyn Error>> {
    let template = vec![Attribute::Class(ObjectClass::CERTIFICATE)];
    let wanted = vec![
        AttributeType::Label,
        AttributeType::Subject,
        AttributeType::Issuer,
    ];
    let mut count = 0;

    for object in session.iter_objects(&template)? {
        let object = object?;
        let _attributes = session.get_attributes(object, &wanted)?;
        count += 1;
    }

    Ok(count)
}

fn enrich_certificate_native(
    summary: &mut CertificateSummary,
    der: &[u8],
) -> Result<(), Box<dyn Error>> {
    let (_, certificate) =
        parse_x509_certificate(der).map_err(|error| format!("X.509 DER parse failed: {error}"))?;

    summary.common_name = certificate
        .subject()
        .iter_common_name()
        .next()
        .and_then(|attribute| attribute.as_str().ok())
        .map(ToOwned::to_owned);
    summary.organization = certificate
        .subject()
        .iter_organization()
        .next()
        .and_then(|attribute| attribute.as_str().ok())
        .map(ToOwned::to_owned);
    summary.issuer_name = certificate
        .issuer()
        .iter_common_name()
        .next()
        .and_then(|attribute| attribute.as_str().ok())
        .map(ToOwned::to_owned);
    summary.serial_number = Some(
        certificate
            .raw_serial_as_string()
            .replace(':', "")
            .to_lowercase(),
    );
    summary.valid_from = certificate.validity().not_before.to_rfc2822().ok();
    summary.valid_to = certificate.validity().not_after.to_rfc2822().ok();
    summary.fingerprint = Some(hex_upper(&Sha1::digest(der)));

    Ok(())
}

fn hex_upper(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02X}")).collect()
}

fn display_library_path(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| "PKCS#11 module".to_string())
}

fn physical_token_hint() -> Option<String> {
    match env::consts::OS {
        "macos" => command_output(
            "sh",
            &[
                "-c",
                "ioreg -p IOUSB -l -w 0 | awk '/USB TOKEN@|proxkey|epass|watchdata|hypersecu|hyperscu|feitian/{flag=1; count=0} flag && count < 24 {print; count++} count >= 24 {flag=0}' | grep -Ei \"HYPERSECU|USB TOKEN|USB Product Name|USB Vendor Name|kUSBProductString|kUSBVendorString|proxkey|epass|watchdata|hyperscu|feitian\"",
            ],
        ),
        "windows" => command_output(
            "powershell",
            &[
                "-NoProfile",
                "-ExecutionPolicy",
                "Bypass",
                "-Command",
                "$ErrorActionPreference='SilentlyContinue'; $d1 = Get-PnpDevice -Class SmartCard -ErrorAction SilentlyContinue; $d2 = Get-PnpDevice -Class SmartCardReader -ErrorAction SilentlyContinue; $all = @(); if ($d1) { $all += $d1 }; if ($d2) { $all += $d2 }; $all | Select-Object -ExpandProperty FriendlyName | Out-String",
            ],
        ),
        _ => command_output(
            "sh",
            &[
                "-c",
                "lsusb 2>/dev/null | grep -i \"smart\\|card\\|token\\|proxkey\\|epass\\|watchdata\\|hypersecu\\|hyperscu\\|feitian\"",
            ],
        ),
    }
}

fn command_output(program: &str, args: &[&str]) -> Option<String> {
    let output = Command::new(program).args(args).output().ok()?;
    if !output.status.success() {
        return None;
    }

    let text = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if text.is_empty() {
        None
    } else {
        Some(text)
    }
}

#[cfg(test)]
mod tests {
    use super::token_configs;
    use std::path::Path;

    #[test]
    fn token_configs_do_not_default_to_known_pins() {
        let configs = token_configs(Path::new("assets/lib/dsc"), None, None);

        assert!(!configs.is_empty());
        assert!(configs.iter().all(|config| config.pins.is_empty()));
    }

    #[test]
    fn explicit_pin_is_applied_to_each_token_config() {
        let configs = token_configs(
            Path::new("assets/lib/dsc"),
            None,
            Some(vec!["2468".to_string()]),
        );

        assert!(!configs.is_empty());
        assert!(configs
            .iter()
            .all(|config| config.pins == vec!["2468".to_string()]));
    }
}
