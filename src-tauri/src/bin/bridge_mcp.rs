//! Local Bridge MCP entry point.
fn main() {
    if std::env::args().nth(1).as_deref() == Some("--confirm-journal") {
        std::process::exit(if bridge_lib::agent::run_confirmation() {
            0
        } else {
            1
        });
    }
    let runtime = tokio::runtime::Runtime::new().expect("MCP runtime");
    if let Err(error) = runtime.block_on(bridge_lib::agent::run_stdio()) {
        eprintln!("bridge-mcp: {error}");
        std::process::exit(1);
    }
}
