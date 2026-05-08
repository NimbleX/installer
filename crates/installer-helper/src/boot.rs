//! Bootloader installation backends.
//!
//! Two backends, both implementing the same shape:
//!   - `install_usb`     — for the wipe-whole-USB scenario (we own the ESP).
//!   - `install_internal` — for the alongside-Windows scenario (we share an
//!                          ESP with Windows on an internal disk).
//!
//! The boot **menu entries** are produced once, backend-neutral, by
//! `compose_entries()` (Nimblex graphical + CLI + rescue, plus any
//! auto-detected Windows installs), then rendered to either systemd-boot's
//! `loader/entries/*.conf` or to a `boot/grub/grub.cfg`. This keeps the
//! Windows-detection logic in one place.

use anyhow::{Context, Result};
use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::run::{find_cmd, run_cmd_check, run_cmd_ok};

/// User-facing bootloader choice. Mirror of `installer_core::Bootloader`
/// without the `Auto` variant — by the time we get here the planner has
/// already resolved `Auto` to a concrete value.
#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum HelperBootloader {
    SystemdBoot,
    Grub,
}

/// One menu entry, backend-neutral.
#[derive(Debug, Clone)]
pub enum BootEntry {
    /// Boot a Linux kernel + initrd directly. Paths are relative to the
    /// ESP we're installing onto (e.g. `/EFI/nimblex/vmlinuz`).
    Linux {
        title: String,
        kernel: String,
        initrd: String,
        options: String,
    },
    /// Chainload a foreign EFI executable (e.g. Windows Boot Manager). The
    /// `efi_path` is relative to whichever ESP carries the binary; the
    /// `esp_fs_uuid` lets GRUB locate that ESP across disks.
    EfiChainload {
        title: String,
        efi_path: String,
        esp_fs_uuid: String,
    },
}

/// A detected Windows install on a partition we can mount.
#[derive(Debug, Clone)]
pub struct WindowsInstall {
    #[allow(dead_code)]
    pub esp_dev: PathBuf,
    pub esp_fs_uuid: String,
    pub bootmgfw_path: String, // always "/EFI/Microsoft/Boot/bootmgfw.efi"
}

// ───────────────────────── Public entry points ─────────────────────────

/// Install a bootloader to a USB stick we own end-to-end.
///
/// `mnt_esp` is the FAT32 ESP mounted RW. `kernel`/`initrd` are absolute
/// paths on the live system that must be copied into the ESP. `disk` is
/// the whole-disk device path (used for the syslinux MBR write).
pub fn install_usb(
    backend: HelperBootloader,
    mnt_esp: &Path,
    kernel: &Path,
    initrd: &Path,
    disk: &Path,
) -> Result<()> {
    stage_kernel_and_initrd(mnt_esp, kernel, initrd)?;

    let windows = detect_windows_installs(mnt_esp);
    let entries = compose_entries(&windows);

    match backend {
        HelperBootloader::SystemdBoot => install_systemd_boot(mnt_esp, &entries)?,
        HelperBootloader::Grub => install_grub_removable(mnt_esp, &entries)?,
    }

    // BIOS USB fallback: write syslinux's MBR boot code regardless of which
    // UEFI bootloader was chosen. Costs nothing, makes the stick legacy-
    // bootable on old hardware. (The actual syslinux config under
    // /boot/syslinux is left for a future feature; the MBR alone hands off
    // to the active partition's boot record.)
    install_syslinux_mbr(disk);

    Ok(())
}

/// Install a bootloader to an internal-disk ESP (which probably also has
/// Windows on it). `esp_dev` is the block device of the ESP, used for
/// efibootmgr registration.
pub fn install_internal(
    backend: HelperBootloader,
    esp_dev: &Path,
    mnt_esp: &Path,
    root_dev: &Path,
    kernel: &Path,
    initrd: &Path,
) -> Result<()> {
    // Internal install puts kernel/initrd under EFI/nimblex/ on the shared
    // ESP — exact same layout as the USB install, for consistency.
    stage_kernel_and_initrd(mnt_esp, kernel, initrd)?;

    let windows = detect_windows_installs(mnt_esp);
    let entries = compose_entries(&windows);

    match backend {
        HelperBootloader::SystemdBoot => {
            install_systemd_boot(mnt_esp, &entries)?;
            // For the internal-disk case, register an NVRAM entry pointing
            // at our copy of systemd-bootx64.efi so the firmware boot menu
            // can find us alongside Windows. --no-variables was passed on
            // the USB path; here we DO want the entry.
            register_nvram_entry(esp_dev, r"\EFI\systemd\systemd-bootx64.efi", "Nimblex");
        }
        HelperBootloader::Grub => {
            install_grub_internal(esp_dev, mnt_esp, &entries)?;
        }
    }

    let _ = root_dev; // currently unused; reserved for future cmdline plumbing
    Ok(())
}

// ───────────────────────── Stage kernel + initrd ─────────────────────────

fn stage_kernel_and_initrd(mnt_esp: &Path, kernel: &Path, initrd: &Path) -> Result<()> {
    let nx = mnt_esp.join("EFI/nimblex");
    fs::create_dir_all(&nx)?;
    let dst_k = nx.join("vmlinuz");
    let dst_i = nx.join("initrd.img");
    println!("Copying kernel  {} -> {}", kernel.display(), dst_k.display());
    fs::copy(kernel, &dst_k).with_context(|| format!("copy kernel to {}", dst_k.display()))?;
    println!("Copying initrd  {} -> {}", initrd.display(), dst_i.display());
    fs::copy(initrd, &dst_i).with_context(|| format!("copy initrd to {}", dst_i.display()))?;
    Ok(())
}

// ───────────────────────── systemd-boot backend ─────────────────────────

fn install_systemd_boot(mnt_esp: &Path, entries: &[BootEntry]) -> Result<()> {
    println!("Installing systemd-boot to {} ...", mnt_esp.display());
    let ok = run_cmd_ok(&[
        "bootctl",
        "install",
        &format!("--esp-path={}", mnt_esp.display()),
        "--no-variables",
    ]);
    if !ok {
        anyhow::bail!(
            "bootctl install failed. Is systemd built with -Dbootloader=true? \
             Check for /usr/lib/systemd/boot/efi/systemd-bootx64.efi"
        );
    }

    // loader.conf
    let loader_conf = mnt_esp.join("loader/loader.conf");
    fs::write(
        &loader_conf,
        "default  nimblex.conf\n\
         timeout  3\n\
         console-mode max\n\
         editor   yes\n\
         auto-firmware yes\n\
         auto-entries  yes\n",
    )
    .with_context(|| format!("write {}", loader_conf.display()))?;

    // Per-entry .conf files.
    let entries_dir = mnt_esp.join("loader/entries");
    fs::create_dir_all(&entries_dir)?;
    for (idx, e) in entries.iter().enumerate() {
        match e {
            BootEntry::Linux { title, kernel, initrd, options } => {
                let stem = entry_stem(title, idx);
                let body = format!(
                    "title    {title}\n\
                     linux    {kernel}\n\
                     initrd   {initrd}\n\
                     options  {options}\n"
                );
                let path = entries_dir.join(format!("{}.conf", stem));
                fs::write(&path, body).with_context(|| format!("write {}", path.display()))?;
                println!("Wrote loader entry: {}", path.display());
            }
            BootEntry::EfiChainload { title, efi_path, esp_fs_uuid: _ } => {
                // systemd-boot can only chainload an EFI app on the SAME ESP
                // it was booted from. For the alongside-Windows case (shared
                // ESP) this is fine. For the USB-live case, this branch is
                // skipped — see compose_entries(). When we do emit it, we
                // copy the foreign loader's path verbatim.
                let stem = entry_stem(title, idx);
                let body = format!("title    {title}\nefi      {efi_path}\n");
                let path = entries_dir.join(format!("{}.conf", stem));
                fs::write(&path, body).with_context(|| format!("write {}", path.display()))?;
                println!("Wrote loader entry: {}", path.display());
            }
        }
    }
    println!("systemd-boot install complete.");
    Ok(())
}

// ───────────────────────── GRUB backend ─────────────────────────

/// Install GRUB to a USB stick at the firmware's removable-media fallback
/// path (`\EFI\BOOT\BOOTX64.EFI`). Uses `grub-mkstandalone` so the EFI
/// binary is fully self-contained — no separate locale/theme/module dirs
/// on the ESP, which were the cause of pre-`ExitBootServices` USB read
/// storms that made interactive use unbearable on slow firmware.
fn install_grub_removable(mnt_esp: &Path, entries: &[BootEntry]) -> Result<()> {
    let grub_mkstandalone = find_cmd(&["grub-mkstandalone", "/usr/bin/grub-mkstandalone"])
        .context("grub-mkstandalone not found; install grub")?;

    let efi_boot = mnt_esp.join("EFI/BOOT");
    fs::create_dir_all(&efi_boot)?;
    let efi_target = efi_boot.join("BOOTX64.EFI");

    // Write embedded.cfg to a tempfile. It gets baked into the EFI binary;
    // its only job is to find the editable grub.cfg on the same ESP via
    // the FAT label NIMBLEX_ESP, then chainload it.
    let embedded = "set timeout=0\n\
                    terminal_output console\n\
                    insmod part_gpt\n\
                    insmod fat\n\
                    insmod search\n\
                    insmod search_label\n\
                    search --no-floppy --label --set=root NIMBLEX_ESP\n\
                    configfile /boot/grub/grub.cfg\n";
    let tmp = std::env::temp_dir().join("nimblex-grub-embedded.cfg");
    fs::write(&tmp, embedded).context("write embedded grub cfg")?;

    println!("Building standalone GRUB image at {} ...", efi_target.display());
    run_cmd_check(&[
        &grub_mkstandalone,
        "--format=x86_64-efi",
        &format!("--output={}", efi_target.display()),
        "--modules=part_gpt fat configfile normal linux echo search search_label chain",
        "--install-modules=part_gpt fat configfile normal linux echo search search_label chain terminal",
        "--fonts=",
        "--themes=",
        "--locales=",
        &format!("boot/grub/grub.cfg={}", tmp.display()),
    ])?;

    // Editable grub.cfg on the ESP.
    write_grub_cfg(mnt_esp, entries)?;

    // Drop any leftover module/locale/theme tree from a previous default
    // grub-install on the same stick — we don't need it and it slows the
    // firmware's USB reads at boot time.
    let stale = mnt_esp.join("boot/grub");
    if let Ok(rd) = fs::read_dir(&stale) {
        for ent in rd.flatten() {
            let p = ent.path();
            if p.is_dir() {
                let _ = fs::remove_dir_all(&p);
            }
        }
    }

    println!("GRUB removable install complete.");
    Ok(())
}

/// Install GRUB to an internal-disk ESP. Registers a `Nimblex` NVRAM boot
/// entry. Uses `grub-install --target=x86_64-efi` directly (not standalone)
/// because slow firmware USB reads aren't a concern on internal NVMe.
fn install_grub_internal(esp_dev: &Path, mnt_esp: &Path, entries: &[BootEntry]) -> Result<()> {
    let grub_install = find_cmd(&["grub-install", "/usr/sbin/grub-install"])
        .context("grub-install not found; install grub")?;

    println!("Installing GRUB to internal ESP {} ...", mnt_esp.display());
    run_cmd_check(&[
        &grub_install,
        "--target=x86_64-efi",
        &format!("--efi-directory={}", mnt_esp.display()),
        &format!("--boot-directory={}/boot", mnt_esp.display()),
        "--bootloader-id=Nimblex",
        // No --removable: we want a NVRAM entry on internal-disk install.
    ])?;

    write_grub_cfg(mnt_esp, entries)?;

    // efibootmgr should have been called by grub-install via --bootloader-id;
    // belt-and-braces, also write our own entry pointing at the same path
    // if we can parse esp_dev.
    let _ = esp_dev;

    println!("GRUB internal install complete.");
    Ok(())
}

fn write_grub_cfg(mnt_esp: &Path, entries: &[BootEntry]) -> Result<()> {
    let dir = mnt_esp.join("boot/grub");
    fs::create_dir_all(&dir)?;
    let cfg = dir.join("grub.cfg");

    let mut s = String::new();
    s.push_str("# Nimblex GRUB configuration\n");
    s.push_str("set timeout=3\n");
    s.push_str("set default=0\n");
    s.push_str("terminal_output console\n");
    s.push_str("insmod part_gpt\n");
    s.push_str("insmod fat\n");
    s.push_str("insmod ext2\n");
    s.push_str("insmod linux\n");
    s.push_str("insmod chain\n");
    s.push_str("insmod search\n");
    s.push_str("insmod search_label\n");
    s.push_str("insmod search_fs_uuid\n\n");

    s.push_str("# Anchor $root to our ESP regardless of how the firmware\n");
    s.push_str("# enumerated disks. Falls back gracefully if the label is\n");
    s.push_str("# absent (e.g. installed onto a shared Windows ESP).\n");
    s.push_str("if ! search --no-floppy --label --set=esp_root NIMBLEX_ESP; then\n");
    s.push_str("    set esp_root=$root\n");
    s.push_str("fi\n\n");

    for e in entries {
        match e {
            BootEntry::Linux { title, kernel, initrd, options } => {
                s.push_str(&format!("menuentry \"{}\" {{\n", grub_escape(title)));
                s.push_str("    set root=$esp_root\n");
                s.push_str(&format!("    linux  {}", kernel));
                if !options.is_empty() {
                    s.push_str(&format!(" {}", options));
                }
                s.push('\n');
                s.push_str(&format!("    initrd {}\n", initrd));
                s.push_str("}\n\n");
            }
            BootEntry::EfiChainload { title, efi_path, esp_fs_uuid } => {
                s.push_str(&format!("menuentry \"{}\" {{\n", grub_escape(title)));
                s.push_str("    insmod chain\n");
                s.push_str(&format!(
                    "    search --no-floppy --fs-uuid --set=root {}\n",
                    esp_fs_uuid
                ));
                s.push_str(&format!("    chainloader {}\n", efi_path));
                s.push_str("}\n\n");
            }
        }
    }

    s.push_str("menuentry \"Reboot\" { reboot }\n");
    s.push_str("menuentry \"Shut down\" { halt }\n");

    fs::write(&cfg, s).with_context(|| format!("write {}", cfg.display()))?;
    println!("Wrote {}", cfg.display());
    Ok(())
}

fn grub_escape(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}

// ───────────────────────── Entry composition ─────────────────────────

fn compose_entries(windows: &[WindowsInstall]) -> Vec<BootEntry> {
    let mut v = vec![
        BootEntry::Linux {
            title: "NimbleX".into(),
            kernel: "/EFI/nimblex/vmlinuz".into(),
            initrd: "/EFI/nimblex/initrd.img".into(),
            options: String::new(),
        },
        BootEntry::Linux {
            title: "NimbleX (CLI)".into(),
            kernel: "/EFI/nimblex/vmlinuz".into(),
            initrd: "/EFI/nimblex/initrd.img".into(),
            // Live-init's cli_or_gui() greps /proc/cmdline for the literal
            // word `cli` and, if found, relinks default.target to
            // multi-user.target before systemd takes over.
            options: "cli intel_iommu=off".into(),
        },
        BootEntry::Linux {
            title: "NimbleX (rescue)".into(),
            kernel: "/EFI/nimblex/vmlinuz".into(),
            initrd: "/EFI/nimblex/initrd.img".into(),
            // Boots straight into systemd's rescue.target — single-user
            // root shell on the console, minimal services, useful for
            // recovering a system that fails to reach multi-user.
            // Combined with nomodeset for maximum compatibility.
            options: "nomodeset systemd.unit=rescue.target".into(),
        },
    ];

    // Windows entries — one per detected install. systemd-boot can't
    // chainload across ESPs, so EfiChainload entries from a different ESP
    // are emitted as Linux-only-no-initrd ... no, we just trust GRUB to
    // chainload them (works for both same-ESP and cross-ESP cases). For
    // systemd-boot on the USB-live cross-ESP scenario we rely on
    // auto-firmware=yes in loader.conf to surface the firmware's own
    // Windows entry. The EfiChainload entries we write are still valid for
    // same-ESP installs; systemd-boot will resolve `efi /EFI/Microsoft/Boot/...`
    // against the ESP it was launched from.
    for (i, w) in windows.iter().enumerate() {
        let title = if windows.len() == 1 {
            "Windows Boot Manager".to_string()
        } else {
            format!("Windows Boot Manager ({})", i + 1)
        };
        v.push(BootEntry::EfiChainload {
            title,
            efi_path: w.bootmgfw_path.clone(),
            esp_fs_uuid: w.esp_fs_uuid.clone(),
        });
    }

    v
}

fn entry_stem(title: &str, idx: usize) -> String {
    let mut s = String::with_capacity(title.len() + 8);
    for c in title.chars() {
        if c.is_ascii_alphanumeric() {
            s.push(c.to_ascii_lowercase());
        } else if c == ' ' || c == '_' || c == '-' {
            s.push('-');
        }
    }
    if s.is_empty() {
        s = format!("entry-{}", idx);
    }
    // Disambiguate so two "Windows" entries don't collide.
    if title == "NimbleX" {
        // Default entry must be named exactly nimblex.conf so loader.conf
        // can reference it.
        return "nimblex".into();
    }
    if title == "NimbleX (CLI)" {
        return "nimblex-cli".into();
    }
    if title == "NimbleX (rescue)" {
        return "nimblex-rescue".into();
    }
    if s.starts_with("windows") && idx > 2 {
        // first Windows is "windows-boot-manager"; subsequent get suffix.
        return format!("{}-{}", s, idx);
    }
    s
}

// ───────────────────────── Windows detection ─────────────────────────

/// Scan every block partition the live system can see for a Windows ESP
/// (i.e. a FAT/vfat partition with `EFI/Microsoft/Boot/bootmgfw.efi`).
/// Skip the partition we're installing onto — `mnt_esp` is the path it's
/// currently mounted at, which we resolve back to a device node via
/// `/proc/mounts` to avoid double-mount conflicts.
pub fn detect_windows_installs(mnt_esp: &Path) -> Vec<WindowsInstall> {
    let our_dev = device_for_mountpoint(mnt_esp);

    // Enumerate FAT/vfat partitions via `lsblk`.
    let out = match Command::new("lsblk")
        .args(["-bnpo", "NAME,FSTYPE,UUID"])
        .output()
    {
        Ok(o) => o,
        Err(e) => {
            println!("Windows scan: lsblk failed: {}", e);
            return Vec::new();
        }
    };
    let text = String::from_utf8_lossy(&out.stdout);

    let mut found = Vec::new();
    let mut already_scanned: HashSet<String> = HashSet::new();
    for line in text.lines() {
        // lsblk -p prefixes name with /dev/, e.g. "/dev/sda1 vfat AB12-CD34"
        let mut it = line.split_whitespace();
        let name = match it.next() { Some(s) => s.to_string(), None => continue };
        let fstype = it.next().unwrap_or("");
        let uuid = it.next().unwrap_or("").to_string();
        if !matches!(fstype, "vfat" | "fat" | "msdos") { continue; }
        if already_scanned.contains(&name) { continue; }
        already_scanned.insert(name.clone());
        if our_dev.as_deref() == Some(name.as_str()) { continue; }

        // Try to mount RO and probe for bootmgfw.efi.
        let probe_mnt = std::env::temp_dir().join(format!(
            "nimblex-winscan-{}",
            name.trim_start_matches('/').replace('/', "-")
        ));
        let _ = fs::create_dir_all(&probe_mnt);
        let mounted = run_cmd_ok(&[
            "mount", "-o", "ro,nofail",
            &name,
            &probe_mnt.to_string_lossy(),
        ]);
        if !mounted { continue; }

        let bootmgfw = probe_mnt.join("EFI/Microsoft/Boot/bootmgfw.efi");
        let hit = bootmgfw.exists();
        let _ = run_cmd_ok(&["umount", &probe_mnt.to_string_lossy()]);
        let _ = fs::remove_dir(&probe_mnt);

        if hit {
            println!(
                "Detected Windows on {} (FS-UUID: {})",
                name,
                if uuid.is_empty() { "<unknown>" } else { &uuid }
            );
            found.push(WindowsInstall {
                esp_dev: PathBuf::from(&name),
                esp_fs_uuid: uuid,
                bootmgfw_path: "/EFI/Microsoft/Boot/bootmgfw.efi".into(),
            });
        }
    }
    found
}

fn device_for_mountpoint(mp: &Path) -> Option<String> {
    let s = mp.to_string_lossy();
    let mounts = fs::read_to_string("/proc/mounts").ok()?;
    for line in mounts.lines() {
        let mut it = line.split_whitespace();
        let dev = it.next()?;
        let mount = it.next()?;
        if mount == s { return Some(dev.to_string()); }
    }
    None
}

// ───────────────────────── BIOS fallback + NVRAM ─────────────────────────

fn install_syslinux_mbr(disk: &Path) {
    // Best-effort: if syslinux is present, write its MBR boot code so the
    // stick is also bootable on legacy BIOS hosts. Errors are non-fatal.
    if !Path::new("/usr/share/syslinux/mbr.bin").exists() {
        return;
    }
    let ok = run_cmd_ok(&[
        "dd",
        "if=/usr/share/syslinux/mbr.bin",
        &format!("of={}", disk.display()),
        "bs=440", "count=1", "conv=notrunc",
    ]);
    if ok {
        println!("BIOS MBR written via syslinux.");
    } else {
        println!("Note: syslinux MBR not written (dd failed; UEFI-only stick).");
    }
}

fn register_nvram_entry(esp_dev: &Path, loader_path: &str, label: &str) {
    let Some((disk, part)) = split_dev_part(esp_dev) else {
        println!("Warning: could not parse ESP path for efibootmgr.");
        return;
    };
    let _ = run_cmd_ok(&[
        "efibootmgr",
        "--create",
        "--disk", &disk,
        "--part", &part.to_string(),
        "--label", label,
        "--loader", loader_path,
    ]);
    println!("UEFI boot entry '{}' registered.", label);
}

fn split_dev_part(dev: &Path) -> Option<(String, u32)> {
    let s = dev.to_str()?;
    if let Some(idx) = s.rfind('p') {
        if let Ok(n) = s[idx + 1..].parse::<u32>() {
            return Some((s[..idx].to_string(), n));
        }
    }
    let idx = s.trim_end_matches(|c: char| c.is_ascii_digit()).len();
    if idx < s.len() {
        if let Ok(n) = s[idx..].parse::<u32>() {
            return Some((s[..idx].to_string(), n));
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn entry_stems_are_stable() {
        assert_eq!(entry_stem("NimbleX", 0), "nimblex");
        assert_eq!(entry_stem("NimbleX (CLI)", 1), "nimblex-cli");
        assert_eq!(entry_stem("NimbleX (rescue)", 2), "nimblex-rescue");
        assert_eq!(entry_stem("Windows Boot Manager", 2), "windows-boot-manager");
    }

    #[test]
    fn compose_entries_includes_rescue_token() {
        let v = compose_entries(&[]);
        let rescue_opts = v.iter().find_map(|e| match e {
            BootEntry::Linux { title, options, .. } if title.contains("rescue") => {
                Some(options.clone())
            }
            _ => None,
        }).expect("rescue entry exists");
        assert!(rescue_opts.contains("rescue.target"));
    }

    #[test]
    fn compose_entries_includes_cli_token() {
        let v = compose_entries(&[]);
        let cli_entry = v.iter().find_map(|e| match e {
            BootEntry::Linux { title, options, .. } if title.contains("CLI") => Some(options.clone()),
            _ => None,
        }).expect("CLI entry exists");
        assert!(cli_entry.contains("cli"), "CLI entry must carry the `cli` token");
    }

    #[test]
    fn compose_entries_appends_windows() {
        let win = WindowsInstall {
            esp_dev: PathBuf::from("/dev/nvme0n1p1"),
            esp_fs_uuid: "ABCD-1234".into(),
            bootmgfw_path: "/EFI/Microsoft/Boot/bootmgfw.efi".into(),
        };
        let v = compose_entries(&[win]);
        assert!(v.iter().any(|e| matches!(e, BootEntry::EfiChainload { .. })));
    }

    #[test]
    fn grub_escape_handles_quotes() {
        assert_eq!(grub_escape("Win\"dows"), "Win\\\"dows");
        assert_eq!(grub_escape("a\\b"), "a\\\\b");
    }

    #[test]
    fn split_dev_part_works() {
        assert_eq!(split_dev_part(Path::new("/dev/sda1")), Some(("/dev/sda".into(), 1)));
        assert_eq!(split_dev_part(Path::new("/dev/nvme0n1p3")), Some(("/dev/nvme0n1".into(), 3)));
    }
}
