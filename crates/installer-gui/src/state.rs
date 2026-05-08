//! Shared mutable model passed between screens.
//!
//! Screens hold `Rc<RefCell<AppState>>` and never reference each other.
//! Whenever the user advances, the source screen writes its outputs into
//! `AppState`; the destination screen reads them in `refresh()`.

use installer_core::{Bootloader, Disk, InstallMode, Plan};

#[derive(Default)]
pub struct AppState {
    /// Disks discovered by the most recent scan.
    pub disks: Vec<Disk>,
    /// Index into `disks` of the disk the user picked.
    pub selected_disk: Option<usize>,
    /// Top-level operation chosen on the destination screen. `None` until
    /// the user toggles a mode.  In Erase mode the slider is hidden and
    /// `requested_reclaim_bytes` is ignored.
    pub install_mode: Option<InstallMode>,
    /// User-chosen reclaim amount, in bytes. Interpreted only when
    /// `install_mode == Some(AlongsideWindows)`. `None` until the slider
    /// is shown for the first time.
    pub requested_reclaim_bytes: Option<u64>,
    /// Bootloader backend chosen via CLI flag at launch. Defaults to
    /// `Auto`, which the planner resolves to `SystemdBoot` on UEFI and
    /// `Grub` on legacy BIOS.
    pub bootloader: Bootloader,
    /// The plan generated when the user clicks Continue.  Only set after
    /// the confirmation overlay has been accepted.
    pub plan: Option<Plan>,
}
