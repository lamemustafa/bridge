//! Manual-only availability boundary for Tally's native outstandings oracle.
//!
//! A verified native Bills Receivable export profile does not yet exist. This
//! binary consequently never sends an unreviewed `<TYPE>Data</TYPE>` request:
//! it health-probes through an existing sealed read profile, then emits a
//! non-success skip token bound to the contacted port.

use std::process::ExitCode;

use bridge_tally_live_read::read_only_oracle_health;

const GATEWAY_UNREACHABLE_EXIT: u8 = 20;
const NATIVE_PROFILE_UNAVAILABLE_EXIT: u8 = 21;

#[tokio::main]
async fn main() -> ExitCode {
    let Some(port) = parse_port() else {
        eprintln!("tally_oracle_skipped:usage_port_required");
        return ExitCode::from(NATIVE_PROFILE_UNAVAILABLE_EXIT);
    };
    match read_only_oracle_health(port).await {
        Err(_) => {
            println!("tally_oracle_skipped:gateway_unreachable:port={port}");
            ExitCode::from(GATEWAY_UNREACHABLE_EXIT)
        }
        Ok(()) => {
            println!(
                "tally_oracle_skipped:native_bills_receivable_profile_unavailable:port={port}"
            );
            ExitCode::from(NATIVE_PROFILE_UNAVAILABLE_EXIT)
        }
    }
}

fn parse_port() -> Option<u16> {
    let mut arguments = std::env::args().skip(1);
    (arguments.next()?.as_str() == "--port")
        .then(|| arguments.next())
        .flatten()
        .and_then(|value| value.parse().ok())
        .filter(|_| arguments.next().is_none())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn skip_exit_codes_are_non_success_and_distinct() {
        assert_ne!(GATEWAY_UNREACHABLE_EXIT, 0);
        assert_ne!(NATIVE_PROFILE_UNAVAILABLE_EXIT, 0);
        assert_ne!(GATEWAY_UNREACHABLE_EXIT, NATIVE_PROFILE_UNAVAILABLE_EXIT);
    }
}
