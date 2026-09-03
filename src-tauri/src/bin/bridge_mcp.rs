//! Read-only Bridge MCP entry point.  The implementation lives in the library
//! so the same handler can be exercised without launching Tauri.

#[tokio::main]
async fn main() {
    if let Err(error) = bridge_lib::agent::run_stdio().await {
        eprintln!("bridge-mcp: {error}");
        std::process::exit(1);
    }
}
