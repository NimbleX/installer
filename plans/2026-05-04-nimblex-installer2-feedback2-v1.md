# Nimblex Installer — Feedback Round 2 Addendum

This addendum extends `plans/2026-05-04-nimblex-installer2-v1.md` and `plans/2026-05-04-nimblex-installer2-feedback1-v1.md` based on a verification audit of the live system after the user installed `os-prober`, `chntpw`, `bootctl`, `f3`, `pv`, and `shim-signed`.

## Objective

Resolve three blockers discovered during verification before Phase 1 (Domain Model) begins:
1. The newly installed packages live only in the tmpfs overlay; none is in a `.lzm` bundle yet.
2. `systemd-boot` is non-functional in this Slackware build — the `systemd-bootx64.efi` stub is absent, so `bootctl install` will fail.
3. `shim-signed` is incomplete (no `mmx64.efi`) and the manifest is duplicated.

## Verification Snapshot (audit results)

- `os-prober-1.84-x86_64-2` — already present in `01-Core64.lzm/var/lib/pkgtools/packages/`. Pre-existing; no action required.
- `chntpw-140201-x86_64-1`, `f3-8.0-x86_64-1`, `pv-1.10.3-x86_64-1`, `shim-signed-1.47-x86_64-1` — present only in `/var/lib/pkgtools/packages/` on the writable aufs branch (`/proc/mounts:3`); absent from every `.lzm` under `/mnt/live/memory/bundles/`.
- `shim-signed` is installed **twice** (duplicate manifest with `_SBo` suffix); ships only `usr/lib/shim/shimx64.efi.signed` and `usr/share/shim-signed/MicCorUEFCA2011_2011-06-27.crt`. **MokManager (`mmx64.efi`) is not included.**
- `systemd-258-x86_64-1` ships `bootctl` and the `systemd-boot-check-no-failures` service file but **not** `systemd-bootx64.efi`. The Slackware build was compiled without `-Defi=true`. `bootctl install` will return "Failed to find EFI binary".
- `grub-2.14-x86_64-3` lives in `07-Devel64.lzm` (out of our dependency scope — do not rely on it).
- `refind` is correctly absent.
- `ntfs-3g-2022.10.3` provides `ntfsresize`, `ntfsfix`, `mkntfs`, `ntfsclone` — all confirmed.

## Blocker 1 — Persist Newly Installed Tools into a Bundle

- [ ] Task F2.1. Decide bundle strategy. **Recommended:** create a new `03-Installer64.lzm` containing `chntpw`, `f3`, `pv`, `shim-signed`, plus the installer GUI/helper artifacts themselves. Less invasive than rebuilding `01-Core64.lzm` and easy to revoke. Alternative: rebuild `01-Core64.lzm` from `/mnt/nvme0n1p5/Work/NimbleX/01-Core64-work/` (the location referenced in `systemd-258-x86_64-1:4` and `ntfs-3g-2022.10.3-x86_64-1:4`).
- [ ] Task F2.2. Before bundling, **deduplicate `shim-signed`** with `removepkg shim-signed-1.47-x86_64-1_SBo` (keep the build-pipeline-tagged one) or whichever manifest matches the canonical build root. Verify with `ls /var/lib/pkgtools/packages/shim-signed*` afterwards.
- [ ] Task F2.3. Validate the new bundle with `mksquashfs … -comp xz` and confirm via loopback mount that `/var/lib/pkgtools/packages/{chntpw,f3,pv,shim-signed}*` are all visible inside the bundle.
- [ ] Task F2.4. Update v1 Task 5.4's `ldd` allowlist generator to include the new bundle as a valid soname source.
- [ ] Task F2.5. Document the bundle decision in a one-line code comment in the helper crate (no separate ADR document needed).

## Blocker 2 — systemd-boot EFI Stub Missing → Bootloader Pivot

The feedback-1 plan named systemd-boot as the default UEFI bootloader. That choice is currently unimplementable. Pick one of the following four resolutions; the recommendation is Option A.

### Option A (recommended) — EFI stub boot, no bootloader

Modern Linux kernels are PE/COFF EFI binaries. Boot them directly via `efibootmgr`; no bootloader at all.

- [ ] Task F2.A1. Verify the Nimblex kernel is built with `CONFIG_EFI_STUB=y`. If yes, the kernel itself is a bootable EFI executable.
- [ ] Task F2.A2. Installer copies `vmlinuz` → `\EFI\nimblex\vmlinuz.efi` and `initrd.img` → `\EFI\nimblex\initrd.img` on the ESP.
- [ ] Task F2.A3. Register via `efibootmgr -c -d <disk> -p <esp_part> -L "Nimblex" -l '\EFI\nimblex\vmlinuz.efi' -u 'initrd=\EFI\nimblex\initrd.img root=UUID=… ro quiet splash'` with the initrd passed through the kernel cmdline.
- [ ] Task F2.A4. For multi-entry support (rescue mode, persistence on/off), register multiple `efibootmgr` entries — one per variant. Trade-off: no graphical menu; relies on firmware F12. Acceptable given the 3-screen minimalism.
- [ ] Task F2.A5. Pros: zero bootloader code shipped, smallest attack surface, no `bootctl install` dependency, works today. Cons: no in-OS menu (firmware F12 only), no Windows chainload menu (Windows Boot Manager remains a separate firmware entry — exactly what we want anyway).

### Option B — ELILO (already in 01-Core, zero new packaging)

`elilo-3.16` is fully packaged in `01-Core64.lzm`. Single text config file `elilo.conf`. Old but works.

- [ ] Task F2.B1. Use `01-Core64.lzm/usr/sbin/eliloconfig` as a reference for proper config generation.
- [ ] Task F2.B2. Install `elilo.efi` to the ESP at `\EFI\nimblex\elilo.efi`, write `elilo.conf` next to it, register with `efibootmgr`.
- [ ] Task F2.B3. Trade-off: provides a minimal text menu; well-tested; no Slackware rebuild needed. Cons: ELILO is essentially unmaintained upstream since ~2013.

### Option C — Ship `systemd-bootx64.efi` as a binary asset

Extract `systemd-bootx64.efi` (~150 KB) from upstream Arch/Fedora systemd packages, ship in our installer bundle at `/usr/lib/systemd/boot/efi/systemd-bootx64.efi`. After this, `bootctl install` works.

- [ ] Task F2.C1. Extract the EFI binary from a known-good upstream package (Arch's `systemd` is the simplest source; the EFI binary is GPL-licensed and freely redistributable).
- [ ] Task F2.C2. Place it at the path systemd-258 expects (`/usr/lib/systemd/boot/efi/systemd-bootx64.efi`) inside the new `03-Installer64.lzm` bundle.
- [ ] Task F2.C3. Trade-off: best UX (graphical-ish menu, atomic `bootctl install`), but ships a third-party-built binary; need to track upstream versions and Secure Boot signatures separately.

### Option D — Rebuild Slackware systemd with `-Defi=true`

Rejected. Touches a core component for one feature; not justified.

### Recommended choice and fallback

- [ ] Task F2.6. Adopt **Option A (EFI stub)** as the default UEFI path. Use it for both USB and internal-alongside-Windows installs.
- [ ] Task F2.7. Keep **Option B (ELILO)** wired in as a secondary path, automatically selected if `efibootmgr` fails to register a stub-boot entry on a quirky firmware. ELILO requires no extra packaging, so this is essentially free.
- [ ] Task F2.8. Update all references to `systemd-boot`, `bootctl`, and `loader/entries/nimblex.conf` in feedback-1 Tasks B.2, B.4, A.3, A.4 to instead refer to the EFI stub path. **Delete Task A.3 (package systemd-boot)** and **Task A.4 (rEFInd)** from feedback-1 — neither is needed under the new strategy.

## Blocker 3 — Shim / Secure Boot Strategy

`shim-signed-1.47` ships only the signed shim binary; `mmx64.efi` (MokManager) is missing. Without MokManager, only Microsoft-CA-signed second-stage loaders will boot under Secure Boot.

- [ ] Task F2.9. **Default v1 stance**: do **not** attempt Secure Boot support. Detect SB via `/sys/firmware/efi/efivars/SecureBoot-*`; if enabled, surface a polite one-screen notice on Screen 3: "Secure Boot is on. To install Nimblex, please disable Secure Boot in your firmware setup, or continue and we'll install in non-Secure-Boot mode for this entry only." Provide a "Show me how" link that opens a docs URL placeholder.
- [ ] Task F2.10. **v1.1 / opt-in path**: package `mmx64.efi` either by (a) extracting from an upstream `shim-signed` Debian package, or (b) building `shim` from source with `make BOOTGUID=…`. Combined with a self-signed Nimblex EFI stub kernel, this enables proper SB support. Track as `feedback-2/B6-secure-boot` follow-up; not blocking v1.
- [ ] Task F2.11. Remove the references to "shim → bootloader chain" and "Microsoft-CA chain" from feedback-1 Tasks B.5 and A.8 — replaced by Task F2.9 above.

## Updated Tool Inventory (post-audit)

| Tool | Bundle (final target) | Status | v1 Use |
|---|---|---|---|
| `os-prober` | `01-Core64.lzm` | ✅ already there | Windows detection (Task 1.1) |
| `chntpw` | `03-Installer64.lzm` (new) | ⚠️ to be bundled (F2.1) | Fast Startup auto-disable (feedback-1 A.2) |
| `f3` | `03-Installer64.lzm` (new) | ⚠️ to be bundled (F2.1) | USB fake-capacity probe (feedback-1 A.5) |
| `pv` | `03-Installer64.lzm` (new) | ⚠️ to be bundled (F2.1) | Copy progress (feedback-1 A.6) |
| `shim-signed` | `03-Installer64.lzm` (new), v1.1 only | ⚠️ to be bundled, dedup first (F2.2); incomplete (no mmx64.efi) | Deferred per F2.9 |
| `systemd-boot` | — | ❌ broken; not used | Replaced by EFI stub (Option A) |
| `efibootmgr-18` | `01-Core64.lzm` | ✅ already there | Boot entry registration |
| `elilo-3.16` | `01-Core64.lzm` | ✅ already there | Fallback bootloader (Option B) |
| `syslinux-4.07` | `01-Core64.lzm` | ✅ already there | BIOS bootloader (USB + internal BIOS) |
| `ntfs-3g` (incl. `ntfsresize`, `ntfsfix`) | `01-Core64.lzm` | ✅ already there | NTFS shrink / fix |
| `parted`, `gptfdisk`, `util-linux` | `01-Core64.lzm` | ✅ already there | Partitioning |

## Verification Criteria (additions)

- After `03-Installer64.lzm` is built, mounting it on a fresh Nimblex live boot makes `chntpw`, `f3probe`, `f3write`, `pv` available in `$PATH` and their package manifests visible under the bundle's `var/lib/pkgtools/packages/`.
- `pkgtool` lists exactly **one** `shim-signed` package (no `_SBo` duplicate) after F2.2.
- The installer's bootloader path runs `efibootmgr -c …` directly with kernel-as-EFI-stub on UEFI test images; no `bootctl` invocation appears anywhere in the helper's command allowlist.
- On a Secure-Boot-enabled test machine, the installer detects SB and shows the F2.9 notice instead of silently failing.
- `ldd` of the installer GUI + helper references only sonames present in `01-Core64.lzm`, `02-Xorg64.lzm`, and `03-Installer64.lzm` (CI gate updated per F2.4).

## Potential Risks and Mitigations (additions)

1. **EFI stub boot fails on quirky firmware** (e.g. some Lenovo/HP laptops drop entries that have spaces or backslashes in `OptionalData`).
   Mitigation: F2.7's automatic ELILO fallback; emit a diagnostic if `efibootmgr -v` post-write does not show our entry.

2. **Kernel cmdline encoding bugs** when passing `initrd=...` via `efibootmgr -u`.
   Mitigation: keep paths ASCII, test against a known-quirky firmware image (OVMF + downstream patches), document the encoding in a code comment.

3. **`03-Installer64.lzm` bundle ordering** — if loaded before `01-Core64.lzm` in some builds, libraries it depends on (e.g. glibc) won't be available.
   Mitigation: numeric prefix `03-` ensures aufs ordering after Core/Xorg; verify by reading `/proc/mounts` order on a test boot.

4. **User installs without realizing Secure Boot blocks the result** (under F2.9 default).
   Mitigation: the F2.9 notice is mandatory and modal on Screen 3 when SB is detected; cannot be dismissed without an explicit "I'll disable Secure Boot in firmware" acknowledgement.

5. **Future systemd update overwrites the shipped `systemd-bootx64.efi`** if we later adopt Option C.
   Mitigation: not relevant under recommended Option A; if Option C is ever taken, pin via package post-install script.

## Alternative Approaches (additions)

1. **Use GRUB from `07-Devel64.lzm`**. Trade-off: bumps the dependency surface from 2 bundles to 3; violates the "01-Core + 02-Xorg only" constraint set by the user; rejected.

2. **Rebuild systemd with EFI support enabled, then proceed as in feedback-1**. Trade-off: clean long-term answer, but touches a core service for a single feature; rejected for v1, can be done independently of the installer work.

3. **Build a minimal custom EFI menu** (~10 KB EDK2-based). Trade-off: total control, but adds a build dependency on EDK2 and a new attack surface; rejected — EFI stub Option A already eliminates the need for any menu at install time.
