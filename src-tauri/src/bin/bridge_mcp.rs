//! Local Bridge MCP entry point.
fn main() {
    if let Some(status) =
        bridge_lib::run_journal_confirmation_child_from_args(std::env::args().skip(1))
    {
        std::process::exit(status);
    }
    let runtime = tokio::runtime::Runtime::new().expect("MCP runtime");
    if let Err(error) = runtime.block_on(bridge_lib::agent::run_stdio()) {
        eprintln!("bridge-mcp: {error}");
        std::process::exit(1);
    }
}
