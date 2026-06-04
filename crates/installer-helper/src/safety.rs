//! Root-side backstop against destroying the running live session.
//!
//! The GUI already gates unsafe targets, but the helper is the only component
//! that actually writes to disk, so it performs its own **independent** check:
//! it resolves the live-backing device with [`installer_core::live::LiveMedia`]
//! and refuses any step that would reformat or truncate the device the running
//! session booted from. A GUI or planner bug therefore cannot wipe the system
//! the user is sitting in front of.
//!
//! Two step shapes are dangerous and guarded here:
//!
//! * `mkfs.*  <dev>` — formatting the live partition is instantly fatal.
//! * `nimblex-installer-helper-internal copy-system --root <dev>` *without*
//!   `--inplace-live` — the legacy truncating copy corrupts loop-mounted
//!   bundles. The crash-safe `--inplace-live` path is explicitly allowed.
//!
//! When the system runs from RAM (copy2ram) `LiveMedia` reports no backing
//! partition, so every step passes — modifying the medium is then safe. When
//! the topology is exotic and the device cannot be named, we cannot match it
//! against a step's argv; that residual gap is acknowledged in the plan.

use anyhow::{bail, Result};
use installer_core::live::LiveMedia;
use installer_core::Step;
use std::path::{Path, PathBuf};

/// Validate every step against the detected live medium. Called before
/// execution as a pre-flight pass. Returns an error naming the offending
/// step if it would endanger the running session.
pub fn guard_steps(steps: &[Step]) -> Result<()> {
    let live = LiveMedia::detect();
    // Fast path: nothing to protect (installed host, or running from RAM).
    if live.backing_partition.is_none() {
        return Ok(());
    }
    for (i, s) in steps.iter().enumerate() {
        guard_argv(&s.argv, &live)
            .map_err(|e| anyhow::anyhow!("step {} ({}): {}", i + 1, s.label, e))?;
    }
    Ok(())
}

/// Core check for one argv against a resolved [`LiveMedia`]. Split out so it
/// is unit-testable with synthetic media and argv shapes.
pub fn guard_argv(argv: &[String], live: &LiveMedia) -> Result<()> {
    if argv.is_empty() {
        return Ok(());
    }
    let prog = Path::new(&argv[0])
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or(&argv[0]);

    // --- mkfs.* — refuse to format the live partition or any partition that
    //     IS the live-backing device. ---
    if prog.starts_with("mkfs.") {
        for a in &argv[1..] {
            if a.starts_with("/dev/") && is_live_target(Path::new(a), live) {
                bail!(
                    "refusing to format {} — it is the device the running \
                     NimbleX session booted from. Reboot with the 'Copy to RAM' \
                     option, or run the installer from a separate USB stick.",
                    a
                );
            }
        }
        return Ok(());
    }

    // --- copy-system without --inplace-live — refuse the truncating copy
    //     onto the live partition. ---
    let is_internal = prog == "nimblex-installer-helper-internal";
    if is_internal && argv.iter().any(|a| a == "copy-system") {
        let inplace = argv.iter().any(|a| a == "--inplace-live");
        if let Some(root) = flag_value(argv, "--root") {
            if is_live_target(Path::new(&root), live) && !inplace {
                bail!(
                    "refusing a destructive copy onto the live partition {} \
                     without the crash-safe in-place strategy. This is an \
                     internal safety check; the planner must emit \
                     'copy-system --inplace-live' for the running partition.",
                    root.display()
                );
            }
        }
    }

    Ok(())
}

/// True when `dev` is the live partition itself or the whole disk it lives on.
fn is_live_target(dev: &Path, live: &LiveMedia) -> bool {
    live.is_live_partition(dev) || live.is_live_disk(dev)
}

/// Value following `flag` in `argv` (supports `--flag value`).
fn flag_value(argv: &[String], flag: &str) -> Option<PathBuf> {
    let mut it = argv.iter();
    while let Some(a) = it.next() {
        if a == flag {
            return it.next().map(PathBuf::from);
        }
        if let Some(v) = a.strip_prefix(&format!("{flag}=")) {
            return Some(PathBuf::from(v));
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn live() -> LiveMedia {
        LiveMedia {
            backing_partition: Some(PathBuf::from("/dev/nvme0n1p5")),
            backing_disk: Some(PathBuf::from("/dev/nvme0n1")),
            running_from_ram: false,
            unresolved: false,
        }
    }

    fn argv(parts: &[&str]) -> Vec<String> {
        parts.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn blocks_mkfs_on_live_partition() {
        let r = guard_argv(
            &argv(&["mkfs.ext4", "-F", "-L", "X", "/dev/nvme0n1p5"]),
            &live(),
        );
        assert!(r.is_err());
    }

    #[test]
    fn blocks_mkfs_on_live_disk() {
        let r = guard_argv(&argv(&["mkfs.ext4", "/dev/nvme0n1"]), &live());
        assert!(r.is_err());
    }

    #[test]
    fn allows_mkfs_on_other_device() {
        let r = guard_argv(&argv(&["mkfs.ext4", "-F", "/dev/sdb2"]), &live());
        assert!(r.is_ok());
    }

    #[test]
    fn blocks_truncating_copy_onto_live() {
        let r = guard_argv(
            &argv(&[
                "nimblex-installer-helper-internal",
                "copy-system",
                "--root",
                "/dev/nvme0n1p5",
            ]),
            &live(),
        );
        assert!(r.is_err());
    }

    #[test]
    fn allows_inplace_copy_onto_live() {
        let r = guard_argv(
            &argv(&[
                "nimblex-installer-helper-internal",
                "copy-system",
                "--root",
                "/dev/nvme0n1p5",
                "--inplace-live",
            ]),
            &live(),
        );
        assert!(r.is_ok());
    }

    #[test]
    fn allows_copy_onto_other_device() {
        let r = guard_argv(
            &argv(&[
                "nimblex-installer-helper-internal",
                "copy-system",
                "--root",
                "/dev/sdb2",
            ]),
            &live(),
        );
        assert!(r.is_ok());
    }

    #[test]
    fn from_ram_blocks_nothing() {
        let ram = LiveMedia {
            running_from_ram: true,
            ..Default::default()
        };
        // backing_partition is None when from RAM; guard_steps short-circuits,
        // and guard_argv matches nothing.
        assert!(guard_argv(&argv(&["mkfs.ext4", "/dev/sda1"]), &ram).is_ok());
    }
}
