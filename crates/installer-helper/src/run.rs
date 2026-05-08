//! Small shared subprocess-spawning helpers used by `internal.rs` and
//! `boot.rs`. Kept private to the helper crate.

use anyhow::{Context, Result};
use std::path::Path;
use std::process::Command;

/// Run a command and return true iff it exits 0.
pub fn run_cmd_ok(argv: &[&str]) -> bool {
    Command::new(argv[0])
        .args(&argv[1..])
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// Run a command, discarding both stdout and stderr.  Returns true iff it
/// exits 0.  Use for tools that produce expected-but-alarming output on
/// their normal fallback code-paths.
pub fn run_cmd_silent(argv: &[&str]) -> bool {
    Command::new(argv[0])
        .args(&argv[1..])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// Run a command, propagating a non-zero exit as an error.
pub fn run_cmd_check(argv: &[&str]) -> Result<()> {
    let status = Command::new(argv[0])
        .args(&argv[1..])
        .status()
        .with_context(|| format!("failed to spawn {}", argv[0]))?;
    if status.success() {
        Ok(())
    } else {
        anyhow::bail!(
            "{} exited with code {}",
            argv[0],
            status.code().unwrap_or(-1)
        )
    }
}

/// Find the first existing path from a list of candidates.
pub fn find_cmd<'a>(candidates: &[&'a str]) -> Result<String> {
    for &c in candidates {
        if Path::new(c).exists()
            || Command::new("which")
                .arg(c)
                .status()
                .map(|s| s.success())
                .unwrap_or(false)
        {
            return Ok(c.to_string());
        }
    }
    anyhow::bail!("none of {:?} found on PATH", candidates)
}
