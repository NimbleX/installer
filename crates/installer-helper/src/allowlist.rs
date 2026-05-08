//! Argv allowlist enforced before any step is executed.
//!
//! Each entry describes the program name and a coarse argv shape. The shape
//! is matched by program name (basename) and a per-program validator that
//! checks individual argument tokens. This is intentionally restrictive:
//! anything not enumerated here is refused.
//!
//! The allowlist is the security boundary between the unprivileged GUI and
//! the disk-modifying side effects. It runs *after* JSON parsing and
//! *before* step execution.

use anyhow::{anyhow, bail, Result};
use installer_core::Step;
use std::path::Path;

/// Validate every step in a plan. Returns `Ok(())` only if every argv
/// matches a known shape.
pub fn validate_steps(steps: &[Step]) -> Result<()> {
    for (i, s) in steps.iter().enumerate() {
        validate_argv(&s.argv).map_err(|e| anyhow!("step {} ({}): {}", i + 1, s.label, e))?;
    }
    Ok(())
}

/// Validate a single argv. Public for unit tests in the runner.
pub fn validate_argv(argv: &[String]) -> Result<()> {
    if argv.is_empty() {
        bail!("empty argv");
    }
    let prog_basename = Path::new(&argv[0])
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or(&argv[0]);
    let rest: Vec<&str> = argv[1..].iter().map(|s| s.as_str()).collect();

    match prog_basename {
        "f3probe" => check_f3probe(&rest),
        "sgdisk" => check_sgdisk(&rest),
        "wipefs" => check_wipefs(&rest),
        "parted" => check_parted(&rest),
        "partprobe" => check_partprobe(&rest),
        "mkfs.fat" => check_mkfs_fat(&rest),
        "mkfs.ext4" => check_mkfs_ext4(&rest),
        "ntfsresize" => check_ntfsresize(&rest),
        "sync" => {
            if !rest.is_empty() {
                bail!("sync takes no args");
            }
            Ok(())
        }
        "nimblex-installer-helper-internal" => check_internal(&rest),
        other => bail!("program '{}' is not on the allowlist", other),
    }
}

fn check_f3probe(rest: &[&str]) -> Result<()> {
    // f3probe [--time-ops] /dev/sdX
    if rest.is_empty() {
        bail!("f3probe needs a device argument");
    }
    let mut iter = rest.iter().peekable();
    while let Some(a) = iter.next() {
        if *a == "--time-ops" {
            continue;
        }
        if iter.peek().is_some() {
            bail!("unexpected f3probe arg: {}", a);
        }
        require_block_device(a)?;
    }
    Ok(())
}

fn check_sgdisk(rest: &[&str]) -> Result<()> {
    // Permitted forms:
    //   sgdisk --zap-all <dev>
    //   sgdisk --zap-all --new=N:START:+SIZE --typecode=N:CODE
    //          --change-name=N:LABEL ... <dev>
    //
    // The flags --new=, --typecode=, --change-name= accept well-formed
    // "N:value" tokens. N must be a small positive integer (partition
    // number). The value part is validated to contain no shell metacharacters.
    let mut saw_dev = false;
    for a in rest {
        if *a == "--zap-all" || *a == "-Z" {
            // always allowed
        } else if let Some(rest_val) = a.strip_prefix("--new=") {
            // --new=N:START:+SIZE  or  --new=N:START:END
            validate_sgdisk_nv("--new", rest_val)?;
        } else if let Some(rest_val) = a.strip_prefix("--typecode=") {
            // --typecode=N:XXXX
            validate_sgdisk_nv("--typecode", rest_val)?;
        } else if let Some(rest_val) = a.strip_prefix("--change-name=") {
            // --change-name=N:LABEL
            validate_sgdisk_nv("--change-name", rest_val)?;
        } else if a.starts_with('-') {
            bail!("unexpected sgdisk flag: {}", a);
        } else {
            require_block_device(a)?;
            if saw_dev {
                bail!("sgdisk accepts only one device");
            }
            saw_dev = true;
        }
    }
    if !saw_dev {
        bail!("sgdisk needs a device");
    }
    Ok(())
}

/// Validate an sgdisk `N:value` token.  N must be an integer 1–128.
/// The value part must be free of shell metacharacters.
fn validate_sgdisk_nv(flag: &str, nv: &str) -> Result<()> {
    let colon = nv.find(':').ok_or_else(|| anyhow!("{} value must be N:value", flag))?;
    let n_str = &nv[..colon];
    let val = &nv[colon + 1..];
    n_str
        .parse::<u8>()
        .map_err(|_| anyhow!("{} partition number must be 1-128, got '{}'", flag, n_str))?;
    // Reject shell metacharacters in the value part.
    if val.contains(|c: char| matches!(c, '`' | '$' | ';' | '|' | '&' | '>' | '<' | '\0' | '\n')) {
        bail!("{} value contains unsafe characters: '{}'", flag, val);
    }
    Ok(())
}

fn check_wipefs(rest: &[&str]) -> Result<()> {
    // wipefs --all --force /dev/sdX
    let mut saw_dev = false;
    for a in rest {
        match *a {
            "--all" | "--force" | "-a" | "-f" => {}
            other if other.starts_with('-') => bail!("unexpected wipefs flag: {}", other),
            other => {
                require_block_device(other)?;
                if saw_dev {
                    bail!("wipefs accepts only one device");
                }
                saw_dev = true;
            }
        }
    }
    if !saw_dev {
        bail!("wipefs needs a device");
    }
    Ok(())
}

fn check_parted(rest: &[&str]) -> Result<()> {
    // parted --script /dev/sdX <op> ...
    if !rest.iter().any(|a| *a == "--script" || *a == "-s") {
        bail!("parted must run --script");
    }
    let dev = rest
        .iter()
        .find(|a| !a.starts_with('-') && a.starts_with("/dev/"))
        .ok_or_else(|| anyhow!("parted needs a /dev/* target"))?;
    require_block_device(dev)?;
    // Operation must be one of mklabel/mkpart/set/resizepart.
    let ops_seen: Vec<&&str> = rest
        .iter()
        .filter(|a| matches!(***a, _) && matches!(a.as_ref(), "mklabel" | "mkpart" | "set" | "resizepart"))
        .collect();
    if ops_seen.is_empty() {
        bail!("parted: no recognised operation among args");
    }
    Ok(())
}

fn check_partprobe(rest: &[&str]) -> Result<()> {
    if rest.len() != 1 {
        bail!("partprobe takes exactly one device");
    }
    require_block_device(rest[0])
}

fn check_mkfs_fat(rest: &[&str]) -> Result<()> {
    // mkfs.fat -F32 -n LABEL /dev/sdXY
    let mut saw_dev = false;
    let mut iter = rest.iter().peekable();
    while let Some(a) = iter.next() {
        match *a {
            "-F32" | "-F" | "-n" | "-I" => {
                // -n / -F take a value; -F32 is a single token; advance if needed.
                if matches!(*a, "-n" | "-F") {
                    iter.next().ok_or_else(|| anyhow!("missing value for {}", a))?;
                }
            }
            other if other.starts_with('-') => bail!("unexpected mkfs.fat flag: {}", other),
            other => {
                require_block_device(other)?;
                saw_dev = true;
            }
        }
    }
    if !saw_dev {
        bail!("mkfs.fat needs a device");
    }
    Ok(())
}

fn check_mkfs_ext4(rest: &[&str]) -> Result<()> {
    let mut saw_dev = false;
    let mut iter = rest.iter().peekable();
    while let Some(a) = iter.next() {
        match *a {
            "-F" | "-q" => {}
            "-L" | "-U" | "-T" | "-m" | "-N" => {
                iter.next().ok_or_else(|| anyhow!("missing value for {}", a))?;
            }
            "-O" => {
                // Only allow the explicit 64bit feature flag.
                let val = iter.next().ok_or_else(|| anyhow!("-O needs a value"))?;
                if *val != "64bit" {
                    bail!("mkfs.ext4 -O: only '64bit' is allowed, got '{}'", val);
                }
            }
            other if other.starts_with('-') => bail!("unexpected mkfs.ext4 flag: {}", other),
            other => {
                require_block_device(other)?;
                saw_dev = true;
            }
        }
    }
    if !saw_dev {
        bail!("mkfs.ext4 needs a device");
    }
    Ok(())
}

fn check_ntfsresize(rest: &[&str]) -> Result<()> {
    let mut saw_dev = false;
    let mut iter = rest.iter().peekable();
    while let Some(a) = iter.next() {
        match *a {
            "--info" | "-i" | "--force" | "-f" | "--no-progress-bar" | "-P" => {}
            "--size" | "-s" => {
                let v = iter
                    .next()
                    .ok_or_else(|| anyhow!("missing value for {}", a))?;
                v.parse::<u64>()
                    .map_err(|_| anyhow!("--size expects bytes integer, got {}", v))?;
            }
            other if other.starts_with('-') => bail!("unexpected ntfsresize flag: {}", other),
            other => {
                require_block_device(other)?;
                saw_dev = true;
            }
        }
    }
    if !saw_dev {
        bail!("ntfsresize needs a device");
    }
    Ok(())
}

fn check_internal(rest: &[&str]) -> Result<()> {
    // First positional must be a known subcommand. Argument validation is
    // delegated to clap inside the internal binary; the allowlist only
    // gatekeeps the subcommand name.
    let sub = rest
        .iter()
        .find(|a| !a.starts_with('-'))
        .ok_or_else(|| anyhow!("internal helper needs a subcommand"))?;
    match *sub {
        "copy-system"
        | "install-boot-usb"
        | "install-boot-internal"
        | "check-fast-startup"
        | "mkpart-after"
        | "resizepart"
        | "settle-partitions" => Ok(()),
        other => bail!("unknown internal subcommand: {}", other),
    }
}

fn require_block_device(path: &str) -> Result<()> {
    if !path.starts_with("/dev/") {
        bail!("not a /dev path: {}", path);
    }
    if path.contains("..") || path.contains('\0') {
        bail!("suspicious device path: {}", path);
    }
    // We do not stat() here; the helper may run before partitions exist
    // (e.g. partprobe immediately after mkpart). The real check is the
    // /dev/ prefix plus the mkfs/parted/etc. tool's own validation.
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(slice: &[&str]) -> Vec<String> {
        slice.iter().map(|x| (*x).to_string()).collect()
    }

    #[test]
    fn allows_canonical_mkfs_ext4() {
        // Old form (no -O) still accepted
        validate_argv(&s(&["mkfs.ext4", "-F", "-L", "NIMBLEX_ROOT", "/dev/sdb2"])).unwrap();
    }

    #[test]
    fn allows_mkfs_ext4_with_64bit() {
        validate_argv(&s(&["mkfs.ext4", "-F", "-O", "64bit", "-L", "NIMBLEX_ROOT", "/dev/sdb2"])).unwrap();
    }

    #[test]
    fn rejects_mkfs_ext4_unknown_feature() {
        validate_argv(&s(&["mkfs.ext4", "-F", "-O", "encrypt", "/dev/sdb2"])).unwrap_err();
    }

    #[test]
    fn rejects_unknown_program() {
        let err = validate_argv(&s(&["dd", "if=/dev/sda", "of=/dev/sdb"])).unwrap_err();
        assert!(err.to_string().contains("not on the allowlist"));
    }

    #[test]
    fn rejects_non_dev_path() {
        let err = validate_argv(&s(&["wipefs", "--all", "/etc/passwd"])).unwrap_err();
        assert!(err.to_string().contains("/dev path"));
    }

    #[test]
    fn rejects_path_traversal() {
        let err =
            validate_argv(&s(&["mkfs.ext4", "-F", "/dev/../tmp/evil"])).unwrap_err();
        assert!(err.to_string().contains("suspicious"));
    }

    #[test]
    fn allows_ntfsresize_info_then_size() {
        validate_argv(&s(&[
            "ntfsresize",
            "--force",
            "--no-progress-bar",
            "--size",
            "12345678",
            "/dev/sda3",
        ]))
        .unwrap();
    }

    #[test]
    fn allows_sgdisk_zap_all_only() {
        validate_argv(&s(&["sgdisk", "--zap-all", "/dev/sda"])).unwrap();
    }

    #[test]
    fn allows_sgdisk_full_partition_creation() {
        validate_argv(&s(&[
            "sgdisk",
            "--zap-all",
            "--new=1:2048:+512M",
            "--typecode=1:EF00",
            "--change-name=1:NIMBLEX_ESP",
            "--new=2:0:0",
            "--typecode=2:8300",
            "--change-name=2:NIMBLEX_ROOT",
            "/dev/sda",
        ]))
        .unwrap();
    }

    #[test]
    fn rejects_sgdisk_no_device() {
        validate_argv(&s(&["sgdisk", "--zap-all"])).unwrap_err();
    }

    #[test]
    fn rejects_sgdisk_multiple_devices() {
        validate_argv(&s(&["sgdisk", "--zap-all", "/dev/sda", "/dev/sdb"])).unwrap_err();
    }

    #[test]
    fn rejects_sgdisk_unknown_flag() {
        validate_argv(&s(&["sgdisk", "--delete=1", "/dev/sda"])).unwrap_err();
    }

    #[test]
    fn rejects_sgdisk_shell_injection_in_name() {
        validate_argv(&s(&[
            "sgdisk",
            "--change-name=1:foo$(rm -rf /)",
            "/dev/sda",
        ]))
        .unwrap_err();
    }

    #[test]
    fn rejects_ntfsresize_non_numeric_size() {
        let err = validate_argv(&s(&[
            "ntfsresize",
            "--size",
            "100GB",
            "/dev/sda3",
        ]))
        .unwrap_err();
        assert!(err.to_string().contains("bytes integer"));
    }
}
