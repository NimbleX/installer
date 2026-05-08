# Nimblex Installer — Feedback Round 3 Addendum

This addendum supersedes the bootloader portion of `plans/2026-05-04-nimblex-installer2-feedback2-v1.md` (Tasks F2.A1–F2.A5, F2.6, F2.7) following two facts confirmed by the user:

1. The newly installed packages (`chntpw`, `f3`, `pv`, `shim-signed`) are already in the Nimblex build script and will be persisted in the next rebuild — no separate `03-Installer64.lzm` work needed.
2. `systemd` has been rebuilt with `-Defi=true`, so `systemd-boot` (`bootctl install`, `systemd-bootx64.efi`, `linuxx64.efi.stub`) is now functional.
3. The kernel at `/mnt/nvme0n1p5/Work/aufs5-linux` has `CONFIG_EFI_STUB=y`, `CONFIG_EFI_HANDOVER_PROTOCOL=y`, `CONFIG_RELOCATABLE=y`, `CONFIG_RANDOMIZE_BASE=y` (verified in `.config:496-522`). Kernel version 6.19.0 (`.config:3`). The compiled `arch/x86/boot/bzImage` is itself a valid PE/EFI binary; no conversion required.

## Objective

Adopt a **hybrid bootloader strategy** that exploits both EFI stub kernels and the now-functional systemd-boot, optimizing each install scenario separately:

- **USB install** → single Unified Kernel Image (UKI) at the firmware fallback path. Zero per-machine setup, boots on any UEFI computer the stick is plugged into.
- **Internal alongside Windows** → systemd-boot installed to the existing Windows ESP, with a UKI as its entry. Provides a clean menu, editable cmdline, and future multi-entry support without ever touching Windows Boot Manager.
- **Legacy BIOS** (USB or internal) → syslinux/extlinux as before, loading the same `bzImage` and initrd directly.

## Background — How EFI Stub Works (one-liner reference)

UEFI firmware loads `\EFI\BOOT\BOOTX64.EFI` (or any registered entry path) into memory and jumps to it. With `CONFIG_EFI_STUB=y`, the kernel's `bzImage` *is* that PE/EFI executable. The stub runs in the EFI environment, opens the initrd via EFI file services per the `initrd=…` cmdline, calls `ExitBootServices`, and hands off to the regular kernel boot path. No bootloader is required.

A **Unified Kernel Image (UKI)** uses `objcopy` (or `ukify`) to bundle kernel + initrd + cmdline + os-release into a single PE file by adding `.linux`, `.initrd`, `.cmdline`, `.osrel` sections to systemd's `linuxx64.efi.stub` template (now installed with the systemd EFI rebuild). One file = one bootable artifact, no external initrd, no external cmdline.

## Implementation Plan (replaces feedback-2 §Blocker 2)

### Phase F3.A — UKI Build Pipeline

- [ ] Task F3.A1. Add a build step (inside the Nimblex live-build pipeline, not at install time) that produces `nimblex.efi` from `bzImage` + `initrd.img` + a static `cmdline.txt`. Use `objcopy` against `/usr/lib/systemd/boot/efi/linuxx64.efi.stub` — confirm this file exists after the systemd rebuild; if not, fall back to using `bzImage` directly as the stub.
- [ ] Task F3.A2. Define the canonical USB cmdline: `root=LABEL=NIMBLEX_ROOT rootfstype=ext4 ro quiet splash` plus whatever Nimblex's dracut/initrd expects to locate the `.lzm` bundles (mirror what the legacy live boot uses today — read it from the running system's `/proc/cmdline` and bake the same into `cmdline.txt`).
- [ ] Task F3.A3. Define the canonical internal-install cmdline: same as USB but `root=UUID=<generated-at-install-time>` resolved per-machine. This means the internal install path **regenerates the UKI on the target machine** with the right UUID, or — simpler — uses external kernel+initrd+cmdline registered via `efibootmgr` for internal installs (UKI is USB-only). Pick the latter for simplicity in v1.
- [ ] Task F3.A4. Verify each built UKI with `file nimblex.efi` (must report `PE32+ executable (EFI application)`) and by booting it in `qemu-system-x86_64` with OVMF firmware.

### Phase F3.B — USB Install Path (UKI + syslinux dual)

- [ ] Task F3.B1. Partition the USB: GPT with protective MBR, ESP=FAT32 512 MB labelled `NIMBLEX_ESP`, root=ext4 rest labelled `NIMBLEX_ROOT`.
- [ ] Task F3.B2. Copy the pre-built UKI to `ESP:/EFI/BOOT/BOOTX64.EFI` (the firmware-mandated removable-media fallback path — boots without any NVRAM entry on any UEFI machine).
- [ ] Task F3.B3. Copy the Nimblex bundles (`*.lzm`, kernel, initrd) to `NIMBLEX_ROOT:/nimblex/`.
- [ ] Task F3.B4. Install syslinux to the protective MBR (`syslinux --install /dev/sdX1`) and the ext4 PBR; write `syslinux.cfg` pointing at the same `bzImage` + `initrd` already on the ext4 partition. Result: same USB boots BIOS and UEFI machines from the same media.
- [ ] Task F3.B5. Sync, eject; user can plug it into any modern PC.

### Phase F3.C — Internal Alongside Windows (systemd-boot + external kernel)

- [ ] Task F3.C1. Mount the existing Windows ESP read-write (only during this step).
- [ ] Task F3.C2. Run `bootctl install --esp-path=<mount>` to install `systemd-bootx64.efi` to the ESP and register a `systemd-boot` UEFI entry via `efibootmgr`. **Do not** alter `BootOrder` to put it first — append.
- [ ] Task F3.C3. Copy `vmlinuz` → `ESP:/EFI/nimblex/vmlinuz.efi` and `initrd.img` → `ESP:/EFI/nimblex/initrd.img`.
- [ ] Task F3.C4. Write `ESP:/loader/entries/nimblex.conf`:
  ```
  title    Nimblex
  linux    /EFI/nimblex/vmlinuz.efi
  initrd   /EFI/nimblex/initrd.img
  options  root=UUID=<uuid> ro quiet splash
  ```
- [ ] Task F3.C5. Write `ESP:/loader/loader.conf`:
  ```
  default  nimblex.conf
  timeout  3
  console-mode max
  editor   no
  ```
  Setting a small (3 s) timeout means: if the user does nothing, Nimblex boots; if they press a key, they see a menu including the auto-detected Windows Boot Manager entry. systemd-boot autoscan picks Windows up from the same ESP.
- [ ] Task F3.C6. Unmount the ESP. Verify `efibootmgr -v` lists both `Windows Boot Manager` (unchanged) and a new `Linux Boot Manager` entry pointing at systemd-bootx64.efi.

### Phase F3.D — Legacy BIOS Path (USB and internal)

- [ ] Task F3.D1. Internal-BIOS install: write extlinux to the Nimblex ext4 partition's PBR only (never the MBR). Generate `extlinux.conf` with one default Nimblex entry plus a chainload entry to the Windows partition for convenience.
- [ ] Task F3.D2. USB-BIOS install: covered by Task F3.B4 (syslinux on the USB).

### Phase F3.E — Removed / Replaced Tasks

- [ ] Task F3.E1. **Remove** feedback-2 Tasks F2.A1–F2.A5 (EFI-stub-only-everywhere) and F2.6 — replaced by F3.A through F3.D.
- [ ] Task F3.E2. **Remove** feedback-2 Tasks F2.1–F2.5 (separate `03-Installer64.lzm` bundle) — superseded by user's confirmation that the new tools are already in the build script and will appear in `01-Core64.lzm` after the next rebuild.
- [ ] Task F3.E3. **Remove** feedback-2 Tasks F2.B1–F2.B3 and F2.7 (ELILO fallback) — no longer needed; systemd-boot covers UEFI, syslinux/extlinux covers BIOS.
- [ ] Task F3.E4. **Remove** feedback-2 Tasks F2.C1–F2.C3 (ship systemd-bootx64.efi as a blob) — superseded by user's systemd rebuild.
- [ ] Task F3.E5. **Keep** feedback-2 Tasks F2.9–F2.11 (Secure Boot deferred to v1.1) unchanged.

## Disadvantages of EFI Stub / UKI (acknowledged trade-offs)

| Disadvantage | Severity | How v1 handles it |
|---|---|---|
| No graphical menu when EFI-stub is used alone | Medium for USB | USB targets the firmware F12 user; users dual-booting from internal disk get systemd-boot's text menu instead. |
| Cmdline edits require rebuilding the UKI | Medium | USB ships one cmdline, regenerated at distro build time, not at install time. Internal installs use external kernel+initrd so cmdline lives in `loader/entries/nimblex.conf` — editable in seconds. |
| No fallback kernel on the same media | Low | Acceptable for an installer USB; out of scope for v1. |
| Initrd path encoding quirks on old firmware | Low | Kernel 6.19.0 + modern UEFI firmware (post-2018) are fine. |
| Secure Boot requires the UKI/kernel to be signed | Deferred | v1.1 — see F2.9. |
| Multi-OS picking on USB uses firmware F12 | Low | Same as today's Nimblex live boot; no regression. |
| UKI file is 30–60 MB (kernel + initrd combined) | Low | Only one such file per USB; size budget already includes it. |
| Hardware diagnostic EFI tools cannot be launched without a menu | Low | Out of scope for v1 installer. |

## Verification Criteria (additions)

- `file <built-uki>` reports `PE32+ executable (EFI application)`; same applies to `bzImage`.
- A USB built with the F3.B pipeline boots successfully in `qemu-system-x86_64 -bios OVMF.fd` (UEFI) **and** in `qemu-system-x86_64` (default SeaBIOS) without modification.
- After an internal-alongside-Windows install on a test image, `efibootmgr -v` shows `Windows Boot Manager` at its original `Boot####` slot, unchanged, plus a new `Linux Boot Manager` entry registered last in `BootOrder`.
- systemd-boot autoscan (no manual config) detects the existing Windows entry and shows it in the boot menu alongside Nimblex.
- The Windows ESP partition is mounted read-write only during the F3.C2–F3.C5 steps and read-only or unmounted at all other times (asserted by the helper's mount tracking).
- The MBR sector of any GPT disk containing a Windows install is byte-identical before and after the internal install.

## Potential Risks and Mitigations (additions)

1. **`/usr/lib/systemd/boot/efi/linuxx64.efi.stub` missing despite EFI-enabled systemd rebuild** (e.g. meson option present but stub still skipped).
   Mitigation: F3.A1 falls back to using `bzImage` directly as the PE container; verify presence in the Nimblex live image before relying on the stub.

2. **systemd-boot autoscan misidentifies Windows partitions on multi-disk setups.**
   Mitigation: leave `auto-entries yes` (default) but verify on a test multi-disk image; if misdetection occurs, write a manual `windows.conf` entry from `os-prober` data instead of relying on autoscan.

3. **Existing Windows ESP is full** (rare but possible on tightly-provisioned OEM laptops).
   Mitigation: pre-flight check measures free space on the ESP before starting; if < 60 MB free, abort with a graphical message and offer USB install instead. Do **not** expand the ESP — that's a wasp's nest.

4. **UKI cmdline baked at distro-build time becomes wrong** if a Nimblex update changes label/UUID conventions.
   Mitigation: cmdline is part of the live-build pipeline and reviewed when bundles change; same review cadence as the rest of the live boot.

5. **bzImage signature breaks** when objcopy modifies it for UKI (relevant only under Secure Boot).
   Mitigation: deferred to v1.1 along with the rest of SB support.

## Alternative Approaches (additions)

1. **Use systemd-boot on USB too** (drop UKI). Trade-off: gives a menu on USB, but requires per-machine NVRAM registration *or* relying on the same `\EFI\BOOT\BOOTX64.EFI` fallback (where `BOOTX64.EFI` would be `systemd-bootx64.efi` instead of a UKI). Reasonable; rejected for v1 because UKI is one self-contained file with no `loader/` directory or autoscan complexity. Easy to switch later.

2. **Use UKI on internal install too** (drop external kernel+initrd). Trade-off: cmdline regeneration needs `objcopy` available at install time — it is (binutils in 01-Core), but it adds a build step inside the helper and makes per-machine cmdline tweaks harder. Rejected for v1.

3. **Use the kernel directly without UKI on USB** (kernel as `BOOTX64.EFI`, separate `initrd.img`). Trade-off: simpler build pipeline, but cmdline must come from `CONFIG_CMDLINE` (currently empty) — would require a kernel rebuild for any cmdline change. Rejected; UKI is more flexible.
