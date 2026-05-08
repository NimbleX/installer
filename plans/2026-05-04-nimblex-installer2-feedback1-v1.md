# Nimblex Graphical Installer — Feedback Round 1 Addendum

This addendum extends `plans/2026-05-04-nimblex-installer2-v1.md` with answers to four strategic questions: missing tools, boot strategy, filesystem choices, and the canonical ideal-path user journey. **All v1 tasks remain in force**; the items below augment Phase 0/1/2/5 of v1.

## Objective

Lock in the concrete technical choices (tools to add, bootloaders, filesystems) so v1's checkbox tasks have unambiguous targets, and define the canonical happy-path scenario as the system's design north-star.

## A. Tools to Add (small footprint, high value)

Total added footprint target: **≤ 10 MB compressed**, packageable as a new bundle `03-Installer64.lzm` or appended into `01-Core64.lzm`.

- [ ] Task A.1. Package **`os-prober`** (~80 KB). Use it to authoritatively detect Windows installs and their boot paths instead of hand-rolling NTFS sniffing. Becomes the source of truth for the `WindowsSystem(C:)` role in v1 Task 1.1.
- [ ] Task A.2. Package **`chntpw`** (~500 KB). Use it to (a) detect whether Windows Fast Startup / hibernation is enabled by reading the `HKLM\SYSTEM\CurrentControlSet\Control\Session Manager\Power` registry key, and (b) optionally toggle `HiberbootEnabled` to 0 with the user's consent before `ntfsresize`. Eliminates the #1 cause of NTFS corruption in dual-boot installs.
- [ ] Task A.3. Package **`systemd-boot`** (`bootctl`, ~200 KB if systemd is already present; otherwise pull the standalone EFI binary only). Default UEFI bootloader.
- [ ] Task A.4. Package **`rEFInd`** (~5 MB) as an **opt-in** alternative UEFI bootloader exposed via an "Advanced" toggle on screen 3. Used when the user wants a graphical OS picker.
- [ ] Task A.5. Package **`f3`** (`f3probe`, `f3write`, `f3read`, ~100 KB). Run `f3probe --time-ops` on the selected USB before any destructive action. If it reports fake capacity, abort with a graphical warning.
- [ ] Task A.6. Package **`pv`** (~50 KB). Pipe `cp`/`tar`/`dd` through `pv -n` so the helper emits a real byte-progress stream the GUI can render as percentages.
- [ ] Task A.7. Package **`hwinfo`** OR **`lshw`** (~1 MB, pick one). Use for richer disk metadata (vendor, model, NVMe namespace, USB bus speed) shown in the disk picker.
- [ ] Task A.8. Package **`shim-signed`** + **`mokutil`** (~2 MB). Enables boot under Secure Boot without asking the user to disable it in firmware. Bootloader install path uses shim → systemd-boot/rEFInd chain.
- [ ] Task A.9. Explicitly **do not** package GRUB. Document this decision in `docs/decisions/0001-no-grub.md` (a code-comment-style ADR, not user-facing docs).
- [ ] Task A.10. Update v1 Task 5.4's CI allowlist to include the sonames introduced by A.1–A.8.

## B. Boot Strategy (one rule per scenario)

- [ ] Task B.1. **USB stick install** — GPT with protective MBR; ESP=FAT32 (512 MB) + root=ext4 (rest). Install systemd-boot to ESP for UEFI, syslinux to protective MBR + ext4 PBR for BIOS. Result: USB boots on both firmware types, no per-machine configuration.
- [ ] Task B.2. **Internal alongside Windows, UEFI** (default modern path) — **reuse the existing ESP**, never create a second one. Mount it, create `EFI/nimblex/` with kernel + initrd, write `loader/entries/nimblex.conf` for systemd-boot, register via `efibootmgr -c -L Nimblex -l '\EFI\systemd\systemd-bootx64.efi'`. **Windows Boot Manager is read but never written.**
- [ ] Task B.3. **Internal alongside Windows, BIOS** (legacy fallback) — install **extlinux to the Nimblex partition's PBR only**. **Never write the MBR by default.** Document that the user picks the OS via firmware F12. Provide an opt-in "Add Nimblex to Windows boot menu via `bcdedit`" toggle (default off, deferred to v1.1 — track as Task B.6).
- [ ] Task B.4. **Optional graphical OS picker** — if the user toggles "Use a graphical boot menu" on screen 3, install rEFInd to ESP instead of systemd-boot. rEFInd auto-detects Windows; no further configuration needed.
- [ ] Task B.5. **Secure Boot** — when firmware reports Secure Boot enabled (read `/sys/firmware/efi/efivars/SecureBoot-*`), install via shim → bootloader chain; emit a one-time MOK enrollment notice in the success screen.
- [ ] Task B.6 (deferred to v1.1). BCD chainload entry for BIOS dual-boot users who want a single Windows-managed boot menu.
- [ ] Task B.7. Remove all GRUB references from any code path imported from `01-Core64.lzm/usr/bin/nimblex-install:222-255`.

## C. Filesystems (one default per use case)

- [ ] Task C.1. ESP (USB + reused on internal): **FAT32**, mkfs via `mkfs.fat -F32 -n NIMBLEX_ESP`.
- [ ] Task C.2. Nimblex root (USB and internal): **ext4**, mkfs via `mkfs.ext4 -L NIMBLEX_ROOT -O ^has_journal=false` (journal ON; the `^` is illustrative — keep journal enabled for safety).
- [ ] Task C.3. Persistence (USB): **directory on the ext4 root**, no fixed-size loop file. Migration path from the legacy `nimblex.data` XFS-loop-file approach (`01-Core64.lzm/usr/bin/create_nimblex.data`) is documented but not required for v1.
- [ ] Task C.4. Swap: **swapfile on ext4** (default 2 GiB; skipped if RAM ≥ 16 GB). No swap partition. Created via `fallocate` + `mkswap` + `swapon`. Recorded in `/etc/fstab`.
- [ ] Task C.5. Windows partition: **NTFS, read-only except for `ntfsresize`**. No reformat, no defrag, no chkdsk invoked from Linux side.
- [ ] Task C.6. Filesystems explicitly **out of scope for v1**: btrfs, xfs, jfs, f2fs, bcachefs, LUKS-on-root. All remain available in 01-Core for future use; revisit in v2 with telemetry data.

## D. Ideal-Scenario Specification (design north-star)

The "Maria on Windows 11" scenario is adopted as the canonical happy path. v1's GUI and helper must produce this exact behaviour for an in-scope dual-boot install:

- [ ] Task D.1. Auto-launch the installer on live-session start via `nimblex-installer.desktop` autostart entry (already in v1 Task 5.3).
- [ ] Task D.2. Screen 1 to Continue button on Screen 2 must be reachable in **≤ 5 seconds** of CPU time on a 2020-era laptop, with all probes (`lsblk`, `os-prober`, `ntfsresize --info`, `chntpw` registry read, `f3probe --quick` if USB target) running in parallel.
- [ ] Task D.3. The Fast Startup modal (Task A.2) appears **only if** `chntpw` reports `HiberbootEnabled=1`; offers two buttons: "Turn it off for me" (one-shot registry edit + immediate re-probe) and "I'll do it myself" (returns to Screen 2 with a banner).
- [ ] Task D.4. Screen 2's resize splitter is **pre-positioned at `max(ntfsresize_min × 1.25, 40 GiB Windows residual)`** — never at the absolute minimum. User must drag *outward* to give Nimblex more space, never *inward* into a danger zone (clamped).
- [ ] Task D.5. Screen 3 always shows the same six pictographic steps in the same order: **shrink → create → format → copy → register-boot → done** (or **format → copy → install-boot → done** for USB). Captions ≤ 6 words.
- [ ] Task D.6. Live execution view animates the same six icons; expandable "Details" disclosure streams `pv`-driven byte progress and helper JSON events.
- [ ] Task D.7. Success screen confirms three invariants in plain icons: ✓ Windows untouched, ✓ Nimblex installed, ✓ Boot entry registered. Two buttons: **Reboot now**, **Close**. **No auto-reboot.**
- [ ] Task D.8. End-to-end timing budget on reference hardware (NVMe SSD, ~3 GB modules): probe ≤ 5 s, shrink 60–120 s, partition+format ≤ 5 s, copy ≤ 4 min, bootloader register ≤ 3 s. Total target: **under 7 minutes**.

## Verification Criteria (additions)

- `os-prober` output is the only source for "is there a Windows here" decisions; no ad-hoc NTFS sniffing remains in the codebase (grep gate in CI).
- After install, on a test image: `efibootmgr -v` shows both `Windows Boot Manager` and `Nimblex` entries; the Windows entry's `Boot####` number is unchanged from pre-install (snapshot test).
- The ESP is mounted read-only by the GUI for inspection and only mounted read-write by the helper during the bootloader-register step.
- The MBR sector (LBA 0) of any disk containing a Windows install is byte-identical before and after install in alongside-Windows mode (snapshot test against a loopback image).
- No process spawned by the installer ever writes to a partition whose role is `EFISystem`/`MicrosoftReserved`/`WindowsRecovery`/`WindowsSystem` other than the documented exceptions (ESP write during bootloader register; NTFS shrink on `WindowsSystem`). Asserted by an `strace`-based test on the helper.
- `f3probe` is invoked on every USB target and a fake-capacity result blocks the install path entirely.
- The added tools (A.1–A.8) increase the live ISO size by **≤ 10 MB compressed**, measured against the pre-addition baseline.

## Potential Risks and Mitigations (additions)

1. **`chntpw` modifies the wrong registry hive or corrupts `SYSTEM`.**
   Mitigation: copy `C:\Windows\System32\config\SYSTEM` to a backup file on the same partition before any write; if `chntpw` exit code is non-zero, restore the backup; only ever flip `HiberbootEnabled`, never any other key (allowlist enforced in helper).

2. **`os-prober` mounts and unmounts NTFS partitions as a side effect, leaving them dirty if interrupted.**
   Mitigation: run `os-prober` once at startup; if its run is interrupted, run `ntfsfix -d` on any NTFS partition with a dirty bit before proceeding; gate the next probe behind a clean status.

3. **`efibootmgr` BootOrder change accidentally demotes Windows Boot Manager.**
   Mitigation: read `BootOrder` before any change, append the new Nimblex entry to the **end** of `BootOrder` rather than the front by default; offer a "Make Nimblex default" toggle (off by default).

4. **rEFInd auto-detects Windows incorrectly on multi-disk setups and presents broken entries.**
   Mitigation: write an explicit `refind.conf` listing only the entries we verified via `os-prober`; disable rEFInd's autoscan with `scanfor manual`.

5. **Secure Boot shim chain fails on firmware that rejects unknown vendor signatures.**
   Mitigation: detect via `mokutil --sb-state`; if shim chain fails to register, fall back to a plain systemd-boot install and surface a "Secure Boot may need to be disabled in firmware" notice; never auto-disable Secure Boot.

## Alternative Approaches (additions)

1. **GRUB2 instead of systemd-boot + rEFInd.** Trade-off: industry-standard, single tool covers both BIOS and UEFI, but adds ~15 MB and significantly more complexity (Lua-ish config, `grub-mkconfig`/`os-prober` integration quirks, well-known Windows-BCD-clobbering bugs). Rejected for v1.

2. **Single-FAT32 USB layout** (Porteus-style, no separate root). Trade-off: simplest possible USB layout, boots on anything, but limits any single file to 4 GB and complicates persistence. Rejected; ext4 root is worth the small complexity cost.

3. **Btrfs root with subvolumes for snapshots before install steps.** Trade-off: enables atomic rollback, but the installer is a one-shot tool — rollback complexity is better solved by simply not making destructive mistakes (planner correctness). Rejected for v1.

4. **Skip `chntpw`, just refuse to install when Fast Startup is on.** Trade-off: ~500 KB saved, but forces every Windows-11-default user to reboot into Windows, navigate Control Panel, and come back. Rejected — the UX cost is far higher than the package cost.

5. **Skip `os-prober`, parse `lsblk`+`blkid` manually.** Trade-off: ~80 KB saved, but reinvents detection logic that has been hardened over a decade. Rejected — false economy.
