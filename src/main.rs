use clap::Parser;

use co_review::cli::Cli;

fn main() -> std::process::ExitCode {
    let cli = Cli::parse();
    match co_review::run(cli) {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(e) => {
            // anyhow's Display chains the causes with `: `.
            eprintln!("co-review: error: {e:#}");
            std::process::ExitCode::FAILURE
        }
    }
}
