//! `co-review` — interactive, split-screen PR co-review between you and your AI
//! agent, inside [Herdr](https://herdr.dev).
//!
//! See `docs/DECISIONS.md` for the design rationale. The crate is split into:
//!
//! - [`model`] — the shared `State` and everything in it.
//! - [`store`] — lock-guarded persistence of that state.
//! - [`config`] / [`paths`] / [`pr`] / [`protocol`] — configuration & inputs.
//! - [`git`] / [`github`] / [`herdr`] / [`exec`] — external integrations.
//! - [`diffview`] — turning a finding into the "related code" the human sees.
//! - [`commands`] — agent/human subcommands; [`orchestrate`] — `start`; [`tui`] —
//!   the navigator.

pub mod cli;
pub mod commands;
pub mod config;
pub mod diffview;
pub mod exec;
pub mod git;
pub mod github;
pub mod herdr;
pub mod model;
pub mod orchestrate;
pub mod paths;
pub mod pr;
pub mod protocol;
pub mod store;
pub mod tui;
pub mod util;

use anyhow::Result;

use cli::{Cli, Command};

/// Dispatch a parsed CLI invocation.
pub fn run(cli: Cli) -> Result<()> {
    match &cli.command {
        Command::Start(args) => orchestrate::start(args),
        Command::View(args) => tui::view(args),
        Command::AddFinding(args) => commands::add_finding(args),
        Command::Import(args) => commands::import(args),
        Command::List(args) => commands::list(args),
        Command::Show(args) => commands::show(args),
        Command::Verdict(args) => commands::verdict(args),
        Command::Edit(args) => commands::edit(args),
        Command::Wait(args) => commands::wait(args),
        Command::Post(args) => commands::post(args),
        Command::MarkPosted(args) => commands::mark_posted(args),
        Command::SetStatus(args) => commands::set_status(args),
        Command::Status(args) => commands::status(args),
        Command::Sessions => commands::sessions(),
        Command::End(args) => commands::end(args),
        Command::Protocol => commands::protocol(),
        Command::Prompt => commands::prompt(),
        Command::Doctor => commands::doctor(),
        Command::Completions(args) => commands::completions(args),
    }
}
