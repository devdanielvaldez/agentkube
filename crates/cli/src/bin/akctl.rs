//! Minimal `akctl` process entrypoint.
//!
//! Argument parsing, configuration, client construction, command execution,
//! and exit-code conversion live here. All business logic remains in the
//! library crate so it can be tested directly.

use agentkube_cli::{args::Cli, client::ApiClient, command, config, error::CliError, update};
use clap::Parser;

#[tokio::main]
async fn main() {
    std::process::exit(run().await);
}

async fn run() -> i32 {
    let cli = Cli::parse();
    let code = match run_with(cli).await {
        Ok(()) => 0,
        Err(error) => {
            if error.is_broken_pipe() {
                return 0;
            }
            eprintln!("akctl: {error}");
            error.exit_code()
        }
    };
    if let Some(notice) = update::update_notice(env!("CARGO_PKG_VERSION")).await {
        eprintln!("{notice}");
    }
    code
}

async fn run_with(cli: Cli) -> Result<(), CliError> {
    let resolved = config::build_config(
        cli.server.as_deref(),
        &cli.timeout,
        cli.output,
        cli.no_color,
        cli.verbose,
    )?;
    if resolved.verbose > 0 {
        eprintln!("akctl: using server {}", resolved.server_url);
    }
    let client = ApiClient::new(
        resolved.server_url.clone(),
        resolved.timeout,
        resolved.token.clone(),
        resolved.verbose,
    )?;
    command::execute(cli.command, &resolved, &client).await
}
