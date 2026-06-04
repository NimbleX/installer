//! Shared mutable model passed between screens.
//!
//! Screens hold `Rc<RefCell<AppState>>` and never reference each other.
//! Whenever the user advances, the source screen writes its outputs into
//! `AppState`; the destination screen reads them in `refresh()`.

use installer_core::{Bootloader, Disk, InstallMode, LiveMedia, Plan};

#[derive(Default)]
pub struct AppState {
    /// Disks discovered by the most recent scan.
    pub disks: Vec<Disk>,
    /// The live boot medium detected for this session. Decides which targets
    /// are safe: the device we booted from must not be reformatted or
    /// repartitioned unless we are running from RAM, and an in-place reinstall
    /// onto it must use the crash-safe copy strategy. Detected once per scan.
    pub live_media: LiveMedia,
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
    /// Whether to reformat the existing partition before installing, in
    /// `InstallMode::ReuseNimblex`. Defaults to `false`: when a compatible
    /// Nimblex partition already exists we install onto it in place and only
    /// reformat if the user explicitly ticks the "Format" checkbox.
    pub reuse_format: bool,
    /// Whether the `ReuseNimblex` target is the partition the running session
    /// booted from (and we are not running from RAM). When true the install
    /// must use the crash-safe in-place copy strategy and must not format or
    /// unmount the target. Recomputed per selection in `refresh_layout`.
    pub reuse_inplace_live: bool,
    /// Bootloader backend chosen via CLI flag at launch. Defaults to
    /// `Auto`, which the planner resolves to `SystemdBoot` on UEFI and
    /// `Grub` on legacy BIOS.
    pub bootloader: Bootloader,
    /// The plan generated when the user clicks Continue.  Only set after
    /// the confirmation overlay has been accepted.
    pub plan: Option<Plan>,
}
