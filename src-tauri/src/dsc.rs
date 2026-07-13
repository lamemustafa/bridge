use std::env;
use std::error::Error;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::time::{Duration, Instant};

use cryptoki::context::{CInitializeArgs, CInitializeFlags, Pkcs11};
use cryptoki::object::{Attribute, AttributeType, ObjectClass};
use cryptoki::session::UserType;
use cryptoki::types::AuthPin;
use pkcs11::types::{
    CKA_CLASS, CKA_ID, CKA_LABEL, CKA_TOKEN, CKA_VALUE, CKF_RW_SESSION, CKF_SERIAL_SESSION,
    CKO_CERTIFICATE, CKU_USER, CK_ATTRIBUTE, CK_BBOOL, CK_TRUE,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use x509_parser::prelude::parse_x509_certificate;
use zeroize::{Zeroize, Zeroizing};

const CHILD_TIMEOUT: Duration = Duration::from_secs(30);
const PROBE_OPERATION_TIMEOUT: Duration = Duration::from_secs(45);
const CHILD_OUTPUT_LIMIT: usize = 1024 * 1024;

#[cfg(windows)]
struct ProcessContainment {
    job: windows_sys::Win32::Foundation::HANDLE,
}

#[cfg(windows)]
impl ProcessContainment {
    fn new(child: &std::process::Child) -> std::io::Result<Self> {
        use std::os::windows::io::AsRawHandle;
        use windows_sys::Win32::System::JobObjects::{
            AssignProcessToJobObject, CreateJobObjectW, JobObjectExtendedLimitInformation,
            SetInformationJobObject, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
            JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
        };

        let job = unsafe { CreateJobObjectW(std::ptr::null(), std::ptr::null()) };
        if job.is_null() {
            return Err(std::io::Error::last_os_error());
        }
        let mut information = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
        information.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        let configured = unsafe {
            SetInformationJobObject(
                job,
                JobObjectExtendedLimitInformation,
                &information as *const _ as *const std::ffi::c_void,
                std::mem::size_of_val(&information) as u32,
            )
        };
        let assigned = unsafe { AssignProcessToJobObject(job, child.as_raw_handle() as _) };
        if configured == 0 || assigned == 0 {
            unsafe { windows_sys::Win32::Foundation::CloseHandle(job) };
            return Err(std::io::Error::last_os_error());
        }
        Ok(Self { job })
    }

    fn terminate(&self) {
        unsafe {
            windows_sys::Win32::System::JobObjects::TerminateJobObject(self.job, 1);
        }
    }
}

#[cfg(windows)]
impl Drop for ProcessContainment {
    fn drop(&mut self) {
        unsafe {
            windows_sys::Win32::Foundation::CloseHandle(self.job);
        }
    }
}

#[cfg(unix)]
struct ProcessContainment {
    process_group: i32,
}

#[cfg(unix)]
impl ProcessContainment {
    fn new(child: &std::process::Child) -> std::io::Result<Self> {
        Ok(Self {
            process_group: child.id() as i32,
        })
    }

    fn terminate(&self) {
        unsafe {
            libc::kill(-self.process_group, libc::SIGKILL);
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProbeReport {
    pub platform: String,
    pub arch: String,
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

#[derive(Clone)]
struct TokenConfig {
    token_type: String,
    library_path: PathBuf,
    pins: Vec<String>,
}

impl Drop for TokenConfig {
    fn drop(&mut self) {
        self.pins.zeroize();
    }
}

pub fn run_probe_isolated(
    detect_only: bool,
    explicit_library: Option<String>,
    explicit_pins: Option<Vec<String>>,
    force_load: bool,
) -> Result<ProbeReport, Box<dyn Error>> {
    let mut pins = explicit_pins.unwrap_or_default();
    let report = (|| {
        let library_root = runtime_library_root()?;
        let configs = token_configs(&library_root, explicit_library);
        let deadline = Instant::now() + PROBE_OPERATION_TIMEOUT;
        let mut attempts = Vec::new();

        for config in &configs {
            if Instant::now() >= deadline {
                attempts.push(skipped_attempt(config, "DSC probe operation timed out"));
                break;
            }
            if !config.library_path.exists() {
                attempts.push(skipped_attempt(config, "PKCS#11 library does not exist"));
                continue;
            }
            if !force_load {
                attempts.push(skipped_attempt(
                    config,
                    "native library loading was not authorized",
                ));
                continue;
            }
            if detect_only {
                attempts.push(run_child_attempt(config, true, deadline));
                continue;
            }

            let detection = run_child_attempt(config, true, deadline);
            if detection.loaded && detection.initialized && detection.slot_count > 0 {
                let mut selected = config.clone();
                selected.pins = std::mem::take(&mut pins);
                attempts.push(run_child_attempt(&selected, false, deadline));
                break;
            }
            attempts.push(detection);
        }

        for attempt in &mut attempts {
            sanitize_attempt(attempt);
        }
        Ok(ProbeReport {
            platform: env::consts::OS.to_string(),
            arch: env::consts::ARCH.to_string(),
            force_load,
            detect_only,
            attempts,
        })
    })();
    pins.zeroize();
    report
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

    let mut attempt = probe_config(&config, detect_only, true, true);
    sanitize_attempt(&mut attempt);
    attempt
}

fn sanitize_attempt(attempt: &mut ProbeAttempt) {
    attempt.error = attempt.error.as_deref().map(sanitize_probe_error);
    for certificate in &mut attempt.certificates {
        if certificate.parse_error.is_some() {
            certificate.parse_error = Some("Certificate data could not be parsed".to_string());
        }
    }
}

fn sanitize_probe_error(error: &str) -> String {
    let normalized = error.to_ascii_lowercase();
    if normalized.contains("locked") {
        "The DSC token PIN appears to be locked".to_string()
    } else if normalized.contains("pin") || normalized.contains("login") {
        "The DSC token could not authenticate with the provided PIN".to_string()
    } else if normalized.contains("no token") || normalized.contains("slot") {
        "No usable DSC token was found".to_string()
    } else if normalized.contains("library") || normalized.contains("pkcs") {
        "The DSC token driver could not be loaded".to_string()
    } else {
        "The DSC token operation failed".to_string()
    }
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
        let mut input = Zeroizing::new(String::new());
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

fn runtime_library_root() -> Result<PathBuf, Box<dyn Error>> {
    let executable = env::current_exe()?;
    let executable_dir = executable
        .parent()
        .ok_or_else(|| "could not resolve the Bridge executable directory".to_string())?;
    Ok(executable_dir.join("assets").join("lib").join("dsc"))
}

fn token_configs(library_root: &Path, explicit_library: Option<String>) -> Vec<TokenConfig> {
    if let Some(library) = explicit_library {
        return vec![TokenConfig {
            token_type: "explicit".to_string(),
            library_path: PathBuf::from(library),
            pins: Vec::new(),
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
        login_success: false,
        certificate_count: None,
        certificates: Vec::new(),
        error: Some(error.to_string()),
    }
}

fn run_child_attempt(config: &TokenConfig, detect_only: bool, deadline: Instant) -> ProbeAttempt {
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
        .stdin(Stdio::piped());

    if detect_only {
        command.arg("--detect-only");
    }

    let (mut child, stdout, stderr, containment) = match spawn_with_output_files(&mut command) {
        Ok(output) => output,
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
            containment.terminate();
            let _ = child.kill();
            let _ = child.wait();
            return skipped_attempt(config, &error);
        }
    }

    match wait_with_limited_output(child, stdout, stderr, containment, deadline) {
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

fn spawn_with_output_files(
    command: &mut Command,
) -> std::io::Result<(std::process::Child, File, File, ProcessContainment)> {
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    let stdout = tempfile::tempfile()?;
    let stderr = tempfile::tempfile()?;
    command
        .stdout(Stdio::from(stdout.try_clone()?))
        .stderr(Stdio::from(stderr.try_clone()?));
    let mut child = command.spawn()?;
    let containment = match ProcessContainment::new(&child) {
        Ok(containment) => containment,
        Err(error) => {
            let _ = child.kill();
            let _ = child.wait();
            return Err(error);
        }
    };
    Ok((child, stdout, stderr, containment))
}

fn wait_with_limited_output(
    mut child: std::process::Child,
    mut stdout: File,
    mut stderr: File,
    containment: ProcessContainment,
    operation_deadline: Instant,
) -> std::io::Result<Output> {
    let deadline = operation_deadline.min(Instant::now() + CHILD_TIMEOUT);
    let status = loop {
        if let Some(status) = child.try_wait()? {
            containment.terminate();
            break status;
        }
        if stdout.metadata()?.len() > CHILD_OUTPUT_LIMIT as u64
            || stderr.metadata()?.len() > CHILD_OUTPUT_LIMIT as u64
        {
            containment.terminate();
            let _ = child.kill();
            let _ = child.wait();
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "DSC probe child exceeded its output limit",
            ));
        }
        if Instant::now() >= deadline {
            containment.terminate();
            let _ = child.kill();
            let _ = child.wait();
            return Err(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                "DSC probe child timed out",
            ));
        }
        std::thread::sleep(Duration::from_millis(10));
    };
    stdout.seek(SeekFrom::Start(0))?;
    stderr.seek(SeekFrom::Start(0))?;
    let stdout = read_limited(&mut stdout, CHILD_OUTPUT_LIMIT)?;
    let stderr = read_limited(&mut stderr, CHILD_OUTPUT_LIMIT)?;
    if stdout.len() > CHILD_OUTPUT_LIMIT || stderr.len() > CHILD_OUTPUT_LIMIT {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "DSC probe child exceeded its output limit",
        ));
    }
    Ok(Output {
        status,
        stdout,
        stderr,
    })
}

fn read_limited<R: Read>(reader: R, limit: usize) -> std::io::Result<Vec<u8>> {
    let mut output = Vec::new();
    reader.take(limit as u64 + 1).read_to_end(&mut output)?;
    Ok(output)
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
    summary.fingerprint = Some(hex_upper(&Sha256::digest(der)));

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

#[cfg(test)]
mod tests {
    use super::{
        spawn_with_output_files, token_configs, wait_with_limited_output, CHILD_OUTPUT_LIMIT,
    };
    use std::path::Path;
    use std::process::{Command, Stdio};
    use std::time::{Duration, Instant};

    #[test]
    fn token_configs_do_not_default_to_known_pins() {
        let configs = token_configs(Path::new("assets/lib/dsc"), None);

        assert!(!configs.is_empty());
        assert!(configs.iter().all(|config| config.pins.is_empty()));
    }

    #[test]
    fn discovered_token_configs_never_contain_pins() {
        let configs = token_configs(Path::new("assets/lib/dsc"), None);

        assert!(!configs.is_empty());
        assert!(configs.iter().all(|config| config.pins.is_empty()));
    }

    #[test]
    fn child_process_deadline_kills_and_reaps() {
        let mut command = if cfg!(windows) {
            let mut command = Command::new("powershell");
            command.args(["-NoProfile", "-Command", "Start-Sleep -Seconds 5"]);
            command
        } else {
            let mut command = Command::new("sh");
            command.args(["-c", "sleep 5"]);
            command
        };
        command.stdin(Stdio::null());
        let (child, stdout, stderr, containment) =
            spawn_with_output_files(&mut command).expect("spawn sleeping child");
        let error = wait_with_limited_output(
            child,
            stdout,
            stderr,
            containment,
            Instant::now() + Duration::from_millis(100),
        )
        .expect_err("child should time out");
        assert_eq!(error.kind(), std::io::ErrorKind::TimedOut);
    }

    #[test]
    fn child_process_output_is_capped() {
        let mut command = if cfg!(windows) {
            let mut command = Command::new("powershell");
            command.args(["-NoProfile", "-Command", "Start-Sleep -Seconds 5"]);
            command
        } else {
            let mut command = Command::new("sh");
            command.args(["-c", "sleep 5"]);
            command
        };
        command.stdin(Stdio::null());
        let (child, stdout, stderr, containment) =
            spawn_with_output_files(&mut command).expect("spawn noisy child");
        stdout
            .set_len(CHILD_OUTPUT_LIMIT as u64 + 1)
            .expect("expand captured output beyond the limit");
        let error = wait_with_limited_output(
            child,
            stdout,
            stderr,
            containment,
            Instant::now() + Duration::from_secs(1),
        )
        .expect_err("child output should be capped");
        assert_eq!(error.kind(), std::io::ErrorKind::InvalidData);
    }

    #[test]
    fn child_process_timeout_terminates_descendants() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let marker = directory.path().join("descendant-survived.txt");
        let mut command = if cfg!(windows) {
            let marker = marker.display().to_string().replace('\'', "''");
            let script = format!(
                "$child = \"Start-Sleep -Milliseconds 800; Set-Content -LiteralPath '{marker}' -Value survived\"; Start-Process powershell -WindowStyle Hidden -ArgumentList @('-NoProfile','-Command',$child); Start-Sleep -Seconds 5"
            );
            let mut command = Command::new("powershell");
            command.args(["-NoProfile", "-Command", &script]);
            command
        } else {
            let script = format!(
                "(sleep 1; printf survived > '{}') & sleep 5",
                marker.display()
            );
            let mut command = Command::new("sh");
            command.args(["-c", &script]);
            command
        };
        command.stdin(Stdio::null());
        let (child, stdout, stderr, containment) =
            spawn_with_output_files(&mut command).expect("spawn process tree");
        let error = wait_with_limited_output(
            child,
            stdout,
            stderr,
            containment,
            Instant::now() + Duration::from_millis(200),
        )
        .expect_err("process tree should time out");
        assert_eq!(error.kind(), std::io::ErrorKind::TimedOut);
        std::thread::sleep(Duration::from_millis(1200));
        assert!(
            !marker.exists(),
            "descendant survived process-tree termination"
        );
    }
}
