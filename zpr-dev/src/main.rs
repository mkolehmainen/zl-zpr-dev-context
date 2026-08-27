//! `zpr-dev` — synchronizes the ZPR workspace and renders shared agent context.
//!
//! See `docs/specs/spec-001-zpr-dev.md` for the specification this implements.

mod commands;
mod config;
mod generate;
mod git;

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use anyhow::Result;
use clap::{Parser, Subcommand};

/// Default context repository, cloned by `setup` when absent (spec §5.2).
const DEFAULT_CONTEXT_URL: &str = "git@github.com:org-zpr/zpr-dev-context.git";

#[derive(Parser, Debug)]
#[command(
    name = "zpr-dev",
    version,
    about = "Manage the ZPR development workspace"
)]
struct Cli {
    /// Override the workspace directory
    #[arg(long, global = true, value_name = "PATH")]
    workspace: Option<PathBuf>,

    /// Override the zpr-dev-context checkout
    #[arg(long, global = true, value_name = "PATH")]
    context: Option<PathBuf>,

    /// Show additional detail
    #[arg(short, long, global = true)]
    verbose: bool,

    /// Suppress non-error output
    #[arg(short, long, global = true)]
    quiet: bool,

    /// Show intended changes without modifying anything
    #[arg(long, global = true)]
    dry_run: bool,

    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand, Debug)]
enum Command {
    /// Clone the workspace, generate context files, and validate
    Setup {
        /// Git URL of the context repository
        #[arg(long, value_name = "GIT-URL", default_value = DEFAULT_CONTEXT_URL)]
        context_url: String,

        /// Branch to clone for the context repository
        #[arg(long, value_name = "BRANCH")]
        branch: Option<String>,

        /// Do not clone missing source repositories
        #[arg(long)]
        no_clone: bool,
    },

    /// Fetch and fast-forward repositories, then regenerate context files
    Update {
        /// Also update source repositories
        #[arg(long)]
        all: bool,

        /// Update only the named repository
        #[arg(long, value_name = "NAME")]
        repo: Option<String>,

        /// Skip regeneration afterward
        #[arg(long)]
        no_generate: bool,
    },

    /// Report repository and generated-context state
    Status {
        /// Machine-readable, tab-separated output
        #[arg(long)]
        porcelain: bool,

        /// Restrict to one repository
        #[arg(long, value_name = "NAME")]
        repo: Option<String>,
    },

    /// Write the generated context files
    Sync,

    /// Check workspace health
    Validate,
}

/// Everything a command needs that is not specific to that command (spec §6.2).
#[derive(Debug)]
pub struct Ctx {
    /// Directory holding the repository checkouts.
    pub workspace: PathBuf,
    /// The `zpr-dev-context` checkout providing the shared context.
    pub context: PathBuf,
    /// When set, no mutation of any kind is performed (spec §5.1).
    pub dry_run: bool,
    pub verbose: bool,
    pub quiet: bool,
}

/// Parses arguments, builds the [`Ctx`], and dispatches to the command.
fn run() -> Result<ExitCode> {
    let cli = Cli::parse();

    let home = std::env::var("HOME").unwrap_or_default();
    let env_workspace = std::env::var("ZPR_WORKSPACE").ok();
    let workspace = config::resolve_workspace(
        cli.workspace.as_deref(),
        env_workspace.as_deref(),
        Path::new(&home),
    );
    let context = config::resolve_context(cli.context.as_deref(), &workspace);

    let ctx = Ctx {
        workspace,
        context,
        dry_run: cli.dry_run,
        verbose: cli.verbose,
        quiet: cli.quiet,
    };

    match &cli.command {
        Command::Setup {
            context_url,
            branch,
            no_clone,
        } => commands::setup(&ctx, context_url, branch.as_deref(), *no_clone),
        Command::Update {
            all,
            repo,
            no_generate,
        } => commands::update(&ctx, *all, repo.as_deref(), *no_generate),
        Command::Status { porcelain, repo } => commands::status(&ctx, *porcelain, repo.as_deref()),
        Command::Sync => commands::sync(&ctx),
        Command::Validate => commands::validate(&ctx),
    }
}

/// Maps the command result to a process exit code: an error that reaches here
/// is a command or configuration failure and exits `2` (spec §6.4).
fn main() -> ExitCode {
    match run() {
        Ok(code) => code,
        Err(error) => {
            eprintln!("error: {error:#}");
            ExitCode::from(2)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;

    /// Catches every clap misconfiguration (duplicate short flags, bad global
    /// placement) at test time rather than at first run.
    #[test]
    fn cli_definition_is_valid() {
        Cli::command().debug_assert();
    }
}
