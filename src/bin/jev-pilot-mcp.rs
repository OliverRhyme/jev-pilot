//! An MCP server for driving a device, spoken over stdin and stdout.
//!
//! Everything it can do, the `jev-pilot` command line can already do. What
//! this adds is the seam: a run that cannot decide stops and asks, and over
//! MCP the thing holding the conversation is what answers.

fn main() -> std::process::ExitCode {
    match jev_pilot::mcp::serve(std::io::stdin().lock(), &mut std::io::stdout().lock()) {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("jev-pilot-mcp: {error}");
            std::process::ExitCode::FAILURE
        }
    }
}
