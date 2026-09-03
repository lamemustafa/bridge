//! Read-only Bridge MCP entry point. The implementation is deliberately
//! additive: compatibility-sealed library surfaces remain untouched.

#[path = "../agent.rs"]
mod agent;

#[tokio::main]
async fn main() {
    if let Err(error) = agent::run_stdio().await {
        eprintln!("bridge-mcp: {error}");
        std::process::exit(1);
    }
}
