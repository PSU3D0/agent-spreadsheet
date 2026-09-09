#[cfg(not(target_arch = "wasm32"))]
use anyhow::Result;

#[cfg(not(target_arch = "wasm32"))]
#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<()> {
    #[cfg(feature = "recalc-formualizer")]
    if agent_spreadsheet::native_host::run_if_requested().await? { return Ok(()); }
    agent_spreadsheet::cli::run().await
}

#[cfg(target_arch = "wasm32")]
fn main() {
    eprintln!("agent-spreadsheet is unsupported on wasm32 targets");
    std::process::exit(1);
}
