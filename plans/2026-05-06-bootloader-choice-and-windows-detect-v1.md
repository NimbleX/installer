# Pluggable Bootloader (systemd-boot default, GRUB opt-in) + Windows Auto-Detection

## Objective

Wire both **systemd-boot** and **GRUB** into the installer as supported bootloader backends, with **systemd-boot as the default** and GRUB selectable via a `--bootloader=grub` CLI flag (also `--bootloader=systemd-boot` for explicit form, and `--bootloader=auto` which is the implicit default and falls back to GRUB on BIOS-only firmware). Both backends must produce a working USB boot (UEFI primary path) and a working internal-disk boot. **Both backends must auto-detect any Windows install on the target system and add a Windows menu entry**, including across separate ESPs (i.e. the USB-live case where Windows lives on internal NVMe).

This locks in the boot fixes proven on a real USB during the previous session (single-file standalone GRUB image with label-anchored `$root`; or `bootctl install` + `loader/entries/*.conf`), and extends them with bootloader choice and cross-ESP Windows discovery.

## Scope notes (assumptions made autonomously)

- The target machines are UEFI x86_64; BIOS-only is supported by GRUB only.
- Current systemd build now ships `usr/lib/systemd/boot/efi/{systemd-bootx64.efi, linuxx64.efi.stub, addonx64.efi.stub}` (verified during session).
- Live boot files come from `live_source_dirs()` (already used by both helper paths). For the in-development workflow the kernel is `vmlinuz64-6.19` and initramfs is `initramfs64-nx2` from `ISO-test64/boot/`; the helper picks the newest by mtime, which matches.
- Windows is identified by the presence of `EFI/Microsoft/Boot/bootmgfw.efi` on **any** ESP-shaped (FAT) partition reachable from the running live system, not just the one we're installing onto.
- `--bootloader` is an **installer-level** flag that flows through state → planner → step argv → helper subcommand. No new helper subcommands; instead, the existing `install-boot-usb` / `install-boot-internal` subcommands grow a `--bootloader` arg.
- BIOS USB fallback (the existing `syslinux` MBR write at `crates/installer-helper/src/internal.rs:484-500`) stays in place regardless of which UEFI bootloader was chosen — it costs nothing and keeps the stick legacy-bootable.

## Implementation Plan

### Phase 1 — Core types and CLI surface

- [ ] Task 1.1. Add `pub enum Bootloader { Auto, SystemdBoot, Grub }` to `crates/installer-core/src/lib.rs` (or a new `crates/installer-core/src/bootloader.rs`). Implement `Default = Auto`, `FromStr`/`Display` for the kebab-case values (`auto`, `systemd-boot`, `grub`), and `serde::Serialize/Deserialize`. Rationale: shared by GUI, planner, and tests; needs to round-trip through CLI and JSON-state both.

- [ ] Task 1.2. Add a `pub fn resolve(&self, firmware: Firmware) -> Bootloader` method that maps `Auto` → `SystemdBoot` on UEFI and `Grub` on BIOS. Introduce a tiny `enum Firmware { Uefi, Bios }` detector (`/sys/firmware/efi` exists ⇒ UEFI). Place the detector in `installer-core` next to `Bootloader`. Rationale: planner needs a deterministic resolved value, but the GUI/CLI surface keeps `Auto` so users don't have to know.

- [ ] Task 1.3. Add `clap` to `crates/installer-gui/Cargo.toml` and parse argv at the top of `main()` in `crates/installer-gui/src/main.rs`. Minimal struct:
  ```
  /// Nimblex installer
  #[derive(Parser)]
  struct Cli {
      /// Bootloader to install. Default: auto.
      #[arg(long, value_enum, default_value_t = Bootloader::Auto)]
      bootloader: Bootloader,
  }
  ```
  Pass the resolved value into the GUI state via the existing state module (Task 1.4). Rationale: the user explicitly asked for a CLI flag; clap-derive + ValueEnum is the smallest surface that survives `--help`.

- [ ] Task 1.4. Add `bootloader: Bootloader` field to the GUI state (`crates/installer-gui/src/state.rs` — exists per directory layout). Default to `Bootloader::Auto`. The GUI does not need a UI control today; the flag is enough. (Optional follow-up: a radio in the destination screen.) Rationale: persistent path between CLI parse and planner call.

### Phase 2 — Plumb through the planner

- [ ] Task 2.1. Change `InstallPlanner::plan_for`, `plan_usb`, `plan_alongside_windows` in `crates/installer-core/src/planner.rs` to take an additional `bootloader: Bootloader` argument. Resolve `Auto` to a concrete value at the entry point (`plan_for`), then pass the concrete value down. Rationale: planner emits argv strings; it must know which bootloader to pass to the helper.

- [ ] Task 2.2. In each plan, change the bootloader step's argv to append `--bootloader <value>` to the existing `install-boot-usb` / `install-boot-internal` invocation. The label string can stay generic ("Install bootloader (UEFI + BIOS)" / "Install bootloader") — implementation detail, not user-facing concept. Rationale: minimal diff; subcommand name stays stable.

- [ ] Task 2.3. Update the call sites in `crates/installer-gui/src/screens/screen_destination.rs:556,582` to pass `state.bootloader` into `plan_for`. Update examples (`crates/installer-core/examples/dump-usb-plan.rs`, `dump-transcript.rs`) to pass `Bootloader::SystemdBoot` (concrete; examples shouldn't need OS detection). Rationale: avoid silent default drift.

- [ ] Task 2.4. Update planner tests at `crates/installer-core/src/planner.rs:414,432` (and add new ones) to assert the bootloader-step argv contains `--bootloader systemd-boot` by default and `--bootloader grub` when explicitly chosen. Rationale: regression guard for the wiring.

### Phase 3 — Helper: refactor `install-boot-usb` and `install-boot-internal`

- [ ] Task 3.1. Add `#[arg(long, value_enum, default_value_t = HelperBootloader::SystemdBoot)] bootloader` to both `InternalCmd::InstallBootUsb` and `InternalCmd::InstallBootInternal` in `crates/installer-helper/src/internal.rs:57-72`. (Define a helper-local mirror of the enum or re-export the core one — pick whichever doesn't bloat the helper's deps.) Rationale: the helper already validates argv via clap; reuse that.

- [ ] Task 3.2. In `cmd_install_boot_usb`, dispatch on the new field:
  - `SystemdBoot` → call a new `install_boot_usb_systemd_boot(boot_src, mnt_esp)` that:
    1. Copies kernel and initrd to `EFI/nimblex/{vmlinuz,initrd.img}` (pattern proven on real USB).
    2. Runs `bootctl install --esp-path=<mnt_esp> --no-variables` (success now that systemd ships the EFI binary).
    3. Writes `loader/loader.conf` (`default nimblex.conf`, `timeout 3`, `console-mode max`, `editor yes`).
    4. Writes `loader/entries/nimblex.conf` and `loader/entries/nimblex-cli.conf` (graphical + CLI; the CLI entry uses cmdline `cli` which the live-init's `cli_or_gui()` reads — verified by extracting `initramfs64-nx2` and grepping `lib/nxkitlib`). 
    5. Adds detected Windows entries (Phase 4).
  - `Grub` → call a new `install_boot_usb_grub(boot_src, mnt_esp)` that:
    1. Copies kernel/initrd to `EFI/nimblex/{vmlinuz,initrd.img}`.
    2. Builds a **standalone** GRUB EFI binary via `grub-mkstandalone --format=x86_64-efi --output=…/EFI/BOOT/BOOTX64.EFI --modules="part_gpt fat configfile normal linux echo search search_label" --install-modules="…" --fonts="" --themes="" --locales="" "boot/grub/grub.cfg=<embedded.cfg>"` where `embedded.cfg` does `search --no-floppy --label --set=root NIMBLEX_ESP; configfile /boot/grub/grub.cfg`.
    3. Writes `boot/grub/grub.cfg` with `terminal_output console`, label-based `search --set=root NIMBLEX_ESP`, two NimbleX entries (graphical + CLI), and detected Windows entries.
  - Always (regardless of bootloader): the existing syslinux MBR write at `crates/installer-helper/src/internal.rs:484-500` for BIOS USB fallback. Rationale: implements both backends; matches what we proved boots on the Windows laptop.

- [ ] Task 3.3. In `cmd_install_boot_internal`, dispatch the same way:
  - `SystemdBoot` → keep most of the existing logic at `crates/installer-helper/src/internal.rs:528-617` (it was correct in shape; was only failing because systemd lacked the EFI binaries — that's now fixed). Add Windows-detection-driven extra entries (Phase 4). Drop the registration-of-NVRAM-entry block when the autoscan ESP already contains the Windows Boot Manager (avoids duplicates).
  - `Grub` → place kernel/initrd at `EFI/nimblex/{vmlinuz,initrd.img}`, run `grub-install --target=x86_64-efi --efi-directory=<mnt_esp> --boot-directory=<mnt_esp>/boot --bootloader-id=Nimblex` (registers an NVRAM entry named `Nimblex`), generate `boot/grub/grub.cfg` with NimbleX entries and detected Windows entries (chainload via `chainloader`). Rationale: GRUB on internal install can register itself in NVRAM and chainload Windows from a separate ESP path on the same disk natively; no need for `--removable` here.

- [ ] Task 3.4. Hoist the loader-entry generators into helpers: `fn write_systemd_boot_entries(esp: &Path, entries: &[BootEntry])` and `fn write_grub_cfg(esp: &Path, entries: &[BootEntry])`, both consuming a small `BootEntry { title, kind: Linux{kernel,initrd,options} | EfiChainload{efi_path, esp_uuid} }` shape. Rationale: keeps Phase 4 (Windows detection) producing a backend-neutral list that either backend can render.

### Phase 4 — Windows auto-detection (the new feature the user asked for)

- [ ] Task 4.1. Add `fn detect_windows_installs() -> Vec<WindowsInstall>` to the helper. Strategy:
  1. Enumerate every block partition the live system can see via `lsblk -bnpo NAME,FSTYPE,LABEL,PARTLABEL` filtered to `vfat`/`fat32`. (Re-using `blkid` is also fine.)
  2. For each candidate, mount read-only at a unique `/tmp/nimblex-winscan-N`, look for `EFI/Microsoft/Boot/bootmgfw.efi`. If present, record `WindowsInstall { esp_dev: PathBuf, esp_uuid: String, esp_part_uuid: String, bootmgfw_path: "/EFI/Microsoft/Boot/bootmgfw.efi" }`. Unmount.
  3. Skip our own NimbleX ESP (match by label `NIMBLEX_ESP` or by being the `mnt_esp` we're installing into).
  Rationale: works for both scenarios — single-ESP dual-boot install and USB-live where Windows is on a different disk's ESP.

- [ ] Task 4.2. For **systemd-boot**:
  - Same-ESP case (internal-disk install onto the Windows ESP): `bootctl install` already auto-detects and adds a "Windows Boot Manager" entry on first boot via systemd-boot's auto-windows scan. Don't write a duplicate.
  - Cross-ESP case (USB live, Windows on internal NVMe): write an explicit entry referencing the **partition UUID** of the Windows ESP:
    ```
    title    Windows Boot Manager
    efi      /EFI/Microsoft/Boot/bootmgfw.efi
    options  partuuid=<WIN_ESP_PARTUUID>
    ```
    systemd-boot doesn't natively chainload across ESPs from a `efi` line — so the right primitive here is to fall back to **firmware boot variables**: detect that the firmware already has a `BootXXXX` entry whose loader path is `\EFI\Microsoft\Boot\bootmgfw.efi`, and set `auto-firmware yes` in `loader.conf`, plus add an explicit "Reboot to Windows" entry that uses `efi-firmware-setup`-style fallback. Practically: ship `auto-firmware yes` and document that the user should pick the firmware's existing Windows entry from systemd-boot's auto-firmware list. **Decision point**: if cross-ESP chainload from systemd-boot proves unreliable, copy `bootmgfw.efi` and the `Boot/` directory into the USB ESP at `EFI/Microsoft/Boot/` so that a same-ESP `efi /EFI/Microsoft/Boot/bootmgfw.efi` works. Document this limitation in README.
  Rationale: be honest about systemd-boot's same-ESP scope; lean on firmware boot entries for cross-disk.

- [ ] Task 4.3. For **GRUB**: emit a chainloader entry per detected Windows install. GRUB *can* cross-ESP cleanly:
  ```
  menuentry "Windows Boot Manager" {
      insmod part_gpt
      insmod fat
      insmod chain
      search --no-floppy --fs-uuid --set=root <WIN_ESP_FS_UUID>
      chainloader /EFI/Microsoft/Boot/bootmgfw.efi
  }
  ```
  No restriction on which disk the FS-UUID lives on. Rationale: this is GRUB's strongest card for the USB-live-on-Windows-laptop scenario; users get a working Windows entry without leaving NimbleX.

- [ ] Task 4.4. Print a one-line summary on the helper's stdout for each detected Windows install (e.g. `Detected Windows on /dev/nvme0n1p1 (FS-UUID: ABCD-1234)`); the runner already surfaces helper stdout as events, so this gives the GUI/CLI user feedback. Rationale: visibility into what got added.

### Phase 5 — Allowlist + sandbox

- [ ] Task 5.1. The internal subcommand allowlist at `crates/installer-helper/src/allowlist.rs:267-273` only gatekeeps the subcommand name; clap inside the helper validates flags. Confirm `install-boot-usb` and `install-boot-internal` still pass with the new `--bootloader` flag (no allowlist change should be needed, but verify with a unit test). Rationale: belt-and-braces — sometimes allowlists assume known flag sets.

- [ ] Task 5.2. Add `grub-install`, `grub-mkstandalone`, `bootctl`, `efibootmgr` to whatever process-execution allowlist exists (search for the tool-allowlist; if there isn't one beyond the internal-subcommand list, no-op). Rationale: the helper runs as root via pkexec; principle of least surprise.

### Phase 6 — Tests

- [ ] Task 6.1. Unit test: `Bootloader::Auto.resolve(Firmware::Uefi) == SystemdBoot`, `…(Firmware::Bios) == Grub`, explicit values pass through.

- [ ] Task 6.2. Planner test: `plan_usb(disk, Bootloader::Auto)` on a faked UEFI host emits a step whose argv ends in `--bootloader systemd-boot`; same with `Bootloader::Grub` → `--bootloader grub`.

- [ ] Task 6.3. Helper integration test (or example binary under `crates/installer-helper/examples/`) that calls the loader-entry generator on a tempdir and asserts:
  - systemd-boot mode produces `loader/loader.conf`, `loader/entries/nimblex.conf`, `loader/entries/nimblex-cli.conf` with expected content.
  - GRUB mode produces `boot/grub/grub.cfg` containing both NimbleX menu entries and the `cli` cmdline.
  - When a `WindowsInstall` is supplied, both modes emit a Windows entry.

- [ ] Task 6.4. Snapshot-test the GUI CLI surface: `nimblex-installer --help` shows the `--bootloader` flag with `auto|systemd-boot|grub` choices.

### Phase 7 — Documentation

- [ ] Task 7.1. Update `README.md`: under the "USB, UEFI" / boot table (currently at `README.md:38`), document the two backends, the default policy, and the CLI flag. Note the limitation that systemd-boot's cross-ESP Windows handling depends on firmware boot variables; GRUB chainloads regardless.

- [ ] Task 7.2. Mark the existing planning doc `plans/2026-05-04-nimblex-installer2-feedback3-v1.md` as superseded for the bootloader pieces by this plan; the UKI approach is no longer the intended path.

## Verification Criteria

- `cargo test -p installer-core -p installer-helper` passes including new bootloader tests.
- `cargo run -p installer-gui -- --help` lists `--bootloader auto|systemd-boot|grub`.
- USB install with no flag (default) on a UEFI machine produces a stick with `EFI/BOOT/BOOTX64.EFI` = systemd-boot, two NimbleX entries (graphical + CLI), one Windows entry per detected Windows install on the host. Boots cleanly on both the Linux dev box and the Windows laptop.
- USB install with `--bootloader=grub` produces a stick with `EFI/BOOT/BOOTX64.EFI` = standalone GRUB, `boot/grub/grub.cfg` containing the same two NimbleX entries, plus a `chainloader` entry per detected Windows. Menu/edit responsiveness is comparable to the systemd-boot stick (because it's a single-file image with no theme/locale tree).
- Internal-disk install with `--bootloader=systemd-boot` on a machine with Windows on the same ESP shows Windows Boot Manager in the systemd-boot menu without an explicit entry being written (auto-discovery).
- Internal-disk install with `--bootloader=grub` writes a grub.cfg with explicit Windows chainloader entries and registers a `Nimblex` NVRAM boot entry via `efibootmgr`.
- The CLI entry boots NimbleX into multi-user.target (no graphical login) on both backends — verified by checking `systemctl get-default` after boot.

## Potential Risks and Mitigations

1. **systemd-boot cannot reliably chainload across ESPs.** Mitigation: rely on `auto-firmware yes` so the firmware's existing Windows boot entry is shown in the menu; document the limitation; offer the GRUB backend as the workaround for users who need a single menu entry that works on any machine.

2. **`grub-install --target=x86_64-efi` for the internal-disk path requires `/usr/lib64/grub/x86_64-efi/` modules to be present.** Verified during the previous session that they are; add a startup check in `install_boot_internal_grub` that fails fast with a clear message if the module dir is missing.

3. **Windows ESP detection mounts NTFS-adjacent FAT partitions read-only.** Mitigation: mount with `-o ro,nofail` and unmount in a defer-style guard; never write to a detected Windows ESP unless the install scenario explicitly targets it.

4. **`bootctl install` is idempotent but rewrites `EFI/BOOT/BOOTX64.EFI` unconditionally.** When installing onto a shared internal ESP that already has a Windows-shipped `BOOTX64.EFI`, this could be surprising. Mitigation: detect the Windows-shipped fallback (`EFI/BOOT/BOOTX64.EFI` size/signature matches `bootmgfw.efi`) and warn the user before overwriting; prefer registering an NVRAM entry pointing at `EFI/systemd/systemd-bootx64.efi` and leaving the firmware fallback alone.

5. **Auto-detection of Windows finds installs the user doesn't want surfaced** (e.g. an old Windows on a secondary disk). Mitigation: print every detected install to stdout with a `[autodetected]` tag so the runner can surface it; future flag `--no-autoboot-windows` to suppress.

6. **CLI entry interaction with the live-init.** The `cli` cmdline token is matched by `grep -wq cli /proc/cmdline` in the initramfs — confirmed in `/tmp/init-x/lib/nxkitlib`. A future `cli=foo` cmdline token would still match because `grep -w` matches whole words. Mitigation: leave the entry's `options` as just `cli`, no value.

## Alternative Approaches

1. **Drop GRUB entirely; ship only systemd-boot.** Trade-off: smaller installer, no BIOS support, no cross-ESP Windows chainload. Rejected because the user maintains a Slackware-derived distro and BIOS PCs are still a real audience.

2. **Drop systemd-boot; ship only GRUB.** Trade-off: one bootloader to test, GRUB does everything. Rejected because the existing internal-disk path already uses `bootctl`, the internal-ESP install integrates beautifully with Windows when systemd-boot is on the same ESP, and systemd-boot is dramatically faster on slow firmware USB drivers without any tuning. We measured this in the previous session.

3. **Ship a Unified Kernel Image (UKI) as `EFI/BOOT/BOOTX64.EFI` and skip bootloaders entirely** — the original plan in `plans/2026-05-04-nimblex-installer2-feedback3-v1.md`. Trade-off: fastest possible boot (no menu, no editor), one PE file. Now feasible since systemd-258 ships `linuxx64.efi.stub`. Rejected as the *default* path because it removes the menu (no CLI vs graphical choice, no Windows entry, no debug entry); could be added later as `--bootloader=uki` for kiosk / appliance installs.

4. **Use rEFInd as a third backend.** Trade-off: prettier menu, very good auto-discovery (including macOS / Windows / other distros). Rejected as scope creep — two backends already cover the matrix. Easy to add a `Refind` variant later if needed.
