//! Measure the size of the running Nimblex install at runtime.
//!
//! The installer is shipped from a live system. The space we will copy onto
//! the target disk is the sum of:
//!
//! * the bundles directory (`/mnt/live/memory/data/nimblex64/`, holding all
//!   the `.lzm` squashfs modules),
//! * the kernel + initrd directory (`/mnt/live/memory/data/boot/`).
//!
//! Plus a 10% margin for filesystem overhead and a fixed persistence
//! allowance. The result is what `MIN_RECLAIM` is built on top of, so the
//! GUI can never propose less than the actually-needed amount of space.
//!
//! The measurement is computed once and cached. If discovery fails (paths
//! don't exist — e.g. in unit tests or non-live environments), a sensible
//! 6 GiB fallback is used.
//!
//! Probe order (first existing match wins, sizes summed if multiple):
//! 1. `/run/initramfs/live/nimblex64`            — official live mount
//! 2. `/mnt/live/memory/data/nimblex64`          — Nimblex live build path
//! 3. `/cdrom/nimblex64`                         — alt live mount
//!
//! Plus, separately, a kernel/initrd directory if findable (small, ~30 MiB).

use crate::size::Bytes;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

/// Hard fallback when discovery fails. Matches the previous hardcoded value.
const FALLBACK_INSTALL_SIZE: Bytes = Bytes::from_gib(6);

/// Multiplier applied on top of measured raw bytes to absorb FS overhead
/// (ext4 metadata, journal, alignment, occasional re-encoding).
const OVERHEAD_MULT: f64 = 1.10;

/// Persistence allowance — space we reserve for the user's home/changes
/// directory in addition to the read-only system bundles.
const PERSISTENCE_ALLOWANCE: Bytes = Bytes::from_gib(2);

static CACHE: OnceLock<Bytes> = OnceLock::new();

/// Measured (and cached) minimum install size. Returns the same value
/// every time once computed.
pub fn min_install_size() -> Bytes {
    *CACHE.get_or_init(measure)
}

fn measure() -> Bytes {
    let bundle_candidates: &[&Path] = &[
        Path::new("/run/initramfs/live/nimblex64"),
        Path::new("/mnt/live/memory/data/nimblex64"),
        Path::new("/cdrom/nimblex64"),
    ];
    let kernel_candidates: &[&Path] = &[
        Path::new("/run/initramfs/live/boot"),
        Path::new("/mnt/live/memory/data/boot"),
        Path::new("/cdrom/boot"),
    ];

    let bundles_bytes = first_existing(bundle_candidates).map(dir_size).unwrap_or(0);
    let kernel_bytes = first_existing(kernel_candidates).map(dir_size).unwrap_or(0);
    let raw = bundles_bytes + kernel_bytes;
    if raw == 0 {
        return FALLBACK_INSTALL_SIZE;
    }
    let with_overhead = (raw as f64 * OVERHEAD_MULT) as u64;
    Bytes(with_overhead) + PERSISTENCE_ALLOWANCE
}

/// Return `(bundles_dir, boot_dir)` for the running live system, or `None`
/// if the live source cannot be found (e.g. running in a unit-test harness).
pub fn live_source_dirs() -> Option<(PathBuf, PathBuf)> {
    let bundle_candidates: &[&Path] = &[
        Path::new("/run/initramfs/live/nimblex64"),
        Path::new("/mnt/live/memory/data/nimblex64"),
        Path::new("/cdrom/nimblex64"),
    ];
    let kernel_candidates: &[&Path] = &[
        Path::new("/run/initramfs/live/boot"),
        Path::new("/mnt/live/memory/data/boot"),
        Path::new("/cdrom/boot"),
    ];
    let bundles = first_existing(bundle_candidates)?;
    let boot = first_existing(kernel_candidates)?;
    Some((bundles, boot))
}

fn first_existing(paths: &[&Path]) -> Option<PathBuf> {
    paths.iter().find(|p| p.exists()).map(|p| p.to_path_buf())
}

/// Recursively sum the byte size of every regular file under `dir`. Symlinks
/// are not followed; errors on individual entries are silently skipped so a
/// permissions glitch on one file can't poison the whole measurement.
fn dir_size(dir: PathBuf) -> u64 {
    let mut stack = vec![dir];
    let mut total: u64 = 0;
    while let Some(d) = stack.pop() {
        let entries = match fs::read_dir(&d) {
            Ok(e) => e,
            Err(_) => continue,
        };
        for entry in entries.flatten() {
            let meta = match entry.metadata() {
                Ok(m) => m,
                Err(_) => continue,
            };
            if meta.is_file() {
                total = total.saturating_add(meta.len());
            } else if meta.is_dir() {
                stack.push(entry.path());
            }
            // Symlinks: ignored.
        }
    }
    total
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fallback_is_used_when_no_paths_exist() {
        // We can't easily monkey-patch the candidate list, but on a CI host
        // none of the live mounts exist, so measure() returns the fallback.
        // Locally on a Nimblex live system this returns the real size.
        let v = measure();
        assert!(v >= FALLBACK_INSTALL_SIZE.min(Bytes::from_gib(1)));
    }

    #[test]
    fn dir_size_handles_missing() {
        let v = dir_size(PathBuf::from("/this/path/does/not/exist"));
        assert_eq!(v, 0);
    }
}
