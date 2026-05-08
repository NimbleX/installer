//! Internal helper subcommands invoked by the planner via the
//! `nimblex-installer-helper-internal` argv0 alias.
//!
//! Every subcommand runs as root (via pkexec) and performs real disk
//! operations. They communicate progress by printing lines to stdout,
//! which the runner captures and emits as `Event::Stdout` JSON lines.

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use installer_core::live_source_dirs;
use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::boot::HelperBootloader;
use crate::run::{run_cmd_check, run_cmd_ok, run_cmd_silent};

#[derive(Parser, Debug)]
#[command(name = "nimblex-installer-helper-internal", version)]
pub struct InternalCli {
    #[command(subcommand)]
    pub command: InternalCmd,
}

#[derive(Subcommand, Debug)]
pub enum InternalCmd {
    /// Probe Windows registry on `--ntfs` partition for HiberbootEnabled.
    /// Exits 0; emits `fast_startup=on|off|unknown` on stdout.
    CheckFastStartup {
        #[arg(long)]
        ntfs: PathBuf,
    },
    /// Resize partition `--number` on `--disk` so its size equals
    /// `--size-bytes`. Wraps `parted resizepart`.
    Resizepart {
        #[arg(long)]
        disk: PathBuf,
        #[arg(long)]
        number: u32,
        #[arg(long, value_name = "BYTES")]
        size_bytes: u64,
    },
    /// Create a partition on `--disk` immediately after partition
    /// `--after-number`, consuming the rest of the disk.
    MkpartAfter {
        #[arg(long)]
        disk: PathBuf,
        #[arg(long)]
        after_number: u32,
        #[arg(long)]
        label: String,
    },
    /// Mount `--root`, copy the live system bundles and boot files into it.
    CopySystem {
        #[arg(long)]
        root: PathBuf,
    },
    /// Install bootloader for the USB scenario (kernel to ESP + syslinux MBR).
    InstallBootUsb {
        #[arg(long)]
        esp: PathBuf,
        #[arg(long)]
        root: PathBuf,
        #[arg(long)]
        disk: PathBuf,
        /// Which UEFI bootloader to install. Defaults to systemd-boot.
        #[arg(long, value_enum, default_value_t = HelperBootloader::SystemdBoot)]
        bootloader: HelperBootloader,
    },
    /// Install bootloader for the alongside-Windows scenario
    /// (systemd-boot or GRUB written to existing ESP + loader entries).
    InstallBootInternal {
        #[arg(long)]
        esp: PathBuf,
        #[arg(long)]
        root: PathBuf,
        /// Which UEFI bootloader to install. Defaults to systemd-boot.
        #[arg(long, value_enum, default_value_t = HelperBootloader::SystemdBoot)]
        bootloader: HelperBootloader,
    },
    /// After creating a new partition table (e.g. with sgdisk), tell the
    /// kernel about the new partitions and wait for udev to create the
    /// device nodes.  Tries partx first (BLKPG ioctls), then falls back to
    /// blockdev --rereadpt, then polls for the expected device nodes.
    SettlePartitions {
        /// The whole-disk device whose new partitions to settle.
        #[arg(long)]
        disk: PathBuf,
        /// Number of partitions to wait for (e.g. 2 → wait for p1 and p2).
        #[arg(long)]
        count: u32,
    },
}

pub fn run(cli: InternalCli) -> Result<()> {
    match cli.command {
        InternalCmd::CheckFastStartup { ntfs } => {
            require_block(&ntfs)?;
            cmd_check_fast_startup(&ntfs)
        }
        InternalCmd::Resizepart { disk, number, size_bytes } => {
            require_block(&disk)?;
            cmd_resizepart(&disk, number, size_bytes)
        }
        InternalCmd::MkpartAfter { disk, after_number, label } => {
            require_block(&disk)?;
            sanitize_label(&label)?;
            cmd_mkpart_after(&disk, after_number, &label)
        }
        InternalCmd::CopySystem { root } => {
            require_block(&root)?;
            cmd_copy_system(&root)
        }
        InternalCmd::InstallBootUsb { esp, root, disk, bootloader } => {
            require_block(&esp)?;
            require_block(&root)?;
            require_block(&disk)?;
            cmd_install_boot_usb(&esp, &root, &disk, bootloader)
        }
        InternalCmd::InstallBootInternal { esp, root, bootloader } => {
            require_block(&esp)?;
            require_block(&root)?;
            cmd_install_boot_internal(&esp, &root, bootloader)
        }
        InternalCmd::SettlePartitions { disk, count } => {
            require_block(&disk)?;
            cmd_settle_partitions(&disk, count)
        }
    }
}

// ── check-fast-startup ────────────────────────────────────────────────────────

fn cmd_check_fast_startup(ntfs: &Path) -> Result<()> {
    let mnt = Path::new("/tmp/nimblex-ntfs-check");
    fs::create_dir_all(mnt)?;

    // Try mounting read-only.  ntfs-3g may refuse if the volume is hibernated;
    // treat that as Fast Startup = on.
    let mount_ok = run_cmd_ok(&[
        "ntfs-3g",
        "-o", "ro,recover,no_def_opts,noatime",
        &ntfs.to_string_lossy(),
        &mnt.to_string_lossy(),
    ]);

    if !mount_ok {
        // Couldn't mount → volume is dirty (Fast Startup / hibernation active).
        println!("fast_startup=on");
        let _ = run_cmd_ok(&["umount", &mnt.to_string_lossy()]);
        return Ok(());
    }

    // Read the SYSTEM hive and look for HiberbootEnabled.
    let hive = mnt.join("Windows/System32/config/SYSTEM");
    let result = if hive.exists() {
        // Use chntpw to query the key.
        let out = Command::new("chntpw")
            .args([
                "-e", &hive.to_string_lossy(),
            ])
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .spawn()
            .and_then(|mut c| {
                // Send the query commands via stdin.
                if let Some(mut stdin) = c.stdin.take() {
                    let _ = stdin.write_all(
                        b"cd \\CurrentControlSet\\Control\\Session Manager\\Power\ncat HiberbootEnabled\nq\n",
                    );
                }
                c.wait_with_output()
            });
        match out {
            Ok(o) => {
                let s = String::from_utf8_lossy(&o.stdout);
                // chntpw prints the value in hex; "01 00 00 00" = enabled.
                if s.contains("01 00 00 00") || s.contains("HiberbootEnabled = 1") {
                    "on"
                } else if s.contains("00 00 00 00") || s.contains("HiberbootEnabled = 0") {
                    "off"
                } else {
                    "unknown"
                }
            }
            Err(_) => "unknown",
        }
    } else {
        "unknown"
    };

    let _ = run_cmd_ok(&["umount", &mnt.to_string_lossy()]);
    println!("fast_startup={}", result);
    Ok(())
}

// ── resizepart ────────────────────────────────────────────────────────────────

fn cmd_resizepart(disk: &Path, number: u32, size_bytes: u64) -> Result<()> {
    // Determine the partition's start offset so we can compute the end.
    let start = get_partition_start(disk, number)
        .with_context(|| format!("could not determine start of partition {}", number))?;
    let end_bytes = start + size_bytes;

    println!(
        "Resizing partition {} on {} to {} bytes (end = {})",
        number, disk.display(), size_bytes, end_bytes
    );

    run_cmd_check(&[
        "parted", "--script", "--fix",
        &disk.to_string_lossy(),
        "unit", "B",
        "resizepart", &number.to_string(), &end_bytes.to_string(),
    ])?;

    // Update the kernel's view of the partition table.
    run_cmd_check(&["partprobe", &disk.to_string_lossy()])?;
    println!("Partition {} resized successfully.", number);
    Ok(())
}

/// Return the start byte of partition `number` via `parted --machine print`.
fn get_partition_start(disk: &Path, number: u32) -> Result<u64> {
    let out = Command::new("parted")
        .args([
            "--script", "--machine",
            &disk.to_string_lossy(),
            "unit", "B", "print",
        ])
        .output()
        .context("parted print failed")?;
    let text = String::from_utf8_lossy(&out.stdout);
    // Lines look like: "1:1048576B:537919487B:536870912B:fat32:EFI:boot;"
    for line in text.lines() {
        let fields: Vec<&str> = line.split(':').collect();
        if fields.len() >= 3 {
            if let Ok(n) = fields[0].trim().parse::<u32>() {
                if n == number {
                    // Field 1 is start, e.g. "1048576B"
                    let start_str = fields[1].trim_end_matches('B');
                    if let Ok(start) = start_str.parse::<u64>() {
                        return Ok(start);
                    }
                }
            }
        }
    }
    anyhow::bail!("partition {} not found on {}", number, disk.display())
}

// ── mkpart-after ─────────────────────────────────────────────────────────────

fn cmd_mkpart_after(disk: &Path, after_number: u32, label: &str) -> Result<()> {
    // Find the end byte of the partition we're appending after.
    let end = get_partition_end(disk, after_number)
        .with_context(|| format!("could not find end of partition {}", after_number))?;

    // Align start to the next 1 MiB boundary.
    const MIB: u64 = 1024 * 1024;
    let start = (end + MIB - 1) / MIB * MIB;

    println!(
        "Creating partition on {} starting at {} bytes (after partition {}, label={}).",
        disk.display(), start, after_number, label
    );

    run_cmd_check(&[
        "parted", "--script", "--fix",
        &disk.to_string_lossy(),
        "unit", "B",
        "mkpart", label, "ext4",
        &start.to_string(), "100%",
    ])?;

    run_cmd_check(&["partprobe", &disk.to_string_lossy()])?;
    println!("New partition created successfully.");
    Ok(())
}

/// Return the end byte of partition `number`.
fn get_partition_end(disk: &Path, number: u32) -> Result<u64> {
    let out = Command::new("parted")
        .args([
            "--script", "--machine",
            &disk.to_string_lossy(),
            "unit", "B", "print",
        ])
        .output()
        .context("parted print failed")?;
    let text = String::from_utf8_lossy(&out.stdout);
    for line in text.lines() {
        let fields: Vec<&str> = line.split(':').collect();
        if fields.len() >= 3 {
            if let Ok(n) = fields[0].trim().parse::<u32>() {
                if n == number {
                    // Field 2 is end, e.g. "537919487B"
                    let end_str = fields[2].trim_end_matches('B');
                    if let Ok(end) = end_str.parse::<u64>() {
                        return Ok(end);
                    }
                }
            }
        }
    }
    anyhow::bail!("partition {} not found on {}", number, disk.display())
}

// ── copy-system ───────────────────────────────────────────────────────────────

fn cmd_copy_system(root_dev: &Path) -> Result<()> {
    let (bundles_src, boot_src) = live_source_dirs()
        .context("Cannot find live system bundles. Is this a Nimblex live system?")?;

    let mnt = PathBuf::from("/tmp/nimblex-target");
    fs::create_dir_all(&mnt)?;

    println!("Mounting {} at {} ...", root_dev.display(), mnt.display());
    run_cmd_check(&[
        "mount", &root_dev.to_string_lossy(), &mnt.to_string_lossy(),
    ])?;

    // Ensure we unmount on exit even if we error out.
    let result = copy_system_inner(&bundles_src, &boot_src, &mnt);

    println!("Syncing filesystem...");
    let _ = run_cmd_ok(&["sync"]);

    println!("Unmounting {}...", mnt.display());
    let _ = run_cmd_ok(&["umount", &mnt.to_string_lossy()]);

    result
}

fn copy_system_inner(bundles_src: &Path, boot_src: &Path, mnt: &Path) -> Result<()> {
    // ── 1. Copy .lzm bundles to nimblex64/ ───────────────────────────────────
    let target_bundles = mnt.join("nimblex64");
    fs::create_dir_all(&target_bundles)?;

    let mut entries: Vec<_> = fs::read_dir(bundles_src)
        .with_context(|| format!("cannot read {}", bundles_src.display()))?
        .flatten()
        .filter(|e| {
            let n = e.file_name();
            let s = n.to_string_lossy();
            s.ends_with(".lzm") && !s.ends_with(".skip")
        })
        .collect();

    // Sort alphabetically so modules are installed in the correct layer order
    // (01-Core64.lzm before 02-Xorg64.lzm etc.).
    entries.sort_by_key(|e| e.file_name());

    // Compute total bytes across ALL bundles for accurate overall progress.
    let total_bytes_all: u64 = entries
        .iter()
        .map(|e| e.metadata().map(|m| m.len()).unwrap_or(0))
        .sum();

    println!(
        "Copying {} bundle(s) ({} MiB total) from {} ...",
        entries.len(),
        total_bytes_all / 1024 / 1024,
        bundles_src.display()
    );
    println!("PROGRESS:0");

    let mut total_copied: u64 = 0;
    let mut last_pct: u32 = 0;
    let mut buf = vec![0u8; 4 * 1024 * 1024];

    for entry in &entries {
        let src = entry.path();
        let dst = target_bundles.join(entry.file_name());
        let file_size = entry.metadata().map(|m| m.len()).unwrap_or(0);
        println!(
            "  {} ({} MiB)",
            src.file_name().unwrap_or_default().to_string_lossy(),
            file_size / 1024 / 1024
        );

        let mut src_f = File::open(&src)
            .with_context(|| format!("open {}", src.display()))?;
        let mut dst_f = File::create(&dst)
            .with_context(|| format!("create {}", dst.display()))?;

        loop {
            let n = src_f.read(&mut buf)?;
            if n == 0 { break; }
            dst_f.write_all(&buf[..n])?;
            total_copied += n as u64;
            if total_bytes_all > 0 {
                let pct = (total_copied * 100 / total_bytes_all) as u32;
                if pct >= last_pct + 2 {
                    last_pct = pct;
                    println!("PROGRESS:{}", pct);
                }
            }
        }
    }

    println!("PROGRESS:100");

    // ── 2. Copy boot files (kernel + initrd) to boot/ ───────────────────────
    let target_boot = mnt.join("boot");
    fs::create_dir_all(&target_boot)?;

    println!(
        "Copying boot files from {} to {} ...",
        boot_src.display(), target_boot.display()
    );

    for entry in fs::read_dir(boot_src)
        .with_context(|| format!("cannot read {}", boot_src.display()))?
        .flatten()
    {
        let src = entry.path();
        if !src.is_file() {
            continue;
        }
        let fname = entry.file_name();
        let dst = target_boot.join(&fname);
        println!("  {}", fname.to_string_lossy());
        fs::copy(&src, &dst)
            .with_context(|| format!("failed to copy {}", src.display()))?;
    }

    println!("System copy complete.");
    Ok(())
}

// ── install-boot-usb ─────────────────────────────────────────────────────────

fn cmd_install_boot_usb(
    esp_dev: &Path,
    _root_dev: &Path,
    disk: &Path,
    bootloader: HelperBootloader,
) -> Result<()> {
    let (bundles_src, boot_src) = live_source_dirs()
        .context("Cannot find live boot files. Is this a Nimblex live system?")?;

    let (kernel, initrd) = pick_kernel_and_initrd(&bundles_src, &boot_src)?;

    let mnt_esp = PathBuf::from("/tmp/nimblex-esp");
    fs::create_dir_all(&mnt_esp)?;

    println!("Mounting ESP {} ...", esp_dev.display());
    run_cmd_check(&[
        "mount", &esp_dev.to_string_lossy(), &mnt_esp.to_string_lossy(),
    ])?;

    let result = crate::boot::install_usb(bootloader, &mnt_esp, &kernel, &initrd, disk);

    println!("Syncing and unmounting ESP...");
    let _ = run_cmd_ok(&["sync"]);
    let _ = run_cmd_ok(&["umount", &mnt_esp.to_string_lossy()]);

    result
}

// ── install-boot-internal ─────────────────────────────────────────────────────

fn cmd_install_boot_internal(
    esp_dev: &Path,
    root_dev: &Path,
    bootloader: HelperBootloader,
) -> Result<()> {
    let (bundles_src, boot_src) = live_source_dirs()
        .context("Cannot find live boot files. Is this a Nimblex live system?")?;

    let (kernel, initrd) = pick_kernel_and_initrd(&bundles_src, &boot_src)?;

    let mnt_esp = PathBuf::from("/tmp/nimblex-esp");
    fs::create_dir_all(&mnt_esp)?;

    println!("Mounting ESP {} ...", esp_dev.display());
    run_cmd_check(&[
        "mount", &esp_dev.to_string_lossy(), &mnt_esp.to_string_lossy(),
    ])?;

    let result = crate::boot::install_internal(
        bootloader,
        esp_dev,
        &mnt_esp,
        root_dev,
        &kernel,
        &initrd,
    );

    println!("Syncing and unmounting ESP...");
    let _ = run_cmd_ok(&["sync"]);
    let _ = run_cmd_ok(&["umount", &mnt_esp.to_string_lossy()]);

    result
}

// ── helpers ───────────────────────────────────────────────────────────────────

/// Pick `(kernel, initrd)` paths from the live system, anchored to the
/// **modules bundle that will actually ship with the install**.
///
/// The bug this fixes: `boot_src` (the live system's `/boot/`) often
/// contains stale kernels left over from previous builds (e.g. 4.18,
/// 4.20rc6, 5.4, 5.16) while `bundles_src` only ships the current
/// `modules64-X.Y.lzm`. Picking the "newest" kernel by mtime/name from
/// `boot_src` is therefore wrong: livekit's live-init can't find matching
/// modules and the boot hangs silently after "Loaded initrd".
///
/// Algorithm:
///   1. Find the active modules bundle in `bundles_src`: `modules64-X.Y.lzm`
///      (skip `*.old` and `*.skip`). Extract `X.Y`.
///   2. In `boot_src`, REQUIRE a kernel whose filename contains that exact
///      version string (`vmlinuz*X.Y*`). If none exists, **fail with a
///      clear error** rather than silently picking a mismatched kernel.
///   3. Initrd: prefer the newest by name (`nx2` > `nx1` by trailing
///      number), then mtime.
///   4. Only when there is **no** modules bundle at all do we fall back
///      to "newest kernel by name" — covers test/scaffolding setups.
fn pick_kernel_and_initrd(bundles_src: &Path, boot_src: &Path) -> Result<(PathBuf, PathBuf)> {
    let modules_version = active_modules_version(bundles_src);

    let kernel = match modules_version.as_deref() {
        Some(v) => {
            println!("Active modules bundle version: {}", v);
            find_kernel_for_version(boot_src, v).with_context(|| {
                let avail = list_kernels(boot_src);
                format!(
                    "No kernel matching modules version {v} found in {bd}.\n\
                     Available kernels: {avail}\n\
                     The live system's boot directory must contain a vmlinuz that matches\n\
                     modules64-{v}.lzm — otherwise the installed system will hang at\n\
                     'Loaded initrd from LINUX_EFI_INITRD_MEDIA_GUID device path' because\n\
                     livekit can't find matching kernel modules.\n\
                     Fix: copy the matching vmlinuz (and a matching initramfs) into {bd}.",
                    v = v,
                    bd = boot_src.display(),
                )
            })?
        }
        None => {
            println!(
                "Warning: no modules64-*.lzm found in {} — falling back to newest kernel by name.",
                bundles_src.display()
            );
            find_newest(boot_src, "vmlinuz")
                .with_context(|| format!("No kernel (vmlinuz*) found in {}", boot_src.display()))?
        }
    };

    let initrd = find_newest_initrd(boot_src)
        .with_context(|| format!("No initrd (initramfs*/initrd*) found in {}", boot_src.display()))?;

    println!("Selected kernel: {}", kernel.display());
    println!("Selected initrd: {}", initrd.display());
    Ok((kernel, initrd))
}

/// List all `vmlinuz*` filenames in `dir`, comma-separated, for error messages.
fn list_kernels(dir: &Path) -> String {
    let mut names: Vec<String> = fs::read_dir(dir)
        .map(|rd| {
            rd.flatten()
                .filter(|e| {
                    e.file_name().to_string_lossy().starts_with("vmlinuz") && e.path().is_file()
                })
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .collect()
        })
        .unwrap_or_default();
    names.sort();
    if names.is_empty() {
        "(none)".to_string()
    } else {
        names.join(", ")
    }
}

/// Find the active modules bundle in `bundles_src` and extract its version.
///
/// `modules64-6.19.lzm` -> `Some("6.19")`. Skips `.old` / `.skip` siblings.
/// If multiple active bundles exist (shouldn't happen), picks the highest
/// version by numeric component compare.
fn active_modules_version(bundles_src: &Path) -> Option<String> {
    let mut versions: Vec<(Vec<u32>, String)> = Vec::new();
    for entry in fs::read_dir(bundles_src).ok()?.flatten() {
        let name = entry.file_name();
        let s = name.to_string_lossy();
        // Active bundles only.
        if !s.starts_with("modules64-") || !s.ends_with(".lzm") {
            continue;
        }
        // skip "modules64-6.18.lzm.old" — already filtered by the .lzm
        // suffix above, but be explicit.
        if s.ends_with(".old.lzm") || s.ends_with(".skip.lzm") {
            continue;
        }
        let v = s
            .trim_start_matches("modules64-")
            .trim_end_matches(".lzm")
            .to_string();
        let parts: Vec<u32> = v.split('.').filter_map(|p| p.parse().ok()).collect();
        if !parts.is_empty() {
            versions.push((parts, v));
        }
    }
    versions.sort();
    versions.pop().map(|(_, v)| v)
}

/// Find a kernel in `boot_src` whose filename contains `version` as a
/// substring delimited by non-alphanumeric chars (so `6.19` matches
/// `vmlinuz64-6.19` but not a hypothetical `vmlinuz-6.190`).
fn find_kernel_for_version(boot_src: &Path, version: &str) -> Option<PathBuf> {
    let rd = fs::read_dir(boot_src).ok()?;
    let mut candidates: Vec<PathBuf> = rd
        .flatten()
        .filter(|e| {
            let n = e.file_name();
            let s = n.to_string_lossy();
            if !s.starts_with("vmlinuz") || !e.path().is_file() {
                return false;
            }
            // Look for exact token match: the version must be flanked by
            // non-digit chars (or string boundary).
            if let Some(idx) = s.find(version) {
                let after = s.as_bytes().get(idx + version.len()).copied();
                let before = if idx == 0 { None } else { s.as_bytes().get(idx - 1).copied() };
                let ok_after = matches!(after, None | Some(b'-' | b'.' | b'_' | b'+'))
                    || !after.map(|b| b.is_ascii_digit() || b == b'.').unwrap_or(false);
                let ok_before = matches!(before, None | Some(b'-' | b'.' | b'_'));
                return ok_before && ok_after;
            }
            false
        })
        .map(|e| e.path())
        .collect();
    // If multiple match, pick newest by mtime then name.
    candidates.sort_by(|a, b| {
        let ta = fs::metadata(a).and_then(|m| m.modified()).ok();
        let tb = fs::metadata(b).and_then(|m| m.modified()).ok();
        tb.cmp(&ta).then(b.file_name().cmp(&a.file_name()))
    });
    candidates.into_iter().next()
}

/// Pick the "best" initrd in `boot_src`. Prefers `initramfs*` over `initrd*`;
/// among those, picks the highest trailing-number suffix (so `initramfs64-nx2`
/// beats `initramfs64-nx1`); ties broken by mtime newest.
fn find_newest_initrd(boot_src: &Path) -> Option<PathBuf> {
    let mut candidates: Vec<PathBuf> = fs::read_dir(boot_src)
        .ok()?
        .flatten()
        .filter(|e| {
            let n = e.file_name();
            let s = n.to_string_lossy();
            (s.starts_with("initramfs") || s.starts_with("initrd")) && e.path().is_file()
        })
        .map(|e| e.path())
        .collect();
    candidates.sort_by(|a, b| {
        let na = a.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let nb = b.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        // "initramfs" before "initrd" (modern wins).
        let pa = na.starts_with("initramfs");
        let pb = nb.starts_with("initramfs");
        // Trailing number after last non-digit run.
        let ka = trailing_number(&na);
        let kb = trailing_number(&nb);
        let ta = fs::metadata(a).and_then(|m| m.modified()).ok();
        let tb = fs::metadata(b).and_then(|m| m.modified()).ok();
        // Sort descending: prefix true > false, then number desc, then mtime desc.
        pb.cmp(&pa).then(kb.cmp(&ka)).then(tb.cmp(&ta)).then(nb.cmp(&na))
    });
    candidates.into_iter().next()
}

/// Extract the trailing decimal number from a string. `"initramfs64-nx2"` -> 2.
/// Returns 0 if none found, so files without a number sort below numbered ones.
fn trailing_number(s: &str) -> u32 {
    let bytes = s.as_bytes();
    let mut end = bytes.len();
    while end > 0 && bytes[end - 1].is_ascii_digit() {
        end -= 1;
    }
    s[end..].parse().unwrap_or(0)
}

/// Find the newest file in `dir` whose name starts with `prefix`.
fn find_newest(dir: &Path, prefix: &str) -> Option<PathBuf> {
    let mut candidates: Vec<_> = fs::read_dir(dir)
        .ok()?
        .flatten()
        .filter(|e| {
            e.file_name()
                .to_string_lossy()
                .starts_with(prefix)
                && e.path().is_file()
        })
        .collect();
    // Sort by mtime descending; fall back to name descending.
    candidates.sort_by(|a, b| {
        let ta = a.metadata().and_then(|m| m.modified()).ok();
        let tb = b.metadata().and_then(|m| m.modified()).ok();
        tb.cmp(&ta).then(b.file_name().cmp(&a.file_name()))
    });
    candidates.first().map(|e| e.path())
}

/// Split `/dev/nvme0n1p1` → `("/dev/nvme0n1", 1)` or `/dev/sdb2` → `("/dev/sdb", 2)`.
#[allow(dead_code)]
fn split_dev_part(dev: &Path) -> Option<(String, u32)> {
    let s = dev.to_str()?;
    // NVMe: ends with `p<n>`
    if let Some(idx) = s.rfind('p') {
        if let Ok(n) = s[idx + 1..].parse::<u32>() {
            return Some((s[..idx].to_string(), n));
        }
    }
    // SCSI/USB: ends with a digit run
    let idx = s.trim_end_matches(|c: char| c.is_ascii_digit()).len();
    if idx < s.len() {
        if let Ok(n) = s[idx..].parse::<u32>() {
            return Some((s[..idx].to_string(), n));
        }
    }
    None
}

fn require_block(p: &Path) -> Result<()> {
    let s = p.to_str().context("non-utf8 device path")?;
    if !s.starts_with("/dev/") {
        anyhow::bail!("not a /dev path: {}", s);
    }
    if s.contains("..") || s.contains('\0') {
        anyhow::bail!("suspicious device path: {}", s);
    }
    Ok(())
}

fn sanitize_label(label: &str) -> Result<()> {
    if label.is_empty() || label.len() > 16 {
        anyhow::bail!("label must be 1..=16 chars");
    }
    if !label
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
    {
        anyhow::bail!("label must be alphanumeric/_/-");
    }
    Ok(())
}

// ── settle-partitions ─────────────────────────────────────────────────────────

/// Tell the kernel about new partitions on `disk` and wait for udev to create
/// the device nodes.
///
/// Strategy (in order of reliability):
///   1. `partx --add <disk>` — uses BLKPG ioctls, works even when
///      `BLKRRPART` is blocked.
///   2. If partx fails: `blockdev --rereadpt <disk>` (fallback).
///   3. `udevadm settle --timeout=10` — waits for udev to process the
///      kernel events and create /dev/<disk>N nodes.
///   4. Poll for each expected partition node with 100 ms sleeps, up to 15 s.
///   5. Lazily unmount any stale mounts on these new partition nodes.
///      Needed when the target disk was previously the live-boot device:
///      udev may have re-activated old mounts after the kernel registered
///      the new partition layout.
fn cmd_settle_partitions(disk: &Path, count: u32) -> Result<()> {
    let dev = disk.to_string_lossy().to_string();

    // Step 1 – partx --add (silent: produces alarming-but-expected output on
    // live-boot devices; real success is confirmed by the polling loop below).
    println!("Informing kernel of new partitions on {} ...", dev);
    let partx_ok = run_cmd_silent(&["partx", "--add", &dev]);
    if !partx_ok {
        // partx fallback: blockdev --rereadpt (also silent — expected to fail
        // on busy devices; polling loop is the authoritative check).
        let _ = run_cmd_silent(&["blockdev", "--rereadpt", &dev]);
    }

    // Step 2 – udevadm settle (silent: udev.conf deprecation warnings suppressed).
    let _ = run_cmd_silent(&["udevadm", "settle", "--timeout=10"]);

    // Step 3 – poll for device nodes (up to 15 seconds)
    let mut all_present = false;
    for attempt in 0..150 {
        all_present = (1..=count).all(|n| {
            let node = partition_path(&dev, n);
            std::path::Path::new(&node).exists()
        });
        if all_present {
            if attempt > 0 {
                println!("Partition nodes appeared after {} ms.", attempt * 100);
            }
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }

    if !all_present {
        anyhow::bail!(
            "Partition nodes for {} did not appear after 15 s. \
             Try unplugging and re-inserting the device, then retry.",
            dev
        );
    }

    println!("Partitions ready: {}", (1..=count).map(|n| partition_path(&dev, n)).collect::<Vec<_>>().join(" "));

    // Step 4 – lazily unmount any stale mounts on the new partition nodes.
    //
    // When the target disk was previously the live-boot device (e.g. the
    // installer is running from the same USB stick being reinstalled), the
    // kernel still has the old filesystems mounted on /dev/sdaX.  Erasing
    // the partition table doesn't automatically unmount them.  We use
    // `umount --lazy` so the unmount succeeds even if something is actively
    // using the mount — the live aufs overlay detaches from the physical
    // device immediately, and the mount entry disappears from /proc/mounts.
    let mounts = fs::read_to_string("/proc/mounts").unwrap_or_default();
    for n in 1..=count {
        let node = partition_path(&dev, n);
        for line in mounts.lines() {
            let fields: Vec<&str> = line.splitn(3, ' ').collect();
            if fields.len() >= 2 && fields[0] == node {
                let mountpoint = fields[1];
                println!("Unmounting stale mount: {} → {}", node, mountpoint);
                // --lazy (-l): detach now, clean up when no longer busy.
                let ok = run_cmd_ok(&["umount", "--lazy", mountpoint]);
                if !ok {
                    // Non-fatal: mkfs has its own "already mounted" check
                    // which will give a clear error if this fails.
                    println!("  (umount returned non-zero; continuing)");
                }
                // Small pause to let the VFS finish the detach.
                std::thread::sleep(std::time::Duration::from_millis(200));
            }
        }
    }

    // Step 5 – second udevadm settle to absorb any mount/umount events.
    let _ = run_cmd_silent(&["udevadm", "settle", "--timeout=5"]);

    Ok(())
}

/// Build the device path for partition `n` on `disk`.
/// e.g. /dev/sda → /dev/sda1, /dev/nvme0n1 → /dev/nvme0n1p1
fn partition_path(disk: &str, n: u32) -> String {
    let last = disk.chars().last().unwrap_or(' ');
    if last.is_ascii_digit() {
        format!("{}p{}", disk, n)
    } else {
        format!("{}{}", disk, n)
    }
}
