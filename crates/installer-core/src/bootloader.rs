//! Pluggable bootloader selection.
//!
//! `Bootloader::Auto` is the user-facing default; it resolves to a concrete
//! backend based on the host firmware (UEFI → systemd-boot, BIOS → GRUB).
//! Planner code calls [`Bootloader::resolve`] before emitting argv so the
//! plan JSON always carries a concrete value (`SystemdBoot` or `Grub`),
//! making it deterministic and machine-shippable.

use serde::{Deserialize, Serialize};
use std::fmt;
use std::path::Path;
use std::str::FromStr;

/// Which bootloader the user (or `Auto` resolution) wants written to the
/// target.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum Bootloader {
    /// Pick a sensible backend based on host firmware: systemd-boot on UEFI,
    /// GRUB on legacy BIOS.
    #[default]
    Auto,
    /// systemd-boot — small, fast, declarative `loader/entries/*.conf`.
    /// UEFI only.
    SystemdBoot,
    /// GRUB — universal, supports BIOS and UEFI, can chainload Windows
    /// across separate ESPs.
    Grub,
}

impl Bootloader {
    /// Resolve `Auto` to a concrete backend for the given firmware. Concrete
    /// values pass through unchanged.
    pub fn resolve(self, firmware: Firmware) -> Bootloader {
        match self {
            Bootloader::Auto => match firmware {
                Firmware::Uefi => Bootloader::SystemdBoot,
                Firmware::Bios => Bootloader::Grub,
            },
            other => other,
        }
    }

    /// Kebab-case wire form used in argv and serde.
    pub fn as_str(self) -> &'static str {
        match self {
            Bootloader::Auto => "auto",
            Bootloader::SystemdBoot => "systemd-boot",
            Bootloader::Grub => "grub",
        }
    }

    /// All variants, useful for `clap::ValueEnum`-style wiring without
    /// pulling clap into installer-core.
    pub fn variants() -> &'static [Bootloader] {
        &[Bootloader::Auto, Bootloader::SystemdBoot, Bootloader::Grub]
    }
}

impl fmt::Display for Bootloader {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for Bootloader {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "auto" => Ok(Bootloader::Auto),
            "systemd-boot" => Ok(Bootloader::SystemdBoot),
            "grub" => Ok(Bootloader::Grub),
            other => Err(format!(
                "unknown bootloader '{}'; expected one of: auto, systemd-boot, grub",
                other
            )),
        }
    }
}

/// Firmware mode of the host running the installer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Firmware {
    Uefi,
    Bios,
}

impl Firmware {
    /// Detect by presence of `/sys/firmware/efi`. The kernel only exposes
    /// that directory when booted via UEFI.
    pub fn detect() -> Firmware {
        if Path::new("/sys/firmware/efi").exists() {
            Firmware::Uefi
        } else {
            Firmware::Bios
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auto_resolves_correctly() {
        assert_eq!(
            Bootloader::Auto.resolve(Firmware::Uefi),
            Bootloader::SystemdBoot
        );
        assert_eq!(
            Bootloader::Auto.resolve(Firmware::Bios),
            Bootloader::Grub
        );
    }

    #[test]
    fn concrete_values_pass_through() {
        assert_eq!(
            Bootloader::SystemdBoot.resolve(Firmware::Bios),
            Bootloader::SystemdBoot
        );
        assert_eq!(
            Bootloader::Grub.resolve(Firmware::Uefi),
            Bootloader::Grub
        );
    }

    #[test]
    fn round_trip_parse_display() {
        for &b in Bootloader::variants() {
            let s = b.to_string();
            assert_eq!(s.parse::<Bootloader>().unwrap(), b);
        }
    }

    #[test]
    fn rejects_garbage() {
        assert!("refind".parse::<Bootloader>().is_err());
        assert!("".parse::<Bootloader>().is_err());
    }

    #[test]
    fn serde_kebab_case() {
        let json = serde_json::to_string(&Bootloader::SystemdBoot).unwrap();
        assert_eq!(json, "\"systemd-boot\"");
        let back: Bootloader = serde_json::from_str("\"grub\"").unwrap();
        assert_eq!(back, Bootloader::Grub);
    }
}
