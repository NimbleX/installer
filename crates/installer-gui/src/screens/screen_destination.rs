//! Screen 1 — Destination.
//!
//! Combines disk selection + auto-locked mode (Alongside / Erase) + a
//! "Now / After" partition preview + reclaim slider + confirm overlay.

use crate::app::STACK_INSTALL;
use crate::state::AppState;
use crate::widgets::{
    app_card, disk_card, partition_preview::PlannedSegment, partition_preview::SegmentAction,
    Header, HeaderStep, PartitionPreview,
};
use gtk4::prelude::*;
use gtk4::{
    glib, Align, Box as GtkBox, Button, CheckButton, FlowBox, Label, Orientation, PolicyType,
    Revealer, RevealerTransitionType, ScrolledWindow, SelectionMode, Stack, TextBuffer, TextView,
    ToggleButton, Window, WrapMode,
};
use installer_core::{
    resize::{
        min_install, min_reclaim, min_windows_residual_after_shrink, WINDOWS_MIN_FREE_BEFORE_SHRINK,
    },
    Bytes, Disk, DiskScanner, InstallMode, InstallPlanner, PartitionRole,
};
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::mpsc;
use std::time::Duration;

/// 512 MiB ESP — matches `InstallPlanner::plan_usb`.
const ESP_SIZE_BYTES: u64 = 512 * 1024 * 1024;
/// 1 MiB alignment gap at the start of the disk for fresh GPT installs.
const GPT_ALIGN_BYTES: u64 = 1024 * 1024;

/// Shown when the user selects the disk they booted from for a destructive
/// whole-disk operation (erase / alongside / free-space) and we are NOT
/// running from RAM. Such a disk cannot be repartitioned or formatted while
/// it is feeding the live session.
const LIVE_DISK_BLOCK_MSG: &str =
    "You started NimbleX from this drive, so it can't be erased or repartitioned \
     while it's running. Reboot and choose the 'Copy to RAM' option, or run the \
     installer from a separate USB stick.";

/// Shown when reinstalling in place onto the partition the session booted
/// from. This is allowed via the crash-safe copy path; formatting is disabled.
const LIVE_INPLACE_INFO_MSG: &str =
    "You're reinstalling onto the partition you're running from. NimbleX will be \
     updated in place safely; the changes take effect after you reboot. Formatting \
     isn't available here because it would erase the running system.";

#[derive(Clone)]
pub struct ScreenDestination {
    root: GtkBox,
    state: Rc<RefCell<AppState>>,
    header: Header,

    cards_box: FlowBox,
    disk_cards: Rc<RefCell<Vec<(ToggleButton, usize)>>>,
    slider_busy: Rc<Cell<bool>>,

    mode_pill: Label,
    preview: PartitionPreview,
    format_existing_check: CheckButton,
    warning: Label,
    continue_btn: Button,

    // Confirm overlay
    revealer: Revealer,
    overlay_summary: Label,
    overlay_check: CheckButton,
    overlay_install_btn: Button,
}

impl ScreenDestination {
    pub fn new(stack: Stack, state: Rc<RefCell<AppState>>) -> Self {
        // ---- Outer card ----
        let root = GtkBox::new(Orientation::Vertical, 0);
        root.set_hexpand(true);
        root.set_vexpand(true);

        let card = app_card();
        let header = Header::new(HeaderStep::Destination);
        card.append(header.widget());

        // ---- Body ----
        let body = GtkBox::new(Orientation::Vertical, 14);
        body.add_css_class("screen-body");

        let h1 = Label::new(Some("Where should we install NimbleX?"));
        h1.add_css_class("screen-h1");
        h1.set_halign(Align::Start);
        body.append(&h1);

        let sub = Label::new(Some(
            "Pick a destination drive. Nothing changes until you confirm.",
        ));
        sub.add_css_class("screen-subtitle");
        sub.set_halign(Align::Start);
        sub.set_wrap(true);
        body.append(&sub);

        // ---- Disk cards ----
        // Wrapped in a ScrolledWindow to prevent large disk counts from
        // forcing the window to expand vertically beyond the screen height.
        let cards_box = FlowBox::builder()
            .orientation(Orientation::Horizontal)
            .min_children_per_line(2)
            .max_children_per_line(4)
            .homogeneous(true)
            .row_spacing(10)
            .column_spacing(10)
            .selection_mode(SelectionMode::None)
            .activate_on_single_click(false)
            .valign(Align::Start)
            .build();
        cards_box.add_css_class("disks-row");

        let cards_scroll = ScrolledWindow::builder()
            .hscrollbar_policy(PolicyType::Never)
            .vscrollbar_policy(PolicyType::Automatic)
            .vexpand(true)
            .child(&cards_box)
            .build();
        body.append(&cards_scroll);

        // (Mode pill removed: redundant with the AFTER-strip badges and the
        // info_row sentence below the preview. We keep a hidden Label in the
        // struct only because other methods reference it; it stays empty.)
        let mode_pill = Label::new(Some(""));
        mode_pill.set_visible(false);

        // ---- Preview ----
        let preview_panel = GtkBox::new(Orientation::Vertical, 8);
        preview_panel.add_css_class("preview-panel");
        let preview_title = Label::new(Some("Partition layout"));
        preview_title.add_css_class("preview-title");
        preview_title.set_halign(Align::Start);
        preview_panel.append(&preview_title);
        let preview = PartitionPreview::new();
        preview_panel.append(preview.widget());
        body.append(&preview_panel);

        // ---- Format-existing checkbox (ReuseNimblex mode only) ----
        // When a compatible Nimblex partition already exists, the default is
        // to install onto it in place. Ticking this reformats it first,
        // wiping any existing data. Hidden for every other mode.
        let format_existing_check = CheckButton::with_label(
            "Format the partition first (erases everything currently on it)",
        );
        format_existing_check.add_css_class("understand");
        format_existing_check.set_visible(false);
        format_existing_check.set_active(false);
        body.append(&format_existing_check);

        let warning = Label::new(Some(""));
        warning.add_css_class("warning-bar");
        warning.set_halign(Align::Start);
        warning.set_wrap(true);
        warning.set_visible(false);
        body.append(&warning);

        card.append(&body);

        // ---- Footer ----
        let footer = GtkBox::new(Orientation::Horizontal, 8);
        footer.add_css_class("app-footer");

        let spacer = GtkBox::new(Orientation::Horizontal, 0);
        spacer.set_hexpand(true);
        footer.append(&spacer);

        let cancel_btn = Button::with_label("Cancel");
        cancel_btn.add_css_class("btn-secondary");
        footer.append(&cancel_btn);

        let continue_btn = Button::with_label("Continue");
        continue_btn.add_css_class("btn-primary");
        continue_btn.set_sensitive(false);
        footer.append(&continue_btn);

        card.append(&footer);

        // ---- Confirm overlay ----
        let revealer = Revealer::new();
        revealer.set_transition_type(RevealerTransitionType::SlideUp);
        revealer.set_transition_duration(220);
        revealer.set_reveal_child(false);
        revealer.set_valign(Align::End);

        let overlay_sheet = GtkBox::new(Orientation::Vertical, 12);
        overlay_sheet.add_css_class("overlay-sheet");

        let overlay_summary = Label::new(Some(""));
        overlay_summary.add_css_class("overlay-summary-bold");
        overlay_summary.set_halign(Align::Start);
        overlay_summary.set_wrap(true);
        overlay_sheet.append(&overlay_summary);

        let overlay_check = CheckButton::with_label("I understand my disk will be modified.");
        overlay_check.add_css_class("understand");
        overlay_sheet.append(&overlay_check);

        let overlay_actions = GtkBox::new(Orientation::Horizontal, 8);
        let cancel_overlay = Button::with_label("Cancel");
        cancel_overlay.add_css_class("btn-secondary");
        let spacer2 = GtkBox::new(Orientation::Horizontal, 0);
        spacer2.set_hexpand(true);
        let install_btn = Button::with_label("Install now");
        install_btn.add_css_class("btn-primary");
        install_btn.set_sensitive(false);
        overlay_actions.append(&cancel_overlay);
        overlay_actions.append(&spacer2);
        overlay_actions.append(&install_btn);
        overlay_sheet.append(&overlay_actions);

        revealer.set_child(Some(&overlay_sheet));

        let outer_overlay = gtk4::Overlay::new();
        outer_overlay.set_child(Some(&card));
        outer_overlay.add_overlay(&revealer);
        root.append(&outer_overlay);

        let me = Self {
            root,
            state: state.clone(),
            header: header.clone(),
            cards_box: cards_box.clone(),
            disk_cards: Rc::new(RefCell::new(Vec::new())),
            mode_pill: mode_pill.clone(),
            preview: preview.clone(),
            format_existing_check: format_existing_check.clone(),
            warning: warning.clone(),
            continue_btn: continue_btn.clone(),
            revealer: revealer.clone(),
            overlay_summary: overlay_summary.clone(),
            overlay_check: overlay_check.clone(),
            overlay_install_btn: install_btn.clone(),
            slider_busy: Rc::new(Cell::new(false)),
        };

        // ---- Wiring ----
        // Reclaim slider drag via PartitionPreview
        let me_slider = me.clone();
        preview.set_on_reclaim_changed(move |byte_offset| {
            if me_slider.slider_busy.get() {
                return;
            }
            // byte_offset is the point where the user dragged the handle.
            // We need to figure out how many bytes they want to reclaim.
            // In Alongside mode, the layout is:
            // [ ... kept ... ] [ Windows (shrunk) ] [ Nimblex (reclaimed) ] [ ... kept ... ]
            // The boundary is between Windows and Nimblex.
            // So `byte_offset` = start of Nimblex.

            // We can calculate `requested_reclaim_bytes` = `win.size.0 - (byte_offset - win.start.0)`.
            // Let's do that.
            let snapshot = {
                let s = me_slider.state.borrow();
                s.selected_disk
                    .and_then(|i| s.disks.get(i))
                    .map(|d| (d.clone(), d.primary_windows_partition().cloned()))
            };
            let (_disk, win_opt) = match snapshot {
                Some(v) => v,
                None => return,
            };
            let win = match win_opt {
                Some(p) => p,
                None => return,
            };

            // Calculate kept bytes
            let kept_bytes = byte_offset.saturating_sub(win.start.0);

            // Reclaim is the rest of the Windows partition
            let reclaim = win.size.0.saturating_sub(kept_bytes);

            let min = min_reclaim();
            let win_kept_floor = min_windows_residual_after_shrink(win.used.unwrap_or(Bytes(0)));
            let max = if win_kept_floor >= win.size {
                min
            } else {
                (win.size - win_kept_floor).max(min)
            };

            let clamped_reclaim = reclaim.clamp(min.0, max.0);

            if let Ok(mut st) = me_slider.state.try_borrow_mut() {
                st.requested_reclaim_bytes = Some(clamped_reclaim);
            } else {
                return;
            }
            me_slider.refresh_slider_labels();
        });

        // Show commands
        let me_show = me.clone();
        header.show_commands_btn().connect_clicked(move |_| {
            me_show.show_commands_dialog();
        });

        // Format-existing checkbox → record choice and re-confirm consent.
        let me_fmt = me.clone();
        format_existing_check.connect_toggled(move |c| {
            if let Ok(mut st) = me_fmt.state.try_borrow_mut() {
                st.reuse_format = c.is_active();
            }
            // Changing the format choice changes how destructive the install
            // is, so any prior "I understand" consent must be re-collected.
            me_fmt.overlay_check.set_active(false);
            me_fmt.revealer.set_reveal_child(false);
        });

        // Continue → reveal overlay (and light up Confirm step)
        let me_cont = me.clone();
        continue_btn.connect_clicked(move |_| {
            if me_cont.build_plan_into_state() {
                me_cont.update_overlay_summary();
                me_cont.overlay_check.set_active(false);
                me_cont.overlay_install_btn.set_sensitive(false);
                me_cont.header.set_active(HeaderStep::Confirm);
                me_cont.revealer.set_reveal_child(true);
            }
        });

        // Cancel → quit app
        cancel_btn.connect_clicked(move |b| {
            if let Some(win) = b.root().and_downcast::<Window>() {
                win.close();
            }
        });

        // Cancel overlay — return stepper to Destination
        let revealer_cancel = revealer.clone();
        let header_cancel = header.clone();
        cancel_overlay.connect_clicked(move |_| {
            revealer_cancel.set_reveal_child(false);
            header_cancel.set_active(HeaderStep::Destination);
        });

        // Understand checkbox enables install
        let install_btn_check = install_btn.clone();
        overlay_check.connect_toggled(move |c| {
            install_btn_check.set_sensitive(c.is_active());
        });

        // Install now → switch screen
        let stack_install = stack.clone();
        install_btn.connect_clicked(move |_| {
            stack_install.set_visible_child_name(STACK_INSTALL);
        });

        let _ = glib::user_runtime_dir();
        me
    }

    pub fn widget(&self) -> &GtkBox {
        &self.root
    }

    /// Show a lightweight loading state and perform the expensive disk scan
    /// off the GTK thread. This lets the window present immediately instead
    /// of blocking on NTFS/ext usage probes before the first frame is drawn.
    pub fn refresh_async(&self) {
        self.show_scanning_state();

        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let result = DiskScanner::scan_with_usage();
            let _ = tx.send(result);
        });

        let me = self.clone();
        let rx = Rc::new(RefCell::new(rx));
        glib::timeout_add_local(Duration::from_millis(40), move || {
            match rx.borrow().try_recv() {
                Ok(result) => {
                    me.finish_refresh(result);
                    glib::ControlFlow::Break
                }
                Err(mpsc::TryRecvError::Empty) => glib::ControlFlow::Continue,
                Err(mpsc::TryRecvError::Disconnected) => {
                    me.finish_refresh(Err(anyhow::anyhow!("disk scan worker stopped")));
                    glib::ControlFlow::Break
                }
            }
        });
    }

    fn show_scanning_state(&self) {
        // Reset stepper.
        self.header.set_active(HeaderStep::Destination);

        // Clear cards.
        while let Some(child) = self.cards_box.first_child() {
            self.cards_box.remove(&child);
        }
        self.disk_cards.borrow_mut().clear();

        {
            let mut s = self.state.borrow_mut();
            s.disks.clear();
            s.selected_disk = None;
            s.requested_reclaim_bytes = None;
            s.install_mode = None;
            s.reuse_format = false;
            s.reuse_inplace_live = false;
            s.plan = None;
        }

        self.preview.set_disk(None);
        self.format_existing_check.set_active(false);
        self.format_existing_check.set_visible(false);
        self.warning.set_text("Scanning disks…");
        self.warning.set_visible(true);
        self.continue_btn.set_sensitive(false);
        self.header.show_commands_btn().set_sensitive(false);
        self.overlay_check.set_active(false);
        self.revealer.set_reveal_child(false);

        let msg = Label::new(Some("Scanning disks…"));
        msg.add_css_class("dim");
        msg.set_halign(Align::Start);
        self.cards_box.append(&msg);
    }

    fn finish_refresh(&self, result: anyhow::Result<Vec<Disk>>) {
        while let Some(child) = self.cards_box.first_child() {
            self.cards_box.remove(&child);
        }
        self.disk_cards.borrow_mut().clear();
        self.warning.set_visible(false);

        let disks = match result {
            Ok(d) => d,
            Err(e) => {
                let text = format!("Could not scan disks: {}", e);
                self.warn(&text);
                let msg = Label::new(Some(&text));
                msg.add_css_class("warning-bar");
                msg.set_wrap(true);
                self.cards_box.append(&msg);
                return;
            }
        };

        if disks.is_empty() {
            let msg = Label::new(Some(
                "No installable drives found. Plug in a target drive and reopen.",
            ));
            msg.add_css_class("dim");
            msg.set_halign(Align::Start);
            self.cards_box.append(&msg);
            return;
        }

        {
            let mut s = self.state.borrow_mut();
            s.disks = disks.clone();
            s.selected_disk = None;
            s.requested_reclaim_bytes = None;
            s.install_mode = None;
            s.reuse_format = false;
            // Detect the live boot medium once per scan. Stable for the
            // session; the gating in `refresh_layout` reads it per selection.
            s.live_media = installer_core::LiveMedia::detect();
        }
        self.format_existing_check.set_active(false);
        self.format_existing_check.set_visible(false);

        // Build a card per disk; group them so they act as radio.
        let mut group_anchor: Option<ToggleButton> = None;
        for (idx, d) in disks.iter().enumerate() {
            let btn = disk_card(d);
            if let Some(anchor) = &group_anchor {
                btn.set_group(Some(anchor));
            } else {
                group_anchor = Some(btn.clone());
            }
            let me = self.clone();
            btn.connect_toggled(move |b| {
                if b.is_active() {
                    if let Ok(mut st) = me.state.try_borrow_mut() {
                        st.selected_disk = Some(idx);
                    } else {
                        return;
                    }
                    // Changing the target disk invalidates any prior
                    // "I understand" consent — reset it and hide the
                    // overlay so the user must re-confirm.
                    me.overlay_check.set_active(false);
                    me.revealer.set_reveal_child(false);
                    me.refresh_layout();
                }
            });
            self.append_card(&btn);
            self.disk_cards.borrow_mut().push((btn, idx));
        }

        // Auto-select first disk to give the user something to look at.
        if let Some((btn, _)) = self.disk_cards.borrow().first() {
            btn.set_active(true);
        }
    }

    /// Wrap a ToggleButton in a FlowBoxChild before appending; this lets
    /// FlowBox arrange disk cards in a responsive grid (1-N per row based
    /// on available width) while preserving the ToggleButton group's radio
    /// behaviour.
    fn append_card(&self, btn: &ToggleButton) {
        self.cards_box.append(btn);
        // Style the wrapping FlowBoxChild so it doesn't add its own border.
        if let Some(parent) = btn.parent() {
            parent.add_css_class("disk-card-cell");
            parent.set_focusable(false);
        }
    }

    /// Update mode pill, preview, slider, and continue-button state based on
    /// the currently-selected disk.
    fn refresh_layout(&self) {
        let disk_opt = {
            let s = self.state.borrow();
            s.selected_disk.and_then(|i| s.disks.get(i).cloned())
        };

        let disk = match disk_opt {
            Some(d) => d,
            None => {
                self.preview.set_disk(None);
                self.continue_btn.set_sensitive(false);
                self.header.show_commands_btn().set_sensitive(false);
                self.warning.set_visible(false);
                self.mode_pill.set_text("");
                return;
            }
        };

        // Decide mode automatically. Only one mode is offered per disk:
        //  - An existing NIMBLEX partition (left by a prior install, or the
        //    medium we booted from) → install onto it in place by default,
        //    with formatting offered as an opt-in checkbox. Highest priority
        //    so an existing Nimblex install is reused rather than wiped — on
        //    internal disks *and* removable media (e.g. refreshing a USB
        //    install, or reinstalling onto the live stick).
        //  - Has Windows + not removable + Windows is BitLocker-encrypted
        //    + enough free space → install into the free gap.
        //  - Has Windows + not removable → install alongside Windows.
        //  - Removable, or no Windows → wipe and install Nimblex.
        let mode = if disk.existing_nimblex_partition().is_some() {
            InstallMode::ReuseNimblex
        } else if disk.has_windows() && !disk.removable {
            let win_is_bitlocker = disk
                .primary_windows_partition()
                .map(|p| p.fs.eq_ignore_ascii_case("bitlocker"))
                .unwrap_or(false);
            let largest_gap = disk.largest_free_gap().map(|g| g.size).unwrap_or(0);
            if win_is_bitlocker && Bytes(largest_gap) >= min_install() {
                InstallMode::FreeSpace
            } else {
                InstallMode::AlongsideWindows
            }
        } else {
            InstallMode::EraseWholeDisk
        };
        self.state.borrow_mut().install_mode = Some(mode);

        self.preview.set_disk(Some(disk.clone()));

        // ── Live-device safety gating ────────────────────────────────────
        // The device the running session booted from cannot be reformatted or
        // repartitioned while it is feeding the live overlay (unless we are
        // running from RAM, in which case nothing is at risk). Reinstalling
        // *in place* onto the live partition IS allowed via the crash-safe
        // copy path and is handled inside the ReuseNimblex branch below.
        let live = self.state.borrow().live_media.clone();
        self.state.borrow_mut().reuse_inplace_live = false;
        if live.disk_is_unsafe_to_modify(&disk.path) && !matches!(mode, InstallMode::ReuseNimblex) {
            self.format_existing_check.set_visible(false);
            self.state.borrow_mut().reuse_format = false;
            self.preview
                .set_planned(build_keep_segments(&disk), disk.size.0);
            self.warn(LIVE_DISK_BLOCK_MSG);
            self.continue_btn.set_sensitive(false);
            self.header.show_commands_btn().set_sensitive(false);
            return;
        }

        // The "Format the partition first" checkbox only applies to the
        // reuse-existing mode; hide it (and clear its choice) everywhere else.
        // The ReuseNimblex branch below re-shows it.
        if !matches!(mode, InstallMode::ReuseNimblex) {
            self.format_existing_check.set_visible(false);
            self.state.borrow_mut().reuse_format = false;
        }

        match mode {
            InstallMode::EraseWholeDisk => {
                self.mode_pill.set_markup(&format!(
                    "<b>Wipe and install</b>  ·  {} will be erased and NimbleX installed onto it.",
                    disk.path.display()
                ));
                self.mode_pill.remove_css_class("mode-pill-alongside");
                self.mode_pill.add_css_class("mode-pill-erase");

                self.warning.set_visible(false);

                // Build planned strip: every existing partition is DELETE,
                // followed by NEW_ESP and NEW_NIMBLEX.
                let segs = build_erase_segments(&disk);
                self.preview.set_planned(segs, disk.size.0);

                self.continue_btn.set_sensitive(true);
                self.header.show_commands_btn().set_sensitive(true);
            }
            InstallMode::AlongsideWindows => {
                self.mode_pill.set_markup(&format!(
                    "<b>Install alongside Windows</b>  ·  Windows is preserved; NimbleX takes the freed space."
                ));
                self.mode_pill.remove_css_class("mode-pill-erase");
                self.mode_pill.add_css_class("mode-pill-alongside");

                let win = match disk.primary_windows_partition().cloned() {
                    Some(p) => p,
                    None => {
                        self.preview
                            .set_planned(build_keep_segments(&disk), disk.size.0);
                        self.warn("This disk does not have a Windows partition.");
                        self.continue_btn.set_sensitive(false);
                        self.header.show_commands_btn().set_sensitive(false);
                        return;
                    }
                };
                if win.fs.eq_ignore_ascii_case("bitlocker") {
                    self.warn(
                        "Windows on this disk is BitLocker-encrypted. \
                         Boot into Windows, open Settings → Privacy & security → \
                         Device encryption (or Manage BitLocker) and turn it OFF \
                         to decrypt the drive, then reopen the installer.",
                    );
                    self.preview
                        .set_planned(build_keep_segments(&disk), disk.size.0);
                    self.continue_btn.set_sensitive(false);
                    self.header.show_commands_btn().set_sensitive(false);
                    return;
                }
                let used = match win.used {
                    Some(u) => u,
                    None => {
                        self.warn(
                            "Could not read Windows usage. Reopen the installer with Windows fully shut down (no Fast Startup).",
                        );
                        self.preview
                            .set_planned(build_keep_segments(&disk), disk.size.0);
                        self.continue_btn.set_sensitive(false);
                        self.header.show_commands_btn().set_sensitive(false);
                        return;
                    }
                };
                let free = win.size - used;
                if free < WINDOWS_MIN_FREE_BEFORE_SHRINK {
                    self.warn(&format!(
                        "Windows has only {} free. Free at least {} inside Windows (Recycle Bin, Downloads, %TEMP%, hibernation file) and reopen.",
                        free, WINDOWS_MIN_FREE_BEFORE_SHRINK
                    ));
                    self.preview
                        .set_planned(build_keep_segments(&disk), disk.size.0);
                    self.continue_btn.set_sensitive(false);
                    self.header.show_commands_btn().set_sensitive(false);
                    return;
                }
                self.warning.set_visible(false);

                let min = min_reclaim();
                let win_kept_floor = min_windows_residual_after_shrink(used);
                let max = if win_kept_floor >= win.size {
                    min
                } else {
                    (win.size - win_kept_floor).max(min)
                };
                let default = min + Bytes((max.0 - min.0) / 2);

                self.state.borrow_mut().requested_reclaim_bytes = Some(default.0);

                self.continue_btn.set_sensitive(true);
                self.header.show_commands_btn().set_sensitive(true);
                self.refresh_slider_labels();
            }
            InstallMode::FreeSpace => {
                self.mode_pill.set_markup(
                    "<b>Install in free space</b>  ·  Windows is BitLocker-encrypted; \
                     NimbleX will be added to the existing unallocated space without \
                     touching Windows.",
                );
                self.mode_pill.remove_css_class("mode-pill-erase");
                self.mode_pill.add_css_class("mode-pill-alongside");
                self.warning.set_visible(false);

                let gap = match disk.largest_free_gap() {
                    Some(g) => g,
                    None => {
                        self.warn("No unallocated space found on this disk.");
                        self.preview
                            .set_planned(build_keep_segments(&disk), disk.size.0);
                        self.continue_btn.set_sensitive(false);
                        self.header.show_commands_btn().set_sensitive(false);
                        return;
                    }
                };
                let segs = build_freespace_segments(&disk, gap.start, gap.size);
                self.preview.set_planned(segs, disk.size.0);

                // Plan_for ignores reclaim_bytes for FreeSpace mode.
                self.state.borrow_mut().requested_reclaim_bytes = Some(gap.size);

                self.continue_btn.set_sensitive(true);
                self.header.show_commands_btn().set_sensitive(true);
            }
            InstallMode::ReuseNimblex => {
                self.mode_pill.set_markup(
                    "<b>Reuse existing NimbleX partition</b>  ·  A previous NimbleX \
                     partition was found; NimbleX will be installed onto it. \
                     Windows and all other partitions are left untouched.",
                );
                self.mode_pill.remove_css_class("mode-pill-erase");
                self.mode_pill.add_css_class("mode-pill-alongside");
                self.warning.set_visible(false);

                let part = match disk.existing_nimblex_partition().cloned() {
                    Some(p) => p,
                    None => {
                        self.warn("No existing NimbleX partition found on this disk.");
                        self.preview
                            .set_planned(build_keep_segments(&disk), disk.size.0);
                        self.continue_btn.set_sensitive(false);
                        self.header.show_commands_btn().set_sensitive(false);
                        return;
                    }
                };
                let segs = build_reuse_segments(&disk, &part);
                self.preview.set_planned(segs, disk.size.0);
                self.state.borrow_mut().requested_reclaim_bytes = None;

                // Is this the very partition we booted from (and not running
                // from RAM)? If so, the install must use the crash-safe
                // in-place strategy and formatting is impossible.
                let inplace_live = live.partition_needs_inplace_live(&part.path);
                self.state.borrow_mut().reuse_inplace_live = inplace_live;

                if inplace_live {
                    // Reinstalling onto the running partition: allowed, but
                    // formatting it would wipe the system we're executing
                    // from, so the format option is disabled here.
                    self.format_existing_check.set_active(false);
                    self.format_existing_check.set_sensitive(false);
                    self.format_existing_check.set_visible(true);
                    self.state.borrow_mut().reuse_format = false;
                    self.warn(LIVE_INPLACE_INFO_MSG);
                } else {
                    // Default: install in place (no format). Offer the
                    // optional reformat as a checkbox, preserving the user's
                    // prior choice.
                    self.format_existing_check.set_sensitive(true);
                    self.format_existing_check.set_visible(true);
                    self.state.borrow_mut().reuse_format = self.format_existing_check.is_active();
                }

                self.continue_btn.set_sensitive(true);
                self.header.show_commands_btn().set_sensitive(true);
            }
        }
    }

    fn refresh_slider_labels(&self) {
        let bytes = self.state.borrow().requested_reclaim_bytes.unwrap_or(0);
        let snapshot = {
            let s = self.state.borrow();
            s.selected_disk
                .and_then(|i| s.disks.get(i))
                .map(|d| (d.clone(), d.primary_windows_partition().cloned()))
        };
        let (disk, win_opt) = match snapshot {
            Some(v) => v,
            None => return,
        };
        let win = match win_opt {
            Some(p) => p,
            None => return,
        };
        let kept = win.size.0.saturating_sub(bytes);

        // Build planned segments for Alongside mode.
        let segs = build_alongside_segments(&disk, &win, kept, bytes);
        self.preview.set_planned(segs, disk.size.0);
    }

    fn warn(&self, msg: &str) {
        self.warning.set_text(msg);
        self.warning.set_visible(true);
    }

    fn build_plan_into_state(&self) -> bool {
        let s = self.state.borrow();
        let disk = match s.selected_disk.and_then(|i| s.disks.get(i)).cloned() {
            Some(d) => d,
            None => return false,
        };
        let mode = match s.install_mode {
            Some(m) => m,
            None => return false,
        };
        let reclaim = s.requested_reclaim_bytes;
        let bootloader = s.bootloader;
        let reuse_format = s.reuse_format;
        let reuse_inplace_live = s.reuse_inplace_live;
        drop(s);
        match InstallPlanner::plan_for(
            &disk,
            mode,
            reclaim,
            bootloader,
            reuse_format,
            reuse_inplace_live,
        ) {
            Ok(plan) => {
                self.state.borrow_mut().plan = Some(plan);
                true
            }
            Err(e) => {
                self.warn(&format!("Cannot build plan: {}", e));
                false
            }
        }
    }

    fn update_overlay_summary(&self) {
        let s = self.state.borrow();
        if let Some(plan) = &s.plan {
            self.overlay_summary.set_text(&plan.summary_one_line());
        }
    }

    fn show_commands_dialog(&self) {
        let plan_opt = {
            let s = self.state.borrow();
            let disk = s.selected_disk.and_then(|i| s.disks.get(i)).cloned();
            let mode = s.install_mode;
            let reclaim = s.requested_reclaim_bytes;
            let bootloader = s.bootloader;
            let reuse_format = s.reuse_format;
            let reuse_inplace_live = s.reuse_inplace_live;
            match (disk, mode) {
                (Some(d), Some(m)) => InstallPlanner::plan_for(
                    &d,
                    m,
                    reclaim,
                    bootloader,
                    reuse_format,
                    reuse_inplace_live,
                )
                .ok(),
                _ => None,
            }
        };
        let transcript = match plan_opt {
            Some(p) => p.shell_transcript(),
            None => "(select a destination first)".into(),
        };
        let parent = self.root.root().and_downcast::<Window>();
        let dialog = Window::builder()
            .title("Commands that will run")
            .modal(true)
            .default_width(720)
            .default_height(520)
            .build();
        if let Some(parent) = &parent {
            dialog.set_transient_for(Some(parent));
        }
        let body = GtkBox::new(Orientation::Vertical, 0);
        body.add_css_class("log-pane");
        let buf = TextBuffer::new(None);
        buf.set_text(&transcript);
        let view = TextView::with_buffer(&buf);
        view.set_editable(false);
        view.set_monospace(true);
        view.set_wrap_mode(WrapMode::WordChar);
        let scroll = ScrolledWindow::builder()
            .hscrollbar_policy(PolicyType::Automatic)
            .vscrollbar_policy(PolicyType::Automatic)
            .vexpand(true)
            .hexpand(true)
            .build();
        scroll.set_child(Some(&view));
        body.append(&scroll);

        let footer = GtkBox::new(Orientation::Horizontal, 8);
        footer.add_css_class("app-footer");
        let spacer = GtkBox::new(Orientation::Horizontal, 0);
        spacer.set_hexpand(true);
        footer.append(&spacer);
        let close_btn = Button::with_label("Close");
        close_btn.add_css_class("btn-primary");
        footer.append(&close_btn);
        let dlg_close = dialog.clone();
        close_btn.connect_clicked(move |_| dlg_close.close());

        let outer = GtkBox::new(Orientation::Vertical, 0);
        outer.append(&body);
        outer.append(&footer);
        dialog.set_child(Some(&outer));
        dialog.present();
    }
}

/// Build `Vec<PlannedSegment>` for the "After" strip in **Erase** mode.
/// All existing partitions are tagged DELETE; new ESP + Nimblex are
/// appended. Existing segments are positioned on top of the new layout to
/// make the deletion visually obvious side-by-side.
///
/// Visual approach: we render existing partitions first, then OVERLAY the
/// new layout on top. Simpler: just emit the new layout (since the disk
/// will literally be wiped) and let the "Now" strip show what was there.
fn build_erase_segments(disk: &Disk) -> Vec<PlannedSegment> {
    let mut segs = Vec::new();
    // 1 MiB GPT alignment gap (free).
    if GPT_ALIGN_BYTES > 0 {
        segs.push(PlannedSegment {
            start: 0,
            size: GPT_ALIGN_BYTES,
            role: PartitionRole::Other,
            label: String::new(),
            action: SegmentAction::Free,
        });
    }
    let esp_start = GPT_ALIGN_BYTES;
    segs.push(PlannedSegment {
        start: esp_start,
        size: ESP_SIZE_BYTES,
        role: PartitionRole::EfiSystem,
        label: "EFI".into(),
        action: SegmentAction::NewEsp,
    });
    let root_start = esp_start + ESP_SIZE_BYTES;
    let root_size = disk.size.0.saturating_sub(root_start);
    segs.push(PlannedSegment {
        start: root_start,
        size: root_size,
        role: PartitionRole::Linux,
        label: "NimbleX".into(),
        action: SegmentAction::NewNimblex,
    });
    segs
}

/// Build `Vec<PlannedSegment>` for the "After" strip in **FreeSpace** mode.
/// Every existing partition is KEPT unchanged; a single new Nimblex
/// partition is placed into the chosen unallocated gap (`gap_start` /
/// `gap_size`). The gap may sit *between* existing partitions, so the
/// Nimblex segment is positioned at `gap_start`, not merely appended.
fn build_freespace_segments(disk: &Disk, gap_start: u64, gap_size: u64) -> Vec<PlannedSegment> {
    let mut segs = Vec::new();
    for p in &disk.partitions {
        segs.push(PlannedSegment {
            start: p.start.0,
            size: p.size.0,
            role: p.role,
            label: if p.label.is_empty() {
                p.role.short_label().to_string()
            } else {
                p.label.clone()
            },
            action: SegmentAction::Keep,
        });
    }
    segs.push(PlannedSegment {
        start: gap_start,
        size: gap_size,
        role: PartitionRole::Linux,
        label: "NimbleX".into(),
        action: SegmentAction::NewNimblex,
    });
    // Render in physical disk order so the strip lays out left-to-right.
    segs.sort_by_key(|s| s.start);
    segs
}

/// Build `Vec<PlannedSegment>` for the "After" strip in **Alongside Windows**
/// mode. Layout:
///   - all partitions before Windows: KEEP (unchanged)
///   - Windows partition: shrunk → RESIZE
///   - new Nimblex partition: NEW · NIMBLEX
///   - all partitions after Windows: KEEP (unchanged) — shifted by the
///     amount Windows shrank ONLY if the original disk had no gap; for
///     simplicity we keep them at their original `start` positions, which
///     is correct because parted will not move them.
fn build_alongside_segments(
    disk: &Disk,
    win: &installer_core::Partition,
    win_kept_bytes: u64,
    nimblex_bytes: u64,
) -> Vec<PlannedSegment> {
    let mut segs = Vec::new();
    let mut sorted: Vec<&installer_core::Partition> = disk.partitions.iter().collect();
    sorted.sort_by_key(|p| p.start.0);

    for p in sorted {
        if p.number == win.number {
            // Windows segment, shrunk.
            segs.push(PlannedSegment {
                start: p.start.0,
                size: win_kept_bytes,
                role: p.role,
                label: if p.label.is_empty() {
                    "Windows".into()
                } else {
                    p.label.clone()
                },
                action: SegmentAction::Resize,
            });
            // Nimblex appended right after Windows in the freed space.
            segs.push(PlannedSegment {
                start: p.start.0 + win_kept_bytes,
                size: nimblex_bytes,
                role: PartitionRole::Linux,
                label: "NimbleX".into(),
                action: SegmentAction::NewNimblex,
            });
        } else {
            segs.push(PlannedSegment {
                start: p.start.0,
                size: p.size.0,
                role: p.role,
                label: if p.label.is_empty() {
                    p.role.short_label().to_string()
                } else {
                    p.label.clone()
                },
                action: SegmentAction::Keep,
            });
        }
    }
    segs
}

/// Build `Vec<PlannedSegment>` that simply renders every existing partition
/// as KEPT, in physical disk order. Used in blocked states (e.g. BitLocker
/// with no free space) so the layout strip still shows the disk's real
/// partitions instead of an empty box.
fn build_keep_segments(disk: &Disk) -> Vec<PlannedSegment> {
    let mut segs: Vec<PlannedSegment> = disk
        .partitions
        .iter()
        .map(|p| PlannedSegment {
            start: p.start.0,
            size: p.size.0,
            role: p.role,
            label: if p.label.is_empty() {
                p.role.short_label().to_string()
            } else {
                p.label.clone()
            },
            action: SegmentAction::Keep,
        })
        .collect();
    segs.sort_by_key(|s| s.start);
    segs
}

/// Build `Vec<PlannedSegment>` for **ReuseNimblex** mode. Every partition is
/// KEPT except the existing Nimblex partition, which is shown as NEW (it will
/// be reformatted in place).
fn build_reuse_segments(disk: &Disk, nimblex: &installer_core::Partition) -> Vec<PlannedSegment> {
    let mut segs: Vec<PlannedSegment> = disk
        .partitions
        .iter()
        .map(|p| {
            let is_nimblex = p.number == nimblex.number;
            PlannedSegment {
                start: p.start.0,
                size: p.size.0,
                role: if is_nimblex {
                    PartitionRole::Linux
                } else {
                    p.role
                },
                label: if is_nimblex {
                    "NimbleX".into()
                } else if p.label.is_empty() {
                    p.role.short_label().to_string()
                } else {
                    p.label.clone()
                },
                action: if is_nimblex {
                    SegmentAction::NewNimblex
                } else {
                    SegmentAction::Keep
                },
            }
        })
        .collect();
    segs.sort_by_key(|s| s.start);
    segs
}
