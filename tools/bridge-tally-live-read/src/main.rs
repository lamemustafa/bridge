use std::{
    io::{self, BufRead, Write},
    path::PathBuf,
    process::ExitCode,
};

use bridge_tally_live_read::{
    confirm_network_challenge, receipt_save_phrase, save_live_receipt_no_replace, LiveRunInputs,
};

#[tokio::main]
async fn main() -> ExitCode {
    match run().await {
        Ok(()) => ExitCode::SUCCESS,
        Err(code) => {
            eprintln!("bridge_tally_live_read_failed:{code}");
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
        || arguments.next().as_deref() != Some("read-only-synthetic")
        || arguments.next().is_some()
    {
        return Err("explicit_consent_option_required");
    }
    if output_path.exists() {
        return Err("receipt_output_exists");
    }

    let inputs = LiveRunInputs::load(&config_path).map_err(|value| value.safe_code())?;
    let output = inputs
        .validate_receipt_output(&output_path)
        .map_err(|value| value.safe_code())?;
    println!(
        "Read-only synthetic qualification is ready. No write/import request exists in this controller."
    );
    println!(
        "Your local profile attests no customer data: {}. Stop now if that assertion is inaccurate.",
        inputs.no_customer_data_attested()
    );
    println!("Type this exact run-bound challenge to permit network reads:");
    println!("{}", inputs.challenge_phrase());
    let typed = read_line()?;
    let consent = confirm_network_challenge(&inputs, &typed).map_err(|value| value.safe_code())?;
    let receipt = inputs
        .execute(consent)
        .await
        .map_err(|value| value.safe_code())?;
    let receipt_bytes = receipt
        .to_pretty_json()
        .map_err(|_| "receipt_serialization_failed")?;

    println!("Exact privacy-reduced receipt preview follows:");
    io::stdout()
        .write_all(&receipt_bytes)
        .and_then(|_| io::stdout().write_all(b"\n"))
        .and_then(|_| io::stdout().flush())
        .map_err(|_| "receipt_preview_failed")?;
    let save_phrase = receipt_save_phrase(&receipt, &output).map_err(|value| value.safe_code())?;
    println!("Type this exact receipt-bound challenge to save without overwrite:");
    println!("{save_phrase}");
    let typed = read_line()?;
    save_live_receipt_no_replace(output, &receipt_bytes, &typed)
        .map_err(|value| value.safe_code())?;
    println!("bridge_tally_live_read_receipt_saved");
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
