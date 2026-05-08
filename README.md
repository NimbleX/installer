# Nimblex Installer

A small, three-screen, safe-by-default graphical installer for
[NimbleX Linux](https://nimblex.net), written in Rust + GTK4.

The design splits cleanly into:

- An **unprivileged GUI** that shows the user three screens (welcome,
  disk picker, confirm) and never touches block devices itself.
- A **privileged helper** that receives a JSON `Plan` on stdin, validates
  every command against an allowlist, and either prints exactly what it
  would run (`--dry-run`) or executes the plan and streams JSON events
  back.
- A pure-logic **core** crate that scans disks, computes installation
  size, validates resize plans, and produces `Plan`s — fully free of any
  side effects so it can be unit-tested without touching the system.

Anything off the allowlist is rejected before any side effect, so the
"Show commands" button in the GUI displays byte-identical text to what
would actually run.

## Repository layout

| Path | Purpose |
| --- | --- |
| `crates/installer-core` | UI-agnostic types and planners (`Disk`, `Plan`, `DiskScanner`, `ResizePlanner`, `InstallPlanner`). Pure logic + read-only probes. |
| `crates/installer-helper` | Privileged executor. Reads a `Plan` JSON on stdin, validates argv against an allowlist (`allowlist.rs`), runs steps, streams JSON events. Has `--dry-run` so the GUI's "Show commands" output is byte-identical to what would run. |
| `crates/installer-gui` | GTK4 frontend. Three screens (`screens/screen1_welcome.rs`, `screen2_picker.rs`, `screen3_confirm.rs`), one custom widget (`widgets/disk_strip.rs`). |
| `packaging/install.sh` | Stages a release build into `usr/bin/`, `usr/libexec/`, `usr/share/applications/`, `usr/share/polkit-1/actions/` for inclusion in a NimbleX live bundle. |
| `packaging/polkit/` | `org.nimblex.installer.policy` — the polkit action invoked by `pkexec` to elevate the helper. |
| `packaging/share/applications/` | Desktop entry. |
| `plans/` | Strategic Markdown design documents. |

## Build

Requires Rust ≥ 1.85, GTK4 development headers, polkit, GRUB or
systemd-boot, and `parted`/`ntfsresize`/`mkfs.fat`/`mkfs.ext4` at runtime.

```sh
cargo build --release
```

Smoke-launch the GUI:

```sh
./target/release/nimblex-installer
```

End-to-end dry run on a synthetic USB plan:

```sh
cargo run -q --example dump-usb-plan -p installer-core \
  | ./target/release/nimblex-installer-helper --dry-run
```

Stage for inclusion in a NimbleX live bundle:

```sh
STAGE=./stage packaging/install.sh
```

## Boot strategy

Two UEFI bootloader backends, selectable on the command line:

```sh
nimblex-installer --bootloader auto          # default: systemd-boot on UEFI, GRUB on BIOS-only
nimblex-installer --bootloader systemd-boot  # force systemd-boot (UEFI only)
nimblex-installer --bootloader grub          # force GRUB (UEFI standalone PE, or BIOS i386-pc)
```

| Scenario | Default loader (`auto`) | With `--bootloader grub` |
| --- | --- | --- |
| USB, UEFI | systemd-boot at `\EFI\BOOT\BOOTX64.EFI` + `loader/entries/*.conf` | GRUB standalone PE at `\EFI\BOOT\BOOTX64.EFI` + `boot/grub/grub.cfg` |
| USB, BIOS | syslinux on protective MBR + ext4 PBR | syslinux on protective MBR + ext4 PBR (GRUB UEFI side too if hybrid) |
| Internal alongside Windows, UEFI | systemd-boot on the shared ESP + `loader/entries/nimblex*.conf` (auto-detects Windows Boot Manager via `auto-firmware`) | GRUB on the shared ESP + `grub.cfg` chainloading Windows via `search --fs-uuid` |
| Internal alongside Windows, BIOS | GRUB i386-pc (systemd-boot is UEFI-only, falls back) | GRUB i386-pc |

Each backend writes three NimbleX menu entries:

- **NimbleX** — graphical (default).
- **NimbleX (CLI)** — appends the `cli` token (and `intel_iommu=off`)
  to the kernel cmdline; the live-init's `cli_or_gui()` re-targets
  `default.target` to `multi-user.target` for a console-only boot.
- **NimbleX (rescue)** — `nomodeset systemd.unit=rescue.target`. Boots
  straight into a single-user root shell on the framebuffer console;
  useful when the GPU driver hangs early or when multi-user fails.

Windows is auto-detected on every connected disk by mounting each FAT
partition read-only and probing for `EFI/Microsoft/Boot/bootmgfw.efi`,
then added as a menu entry. GRUB chainloads it directly via
`search --fs-uuid`; systemd-boot relies on its `auto-firmware yes`
fallback.

## Filesystems

| Use | Filesystem |
| --- | --- |
| ESP | FAT32 (label `NIMBLEX_ESP` on USB installs, anchors GRUB `$root`) |
| NimbleX root | ext4 (label `NIMBLEX_ROOT` on USB installs) |
| Persistence | directory on the ext4 root |
| Swap | swapfile on ext4 (default 2 GiB; skipped if RAM ≥ 16 GB) |
| Windows | NTFS, only ever shrunk via `ntfsresize`, never reformatted |

## Safety guarantees

- The unprivileged GUI cannot run any partitioning or filesystem command
  directly. Everything goes through the helper.
- The helper rejects any argv not on its allowlist before spawning a
  process. New commands require an explicit code change.
- Windows fast-startup is detected before any resize so we never touch a
  hibernated NTFS volume; the GUI surfaces a clear "boot Windows once
  with Shut down" message in that case.
- `--dry-run` prints the exact argv that would be executed, in order.

## License

GPL-3.0-or-later.
