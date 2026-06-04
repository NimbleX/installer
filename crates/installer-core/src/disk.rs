//! Disk and partition data model.

use crate::size::Bytes;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// A physical block device the installer might target.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Disk {
    /// e.g. `/dev/nvme0n1`, `/dev/sda`.
    pub path: PathBuf,
    pub size: Bytes,
    /// True if `lsblk` reports the device as removable (USB sticks, SD cards).
    pub removable: bool,
    /// Device model string from sysfs (vendor + model concatenated). May be empty.
    pub model: String,
    /// Bus transport: `usb`, `nvme`, `sata`, `ata`, or `unknown`.
    pub transport: String,
    pub table_type: TableType,
    pub partitions: Vec<Partition>,
}

impl Disk {
    /// Sum of free space gaps between partitions plus trailing space.
    /// Computed by [`crate::scan::DiskScanner`].
    pub fn unallocated(&self) -> Bytes {
        let used: u64 = self.partitions.iter().map(|p| p.size.0).sum();
        Bytes(self.size.0.saturating_sub(used))
    }

    pub fn has_windows(&self) -> bool {
        self.partitions
            .iter()
            .any(|p| matches!(p.role, PartitionRole::WindowsSystem))
    }

    /// Pick the most likely "C: drive" — the writable NTFS partition the
    /// user actually boots Windows from. Heuristic, in order of preference:
    ///
    /// 1. `role == WindowsSystem` AND label matches `^(C:|Windows|System|OS|Boot)$` (case-insensitive).
    /// 2. Largest `WindowsSystem` partition.
    /// 3. Largest non-protected NTFS partition.
    ///
    /// Returns `None` if no candidate exists.
    pub fn primary_windows_partition(&self) -> Option<&Partition> {
        let by_label = self.partitions.iter().find(|p| {
            matches!(p.role, PartitionRole::WindowsSystem) && {
                let l = p.label.to_ascii_lowercase();
                l == "c:" || l == "windows" || l == "system" || l == "os" || l == "boot"
            }
        });
        if by_label.is_some() {
            return by_label;
        }
        let largest_system = self
            .partitions
            .iter()
            .filter(|p| matches!(p.role, PartitionRole::WindowsSystem))
            .max_by_key(|p| p.size.0);
        if largest_system.is_some() {
            return largest_system;
        }
        // Fall back: largest non-protected NTFS.
        self.partitions
            .iter()
            .filter(|p| !p.protected && p.fs.eq_ignore_ascii_case("ntfs"))
            .max_by_key(|p| p.size.0)
    }

    /// The partition that physically sits last on the disk (highest
    /// `start + size`). Returns `None` if the disk has no partitions.
    /// Used by the "install into trailing free space" path to pick the
    /// `--after-number` argument for `mkpart-after`.
    pub fn last_partition_by_end(&self) -> Option<&Partition> {
        self.partitions.iter().max_by_key(|p| p.start.0 + p.size.0)
    }

    /// Bytes of unallocated space *after* the last partition on the disk.
    /// This is the only contiguous gap we can safely consume without
    /// touching any existing partition (encrypted Windows, MSR, WinRE, …).
    pub fn trailing_free(&self) -> Bytes {
        let last_end = self
            .last_partition_by_end()
            .map(|p| p.start.0 + p.size.0)
            .unwrap_or(0);
        Bytes(self.size.0.saturating_sub(last_end))
    }

    /// All unallocated gaps on the disk, in physical disk order. Each gap
    /// records the number of the partition that physically precedes it
    /// (`None` when the gap sits before the very first partition). Gaps
    /// smaller than 1 MiB (alignment slivers) are ignored.
    ///
    /// Crucial for disks where Windows was shrunk leaving a large gap
    /// *between* the Windows partition and a trailing recovery partition:
    /// [`trailing_free`] would only see the tiny space after the recovery
    /// partition, whereas the real reusable space is the middle gap.
    pub fn free_gaps(&self) -> Vec<FreeGap> {
        const MIN_GAP: u64 = 1024 * 1024; // ignore sub-MiB slivers
        let mut parts: Vec<&Partition> = self.partitions.iter().collect();
        parts.sort_by_key(|p| p.start.0);

        let mut gaps = Vec::new();
        let mut cursor: u64 = 0;
        let mut prev_num: Option<u32> = None;
        for p in &parts {
            if p.start.0 > cursor {
                let size = p.start.0 - cursor;
                if size >= MIN_GAP {
                    gaps.push(FreeGap {
                        start: cursor,
                        size,
                        after_number: prev_num,
                    });
                }
            }
            let end = p.start.0 + p.size.0;
            if end > cursor {
                cursor = end;
            }
            prev_num = Some(p.number);
        }
        if self.size.0 > cursor {
            let size = self.size.0 - cursor;
            if size >= MIN_GAP {
                gaps.push(FreeGap {
                    start: cursor,
                    size,
                    after_number: prev_num,
                });
            }
        }
        gaps
    }

    /// The single largest contiguous unallocated gap on the disk, if any.
    /// Used by the "install into free space" path to find where a new
    /// Nimblex partition can be carved without touching existing data.
    pub fn largest_free_gap(&self) -> Option<FreeGap> {
        self.free_gaps().into_iter().max_by_key(|g| g.size)
    }

    /// Find a pre-existing Nimblex root partition created by a previous
    /// (possibly failed) install: a Linux partition whose GPT name or
    /// filesystem label is `NIMBLEX` / `NIMBLEX_ROOT`.
    ///
    /// Lets the installer **reuse and reformat** its own partition when an
    /// earlier run filled the free gap but didn't complete (e.g. the module
    /// copy errored), instead of dead-ending on the BitLocker warning
    /// because no free space is left.
    pub fn existing_nimblex_partition(&self) -> Option<&Partition> {
        self.partitions.iter().find(|p| {
            matches!(p.role, PartitionRole::Linux) && {
                let l = p.label.to_ascii_uppercase();
                l == "NIMBLEX" || l == "NIMBLEX_ROOT"
            }
        })
    }
}

/// A contiguous run of unallocated space on a disk.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct FreeGap {
    /// Byte offset of the gap from the start of the disk.
    pub start: u64,
    /// Size of the gap in bytes.
    pub size: u64,
    /// Number of the partition that physically precedes this gap, or
    /// `None` if the gap is before the first partition. This is the
    /// `--after-number` argument for the helper's `mkpart-after`.
    pub after_number: Option<u32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TableType {
    Gpt,
    Mbr,
    /// No partition table (raw block device).
    None,
    Unknown,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Partition {
    /// e.g. `/dev/nvme0n1p3`.
    pub path: PathBuf,
    /// Partition number on the parent disk (1-based).
    pub number: u32,
    /// Offset from the start of the disk in bytes.
    pub start: Bytes,
    pub size: Bytes,
    /// Filesystem label as reported by `lsblk` (may be empty).
    pub label: String,
    /// Filesystem name (`ntfs`, `ext4`, `vfat`, `swap`, ...) or empty when unknown.
    pub fs: String,
    pub used: Option<Bytes>,
    pub role: PartitionRole,
    /// True for partitions that the installer must never modify
    /// (ESP/MSR/Recovery on a Windows system).
    pub protected: bool,
}

/// Classification used by the GUI to colour-code the partition strip and
/// by the planner to decide what is allowed to be modified.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PartitionRole {
    /// Windows C: drive (the largest writable NTFS partition).
    WindowsSystem,
    /// Other NTFS data partitions (D:, E:, ...).
    WindowsData,
    /// EFI System Partition (FAT32, type EF00 / esp flag).
    EfiSystem,
    /// Microsoft Reserved Partition (GPT type 0C01).
    MicrosoftReserved,
    /// Windows Recovery (WinRE).
    WindowsRecovery,
    /// Linux ext*/btrfs/xfs root or data partition.
    Linux,
    LinuxSwap,
    /// Anything we recognise but don't treat specially.
    Other,
}

impl PartitionRole {
    /// Whether the planner is permitted to delete or reformat this partition.
    pub fn is_modifiable(&self) -> bool {
        !matches!(
            self,
            PartitionRole::EfiSystem
                | PartitionRole::MicrosoftReserved
                | PartitionRole::WindowsRecovery
        )
    }

    /// Short, user-facing English label for display in the GUI.
    pub fn short_label(&self) -> &'static str {
        match self {
            PartitionRole::WindowsSystem => "Windows",
            PartitionRole::WindowsData => "NTFS",
            PartitionRole::EfiSystem => "EFI",
            PartitionRole::MicrosoftReserved => "MSR",
            PartitionRole::WindowsRecovery => "Recovery",
            PartitionRole::Linux => "Linux",
            PartitionRole::LinuxSwap => "Swap",
            PartitionRole::Other => "Other",
        }
    }
}
