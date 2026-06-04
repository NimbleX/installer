//! Detection of the **live boot medium** — the block device the running
//! Nimblex session was started from.
//!
//! This is safety-critical. The installer copies the system onto a target
//! partition; if the user picks the very device they booted from, naively
//! overwriting (or reformatting) it corrupts the running session, because the
//! `.lzm` squashfs bundles are loop-mounted read-only and paged in on demand.
//!
//! Two facts decide what is safe:
//!
//! * **Which block device backs the live bundles** ([`live_source_dirs`]).
//!   The `.lzm` files physically live on that device's filesystem, so the
//!   mount source for the bundles directory is exactly what must be protected.
//! * **Whether the system runs from RAM** (copy2ram / toram). When the bundles
//!   sit on `tmpfs`/`ramfs`, the physical medium is no longer read on demand
//!   and is therefore safe to modify (even reformat).
//!
//! Detection is entirely read-only and unprivileged: it parses
//! `/proc/self/mountinfo` and walks `/sys`. It never needs root, so the GUI
//! calls it directly; the helper performs its own independent check as a
//! root-side backstop.
//!
//! On a non-live host (CI, unit tests) there is no live source, so
//! [`LiveMedia::detect`] returns an empty value that blocks nothing.

use std::fs;
use std::path::{Path, PathBuf};

/// What we learned about the medium the running session booted from.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LiveMedia {
    /// The partition device backing the live bundles, e.g. `/dev/nvme0n1p5`.
    /// `None` when running from RAM or when no live source was found.
    pub backing_partition: Option<PathBuf>,
    /// The whole-disk device owning [`backing_partition`], e.g. `/dev/nvme0n1`.
    pub backing_disk: Option<PathBuf>,
    /// True when the live bundles are served from `tmpfs`/`ramfs` (copy2ram),
    /// meaning the physical medium is no longer read and is safe to modify.
    pub running_from_ram: bool,
    /// True when a live source directory exists but its backing device could
    /// not be confidently resolved (exotic mount topology). Callers must treat
    /// this as "potentially live and not from RAM" and fail safe.
    pub unresolved: bool,
}

impl LiveMedia {
    /// Detect the live medium for the running session from the mount
    /// topology.
    ///
    /// Returns an empty (blocks-nothing) value when there is no live system
    /// detectable — e.g. on a normal installed system (no live squashfs
    /// bundles) or in a unit-test harness.
    pub fn detect() -> Self {
        Self::detect_from_mounts(&read_mountinfo(), &SysfsReal)
    }

    /// True when `disk` is the whole-disk device the session booted from.
    pub fn is_live_disk(&self, disk: &Path) -> bool {
        self.backing_disk.as_deref() == Some(disk)
    }

    /// True when `part` is the exact partition the session booted from.
    pub fn is_live_partition(&self, part: &Path) -> bool {
        self.backing_partition.as_deref() == Some(part)
    }

    /// Whether *reformatting or repartitioning* `disk` could endanger the
    /// running session. Safe (false) when running from RAM. When the live
    /// device is unresolved we cannot name the disk, so this returns false and
    /// the root-side helper guard becomes the net (see [`crate::live`] docs).
    pub fn disk_is_unsafe_to_modify(&self, disk: &Path) -> bool {
        if self.running_from_ram {
            return false;
        }
        self.is_live_disk(disk)
    }

    /// Whether an **in-place, no-format** install onto `part` must use the
    /// crash-safe (atomic-rename, no-unmount) copy strategy because `part` is
    /// the live partition and we are not running from RAM.
    pub fn partition_needs_inplace_live(&self, part: &Path) -> bool {
        !self.running_from_ram && self.is_live_partition(part)
    }

    // --- testable core ----------------------------------------------------

    /// Resolve the live medium from the mount table plus a sysfs accessor.
    /// Split out from [`detect`] so it can be unit-tested with fixtures.
    ///
    /// Strategy: the running system is built from read-only `squashfs`
    /// bundles loop-mounted under the live memory area. Each loop has a
    /// *backing file*. If that backing file sits on a currently-mounted real
    /// block partition, that partition is the medium we booted from and must
    /// be protected. If every bundle's backing file is RAM-resident (copy2ram
    /// — the backing path is not under any mounted block device), the physical
    /// medium is no longer read and is safe to modify.
    fn detect_from_mounts(mountinfo: &[MountEntry], sysfs: &dyn Sysfs) -> Self {
        // The live bundles: read-only squashfs images loop-mounted inside the
        // live memory area. (A snap/flatpak squashfs elsewhere is ignored by
        // the mountpoint filter.)
        let live_loops: Vec<&MountEntry> = mountinfo
            .iter()
            .filter(|m| m.fstype == "squashfs" && is_loop_device(&m.source))
            .filter(|m| is_live_bundle_mountpoint(&m.mount_point))
            .collect();

        if live_loops.is_empty() {
            // Not a live system (installed target, or CI). Nothing to protect.
            return Self::default();
        }

        let mut resolved_any = false;
        for lp in &live_loops {
            let Some(backing) = sysfs.loop_backing_file(&lp.source) else {
                continue;
            };
            resolved_any = true;
            if let Some(part) = real_block_mount_for(&backing, mountinfo) {
                // Booted from a real device that is still mounted: protect it.
                let partition = PathBuf::from(part);
                let backing_disk = sysfs.parent_disk(&partition);
                return Self {
                    backing_partition: Some(partition),
                    backing_disk,
                    running_from_ram: false,
                    unresolved: false,
                };
            }
        }

        if resolved_any {
            // Every bundle's backing file is RAM-resident (copy2ram): the
            // physical medium is no longer read, so modifying it is safe.
            Self {
                running_from_ram: true,
                ..Default::default()
            }
        } else {
            // Live bundles exist but none could be resolved to a backing file
            // (sysfs unavailable / exotic setup). Do not claim RAM-safe; fail
            // safe and let the root-side helper guard be the net.
            Self {
                unresolved: true,
                ..Default::default()
            }
        }
    }
}

fn is_loop_device(source: &str) -> bool {
    source.starts_with("/dev/loop")
}

/// A real, persistent block-device mount source (`/dev/sda1`, `/dev/nvme0n1p5`,
/// `/dev/mmcblk0p2`, …) — i.e. a `/dev` node that is not a loop device.
fn is_real_block_source(source: &str) -> bool {
    source.starts_with("/dev/") && !is_loop_device(source)
}

/// True for mountpoints that belong to the live system's bundle area, so a
/// stray squashfs (snap at `/snap/...`, flatpak at `/var/lib/flatpak/...`)
/// can't be mistaken for a live bundle.
fn is_live_bundle_mountpoint(mp: &Path) -> bool {
    path_starts_with(mp, Path::new("/mnt/live"))
        || path_starts_with(mp, Path::new("/run/initramfs"))
        || mp.to_string_lossy().contains("/memory/bundles/")
}

/// The source of the real block-device mount whose mountpoint is the longest
/// path-prefix of `path`, or `None` when `path` is not under any mounted real
/// block device (e.g. it is RAM-resident / a stale initramfs path).
fn real_block_mount_for(path: &Path, mounts: &[MountEntry]) -> Option<String> {
    let mut best: Option<&MountEntry> = None;
    let mut best_len = 0usize;
    for m in mounts {
        if is_real_block_source(&m.source) && path_starts_with(path, &m.mount_point) {
            let len = m.mount_point.components().count();
            if best.is_none() || len >= best_len {
                best = Some(m);
                best_len = len;
            }
        }
    }
    best.map(|m| m.source.clone())
}

// --- /proc/self/mountinfo parsing -------------------------------------------

/// A single parsed line of `/proc/self/mountinfo` (only the fields we use).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MountEntry {
    /// Absolute mount point, e.g. `/run/initramfs/live`.
    pub mount_point: PathBuf,
    /// Filesystem type, e.g. `ext4`, `tmpfs`, `squashfs`.
    pub fstype: String,
    /// Mount source, e.g. `/dev/nvme0n1p5`, `tmpfs`, `/dev/loop3`.
    pub source: String,
}

fn read_mountinfo() -> Vec<MountEntry> {
    let text = fs::read_to_string("/proc/self/mountinfo").unwrap_or_default();
    parse_mountinfo(&text)
}

/// Parse `/proc/self/mountinfo`. Format per line:
///
/// ```text
/// ID PID MAJ:MIN ROOT MOUNTPOINT OPTS [OPTIONAL...] - FSTYPE SOURCE SUPEROPTS
/// ```
///
/// The variable-length optional fields end at a literal `-` separator, after
/// which come exactly fstype, source, super-options.
pub fn parse_mountinfo(text: &str) -> Vec<MountEntry> {
    let mut out = Vec::new();
    for line in text.lines() {
        let Some(sep) = line.find(" - ") else {
            continue;
        };
        let (left, right) = (&line[..sep], &line[sep + 3..]);
        let left_fields: Vec<&str> = left.split_whitespace().collect();
        let right_fields: Vec<&str> = right.split_whitespace().collect();
        if left_fields.len() < 5 || right_fields.len() < 2 {
            continue;
        }
        out.push(MountEntry {
            mount_point: PathBuf::from(unescape_octal(left_fields[4])),
            fstype: right_fields[0].to_string(),
            source: unescape_octal(right_fields[1]),
        });
    }
    out
}

/// mountinfo escapes space/tab/newline/backslash as octal (`\040` etc.).
fn unescape_octal(s: &str) -> String {
    if !s.contains('\\') {
        return s.to_string();
    }
    let bytes = s.as_bytes();
    let mut out = String::with_capacity(s.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'\\' && i + 3 < bytes.len() {
            let oct = &s[i + 1..i + 4];
            if let Ok(code) = u8::from_str_radix(oct, 8) {
                out.push(code as char);
                i += 4;
                continue;
            }
        }
        out.push(bytes[i] as char);
        i += 1;
    }
    out
}

/// True when `path` is `prefix` or lies beneath it, comparing whole
/// components (so `/run/live` is not a prefix of `/run/lived`).
fn path_starts_with(path: &Path, prefix: &Path) -> bool {
    let mut pc = path.components();
    for c in prefix.components() {
        match pc.next() {
            Some(p) if p == c => continue,
            _ => return false,
        }
    }
    true
}

// --- sysfs access (mockable) ------------------------------------------------

/// Read-only sysfs queries, abstracted so detection can be unit-tested with
/// fixtures instead of a real `/sys`.
trait Sysfs {
    /// Resolve a `/dev/loopN` device to the file it is backed by, reading
    /// `/sys/block/loopN/loop/backing_file`. Strips a trailing " (deleted)".
    fn loop_backing_file(&self, loop_dev: &str) -> Option<PathBuf>;
    /// Resolve the whole-disk device owning a partition, e.g.
    /// `/dev/nvme0n1p5` -> `/dev/nvme0n1`, `/dev/sdb1` -> `/dev/sdb`.
    fn parent_disk(&self, partition: &Path) -> Option<PathBuf>;
}

struct SysfsReal;

impl Sysfs for SysfsReal {
    fn loop_backing_file(&self, loop_dev: &str) -> Option<PathBuf> {
        let name = Path::new(loop_dev).file_name()?.to_str()?;
        let path = format!("/sys/block/{}/loop/backing_file", name);
        let raw = fs::read_to_string(path).ok()?;
        let trimmed = raw.trim().trim_end_matches(" (deleted)").trim();
        if trimmed.is_empty() {
            None
        } else {
            Some(PathBuf::from(trimmed))
        }
    }

    fn parent_disk(&self, partition: &Path) -> Option<PathBuf> {
        let name = partition.file_name()?.to_str()?;
        // /sys/class/block/<part> is a symlink ending .../<disk>/<part>.
        let link = format!("/sys/class/block/{}", name);
        if let Ok(real) = fs::canonicalize(&link) {
            // Only a partition has a `partition` attribute file.
            if real.join("partition").exists() {
                if let Some(parent) = real
                    .parent()
                    .and_then(|p| p.file_name())
                    .and_then(|p| p.to_str())
                {
                    return Some(PathBuf::from(format!("/dev/{}", parent)));
                }
            }
        }
        // Fallback: strip a trailing partition suffix by string rules.
        Some(PathBuf::from(strip_partition_suffix(partition)))
    }
}

/// `/dev/nvme0n1p5` -> `/dev/nvme0n1`, `/dev/mmcblk0p2` -> `/dev/mmcblk0`,
/// `/dev/sdb1` -> `/dev/sdb`. Used only as a sysfs fallback.
fn strip_partition_suffix(partition: &Path) -> String {
    let s = partition.to_string_lossy();
    let bytes = s.as_bytes();
    // Trim trailing digits.
    let mut end = bytes.len();
    while end > 0 && bytes[end - 1].is_ascii_digit() {
        end -= 1;
    }
    // For nvme/mmcblk style the digit run is preceded by 'p'.
    if end > 0 && bytes[end - 1] == b'p' {
        // Only treat 'p' as a separator when what precedes it is a digit
        // (nvme0n1p5) — otherwise 'p' is part of the disk name (loop has no
        // partitions here so this is conservative).
        if end >= 2 && bytes[end - 2].is_ascii_digit() {
            end -= 1;
        }
    }
    s[..end].to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Sysfs mock driven by fixed maps.
    struct SysfsMock {
        loops: std::collections::HashMap<String, PathBuf>,
        disks: std::collections::HashMap<PathBuf, PathBuf>,
    }
    impl Sysfs for SysfsMock {
        fn loop_backing_file(&self, loop_dev: &str) -> Option<PathBuf> {
            self.loops.get(loop_dev).cloned()
        }
        fn parent_disk(&self, partition: &Path) -> Option<PathBuf> {
            self.disks
                .get(partition)
                .cloned()
                .or_else(|| Some(PathBuf::from(strip_partition_suffix(partition))))
        }
    }
    fn empty_mock() -> SysfsMock {
        SysfsMock {
            loops: Default::default(),
            disks: Default::default(),
        }
    }

    fn entry(mp: &str, fstype: &str, source: &str) -> MountEntry {
        MountEntry {
            mount_point: PathBuf::from(mp),
            fstype: fstype.into(),
            source: source.into(),
        }
    }

    #[test]
    fn parses_mountinfo_basic_line() {
        let line = "36 35 8:5 / /run/initramfs/live rw,noatime shared:1 - ext4 /dev/nvme0n1p5 rw";
        let parsed = parse_mountinfo(line);
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].mount_point, PathBuf::from("/run/initramfs/live"));
        assert_eq!(parsed[0].fstype, "ext4");
        assert_eq!(parsed[0].source, "/dev/nvme0n1p5");
    }

    #[test]
    fn mountinfo_unescapes_spaces() {
        let line = "36 35 0:5 / /mnt/with\\040space rw - tmpfs tmpfs rw";
        let parsed = parse_mountinfo(line);
        assert_eq!(parsed[0].mount_point, PathBuf::from("/mnt/with space"));
    }

    #[test]
    fn copy2ram_bundles_in_ram_means_from_ram() {
        // Real NimbleX copy2ram topology: squashfs bundles loop-mounted under
        // /mnt/live/memory/bundles, backed by files at the stale initramfs
        // path /nimblex64/*.lzm — not under any mounted real block device.
        // The boot USB (/dev/sda) is NOT mounted.
        let mut mock = empty_mock();
        mock.loops.insert(
            "/dev/loop2".into(),
            PathBuf::from("/nimblex64/01-Core64.lzm"),
        );
        let mounts = vec![
            entry("/mnt/live", "tmpfs", "tmpfs"),
            entry("/", "aufs", "aufs"),
            entry(
                "/mnt/live/memory/bundles/01-Core64.lzm",
                "squashfs",
                "/dev/loop2",
            ),
        ];
        let lm = LiveMedia::detect_from_mounts(&mounts, &mock);
        assert!(lm.running_from_ram, "copy2ram must be detected as from-RAM");
        assert!(lm.backing_partition.is_none());
        assert!(!lm.unresolved);
        // Erasing the boot USB is therefore allowed.
        assert!(!lm.disk_is_unsafe_to_modify(Path::new("/dev/sda")));
    }

    #[test]
    fn disk_boot_resolves_to_backing_partition() {
        // Non-copy2ram: the boot device stays mounted (here /dev/sdb1 at
        // /mnt/live/memory/data) and the loop's backing file lives on it.
        let mut mock = empty_mock();
        mock.loops.insert(
            "/dev/loop2".into(),
            PathBuf::from("/mnt/live/memory/data/nimblex64/01-Core64.lzm"),
        );
        let mounts = vec![
            entry("/mnt/live", "tmpfs", "tmpfs"),
            entry("/mnt/live/memory/data", "vfat", "/dev/sdb1"),
            entry(
                "/mnt/live/memory/bundles/01-Core64.lzm",
                "squashfs",
                "/dev/loop2",
            ),
        ];
        let lm = LiveMedia::detect_from_mounts(&mounts, &mock);
        assert!(!lm.running_from_ram);
        assert!(!lm.unresolved);
        assert_eq!(lm.backing_partition, Some(PathBuf::from("/dev/sdb1")));
        assert_eq!(lm.backing_disk, Some(PathBuf::from("/dev/sdb")));
        assert!(lm.disk_is_unsafe_to_modify(Path::new("/dev/sdb")));
        assert!(lm.partition_needs_inplace_live(Path::new("/dev/sdb1")));
    }

    #[test]
    fn installed_system_has_no_live_bundles() {
        // A plain installed target: no live squashfs loops at all.
        let mounts = vec![
            entry("/", "ext4", "/dev/nvme0n1p5"),
            entry("/boot/efi", "vfat", "/dev/nvme0n1p1"),
        ];
        let lm = LiveMedia::detect_from_mounts(&mounts, &empty_mock());
        assert_eq!(lm, LiveMedia::default());
        assert!(!lm.disk_is_unsafe_to_modify(Path::new("/dev/nvme0n1")));
    }

    #[test]
    fn stray_snap_squashfs_is_ignored() {
        // A snap squashfs (not under the live area) must not be mistaken for a
        // live bundle.
        let mut mock = empty_mock();
        mock.loops.insert(
            "/dev/loop9".into(),
            PathBuf::from("/var/lib/snapd/snaps/core.snap"),
        );
        let mounts = vec![
            entry("/", "ext4", "/dev/nvme0n1p5"),
            entry("/snap/core/12345", "squashfs", "/dev/loop9"),
        ];
        let lm = LiveMedia::detect_from_mounts(&mounts, &mock);
        assert_eq!(lm, LiveMedia::default());
    }

    #[test]
    fn live_bundles_unresolvable_fail_safe() {
        // Live squashfs present but sysfs can't resolve any backing file:
        // must NOT claim RAM-safe.
        let mounts = vec![
            entry("/mnt/live", "tmpfs", "tmpfs"),
            entry(
                "/mnt/live/memory/bundles/01-Core64.lzm",
                "squashfs",
                "/dev/loop2",
            ),
        ];
        // empty_mock has no loop backing entries → loop_backing_file None.
        let lm = LiveMedia::detect_from_mounts(&mounts, &empty_mock());
        assert!(lm.unresolved);
        assert!(!lm.running_from_ram);
        assert!(lm.backing_partition.is_none());
    }

    #[test]
    fn strips_partition_suffixes() {
        assert_eq!(strip_partition_suffix(Path::new("/dev/sdb1")), "/dev/sdb");
        assert_eq!(
            strip_partition_suffix(Path::new("/dev/nvme0n1p5")),
            "/dev/nvme0n1"
        );
        assert_eq!(
            strip_partition_suffix(Path::new("/dev/mmcblk0p2")),
            "/dev/mmcblk0"
        );
    }

    #[test]
    fn gating_policy_matrix() {
        // Live on /dev/nvme0n1p5, not from RAM.
        let live = LiveMedia {
            backing_partition: Some(PathBuf::from("/dev/nvme0n1p5")),
            backing_disk: Some(PathBuf::from("/dev/nvme0n1")),
            running_from_ram: false,
            unresolved: false,
        };
        // Destructive whole-disk ops on the live disk are unsafe.
        assert!(live.disk_is_unsafe_to_modify(Path::new("/dev/nvme0n1")));
        // A different disk is fine.
        assert!(!live.disk_is_unsafe_to_modify(Path::new("/dev/sda")));
        // In-place reinstall onto the live partition needs the crash-safe path.
        assert!(live.partition_needs_inplace_live(Path::new("/dev/nvme0n1p5")));
        // A different partition does not.
        assert!(!live.partition_needs_inplace_live(Path::new("/dev/sda1")));

        // From RAM: nothing is at risk, every op is permitted.
        let ram = LiveMedia {
            running_from_ram: true,
            ..Default::default()
        };
        assert!(!ram.disk_is_unsafe_to_modify(Path::new("/dev/nvme0n1")));
        assert!(!ram.partition_needs_inplace_live(Path::new("/dev/nvme0n1p5")));

        // No live source (installed host / CI): blocks nothing.
        let none = LiveMedia::default();
        assert!(!none.disk_is_unsafe_to_modify(Path::new("/dev/nvme0n1")));
        assert!(!none.partition_needs_inplace_live(Path::new("/dev/nvme0n1p5")));
    }
}
