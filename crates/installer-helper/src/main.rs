//! Privileged executor for [`installer_core::Plan`].
//!
//! The GUI runs unprivileged. It serialises a [`Plan`] as JSON to stdin of
//! this binary (typically launched via `pkexec`). The helper:
//!
//! 1. Re-validates every step's argv against an allowlist (`allowlist.rs`).
//!    Anything off the allowlist is refused before any side effect.
//! 2. Runs steps in order, streaming JSON events to stdout (one per line).
//! 3. With `--dry-run`, prints the validated argv and exits 0 without
//!    spawning anything. The GUI's "Show commands" button uses this so
//!    the displayed text is byte-identical to what would be executed.
//!
//! Internal helper subcommands (`copy-system`, `install-boot-usb`,
//! `install-boot-internal`, `check-fast-startup`, `mkpart-after`,
//! `resizepart`) are implemented in [`internal`] and dispatch to this
//! same binary by name (`nimblex-installer-helper-internal`). This keeps
//! the allowlist closed and avoids shelling out to ad-hoc scripts.

mod allowlist;
mod boot;
mod events;
mod internal;
mod run;
mod runner;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use installer_core::Plan;
use std::io::{self, Read};

#[derive(Parser, Debug)]
#[command(name = "nimblex-installer-helper", version)]
struct Cli {
    #[command(subcommand)]
    command: Option<Cmd>,

    /// Read plan from stdin, validate it, and print what would be executed
    /// without making any changes. Default mode when no subcommand is given.
    #[arg(long)]
    dry_run: bool,

    /// Read plan from stdin and execute every step.
    #[arg(long)]
    run: bool,
}

#[derive(Subcommand, Debug)]
enum Cmd {
    /// Internal subcommands invoked by the planner via the
    /// `nimblex-installer-helper-internal` argv0 alias. Not for direct use.
    #[command(name = "internal", hide = true)]
    Internal(internal::InternalCli),
}

fn main() -> Result<()> {
    init_tracing();

    // Argv0 dispatch: when invoked as `nimblex-installer-helper-internal`,
    // forward all args to the internal handler. This lets the planner emit
    // single-string commands like `nimblex-installer-helper-internal copy-system ...`.
    if let Some(name) = std::env::args().next() {
        if name.ends_with("nimblex-installer-helper-internal") {
            let cli = internal::InternalCli::parse();
            return internal::run(cli);
        }
    }

    let cli = Cli::parse();

    if let Some(Cmd::Internal(int)) = cli.command {
        return internal::run(int);
    }

    let plan = read_plan_from_stdin()?;

    if cli.run {
        runner::execute(&plan)
    } else {
        runner::dry_run(&plan)
    }
}

fn read_plan_from_stdin() -> Result<Plan> {
    let mut buf = String::new();
    io::stdin()
        .read_to_string(&mut buf)
        .context("reading plan JSON from stdin")?;
    let plan: Plan = serde_json::from_str(&buf).context("parsing plan JSON")?;
    Ok(plan)
}

fn init_tracing() {
    use tracing_subscriber::EnvFilter;
    let _ = tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")))
        .with_writer(io::stderr)
        .try_init();
}
