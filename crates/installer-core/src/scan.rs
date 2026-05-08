//! Read-only disk probing.
//!
//! Wraps `lsblk -O -J -b` to enumerate block devices and classifies their
//! partitions into [`PartitionRole`]s. Heuristics:
//!
//! * GPT type GUID is preferred when present.
//! * `pttype`/`parttype` from lsblk identifies ESP, MSR, Recovery directly.
//! * The largest writable NTFS partition is treated as `WindowsSystem`,
//!   smaller NTFS partitions become `WindowsData`.

use crate::disk::{Disk, Partition, PartitionRole, TableType};
use crate::size::Bytes;
use anyhow::{Context, Result};
use serde::Deserialize;
use std::path::PathBuf;
use std::process::Command;

const LSBLK_FIELDS: &str = "NAME,KNAME,PATH,SIZE,TYPE,FSTYPE,LABEL,PARTTYPE,PARTLABEL,PARTUUID,RM,TRAN,MODEL,VENDOR,PTTYPE,FSUSED,START";

pub struct DiskScanner;

impl DiskScanner {
    /// Fast scan via `lsblk` only. Mounted partitions get usage from
    /// `lsblk` itself; unmounted ones have `used = None`.
    pub fn scan() -> Result<Vec<Disk>> {
        let out = Command::new("lsblk")
            .args(["-J", "-b", "-o", LSBLK_FIELDS])
            .output()
            .context("failed to invoke lsblk")?;
        if !out.status.success() {
            anyhow::bail!(
                "lsblk failed: {}",
                String::from_utf8_lossy(&out.stderr)
            );
        }
        let parsed: LsblkRoot = serde_json::from_slice(&out.stdout)
            .context("failed to parse lsblk JSON")?;
        Ok(parsed
            .blockdevices
            .into_iter()
            .filter(|b| b.is_targetable_disk())
            .map(|b| b.into_disk())
            .collect())
    }

    /// Same as [`scan`] but additionally probes unmounted NTFS / ext
    /// partitions to fill in usage. Slower (a few seconds per NTFS volume).
    pub fn scan_with_usage() -> Result<Vec<Disk>> {
        let mut disks = Self::scan()?;
        for d in &mut disks {
            crate::usage_probe::probe_partitions(&mut d.partitions);
        }
        Ok(disks)
    }
}

// --- lsblk JSON shape ----------------------------------------------------

#[derive(Debug, Deserialize)]
struct LsblkRoot {
    blockdevices: Vec<LsblkDev>,
}

#[derive(Debug, Deserialize)]
struct LsblkDev {
    #[serde(default)]
    path: String,
    #[serde(default)]
    size: Option<u64>,
    #[serde(rename = "type", default)]
    dev_type: String,
    #[serde(default)]
    fstype: Option<String>,
    #[serde(default)]
    label: Option<String>,
    #[serde(default)]
    parttype: Option<String>,
    #[serde(default)]
    partlabel: Option<String>,
    #[serde(default)]
    rm: Option<bool>,
    #[serde(default)]
    tran: Option<String>,
    #[serde(default)]
    model: Option<String>,
    #[serde(default)]
    vendor: Option<String>,
    #[serde(default)]
    pttype: Option<String>,
    #[serde(default)]
    fsused: Option<u64>,
    #[serde(default)]
    start: Option<u64>,
    #[serde(default)]
    children: Vec<LsblkDev>,
}

impl LsblkDev {
    fn is_targetable_disk(&self) -> bool {
        if self.dev_type != "disk" {
            return false;
        }
        // Skip tiny / virtual block devices.
        if self.size.unwrap_or(0) < 1_000_000_000 {
            return false;
        }
        let path = self.path.as_str();
        if path.starts_with("/dev/loop")
            || path.starts_with("/dev/zram")
            || path.starts_with("/dev/sr")
            || path.starts_with("/dev/ram")
            || path.starts_with("/dev/dm-")
            || path.starts_with("/dev/md")
        {
            return false;
        }
        if let Some(t) = self.tran.as_deref() {
            if matches!(t, "loop" | "rom") {
                return false;
            }
        }
        true
    }

    fn into_disk(self) -> Disk {
        let table_type = match self.pttype.as_deref() {
            Some("gpt") => TableType::Gpt,
            Some("dos") | Some("mbr") => TableType::Mbr,
            None => TableType::None,
            _ => TableType::Unknown,
        };
        let model = format!(
            "{} {}",
            self.vendor.as_deref().unwrap_or("").trim(),
            self.model.as_deref().unwrap_or("").trim()
        )
        .trim()
        .to_string();
        let mut disk = Disk {
            path: PathBuf::from(&self.path),
            size: Bytes(self.size.unwrap_or(0)),
            removable: self.rm.unwrap_or(false),
            model,
            transport: self.tran.unwrap_or_else(|| "unknown".into()),
            table_type,
            partitions: Vec::new(),
        };
        let mut number: u32 = 0;
        for ch in self.children {
            if ch.dev_type != "part" {
                continue;
            }
            number += 1;
            disk.partitions.push(ch.into_partition(number));
        }
        // Re-classify Windows partitions (largest writable NTFS = system).
        promote_windows_system(&mut disk.partitions);
        disk
    }
}

impl LsblkDev {
    fn into_partition(self, number: u32) -> Partition {
        let role = classify_role(&self);
        let protected = !role.is_modifiable();
        Partition {
            path: PathBuf::from(&self.path),
            number,
            start: Bytes(self.start.unwrap_or(0)),
            size: Bytes(self.size.unwrap_or(0)),
            label: self
                .label
                .clone()
                .or_else(|| self.partlabel.clone())
                .unwrap_or_default(),
            fs: self.fstype.clone().unwrap_or_default(),
            used: self.fsused.map(Bytes),
            role,
            protected,
        }
    }
}

/// GPT type GUIDs that identify Windows-related protected partitions.
const ESP_GUID: &str = "c12a7328-f81f-11d2-ba4b-00a0c93ec93b";
const MSR_GUID: &str = "e3c9e316-0b5c-4db8-817d-f92df00215ae";
const WIN_RECOVERY_GUID: &str = "de94bba4-06d1-4d40-a16a-bfd50179d6ac";
const WIN_BASIC_DATA_GUID: &str = "ebd0a0a2-b9e5-4433-87c0-68b6b72699c7";

fn classify_role(dev: &LsblkDev) -> PartitionRole {
    let pt = dev.parttype.as_deref().unwrap_or("").to_ascii_lowercase();
    let plabel = dev.partlabel.as_deref().unwrap_or("").to_ascii_lowercase();
    let label = dev.label.as_deref().unwrap_or("").to_ascii_lowercase();
    let fs = dev.fstype.as_deref().unwrap_or("").to_ascii_lowercase();

    if pt == ESP_GUID || pt == "0xef" || fs == "vfat" && plabel.contains("efi") {
        return PartitionRole::EfiSystem;
    }
    if pt == MSR_GUID || pt == "0x0c01" {
        return PartitionRole::MicrosoftReserved;
    }
    if pt == WIN_RECOVERY_GUID
        || plabel.contains("recovery")
        || plabel.contains("winre")
        || label.contains("winre")
    {
        return PartitionRole::WindowsRecovery;
    }
    if fs == "ntfs" {
        // Disambiguated to System vs Data in promote_windows_system.
        return PartitionRole::WindowsData;
    }
    if fs == "swap" {
        return PartitionRole::LinuxSwap;
    }
    if matches!(fs.as_str(), "ext2" | "ext3" | "ext4" | "btrfs" | "xfs" | "f2fs") {
        return PartitionRole::Linux;
    }
    if pt == WIN_BASIC_DATA_GUID {
        // GPT basic data partition without a recognised filesystem.
        return PartitionRole::Other;
    }
    PartitionRole::Other
}

/// Among NTFS partitions, mark the largest as the Windows system partition.
fn promote_windows_system(parts: &mut [Partition]) {
    let mut idx_max: Option<usize> = None;
    let mut max_size = 0u64;
    for (i, p) in parts.iter().enumerate() {
        if matches!(p.role, PartitionRole::WindowsData) && p.size.0 > max_size {
            max_size = p.size.0;
            idx_max = Some(i);
        }
    }
    if let Some(i) = idx_max {
        parts[i].role = PartitionRole::WindowsSystem;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_efi_by_guid() {
        let d = LsblkDev {
            path: "/dev/x".into(),
            size: Some(512 * 1024 * 1024),
            dev_type: "part".into(),
            fstype: Some("vfat".into()),
            label: None,
            parttype: Some(ESP_GUID.into()),
            partlabel: None,
            rm: None,
            tran: None,
            model: None,
            vendor: None,
            pttype: None,
            fsused: None,
            start: None,
            children: vec![],
        };
        assert_eq!(classify_role(&d), PartitionRole::EfiSystem);
    }

    #[test]
    fn promotes_largest_ntfs_to_system() {
        let mut parts = vec![
            Partition {
                path: "/dev/p1".into(),
                number: 1,
                start: Bytes(0),
                size: Bytes::from_gib(50),
                label: String::new(),
                fs: "ntfs".into(),
                used: None,
                role: PartitionRole::WindowsData,
                protected: false,
            },
            Partition {
                path: "/dev/p2".into(),
                number: 2,
                start: Bytes(0),
                size: Bytes::from_gib(400),
                label: String::new(),
                fs: "ntfs".into(),
                used: None,
                role: PartitionRole::WindowsData,
                protected: false,
            },
        ];
        promote_windows_system(&mut parts);
        assert_eq!(parts[0].role, PartitionRole::WindowsData);
        assert_eq!(parts[1].role, PartitionRole::WindowsSystem);
    }
}
