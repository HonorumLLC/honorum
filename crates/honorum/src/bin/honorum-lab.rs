//! Development labs: `honorum-lab browser`, `honorum-lab perf`, and
//! `honorum-lab capacity`. Each mode is configured by environment variables;
//! see `honorum_server::lab`. Built only with the `lab` feature:
//! `cargo build --release -p honorum --features lab --bin honorum-lab`.

use std::process::ExitCode;

use honorum_server::lab;

const USAGE: &str = "usage: honorum-lab browser|perf|capacity";

fn main() -> ExitCode {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .with_writer(std::io::stderr)
        .init();
    let mode = std::env::args().nth(1).unwrap_or_default();
    let runtime = tokio::runtime::Builder::new_multi_thread().enable_all().build().expect("a Tokio runtime");
    let result = runtime.block_on(async {
        match mode.as_str() {
            "browser" => lab::browser::run().await,
            "perf" => lab::perf::run().await,
            "capacity" => lab::capacity::run().await,
            _ => Err(anyhow::anyhow!("{USAGE}")),
        }
    });
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("honorum-lab {mode}: {err:#}");
            ExitCode::FAILURE
        }
    }
}
