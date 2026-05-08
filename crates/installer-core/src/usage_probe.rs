//! Best-effort usage probing for unmounted partitions.
//!
//! `lsblk -O` only populates `FSUSED` for already-mounted filesystems.
//! On a live USB the user's Windows NTFS partitions and freshly-plugged
//! USB sticks are typically unmounted, so we'd otherwise have to render
//! them as "fully used" or "unknown". This module fills in [`Partition::used`]
//! for the filesystems we can probe non-destructively without root mounts:
//!
//! * **NTFS** → `ntfsresize --info --force --no-progress-bar` (read-only).
//! * **ext2/3/4** → `dumpe2fs -h` (read-only).
//! * **swap** → trivially 0 (used space is irrelevant for the installer).
//!
//! Other filesystems (vfat, exfat, btrfs, xfs, f2fs, …) fall through with
//! `used` left as `None`; the disk-strip widget renders these with a
//! striped pattern so the user sees "we don't know" instead of "fully full".

use crate::disk::Partition;
use crate::size::Bytes;
use std::path::Path;
use std::process::Command;
use std::time::Duration;

/// Probe each partition in the slice and fill in `used` where possible.
/// Already-populated values (from a mounted filesystem) are preserved.
/// Probing is read-only and bounded; failures are silently ignored.
pub fn probe_partitions(parts: &mut [Partition]) {
    for p in parts.iter_mut() {
        if p.used.is_some() {
            continue;
        }
        if let Some(used) = probe_one(&p.path, &p.fs) {
            p.used = Some(used);
        }
    }
}

fn probe_one(dev: &Path, fs: &str) -> Option<Bytes> {
    match fs {
        "ntfs" => probe_ntfs(dev),
        "ext2" | "ext3" | "ext4" => probe_extfs(dev),
        "swap" => Some(Bytes(0)),
        _ => None,
    }
}

fn probe_ntfs(dev: &Path) -> Option<Bytes> {
    // First try the friendly tool — it gives us the exact same number Windows
    // would report. Falls through on dirty/hibernated volumes (Fast Startup).
    if let Some(out) = run_with_timeout(
        Command::new("ntfsresize").args([
            "--info",
            "--force",
            "--no-progress-bar",
        ]).arg(dev),
        Duration::from_secs(8),
    ) {
        if let Some(used) = parse_ntfs_used(&out) {
            return Some(used);
        }
    }
    // Fallback: read the raw $Bitmap. Works on dirty/hibernated volumes
    // because we don't touch the journal or $MFTMirr.
    crate::ntfs_raw::probe_used(dev)
}

/// Looks for a line like:
///     Space in use       :     91189 MB (29.0%)
/// and returns the byte count. `ntfsresize` reports MB == 1_000_000 bytes
/// (decimal) so we multiply accordingly.
fn parse_ntfs_used(text: &str) -> Option<Bytes> {
    for line in text.lines() {
        let l = line.trim();
        if !l.starts_with("Space in use") {
            continue;
        }
        // Find the integer between the colon and " MB".
        let after_colon = l.split_once(':')?.1.trim();
        let num_end = after_colon.find(|c: char| !c.is_ascii_digit())?;
        let n: u64 = after_colon[..num_end].parse().ok()?;
        return Some(Bytes(n * 1_000_000));
    }
    None
}

fn probe_extfs(dev: &Path) -> Option<Bytes> {
    let out = run_with_timeout(
        Command::new("dumpe2fs").arg("-h").arg(dev),
        Duration::from_secs(4),
    )?;
    parse_ext_used(&out)
}

/// dumpe2fs -h prints (among many lines):
///     Block count:              47185920
///     Free blocks:              28948945
///     Block size:               4096
fn parse_ext_used(text: &str) -> Option<Bytes> {
    let mut block_count: Option<u64> = None;
    let mut free_blocks: Option<u64> = None;
    let mut block_size: Option<u64> = None;
    for line in text.lines() {
        let l = line.trim();
        if let Some(v) = scan_kv(l, "Block count:") {
            block_count = Some(v);
        } else if let Some(v) = scan_kv(l, "Free blocks:") {
            free_blocks = Some(v);
        } else if let Some(v) = scan_kv(l, "Block size:") {
            block_size = Some(v);
        }
    }
    let bc = block_count?;
    let fb = free_blocks?;
    let bs = block_size?;
    Some(Bytes(bc.saturating_sub(fb).saturating_mul(bs)))
}

fn scan_kv(line: &str, key: &str) -> Option<u64> {
    let rest = line.strip_prefix(key)?.trim();
    let end = rest.find(|c: char| !c.is_ascii_digit()).unwrap_or(rest.len());
    rest[..end].parse().ok()
}

/// Spawn the command, wait up to `timeout`, return combined stdout+stderr
/// on success. We don't care about the exit code — `ntfsresize --info`
/// returns 1 when the volume is dirty but still prints usable output.
fn run_with_timeout(cmd: &mut Command, timeout: Duration) -> Option<String> {
    use std::io::Read;
    use std::process::Stdio;
    let mut child = cmd
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .ok()?;
    let start = std::time::Instant::now();
    loop {
        match child.try_wait().ok()? {
            Some(_) => break,
            None => {
                if start.elapsed() > timeout {
                    let _ = child.kill();
                    let _ = child.wait();
                    return None;
                }
                std::thread::sleep(Duration::from_millis(50));
            }
        }
    }
    let mut out = String::new();
    if let Some(mut s) = child.stdout.take() {
        let _ = s.read_to_string(&mut out);
    }
    if let Some(mut s) = child.stderr.take() {
        let _ = s.read_to_string(&mut out);
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_ntfs_space_in_use() {
        let s = "Cluster size       : 4096 bytes\n\
                 Current volume size: 250056704000 bytes (250057 MB)\n\
                 Space in use       :     91189 MB (29.0%)\n";
        let used = parse_ntfs_used(s).unwrap();
        assert_eq!(used.0, 91_189u64 * 1_000_000);
    }

    #[test]
    fn parses_ext_block_count() {
        let s = "Block count:              47185920\n\
                 Free blocks:              28948945\n\
                 Block size:               4096\n";
        let used = parse_ext_used(s).unwrap();
        assert_eq!(used.0, (47185920u64 - 28948945) * 4096);
    }

    #[test]
    fn ext_missing_field_returns_none() {
        let s = "Block count:              47185920\n";
        assert!(parse_ext_used(s).is_none());
    }
}
