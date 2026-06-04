//! The two scenarios the installer supports. The GUI picks one on Screen 1
//! and the planner emits a different [`Plan`] for each.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Scenario {
    /// Wipe a removable USB stick and install Nimblex onto it as the only OS.
    UsbFullInstall,
    /// Shrink an existing Windows NTFS partition and install Nimblex into the
    /// reclaimed space, leaving Windows bootable.
    AlongsideWindows,
    /// Install Nimblex into existing unallocated space on the disk without
    /// modifying any existing partition (used when Windows is BitLocker
    /// encrypted but a large enough gap already exists).
    FreeSpace,
    /// Reformat and reuse an existing Nimblex partition left by a previous
    /// (possibly failed) install. No partitioning, no changes to Windows.
    ReuseExisting,
}
