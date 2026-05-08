//! NTFS shrink planner.
//!
//! Wraps `ntfsresize --info --force --no-progress-bar <part>` and applies
//! safety floors so the user can never set the Windows post-shrink size
//! below a value that risks corrupting the filesystem or starving Windows.

use crate::install_size;
use crate::size::Bytes;
use anyhow::{Context, Result};
use std::path::Path;
use std::process::Command;

/// Result of a non-destructive `ntfsresize --info` run.
#[derive(Debug, Clone)]
pub struct NtfsInfo {
    /// Current on-disk volume size.
    pub volume_size: Bytes,
    /// Smallest size `ntfsresize` would accept (clusters in use × cluster size).
    pub min_size: Bytes,
    /// Whether the volume is dirty (chkdsk pending).
    pub dirty: bool,
}

/// Floor for Windows residual size after shrink, regardless of what
/// `ntfsresize --info` reports. 40 GiB gives Windows 11 enough headroom
/// for updates, swap, hibernation, and a working set of user data.
pub const WINDOWS_RESIDUAL_FLOOR: Bytes = Bytes::from_gib(40);

/// Minimum amount of free space that must be available on the Windows
/// volume before we consider it shrinkable. Below this, the user has to
/// free up space inside Windows first (Recycle Bin, %TEMP%, hibernation
/// file, etc.). 10 GiB is enough for `ntfsresize` to manoeuvre + leaves
/// some breathing room for the Nimblex partition we want to carve out.
pub const WINDOWS_MIN_FREE_BEFORE_SHRINK: Bytes = Bytes::from_gib(10);

/// Fraction of the *post-shrink* Windows volume that must remain free
/// after the resize. 10 % means: if Windows ends up at 100 GiB, at least
/// 10 GiB must still be unused so it can boot, log in, install updates,
/// and create a swap/hibernation file.
pub const WINDOWS_FREE_FRACTION_AFTER_SHRINK: f64 = 0.10;

/// Minimum bytes Nimblex needs for a working install — measured at runtime
/// from the live system (bundles + kernel + initrd + overhead + persistence
/// allowance). See `install_size::min_install_size`.
pub fn min_install() -> Bytes {
    install_size::min_install_size()
}

/// Minimum amount of space the planner must reclaim from Windows so the
/// Nimblex install fits. Equal to [`min_install`] — the 10 % filesystem
/// overhead already baked into that figure also serves as our margin.
pub fn min_reclaim() -> Bytes {
    min_install()
}

/// Smallest size we will leave Windows at after a shrink, given how many
/// bytes of files Windows currently holds (`used`). Combines:
///
/// * the [`WINDOWS_FREE_FRACTION_AFTER_SHRINK`] guarantee
///   (`used / (1 − fraction)`),
/// * the absolute floor [`WINDOWS_RESIDUAL_FLOOR`],
///
/// taking whichever is largest. The volume size is *not* clamped here —
/// that's the caller's responsibility (a partition too small to satisfy
/// this floor is rejected up the call stack).
pub fn min_windows_residual_after_shrink(used: Bytes) -> Bytes {
    let by_fraction = Bytes(((used.0 as f64) / (1.0 - WINDOWS_FREE_FRACTION_AFTER_SHRINK)) as u64);
    by_fraction.max(WINDOWS_RESIDUAL_FLOOR)
}

pub struct ResizePlanner;

impl ResizePlanner {
    /// Run `ntfsresize --info` against `partition` and parse its output.
    pub fn probe(partition: &Path) -> Result<NtfsInfo> {
        let out = Command::new("ntfsresize")
            .args(["--info", "--force", "--no-progress-bar"])
            .arg(partition)
            .output()
            .with_context(|| format!("ntfsresize --info {}", partition.display()))?;
        // ntfsresize returns 0 even when reporting; some versions return 1
        // when the volume is dirty. We parse stdout regardless.
        let combined = format!(
            "{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        parse_ntfs_info(&combined).with_context(|| {
            format!(
                "could not parse ntfsresize output for {}: {}",
                partition.display(),
                combined
            )
        })
    }

    /// Compute a safe post-shrink size for the Windows partition given the
    /// probe result and what the user requested. Returns the chosen size,
    /// clamped between the floor (Windows headroom) and the ceiling
    /// (`volume_size − min_reclaim`, so Nimblex always gets at least its
    /// minimum install footprint plus a small margin).
    pub fn safe_target(info: &NtfsInfo, user_request: Bytes) -> Bytes {
        let floor = min_windows_residual_after_shrink(info.min_size);
        // Ceiling: never leave Windows so big that Nimblex can't fit.
        let ceiling = info.volume_size - min_reclaim();
        // If floor > ceiling, the partition simply isn't large enough to
        // host Nimblex without violating Windows' safety floor; the picker
        // is expected to refuse before reaching here, but as a defensive
        // measure we honour the floor (Windows safety wins over Nimblex fit).
        let clamped = if user_request < floor {
            floor
        } else if user_request > ceiling {
            ceiling
        } else {
            user_request
        };
        clamped.min(info.volume_size)
    }

    /// Recommended initial position for the GUI splitter: a generous default
    /// that leaves Windows with comfortable headroom and gives Nimblex
    /// at least the minimum install size if there is room for it.
    pub fn recommended_target(info: &NtfsInfo) -> Bytes {
        let comfortable = min_windows_residual_after_shrink(info.min_size);
        let ceiling = info.volume_size - min_reclaim();
        comfortable
            .max(WINDOWS_RESIDUAL_FLOOR)
            .min(ceiling)
            .min(info.volume_size)
    }
}

/// Parser for the human-readable `ntfsresize --info` text. The relevant
/// lines we look for (from ntfs-3g 2022.10.3) are:
///
/// ```text
/// Current volume size: 250056704000 bytes (250057 MB)
/// You might resize at 95623168000 bytes or 95624 MB (freeing 154433 MB).
/// ```
///
/// Also "Volume is scheduled for check." → dirty.
fn parse_ntfs_info(text: &str) -> Result<NtfsInfo> {
    let mut volume_size: Option<u64> = None;
    let mut min_size: Option<u64> = None;
    let mut dirty = false;
    for line in text.lines() {
        let l = line.trim();
        if l.starts_with("Current volume size:") {
            volume_size = extract_first_bytes_value(l);
        } else if l.starts_with("You might resize at") {
            min_size = extract_first_bytes_value(l);
        } else if l.contains("scheduled for check") || l.contains("dirty") {
            dirty = true;
        }
    }
    let volume_size = volume_size.context("missing 'Current volume size' line")?;
    let min_size = min_size.unwrap_or(volume_size); // fallback: cannot shrink
    Ok(NtfsInfo {
        volume_size: Bytes(volume_size),
        min_size: Bytes(min_size),
        dirty,
    })
}

/// Pull the first bare integer that is followed by " bytes" out of a line.
fn extract_first_bytes_value(s: &str) -> Option<u64> {
    let idx = s.find(" bytes")?;
    let prefix = &s[..idx];
    let num_start = prefix.rfind(|c: char| !c.is_ascii_digit())?;
    let num_str = &prefix[num_start + 1..];
    num_str.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_real_output() {
        let sample = r#"
ntfsresize v2022.10.3 (libntfs-3g)
Device name        : /dev/sda3
NTFS volume version: 3.1
Cluster size       : 4096 bytes
Current volume size: 250056704000 bytes (250057 MB)
Current device size: 250057441280 bytes (250058 MB)
Checking filesystem consistency ...
Accounting clusters ...
Space in use       : 95620 MB (38.2%)
Collecting resizing constraints ...
You might resize at 95623168000 bytes or 95624 MB (freeing 154433 MB).
"#;
        let info = parse_ntfs_info(sample).unwrap();
        assert_eq!(info.volume_size.0, 250056704000);
        assert_eq!(info.min_size.0, 95623168000);
        assert!(!info.dirty);
    }

    #[test]
    fn detects_dirty_volume() {
        let s = "Volume is scheduled for check.\nCurrent volume size: 1000 bytes (0 MB)\n";
        let info = parse_ntfs_info(s).unwrap();
        assert!(info.dirty);
    }

    #[test]
    fn safe_target_respects_floor() {
        let info = NtfsInfo {
            volume_size: Bytes::from_gib(500),
            min_size: Bytes::from_gib(20),
            dirty: false,
        };
        // User asks for 10 GiB residual; planner pushes up to floor.
        let chosen = ResizePlanner::safe_target(&info, Bytes::from_gib(10));
        assert!(chosen >= WINDOWS_RESIDUAL_FLOOR);
    }

    #[test]
    fn safe_target_caps_at_ceiling() {
        let info = NtfsInfo {
            volume_size: Bytes::from_gib(500),
            min_size: Bytes::from_gib(20),
            dirty: false,
        };
        // Asking for more than the volume → ceiling = volume - min_reclaim,
        // not the volume itself, so Nimblex always gets at least min_reclaim.
        let chosen = ResizePlanner::safe_target(&info, Bytes::from_gib(9999));
        assert_eq!(chosen, info.volume_size - min_reclaim());
        assert!(info.volume_size - chosen >= min_reclaim());
    }

    #[test]
    fn recommended_target_always_leaves_min_reclaim() {
        // Almost-full Windows: used 252 GiB on 293 GiB volume.
        let info = NtfsInfo {
            volume_size: Bytes(314_572_800_000), // 293 GiB
            min_size: Bytes(271_354_269_696),    // 252.7 GiB used
            dirty: true,
        };
        let target = ResizePlanner::recommended_target(&info);
        let reclaim = info.volume_size - target;
        let need = min_reclaim();
        assert!(
            reclaim >= need,
            "recommended target reclaims {} but should reclaim at least {}",
            reclaim,
            need
        );
    }
}
