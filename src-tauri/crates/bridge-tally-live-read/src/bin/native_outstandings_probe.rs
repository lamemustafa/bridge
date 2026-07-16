use std::{
    io::{self, BufRead, Write},
    path::PathBuf,
    process::ExitCode,
};

use bridge_tally_live_read::native_outstandings_qualification::{
    confirm_dispatch_challenge, confirm_preflight_challenge, confirm_ui_after_challenge,
    native_probe_save_phrase, save_native_probe_receipt_no_replace, LoadedNativeOutstandingsProbe,
};

#[tokio::main]
async fn main() -> ExitCode {
    match run().await {
        Ok(()) => ExitCode::SUCCESS,
        Err(code) => {
            eprintln!("bridge_tally_native_outstandings_probe_failed:{code}");
            ExitCode::FAILURE
        }
    }
}

async fn run() -> Result<(), &'static str> {
    let mut arguments = std::env::args().skip(1);
    if arguments.next().as_deref() != Some("run") {
        return Err("usage_run_required");
    }
    let config_path = arguments
        .next()
        .map(PathBuf::from)
        .ok_or("config_path_missing")?;
    let output_path = arguments
        .next()
        .map(PathBuf::from)
        .ok_or("output_path_missing")?;
    if arguments.next().as_deref() != Some("--consent")
        || arguments.next().as_deref() != Some("read-only-synthetic-native-outstandings")
        || arguments.next().is_some()
    {
        return Err("explicit_native_probe_consent_option_required");
    }
    if output_path.exists() {
        return Err("receipt_output_exists");
    }

    let loaded =
        LoadedNativeOutstandingsProbe::load(&config_path).map_err(|error| error.safe_code())?;
    let output = loaded
        .validate_receipt_output(&output_path)
        .map_err(|error| error.safe_code())?;

    println!(
        "CandidateV0 synthetic qualification observation. Accounting semantics and support remain unknown."
    );
    println!(
        "Stage 1 permits exactly two sealed identity preflight reads and no Candidate request."
    );
    println!(
        "The reviewed profile attests that no customer or personal data is loaded. Stop if that is inaccurate."
    );
    println!("Type this exact run-bound preflight challenge:");
    println!("{}", loaded.preflight_challenge());
    let typed = read_line()?;
    let consent =
        confirm_preflight_challenge(&loaded, &typed).map_err(|error| error.safe_code())?;
    let ready = loaded
        .run_preflight(consent)
        .await
        .map_err(|error| error.safe_code())?;

    println!(
        "Synthetic identities matched. Stage 2 permits exactly eleven reads: four company/party brackets and three CandidateV0 observations."
    );
    println!("Type this distinct dispatch challenge:");
    println!("{}", ready.dispatch_challenge());
    let typed = read_line()?;
    let consent = confirm_dispatch_challenge(&ready, &typed).map_err(|error| error.safe_code())?;
    let pending = ready
        .dispatch(consent)
        .await
        .map_err(|error| error.safe_code())?;

    println!(
        "Capture the reviewed UI-after observation now. Raw Tally responses will not be printed or retained."
    );
    println!("Type this exact UI-after binding challenge after the capture exists:");
    println!("{}", pending.ui_after_challenge());
    let typed = read_line()?;
    let consent =
        confirm_ui_after_challenge(&pending, &typed).map_err(|error| error.safe_code())?;
    let receipt = pending
        .finalize(consent)
        .map_err(|error| error.safe_code())?;
    let receipt_bytes = receipt
        .to_pretty_json()
        .map_err(|_| "native_probe_receipt_serialization_failed")?;

    println!("Privacy-reduced observation receipt preview follows:");
    io::stdout()
        .write_all(&receipt_bytes)
        .and_then(|_| io::stdout().write_all(b"\n"))
        .and_then(|_| io::stdout().flush())
        .map_err(|_| "native_probe_receipt_preview_failed")?;
    let save_phrase =
        native_probe_save_phrase(&receipt, &output).map_err(|error| error.safe_code())?;
    println!("Type this exact receipt-and-output-bound challenge to save without overwrite:");
    println!("{save_phrase}");
    let typed = read_line()?;
    save_native_probe_receipt_no_replace(output, &receipt_bytes, &typed)
        .map_err(|error| error.safe_code())?;
    println!("bridge_tally_native_outstandings_probe_receipt_saved");
    Ok(())
}

fn read_line() -> Result<String, &'static str> {
    let mut line = String::new();
    io::stdin()
        .lock()
        .read_line(&mut line)
        .map_err(|_| "interactive_input_failed")?;
    if line.len() > 256 {
        return Err("interactive_input_too_long");
    }
    Ok(line)
}
