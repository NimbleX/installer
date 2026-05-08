//! Byte-size newtype with human-readable formatting.

use serde::{Deserialize, Serialize};
use std::fmt;
use std::ops::{Add, Sub};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Bytes(pub u64);

impl Bytes {
    pub const KIB: u64 = 1024;
    pub const MIB: u64 = 1024 * Self::KIB;
    pub const GIB: u64 = 1024 * Self::MIB;
    pub const TIB: u64 = 1024 * Self::GIB;

    pub const fn from_gib(g: u64) -> Self {
        Self(g * Self::GIB)
    }
    pub const fn from_mib(m: u64) -> Self {
        Self(m * Self::MIB)
    }
    pub const fn from_kib(k: u64) -> Self {
        Self(k * Self::KIB)
    }

    pub fn as_u64(self) -> u64 {
        self.0
    }
}

impl fmt::Display for Bytes {
    /// Pretty-print using powers of 1024 with one decimal where useful.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let n = self.0 as f64;
        if n >= Self::TIB as f64 {
            write!(f, "{:.2} TiB", n / Self::TIB as f64)
        } else if n >= Self::GIB as f64 {
            write!(f, "{:.1} GiB", n / Self::GIB as f64)
        } else if n >= Self::MIB as f64 {
            write!(f, "{:.0} MiB", n / Self::MIB as f64)
        } else if n >= Self::KIB as f64 {
            write!(f, "{:.0} KiB", n / Self::KIB as f64)
        } else {
            write!(f, "{} B", self.0)
        }
    }
}

impl Add for Bytes {
    type Output = Bytes;
    fn add(self, rhs: Bytes) -> Bytes {
        Bytes(self.0.saturating_add(rhs.0))
    }
}

impl Sub for Bytes {
    type Output = Bytes;
    fn sub(self, rhs: Bytes) -> Bytes {
        Bytes(self.0.saturating_sub(rhs.0))
    }
}

impl From<u64> for Bytes {
    fn from(v: u64) -> Self {
        Bytes(v)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formatting() {
        assert_eq!(Bytes::from_gib(1).to_string(), "1.0 GiB");
        assert_eq!(Bytes::from_mib(512).to_string(), "512 MiB");
        assert_eq!(Bytes(1500).to_string(), "1 KiB");
    }

    #[test]
    fn arithmetic_saturates() {
        let big = Bytes(u64::MAX);
        assert_eq!((big + Bytes(1)).0, u64::MAX);
        assert_eq!((Bytes(0) - Bytes(1)).0, 0);
    }
}
