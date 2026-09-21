//! An MCP server for driving a device, spoken over stdin and stdout.
//!
//! Everything it can do, the `jev-pilot` command line can already do. What
//! this adds is the seam: a run that cannot decide stops and asks, and over
//! MCP the thing holding the conversation is what answers.

use rmcp::ServiceExt as _;

#[tokio::main(flavor = "current_thread")]
async fn main() -> std::process::ExitCode {
    // One thread. Nothing here is compute, and the run itself is a separate
    // process with a loop that waits on one thing at a time.
    let serving = match jev_pilot::mcp::Pilot::new()
        .serve(rmcp::transport::stdio())
        .await
    {
        Ok(serving) => serving,
        Err(error) => {
            eprintln!("jev-pilot-mcp: {error}");
            return std::process::ExitCode::FAILURE;
        }
    };
    match serving.waiting().await {
        Ok(_) => std::process::ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("jev-pilot-mcp: {error}");
            std::process::ExitCode::FAILURE
        }
    }
}
