//! Plan emission for the two supported scenarios.
//!
//! Each public function returns a fully-populated [`Plan`] given a target
//! [`Disk`] and (for `AlongsideWindows`) a chosen shrink size. The argv in
//! every step matches the helper's allowlist exactly; if you change one,
//! update the other.

use crate::bootloader::{Bootloader, Firmware};
use crate::disk::{Disk, Partition, PartitionRole};
use crate::plan::{Plan, Step, StepCategory};
use crate::resize::min_install;
use crate::scenario::Scenario;
use crate::size::Bytes;
use anyhow::{anyhow, Result};
use std::path::PathBuf;

/// Sentinel used in argv where the helper substitutes the live UUID after
/// formatting. Keeps planner output deterministic for snapshot tests.
pub const NEW_ROOT_UUID_PLACEHOLDER: &str = "{NEW_ROOT_UUID}";

pub struct InstallPlanner;

impl InstallPlanner {
    /// Emit a plan that wipes the whole `disk` and installs Nimblex onto it
    /// (USB scenario). Caller is expected to have verified `disk.removable`.
    ///
    /// `bootloader` must already be resolved to a concrete value
    /// (`SystemdBoot` or `Grub`); the planner never emits `auto` into argv.
    pub fn plan_usb(disk: &Disk, bootloader: Bootloader) -> Result<Plan> {
        let bootloader = match bootloader {
            Bootloader::Auto => bootloader.resolve(Firmware::detect()),
            other => other,
        };
        if disk.size < Bytes::from_gib(2) {
            return Err(anyhow!(
                "{} is only {}, need at least 2 GiB",
                disk.path.display(),
                disk.size
            ));
        }
        let dev = disk.path.display().to_string();
        let part1 = format!("{}{}", dev, partsep(&dev, 1));
        let part2 = format!("{}{}", dev, partsep(&dev, 2));

        let steps = vec![
            Step {
                // Single sgdisk call: zap the old table AND create both new
                // partitions atomically.  sgdisk uses per-partition BLKPG
                // ioctls throughout — it removes old kernel entries one by one
                // and registers new ones the same way.  This avoids the bulk
                // BLKRRPART ioctl that parted triggers (which fails when the
                // kernel still holds stale entries from the live-boot session).
                //
                // Layout:
                //   p1 2048–1050623   (512 MiB, type EF00 = EFI System)
                //   p2 1050624–end     (rest of disk, type 8300 = Linux fs)
                label: "Partition disk (GPT + EFI + Root)".into(),
                category: StepCategory::Partition,
                argv: vec![
                    "sgdisk".into(),
                    "--zap-all".into(),
                    "--new=1:2048:+512M".into(),
                    "--typecode=1:EF00".into(),
                    "--change-name=1:NIMBLEX_ESP".into(),
                    "--new=2:0:0".into(),
                    "--typecode=2:8300".into(),
                    "--change-name=2:NIMBLEX_ROOT".into(),
                    dev.clone(),
                ],
                critical: true,
                destructive: true,
                weight: 2.0,
            },
            Step {
                // partx + udevadm settle: register the new partition nodes
                // in the kernel and wait for /dev/sdaX to be created by udev.
                // Must run before mkfs, which opens the partition by path.
                label: "Settle new partitions".into(),
                category: StepCategory::Partition,
                argv: vec![
                    "nimblex-installer-helper-internal".into(),
                    "settle-partitions".into(),
                    "--disk".into(),
                    dev.clone(),
                    "--count".into(),
                    "2".into(),
                ],
                critical: true,
                destructive: false,
                weight: 0.5,
            },
            Step {
                label: "Format ESP as FAT32".into(),
                category: StepCategory::Format,
                argv: vec![
                    "mkfs.fat".into(),
                    "-F32".into(),
                    "-n".into(),
                    "NIMBLEX_ESP".into(),
                    part1.clone(),
                ],
                critical: true,
                destructive: true,
                weight: 2.0,
            },
            Step {
                label: "Format Nimblex root as ext4".into(),
                category: StepCategory::Format,
                argv: vec![
                    "mkfs.ext4".into(),
                    "-F".into(),
                    "-O".into(),
                    "64bit".into(),
                    "-L".into(),
                    "NIMBLEX_ROOT".into(),
                    part2.clone(),
                ],
                critical: true,
                destructive: true,
                weight: 3.0,
            },
            Step {
                label: "Copy Nimblex bundles".into(),
                category: StepCategory::Copy,
                argv: vec![
                    "nimblex-installer-helper-internal".into(),
                    "copy-system".into(),
                    "--root".into(),
                    part2.clone(),
                ],
                critical: true,
                destructive: true,
                weight: 70.0,
            },
            Step {
                label: "Install bootloader (UEFI + BIOS)".into(),
                category: StepCategory::Bootloader,
                argv: vec![
                    "nimblex-installer-helper-internal".into(),
                    "install-boot-usb".into(),
                    "--esp".into(),
                    part1.clone(),
                    "--root".into(),
                    part2.clone(),
                    "--disk".into(),
                    dev.clone(),
                    "--bootloader".into(),
                    bootloader.to_string(),
                ],
                critical: true,
                destructive: true,
                weight: 8.0,
            },
            Step {
                label: "Flush and finalise".into(),
                category: StepCategory::Finalise,
                argv: vec!["sync".into()],
                critical: true,
                destructive: false,
                weight: 2.0,
            },
        ];

        Ok(Plan {
            scenario: Scenario::UsbFullInstall,
            target_disk: disk.path.clone(),
            shrink_partition: None,
            shrink_to: None,
            new_root_partition: Some(PathBuf::from(part2)),
            new_root_size: disk.size - Bytes::from_mib(513),
            steps,
        })
    }

    /// Emit a plan that shrinks `windows_part` on `disk` to `shrink_to` bytes
    /// and installs Nimblex into the freed space. Reuses the existing ESP.
    ///
    /// `bootloader` must already be resolved to a concrete value.
    pub fn plan_alongside_windows(
        disk: &Disk,
        windows_part: &Partition,
        shrink_to: Bytes,
        bootloader: Bootloader,
    ) -> Result<Plan> {
        let bootloader = match bootloader {
            Bootloader::Auto => bootloader.resolve(Firmware::detect()),
            other => other,
        };
        if !matches!(windows_part.role, PartitionRole::WindowsSystem) {
            return Err(anyhow!(
                "{} is not the Windows system partition",
                windows_part.path.display()
            ));
        }
        let reclaimed = windows_part.size - shrink_to;
        let need = min_install();
        if reclaimed < need {
            return Err(anyhow!(
                "reclaiming only {}, need at least {}",
                reclaimed,
                need
            ));
        }
        let esp = disk
            .partitions
            .iter()
            .find(|p| matches!(p.role, PartitionRole::EfiSystem))
            .ok_or_else(|| anyhow!("no ESP found on {}", disk.path.display()))?;

        // Predicted Linux partition is the next number after windows_part.
        let dev = disk.path.display().to_string();
        let new_part_num = windows_part.number + 1;
        let new_part = format!("{}{}", dev, partsep(&dev, new_part_num));

        let win = windows_part.path.display().to_string();
        let shrink_bytes = shrink_to.0.to_string();

        let steps = vec![
            Step {
                label: "Check Windows Fast Startup".into(),
                category: StepCategory::PrepareWindows,
                argv: vec![
                    "nimblex-installer-helper-internal".into(),
                    "check-fast-startup".into(),
                    "--ntfs".into(),
                    win.clone(),
                ],
                critical: true,
                destructive: false,
                weight: 2.0,
            },
            Step {
                label: "Probe Windows filesystem".into(),
                category: StepCategory::Probe,
                argv: vec![
                    "ntfsresize".into(),
                    "--info".into(),
                    "--force".into(),
                    "--no-progress-bar".into(),
                    win.clone(),
                ],
                critical: true,
                destructive: false,
                weight: 1.0,
            },
            Step {
                label: "Shrink Windows filesystem".into(),
                category: StepCategory::Resize,
                argv: vec![
                    "ntfsresize".into(),
                    "--force".into(),
                    "--no-progress-bar".into(),
                    "--size".into(),
                    shrink_bytes.clone(),
                    win.clone(),
                ],
                critical: true,
                destructive: true,
                weight: 6.0,
            },
            Step {
                label: "Resize Windows partition".into(),
                category: StepCategory::Resize,
                argv: vec![
                    "nimblex-installer-helper-internal".into(),
                    "resizepart".into(),
                    "--disk".into(),
                    dev.clone(),
                    "--number".into(),
                    windows_part.number.to_string(),
                    "--size-bytes".into(),
                    shrink_bytes,
                ],
                critical: true,
                destructive: true,
                weight: 1.0,
            },
            Step {
                label: "Create Nimblex partition".into(),
                category: StepCategory::Partition,
                argv: vec![
                    "nimblex-installer-helper-internal".into(),
                    "mkpart-after".into(),
                    "--disk".into(),
                    dev.clone(),
                    "--after-number".into(),
                    windows_part.number.to_string(),
                    "--label".into(),
                    "NIMBLEX".into(),
                ],
                critical: true,
                destructive: true,
                weight: 0.5,
            },
            Step {
                label: "Format Nimblex partition".into(),
                category: StepCategory::Format,
                argv: vec![
                    "mkfs.ext4".into(),
                    "-F".into(),
                    "-O".into(),
                    "64bit".into(),
                    "-L".into(),
                    "NIMBLEX_ROOT".into(),
                    new_part.clone(),
                ],
                critical: true,
                destructive: true,
                weight: 3.0,
            },
            Step {
                label: "Copy Nimblex bundles".into(),
                category: StepCategory::Copy,
                argv: vec![
                    "nimblex-installer-helper-internal".into(),
                    "copy-system".into(),
                    "--root".into(),
                    new_part.clone(),
                ],
                critical: true,
                destructive: true,
                weight: 60.0,
            },
            Step {
                label: format!("Install bootloader ({}) to existing ESP", bootloader),
                category: StepCategory::Bootloader,
                argv: vec![
                    "nimblex-installer-helper-internal".into(),
                    "install-boot-internal".into(),
                    "--esp".into(),
                    esp.path.display().to_string(),
                    "--root".into(),
                    new_part.clone(),
                    "--bootloader".into(),
                    bootloader.to_string(),
                ],
                critical: true,
                destructive: true,
                weight: 8.0,
            },
            Step {
                label: "Flush and finalise".into(),
                category: StepCategory::Finalise,
                argv: vec!["sync".into()],
                critical: true,
                destructive: false,
                weight: 2.0,
            },
        ];

        Ok(Plan {
            scenario: Scenario::AlongsideWindows,
            target_disk: disk.path.clone(),
            shrink_partition: Some(windows_part.path.clone()),
            shrink_to: Some(shrink_to),
            new_root_partition: Some(PathBuf::from(new_part)),
            new_root_size: reclaimed,
            steps,
        })
    }

    /// Emit a plan that installs Nimblex into the largest unallocated gap
    /// on `disk`, without modifying any existing partition. Reuses the
    /// disk's existing ESP for the Nimblex EFI files.
    ///
    /// Used for disks where Windows is BitLocker-encrypted (so the
    /// `AlongsideWindows` shrink path is not safe) but a large enough
    /// unallocated gap already exists. The gap may sit *between* existing
    /// partitions (e.g. Windows was previously shrunk, leaving free space
    /// before a trailing recovery partition), not just at the disk tail.
    pub fn plan_free_space(disk: &Disk, bootloader: Bootloader) -> Result<Plan> {
        let bootloader = match bootloader {
            Bootloader::Auto => bootloader.resolve(Firmware::detect()),
            other => other,
        };
        let gap = disk
            .largest_free_gap()
            .ok_or_else(|| anyhow!("{} has no unallocated space", disk.path.display()))?;
        let free = Bytes(gap.size);
        let need = min_install();
        if free < need {
            return Err(anyhow!(
                "largest free gap on {} is only {}, need at least {}",
                disk.path.display(),
                free,
                need
            ));
        }
        let after_number = gap.after_number.ok_or_else(|| {
            anyhow!(
                "free gap on {} is before the first partition; unsupported",
                disk.path.display()
            )
        })?;
        let esp = disk
            .partitions
            .iter()
            .find(|p| matches!(p.role, PartitionRole::EfiSystem))
            .ok_or_else(|| anyhow!("no ESP found on {}", disk.path.display()))?;

        let dev = disk.path.display().to_string();
        // parted assigns the new GPT entry the first free slot index, which
        // (with slots 1..=N already used) is N + 1 — regardless of where
        // the partition sits physically on the disk.
        let new_part_num = disk.partitions.iter().map(|p| p.number).max().unwrap_or(0) + 1;
        let new_part = format!("{}{}", dev, partsep(&dev, new_part_num));

        let steps = vec![
            Step {
                label: "Create Nimblex partition in free space".into(),
                category: StepCategory::Partition,
                argv: vec![
                    "nimblex-installer-helper-internal".into(),
                    "mkpart-after".into(),
                    "--disk".into(),
                    dev.clone(),
                    "--after-number".into(),
                    after_number.to_string(),
                    "--label".into(),
                    "NIMBLEX".into(),
                ],
                critical: true,
                destructive: true,
                weight: 0.5,
            },
            Step {
                label: "Format Nimblex partition".into(),
                category: StepCategory::Format,
                argv: vec![
                    "mkfs.ext4".into(),
                    "-F".into(),
                    "-O".into(),
                    "64bit".into(),
                    "-L".into(),
                    "NIMBLEX_ROOT".into(),
                    new_part.clone(),
                ],
                critical: true,
                destructive: true,
                weight: 3.0,
            },
            Step {
                label: "Copy Nimblex bundles".into(),
                category: StepCategory::Copy,
                argv: vec![
                    "nimblex-installer-helper-internal".into(),
                    "copy-system".into(),
                    "--root".into(),
                    new_part.clone(),
                ],
                critical: true,
                destructive: true,
                weight: 60.0,
            },
            Step {
                label: format!("Install bootloader ({}) to existing ESP", bootloader),
                category: StepCategory::Bootloader,
                argv: vec![
                    "nimblex-installer-helper-internal".into(),
                    "install-boot-internal".into(),
                    "--esp".into(),
                    esp.path.display().to_string(),
                    "--root".into(),
                    new_part.clone(),
                    "--bootloader".into(),
                    bootloader.to_string(),
                ],
                critical: true,
                destructive: true,
                weight: 8.0,
            },
            Step {
                label: "Flush and finalise".into(),
                category: StepCategory::Finalise,
                argv: vec!["sync".into()],
                critical: true,
                destructive: false,
                weight: 2.0,
            },
        ];

        Ok(Plan {
            scenario: Scenario::FreeSpace,
            target_disk: disk.path.clone(),
            shrink_partition: None,
            shrink_to: None,
            new_root_partition: Some(PathBuf::from(new_part)),
            new_root_size: free,
            steps,
        })
    }

    /// Emit a plan that **reuses** an existing Nimblex partition created by a
    /// previous (possibly failed) install: copy the system into it and install
    /// the bootloader to the existing ESP. No partitioning step — the
    /// partition already exists. Touches no other partition (Windows, MSR,
    /// WinRE all preserved).
    ///
    /// When `format` is `true` the partition is reformatted (mkfs.ext4)
    /// before the copy, wiping any existing data. When `false` (the default
    /// for a compatible existing partition) the new system is copied straight
    /// onto the partition in place, preserving the existing filesystem.
    ///
    /// When `inplace_live` is `true` the target partition is the device the
    /// running session booted from. In that case we must NOT unmount it (it
    /// backs the live overlay) and we must NOT format it; the copy step uses
    /// the crash-safe `--inplace-live` atomic-rename strategy so the running
    /// kernel keeps reading the old (unlinked) bundle inodes until reboot.
    pub fn plan_reuse(
        disk: &Disk,
        bootloader: Bootloader,
        format: bool,
        inplace_live: bool,
    ) -> Result<Plan> {
        let bootloader = match bootloader {
            Bootloader::Auto => bootloader.resolve(Firmware::detect()),
            other => other,
        };
        let part = disk
            .existing_nimblex_partition()
            .ok_or_else(|| anyhow!("no existing Nimblex partition on {}", disk.path.display()))?;
        let esp = disk
            .partitions
            .iter()
            .find(|p| matches!(p.role, PartitionRole::EfiSystem))
            .ok_or_else(|| anyhow!("no ESP found on {}", disk.path.display()))?;
        let part_path = part.path.display().to_string();

        // In-place reinstall onto the running partition can neither unmount
        // nor reformat the live medium; both would crash the session.
        let format = format && !inplace_live;

        let mut steps = Vec::new();

        // Only unmount when we are NOT operating on the live partition.
        // Detaching the mount that backs the running union is fatal.
        if !inplace_live {
            steps.push(Step {
                label: "Unmount target partition".into(),
                category: StepCategory::Format,
                argv: vec![
                    "nimblex-installer-helper-internal".into(),
                    "unmount-target".into(),
                    "--root".into(),
                    part_path.clone(),
                ],
                critical: true,
                destructive: false,
                weight: 1.0,
            });
        }

        if format {
            steps.push(Step {
                label: "Reformat existing Nimblex partition".into(),
                category: StepCategory::Format,
                argv: vec![
                    "mkfs.ext4".into(),
                    "-F".into(),
                    "-O".into(),
                    "64bit".into(),
                    "-L".into(),
                    "NIMBLEX_ROOT".into(),
                    part_path.clone(),
                ],
                critical: true,
                destructive: true,
                weight: 3.0,
            });
        }

        // The copy step gets the crash-safe strategy flag on the live path.
        let mut copy_argv = vec![
            "nimblex-installer-helper-internal".into(),
            "copy-system".into(),
            "--root".into(),
            part_path.clone(),
        ];
        if inplace_live {
            copy_argv.push("--inplace-live".into());
        }
        let copy_label = if inplace_live {
            "Copy Nimblex bundles (crash-safe in-place)".to_string()
        } else {
            "Copy Nimblex bundles".to_string()
        };

        steps.extend([
            Step {
                label: copy_label,
                category: StepCategory::Copy,
                argv: copy_argv,
                critical: true,
                destructive: true,
                weight: 60.0,
            },
            Step {
                label: format!("Install bootloader ({}) to existing ESP", bootloader),
                category: StepCategory::Bootloader,
                argv: vec![
                    "nimblex-installer-helper-internal".into(),
                    "install-boot-internal".into(),
                    "--esp".into(),
                    esp.path.display().to_string(),
                    "--root".into(),
                    part_path.clone(),
                    "--bootloader".into(),
                    bootloader.to_string(),
                ],
                critical: true,
                destructive: true,
                weight: 8.0,
            },
            Step {
                label: "Flush and finalise".into(),
                category: StepCategory::Finalise,
                argv: vec!["sync".into()],
                critical: true,
                destructive: false,
                weight: 2.0,
            },
        ]);

        Ok(Plan {
            scenario: Scenario::ReuseExisting,
            target_disk: disk.path.clone(),
            shrink_partition: None,
            shrink_to: None,
            new_root_partition: Some(PathBuf::from(part_path)),
            new_root_size: part.size,
            steps,
        })
    }

    /// Single dispatch entry point used by the GUI. Picks `plan_usb` or
    /// `plan_alongside_windows` based on `mode` and auto-selects the Windows
    /// partition via [`Disk::primary_windows_partition`].
    /// `reclaim_bytes` is interpreted only in [`InstallMode::Alongside`]:
    /// it is the amount of space to take from the Windows partition for
    /// Nimblex.  The shrink target is `windows.size - reclaim_bytes`.
    pub fn plan_for(
        disk: &Disk,
        mode: InstallMode,
        reclaim_bytes: Option<u64>,
        bootloader: Bootloader,
        reuse_format: bool,
        reuse_inplace_live: bool,
    ) -> Result<Plan> {
        // Resolve once at the entry point; downstream sees only concrete values.
        let bootloader = match bootloader {
            Bootloader::Auto => bootloader.resolve(Firmware::detect()),
            other => other,
        };
        match mode {
            InstallMode::EraseWholeDisk => Self::plan_usb(disk, bootloader),
            InstallMode::AlongsideWindows => {
                let win = disk
                    .primary_windows_partition()
                    .ok_or_else(|| anyhow!("no Windows partition on {}", disk.path.display()))?;
                let reclaim = reclaim_bytes.map(Bytes).unwrap_or_else(min_install);
                let target = if reclaim >= win.size {
                    Bytes(0)
                } else {
                    win.size - reclaim
                };
                Self::plan_alongside_windows(disk, win, target, bootloader)
            }
            InstallMode::FreeSpace => Self::plan_free_space(disk, bootloader),
            InstallMode::ReuseNimblex => {
                Self::plan_reuse(disk, bootloader, reuse_format, reuse_inplace_live)
            }
        }
    }
}

/// Which top-level operation the user chose on Screen 1.  Replaces the
/// implicit USB-vs-Windows mode used by earlier versions of the GUI.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum InstallMode {
    /// Take space from an existing Windows install (NTFS shrink).
    AlongsideWindows,
    /// Wipe the whole disk (USB stick or any disk the user explicitly chose).
    EraseWholeDisk,
    /// Install into existing unallocated trailing space on the disk,
    /// without touching any existing partition. Used when Windows is
    /// BitLocker-encrypted (so we cannot shrink it) but the disk happens
    /// to have a large enough free gap after the last partition. Reuses
    /// the existing Windows ESP for the bootloader.
    FreeSpace,
    /// Reformat and reuse an existing Nimblex partition left by a previous
    /// (possibly failed) install. No partitioning; reuses the existing ESP.
    /// Lets a failed install be retried without needing free space.
    ReuseNimblex,
}

/// Compute the `pX` separator: `nvme0n1` → `p`, `sda` → ``.
fn partsep(dev: &str, n: u32) -> String {
    let last = dev.chars().last().unwrap_or(' ');
    if last.is_ascii_digit() {
        format!("p{}", n)
    } else {
        n.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::disk::{Disk, TableType};

    fn fake_usb() -> Disk {
        Disk {
            path: "/dev/sdb".into(),
            size: Bytes::from_gib(16),
            removable: true,
            model: "SanDisk Cruzer".into(),
            transport: "usb".into(),
            table_type: TableType::Mbr,
            partitions: vec![],
        }
    }

    #[test]
    fn usb_plan_has_expected_step_categories() {
        let plan = InstallPlanner::plan_usb(&fake_usb(), Bootloader::SystemdBoot).unwrap();
        let cats: Vec<_> = plan.steps.iter().map(|s| s.category).collect();
        assert!(cats.contains(&StepCategory::Format));
        assert!(cats.contains(&StepCategory::Bootloader));
    }

    #[test]
    fn usb_plan_partition_paths_use_p_separator_for_nvme() {
        // Even though we don't recommend installing to internal NVMe via USB
        // path, the partsep helper must work.
        assert_eq!(partsep("/dev/nvme0n1", 2), "p2");
        assert_eq!(partsep("/dev/sda", 2), "2");
    }

    #[test]
    fn usb_plan_too_small_disk_rejected() {
        let mut d = fake_usb();
        d.size = Bytes::from_mib(500);
        assert!(InstallPlanner::plan_usb(&d, Bootloader::SystemdBoot).is_err());
    }

    #[test]
    fn usb_plan_threads_systemd_boot_into_argv() {
        let plan = InstallPlanner::plan_usb(&fake_usb(), Bootloader::SystemdBoot).unwrap();
        let boot_step = plan
            .steps
            .iter()
            .find(|s| s.category == StepCategory::Bootloader)
            .expect("bootloader step");
        let joined = boot_step.argv.join(" ");
        assert!(
            joined.contains("--bootloader systemd-boot"),
            "argv lacks systemd-boot flag: {}",
            joined
        );
    }

    #[test]
    fn usb_plan_threads_grub_into_argv() {
        let plan = InstallPlanner::plan_usb(&fake_usb(), Bootloader::Grub).unwrap();
        let boot_step = plan
            .steps
            .iter()
            .find(|s| s.category == StepCategory::Bootloader)
            .expect("bootloader step");
        let joined = boot_step.argv.join(" ");
        assert!(
            joined.contains("--bootloader grub"),
            "argv lacks grub flag: {}",
            joined
        );
    }

    fn fake_reuse_disk() -> Disk {
        use crate::disk::{Partition, PartitionRole};
        Disk {
            path: "/dev/nvme0n1".into(),
            size: Bytes::from_gib(512),
            removable: false,
            model: "Samsung SSD".into(),
            transport: "nvme".into(),
            table_type: TableType::Gpt,
            partitions: vec![
                Partition {
                    path: "/dev/nvme0n1p1".into(),
                    number: 1,
                    start: Bytes::from_mib(1),
                    size: Bytes::from_mib(512),
                    label: "NIMBLEX_ESP".into(),
                    fs: "vfat".into(),
                    used: None,
                    role: PartitionRole::EfiSystem,
                    protected: false,
                },
                Partition {
                    path: "/dev/nvme0n1p5".into(),
                    number: 5,
                    start: Bytes::from_gib(1),
                    size: Bytes::from_gib(64),
                    label: "NIMBLEX_ROOT".into(),
                    fs: "ext4".into(),
                    used: None,
                    role: PartitionRole::Linux,
                    protected: false,
                },
            ],
        }
    }

    #[test]
    fn reuse_plan_unmounts_target_before_reformat() {
        let plan =
            InstallPlanner::plan_reuse(&fake_reuse_disk(), Bootloader::SystemdBoot, true, false)
                .unwrap();

        // Step 1 must be the tolerant unmount of the target partition, and it
        // must run *before* the mkfs.ext4 reformat — otherwise mke2fs refuses
        // with "is mounted; will not make a filesystem here!".
        let unmount = &plan.steps[0];
        assert_eq!(unmount.argv[0], "nimblex-installer-helper-internal");
        assert_eq!(unmount.argv[1], "unmount-target");
        assert!(unmount.argv.iter().any(|a| a == "/dev/nvme0n1p5"));
        assert!(
            !unmount.destructive,
            "unmount must not be marked destructive"
        );

        let reformat = &plan.steps[1];
        assert_eq!(reformat.argv[0], "mkfs.ext4");
        assert!(reformat.argv.iter().any(|a| a == "/dev/nvme0n1p5"));
    }

    #[test]
    fn reuse_plan_without_format_skips_mkfs() {
        // Default for a compatible existing partition: install in place,
        // no reformat. The plan must still unmount, copy and install boot,
        // but must NOT contain any mkfs.* step (no destructive format).
        let plan =
            InstallPlanner::plan_reuse(&fake_reuse_disk(), Bootloader::SystemdBoot, false, false)
                .unwrap();

        // First step is still the tolerant unmount.
        let unmount = &plan.steps[0];
        assert_eq!(unmount.argv[1], "unmount-target");

        // No mkfs anywhere.
        assert!(
            !plan.steps.iter().any(|s| s.argv[0].starts_with("mkfs")),
            "in-place reuse must not reformat the partition"
        );

        // Copy + bootloader steps still present.
        let cats: Vec<_> = plan.steps.iter().map(|s| s.category).collect();
        assert!(cats.contains(&StepCategory::Copy));
        assert!(cats.contains(&StepCategory::Bootloader));
    }

    #[test]
    fn reuse_plan_inplace_live_skips_unmount_and_flags_copy() {
        // In-place reinstall onto the running partition: must NOT unmount the
        // live partition, must NOT format it (even if asked), and the copy
        // step must carry --inplace-live so the helper uses the crash-safe
        // atomic-rename strategy.
        let plan = InstallPlanner::plan_reuse(
            &fake_reuse_disk(),
            Bootloader::SystemdBoot,
            true, // request format — must be ignored on the live path
            true, // inplace_live
        )
        .unwrap();

        // No unmount-target step (detaching the live mount is fatal).
        assert!(
            !plan
                .steps
                .iter()
                .any(|s| s.argv.iter().any(|a| a == "unmount-target")),
            "in-place live reinstall must not unmount the running partition"
        );

        // No mkfs even though format was requested.
        assert!(
            !plan.steps.iter().any(|s| s.argv[0].starts_with("mkfs")),
            "the live partition can never be reformatted"
        );

        // The copy step carries --inplace-live.
        let copy = plan
            .steps
            .iter()
            .find(|s| s.argv.iter().any(|a| a == "copy-system"))
            .expect("copy-system step present");
        assert!(
            copy.argv.iter().any(|a| a == "--inplace-live"),
            "copy step must request the crash-safe strategy"
        );

        // The summary reflects the in-place path.
        assert!(plan.summary_one_line().contains("in place"));
    }

    #[test]
    fn reuse_plan_normal_path_has_no_inplace_flag() {
        let plan =
            InstallPlanner::plan_reuse(&fake_reuse_disk(), Bootloader::SystemdBoot, false, false)
                .unwrap();
        assert!(
            !plan
                .steps
                .iter()
                .any(|s| s.argv.iter().any(|a| a == "--inplace-live")),
            "non-live reuse must not use the in-place flag"
        );
        // And it DOES unmount in the normal case.
        assert!(plan
            .steps
            .iter()
            .any(|s| s.argv.iter().any(|a| a == "unmount-target")));
    }
}
