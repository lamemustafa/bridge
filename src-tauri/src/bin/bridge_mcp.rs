//! Read-only Bridge MCP entry point.

#[tokio::main]
async fn main() {
    if let Err(error) = bridge_lib::agent::run_stdio().await {
        eprintln!("bridge-mcp: {error}");
        std::process::exit(1);
    }
}
