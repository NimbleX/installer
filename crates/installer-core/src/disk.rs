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
            matches!(p.role, PartitionRole::WindowsSystem)
                && {
                    let l = p.label.to_ascii_lowercase();
                    l == "c:"
                        || l == "windows"
                        || l == "system"
                        || l == "os"
                        || l == "boot"
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
