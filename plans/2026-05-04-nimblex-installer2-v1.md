# Nimblex Graphical Installer (3-Screen, Visual, Safe-by-Default)

## Objective

Build a beautiful, intuitive, low-risk graphical installer for the Nimblex Linux distribution that:

- Runs entirely on libraries available in `01-Core64.lzm` + `02-Xorg64.lzm` (no extra modules required).
- Limits the user journey to **at most 3 screens** with minimal text and rich visual cues.
- Supports two installation targets:
  1. **USB stick** — full format + Nimblex install (destructive but obvious).
  2. **Internal disk alongside Windows** — non-destructive, with NTFS shrink to reclaim free space, never touching ESP/MSR/recovery partitions or going below a safe Windows residual size.
- Auto-computes minimum required space and uses it as the starting point of any resize.
- Requires a single, unambiguous "I understand" confirmation before any destructive action runs.
- Exposes a "Show commands" button revealing the exact shell commands that *will* run, **before** they run.
- Provides a graphical partition/disk picker modeled on MiniTool / EaseUS-style segmented bars (per the supplied screenshots).

## Initial Assessment

### Project Structure Summary

- Working dir: `/mnt/live/memory/data/Work/nimblex-installer2/` — currently empty, greenfield.
- Reference implementation already in distro: `01-Core64.lzm/usr/bin/nimblex-install:1-359` (legacy 2007 bash + `dialog` TUI by Bogdan Radulescu) and helpers `nimblex-update`, `create_nimblex.data` in the same directory. These are the canonical source-of-truth for partition/FS/bootloader sequencing and persistence-file creation, and must be mined before designing flows. This hasn't been used for almost 20 years, so only use it as a very relaxed inspiration at most.
- Bundle layout discovered in `/proc/mounts:1-25` (loop1=01-Core, loop2=02-Xorg).

### Toolchain Available in 01-Core + 02-Xorg (verified via `var/lib/pkgtools/packages/`)

- **GUI toolkits available**: GTK3 (`gtk+3-3.24.52`), GTK4 (`gtk4-4.22.3`), Gtkmm 2/3/4, Qt5 (`qt5-5.15.18`), PyQt5 (`PyQt5-5.15.11`), PyGObject (`pygobject3-3.56.2`), Cairo, Pango, librsvg, gdk-pixbuf.
- **Scripting runtimes**: Python 3.12, Perl 5.42, Bash 5.3, Dash.
- **Partitioning**: `parted-3.7`, `gptfdisk` (sgdisk/gdisk), `util-linux` (sfdisk/lsblk/blkid/wipefs), `ntfs-3g-2022.10.3` (provides `ntfsresize`, `mkntfs`, `ntfsfix`, `ntfsclone`), `e2fsprogs`, `dosfstools`, `exfatprogs`, `xfsprogs`, `btrfs-progs`, `f2fs-tools`, `bcachefs-tools`, `lvm2`, `cryptsetup`, `mdadm`, `libblockdev-3.4.0`.
- **Bootloaders**: `syslinux-4.07` (BIOS), `elilo-3.16` + `efibootmgr-18` (UEFI), `lilo-24.2`. **GRUB is NOT packaged** — must not be referenced.
- **Disk introspection**: `lsblk`, `blkid`, `udisks2-2.11.1`, `smartmontools`, `dmidecode`, `nvme-cli`.
- **Notably absent**: `yad`, `zenity`, `gtkdialog`, `Xdialog`, `kdialog`, `tcl/tk`, `fltk`, `wxWidgets`, `gparted`, `grub`. (If any of this would be required, we should discuss with the user about adding them.)

### Prioritized Risks (highest first)

1. **Data loss on Windows machines via incorrect NTFS resize** — top priority; mitigated by mandatory `ntfsresize -i` dry-run + minimum-free-space floor + skipping ESP/MSR/Recovery partitions.
2. **Bootloader bricking of dual-boot systems** — second priority; mitigated by never overwriting Windows Boot Manager entries; on UEFI register a new `efibootmgr` entry; on BIOS only install syslinux to the Nimblex partition (chainload), never to MBR by default.
3. **Wrong device selected (USB vs internal)** — mitigated by the visual disk picker showing removable/fixed badges, sizes, vendor strings, and color-coded danger states.
4. **No GRUB available** but legacy script references it — must use syslinux/elilo path exclusively.
5. **Locale/X11 startup edge cases** in live mode — mitigated by graceful fallback to a TUI summary if `$DISPLAY` is unavailable.
6. **Partition table type mismatch** (MBR limit of 4 primaries, GPT vs MBR boot) — mitigated by detecting table type up-front and adjusting flow.

### Toolkit Decision (assumption, documented)

**Primary: Rust + GTK4 via the `gtk4-rs` crate**, statically-cross-compiled outside the live system, linked dynamically only against libraries shipped in 02-Xorg (gtk4, gdk-pixbuf2, cairo, pango, glib, librsvg). Rationale: the user explicitly suggested Rust; GTK4 is present in 02-Xorg; Cairo `DrawingArea` lets us render the segmented-bar partition view from the screenshots pixel-perfectly; a single statically-linked-where-possible binary keeps deployment trivial. Privileged disk operations are delegated to a small `polkit`-invoked helper (polkit is in 01-Core).

**Alternative kept in scope**: Python 3.12 + PyQt5 (both in modules already, zero compile step). Considered if Rust toolchain availability becomes a blocker.

## Implementation Plan

### Phase 0 — Foundations & Reference Mining

- [ ] Task 0.1. Read and document the exact partitioning, mkfs, syslinux, and persistence-file sequences in `01-Core64.lzm/usr/bin/nimblex-install:1-359`, `nimblex-update`, and `create_nimblex.data`. Capture every shell command they emit; these become the canonical command templates the installer will run.
- [ ] Task 0.2. Inventory the Nimblex live ISO/USB layout (where `01-Core64.lzm`, `02-Xorg64.lzm`, kernel, initrd, syslinux config live) so the installer knows the **source** files to copy. Produce a manifest spec for "what constitutes a Nimblex install on disk".
- [ ] Task 0.3. Define a fixed minimum-space budget: sum of bundle sizes + kernel + initrd + safety margin + optional `nimblex.data` persistence file size (user-selectable later, default 4 GiB). Output a single `MIN_INSTALL_BYTES` constant function.
- [ ] Task 0.4. Decide on repo layout: `crates/installer-gui/` (Rust+GTK4), `crates/installer-core/` (disk model, command planning, no UI deps), `crates/installer-helper/` (privileged executor invoked via polkit), `assets/` (SVG icons, CSS), `polkit/` (`.policy` file), `packaging/` (build script that produces a single drop-in artifact for the live ISO).

### Phase 1 — Core Domain Model (no UI)

- [ ] Task 1.1. Implement a read-only `DiskScanner` that calls `lsblk -O -J`, `blkid`, and `parted -m -s <dev> print` to build an in-memory model: `Disk { path, size, removable, vendor, model, table_type, partitions: [Partition { path, fs, label, used, free, flags, role }] }`. Classify each partition's `role` as one of: `WindowsSystem(C:)`, `WindowsData`, `EFISystem`, `MicrosoftReserved`, `WindowsRecovery`, `Linux`, `Swap`, `Other`, `Unallocated`. ESP/MSR/Recovery are flagged "do-not-touch".
- [ ] Task 1.2. Implement a `ResizePlanner` that, given a target Windows partition, runs `ntfsresize --info --force` to obtain the true minimum size, then computes a *safe* shrink target = `max(ntfsresize_min, current_used * 1.25, 40 GiB)` — never below a hardcoded Windows-residual floor. Returns the largest possible free region that can be reclaimed without violating the floor.
- [ ] Task 1.3. Implement an `InstallPlanner` that produces a typed `Plan` — an ordered list of `Step`s, each with: a human label, a category (Resize / Partition / Format / Copy / Bootloader / Persistence), the **exact shell command(s)** as `Vec<String>`, and a `destructive: bool` flag. Two planning modes:
  - **USB mode**: full wipe → GPT or MBR table → single FAT32 (boot) + ext4 (root) → install syslinux → copy modules → optionally create `nimblex.data`.
  - **Alongside-Windows mode**: `ntfsresize` shrink → `parted resizepart` → create new ext4 partition in reclaimed space → copy modules → install bootloader (UEFI: `efibootmgr` entry + ESP files; BIOS: syslinux/extlinux on the Nimblex partition, chainload from Windows boot manager NOT touched).
- [ ] Task 1.4. Implement a `PlanFormatter` that turns a `Plan` into both (a) a developer/expert-readable shell transcript (for the "Show commands" dialog) and (b) a short pictographic step list for the confirm screen.
- [ ] Task 1.5. Build a `DryRunValidator` that, before showing the confirm screen, executes only non-destructive probes: `ntfsresize --info`, `parted print`, `df`, free-space checks against `MIN_INSTALL_BYTES`. Any failure short-circuits with a graphical error and never reaches confirmation.

### Phase 2 — Privileged Helper & IPC

- [ ] Task 2.1. Implement `installer-helper` as a small CLI binary that accepts a serialized `Plan` on stdin (JSON), validates it against an allowlist of permitted argv shapes, and executes steps sequentially while streaming structured progress events (JSON lines) on stdout.
- [ ] Task 2.2. Author a polkit `.policy` file (`org.nimblex.installer.policy`) declaring an `org.nimblex.installer.run` action requiring `auth_admin_keep`. Helper is invoked via `pkexec` from the GUI.
- [ ] Task 2.3. Define the GUI↔helper protocol: line-delimited JSON events `{kind: "step_start"|"stdout"|"stderr"|"step_done"|"error"|"complete", ...}`. GUI parses these into the progress UI.
- [ ] Task 2.4. Add a `--dry-run` flag to the helper that prints commands without executing — used by the "Show commands" button so the displayed text is guaranteed to match what would run.

### Phase 3 — GUI: Three Screens

#### Screen 1 — Welcome / Target Choice

- [ ] Task 3.1. Build a single full-window screen with two large, illustrated cards: **"Install on USB stick"** (USB icon, subtext: "The whole stick will be erased") and **"Install alongside Windows"** (laptop+Windows icon, subtext: "Windows will be kept; we'll only use free space"). Clicking a card advances. Add a small footer with version, language switcher (placeholder), and a "Quit" button.
- [ ] Task 3.2. If no suitable target exists for a card (e.g. no removable drive, or no Windows partition detected), gray out that card with a one-line tooltip explaining why.

#### Screen 2 — Visual Disk & Partition Picker

- [ ] Task 3.3. Implement a custom GTK4 `DrawingArea` widget `DiskStripView` that renders each disk as a horizontal bar segmented by partition, with proportional widths, color-coded by role (NTFS=blue, EFI=light-blue, Recovery=gray, Linux=green, Free=hatched, Selected=highlighted), each segment showing label + size, exactly mirroring the supplied MiniTool/EaseUS screenshots. Disks are stacked vertically with a left-side header showing `Disk N · vendor model · total size · GPT/MBR · Removable badge if applicable`.
- [ ] Task 3.4. Selection model: clicking a partition selects it; the planner then proposes (a) for USB mode → the whole disk highlights red with "Will be erased"; (b) for Alongside-Windows mode → an interactive resize handle appears on the selected NTFS partition, with a draggable splitter showing live `Windows kept | Nimblex new` split. The splitter cannot be dragged below the safe Windows floor (visually clamped) nor below `MIN_INSTALL_BYTES` on the Nimblex side.
- [ ] Task 3.5. Always-visible bottom bar shows: required space (✓/✗), reclaimable space, target device path, and a **Continue** button (disabled until selection is valid). Add a secondary **"Show commands"** button that opens a modal with the dry-run shell transcript from `PlanFormatter`.
- [ ] Task 3.6. Add safety overlays: any do-not-touch partition (ESP/MSR/Recovery) is rendered with a small lock icon and is non-selectable; hovering shows "Protected — required by Windows".

#### Screen 3 — Confirm & Execute

- [ ] Task 3.7. Top half: a pictographic summary of the plan (3–6 large icons in sequence: shrink → create → format → copy → bootloader → done) with one short caption each. No paragraphs of text.
- [ ] Task 3.8. A single mandatory checkbox: **"I understand my disk will be modified as shown above."** The big red **"Install now"** button stays disabled until checked. A secondary **"Show commands"** button reveals the exact transcript again.
- [ ] Task 3.9. After confirmation: in-place transition to a progress view (still Screen 3) showing the same pictographic icons now animated/checked as steps complete, with a thin overall progress bar and an expandable "Details" disclosure that streams helper stdout/stderr live.
- [ ] Task 3.10. End states: success view with "Reboot now" / "Close" buttons and a one-line summary; failure view with the failed step highlighted, the captured stderr, and a "Copy diagnostics" button. Never auto-reboot.

### Phase 4 — Visual Design System

- [ ] Task 4.1. Author a single `style.css` (GTK4 CSS) defining a calm palette (neutral background, blue=safe, green=Linux, amber=warn, red=destructive), large typography hierarchy, and 16px spacing grid. Avoid more than 2 font sizes per screen.
- [ ] Task 4.2. Commission/produce SVG icons (rendered via librsvg from 02-Xorg): usb-stick, windows-laptop, shrink, partition, format, copy, boot, lock, check, warning. Ship in `assets/icons/`.
- [ ] Task 4.3. Ensure every screen passes a "5-second test": a non-technical user can describe what will happen by looking at icons + ≤ 8 words of caption.

### Phase 5 — Packaging & Integration with the Live System

- [ ] Task 5.1. Build pipeline produces: the GUI binary, the helper binary, the polkit policy, the assets, a `.desktop` launcher (`nimblex-installer.desktop`), and an `install.sh` that places them at `/usr/bin/nimblex-installer-gui`, `/usr/libexec/nimblex-installer-helper`, `/usr/share/polkit-1/actions/org.nimblex.installer.policy`, `/usr/share/applications/`, `/usr/share/nimblex-installer/`.
- [ ] Task 5.2. Produce a Slackware-format package (`.txz`) so it can be dropped into a Nimblex bundle (`03-Installer64.lzm` or merged into 02-Xorg) using existing `pkgtools-15.1`.
- [ ] Task 5.3. Add a desktop autostart entry that launches the installer on the live session (can be disabled by the user).
- [ ] Task 5.4. Verify the binary's `ldd` output references **only** sonames present in 01-Core + 02-Xorg (gate this in CI with a script that compares `ldd` output against an allowlist generated from the two bundles' `var/lib/pkgtools/packages/`).

### Phase 6 — Testing

- [ ] Task 6.1. Unit-test `ResizePlanner` with synthetic `ntfsresize --info` outputs covering: large free Windows, near-full Windows, encrypted (BitLocker — must abort gracefully), MBR with 4 primaries already used, GPT with recovery partitions.
- [ ] Task 6.2. Integration tests using loopback-mounted disk images (`qemu-img` + `losetup` — both available; `qemu` may need to be optional dev-only) simulating: empty USB, Windows-only GPT, Windows + Linux dual-boot, MBR Windows 10, GPT Windows 11.
- [ ] Task 6.3. Manual QA checklist on real hardware: BIOS-only laptop, UEFI laptop with Secure Boot off, USB 3.0 stick, USB 2.0 stick, NVMe + SATA mix.
- [ ] Task 6.4. Snapshot-test the "Show commands" output for each canonical scenario so that any change to the planner is reviewed deliberately.

## Verification Criteria

- The shipped binary's `ldd` references only sonames present in `01-Core64.lzm` and `02-Xorg64.lzm` (verifiable by an automated check in Task 5.4).
- The user can complete a USB install in **at most 3 screens** and ≤ 5 clicks (Welcome → Pick Disk → Confirm → Install).
- Installing alongside Windows on a test image preserves Windows bootability (Windows boots normally after install) in 100% of QA scenarios.
- `ntfsresize --info` is always run before any actual resize, and the proposed shrink target is always ≥ its reported minimum + a configurable safety margin (default 25% of currently used).
- The "Show commands" dialog text is byte-identical to what the helper subsequently executes in dry-run mode (snapshot test).
- ESP, MSR, and Windows Recovery partitions are never selectable and never appear in any generated command's argv (asserted by planner unit tests).
- The "Install now" button is provably unreachable without the explicit "I understand" checkbox being toggled (UI test).
- Any planner or probe failure short-circuits before the confirm screen and surfaces a graphical error — the helper is never invoked with a destructive plan unless validation passed.
- Total on-disk footprint of the installer artifact is small enough to fit in an existing bundle without rebuilding 01-Core or 02-Xorg (target ≤ 8 MiB compressed).

## Potential Risks and Mitigations

1. **NTFS shrink corrupts Windows filesystem on edge-case volumes (e.g. heavily fragmented, hibernated, BitLocker-encrypted, or with unmovable system files).**
   Mitigation: detect hibernation file (`hiberfil.sys`) and BitLocker (via `blkid`/`cryptsetup status`) and abort with a clear graphical message instructing the user to disable Fast Startup / suspend BitLocker; always run `ntfsresize --info` first; refuse to shrink if `ntfsresize` reports the volume is dirty (run `ntfsfix` only with explicit user opt-in via a secondary confirmation).

2. **Wrong device selected — user's data drive instead of USB.**
   Mitigation: the visual picker labels removable vs fixed with distinct icons and a colored badge; in USB mode, fixed disks are dimmed and require an explicit "Show internal disks" toggle that is off by default.

3. **GRUB referenced by legacy script but not packaged in 01-Core.**
   Mitigation: planner has no GRUB code path; uses syslinux (BIOS) and `efibootmgr`+EFI stub or `elilo` (UEFI) exclusively. Document this divergence from `nimblex-install:222-255` in code comments.

4. **Polkit/pkexec misconfiguration leaves the installer unable to run privileged steps.**
   Mitigation: include a fallback that, if pkexec fails, instructs the user to relaunch the installer from a root terminal; ship and verify the `.policy` file in Task 5.1.

5. **GTK4 ABI drift between dev machine and live system.**
   Mitigation: build against the exact GTK4 sonames found in `02-Xorg64.lzm` (use a chrooted build, or extract the bundle's `usr/include` + `usr/lib` and pin via `PKG_CONFIG_PATH`); enforce in Task 5.4's CI gate.

6. **MBR 4-primary-partition limit blocks alongside-Windows install.**
   Mitigation: planner detects MBR + 4 primaries up-front and offers two options: (a) convert one primary to extended (out-of-scope for v1, gracefully refuse), or (b) abort with a clear "Not enough partition slots" message. Document v1 limitation.

7. **User pulls the USB or loses power mid-install.**
   Mitigation: order steps so that the Windows-side `ntfsresize` + `parted resizepart` complete and are flushed (`sync`) before any new partition is created; emit a single highly visible "Do not power off or remove media" banner during execution; on resume, the installer detects partial state via `blkid`/labels and offers a Resume/Cleanup option (v1.1, document as deferred).

## Alternative Approaches

1. **Python 3.12 + PyQt5** (no compile step) — dramatically faster initial development; ships as plain `.py` files; widget set in 02-Xorg confirmed. Trade-off: slightly heavier runtime memory, less polished native feel than GTK4 with custom Cairo, harder to ship as a single binary, and the segmented partition bar requires `QGraphicsView` custom items vs. the cleaner GTK4 `DrawingArea`. Keep as **fallback** if Rust+GTK4 build infrastructure proves problematic.

2. **Bash + `dialog` (TUI only)** — already partially implemented in `01-Core64.lzm/usr/bin/nimblex-install:1-359`. Trade-off: violates the "must look good and be graphical" requirement, but is 100% guaranteed to run on every system regardless of X11 status. Keep only as an emergency fallback when `$DISPLAY` is unset.

3. **Rust + `slint` UI framework** — modern, beautiful, declarative. Trade-off: Slint is **not** in 02-Xorg; would require statically linking everything except `libX11`/`libwayland`/Mesa. Possible but adds binary size and audit surface; rejected in favor of GTK4 which is already shipped.

4. **C++ + Qt5/Gtkmm** — both toolkits' dev libraries are available (Gtkmm in 02-Xorg, Qt5 too). Trade-off: longer development time, more boilerplate, no clear advantage over Rust+GTK4 for a 3-screen app; rejected.

5. **Wrap an existing installer (Calamares, Refracta installer, Architect)** — none are packaged in 01-Core/02-Xorg; pulling them in would violate the dependency constraint. Rejected.
