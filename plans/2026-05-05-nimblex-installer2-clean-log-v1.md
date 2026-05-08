# Nimblex Installer — Clean Log Output & ext4 64-bit Fix

## Objective

Three focused improvements:
1. Suppress expected-but-scary output from `partx`/`blockdev`/`udevadm` in the `settle-partitions` step.
2. Filter residual sgdisk/udev noise in the GUI log so users see only meaningful messages.
3. Add `-O 64bit` to both `mkfs.ext4` invocations to eliminate the `64-bit filesystem support is not enabled` warning.

## Implementation Plan

- [ ] Task 1. **Add `run_cmd_silent` to `internal.rs`**

  Insert the following function directly after the existing `run_cmd_ok` function
  (`crates/installer-helper/src/internal.rs` line ~627):

  ```rust
  /// Run a command, discarding both stdout and stderr. Returns whether the
  /// command exited successfully. Use for tools that are noisy on expected
  /// failures (e.g. partx, blockdev, udevadm with stale config warnings).
  fn run_cmd_silent(argv: &[&str]) -> bool {
      Command::new(argv[0])
          .args(&argv[1..])
          .stdout(std::process::Stdio::null())
          .stderr(std::process::Stdio::null())
          .status()
          .map(|s| s.success())
          .unwrap_or(false)
  }
  ```

- [ ] Task 2. **Switch `cmd_settle_partitions` to `run_cmd_silent`**

  In `crates/installer-helper/src/internal.rs`, in `cmd_settle_partitions`:
  - Change the `partx --add` call from `run_cmd_ok` → `run_cmd_silent`
  - Change the `blockdev --rereadpt` call from `run_cmd_ok` → `run_cmd_silent`
  - Change both `udevadm settle` calls from `run_cmd_ok` → `run_cmd_silent`
  - Remove the `println!("partx succeeded.")` and `println!("partx failed; trying blockdev --rereadpt ...")` lines — the fallback chain is an implementation detail, not user information
  - Keep the `println!("Informing kernel of new partitions on {} ...", dev)` and `println!("Partitions ready: ...")` and `println!("Unmounting stale mount: ...")` lines — those are user-meaningful

- [ ] Task 3. **Filter noise in the GUI log (`screen_install.rs`)**

  Add a private function at module level (before `impl ScreenInstall`):

  ```rust
  /// Returns true for lines that are technically correct but alarming or
  /// meaningless to end users, so we suppress them from the visible log.
  fn is_noise_line(line: &str) -> bool {
      let noise = [
          "Warning: The kernel is still using the old partition table",
          "The new table will be used at the next reboot",
          "run partprobe(8) or kpartx(8)",
          "GPT data structures destroyed",
          "/etc/udev/udev.conf:",
          "64-bit filesystem support is not enabled",
          "Pass -O 64bit to rectify",
          "Superblock backups stored on blocks:",
      ];
      noise.iter().any(|n| line.contains(n))
  }
  ```

  In `handle_event`, change:

  ```rust
  HelperEvent::Stdout { line, .. } => {
      if let Some(rest) = line.strip_prefix("PROGRESS:") { ... }
      else {
          // WAS:
          self.log.append_line(&line);
          // NEW:
          if !is_noise_line(&line) {
              self.log.append_line(&line);
          }
      }
  }
  HelperEvent::Stderr { line, .. } => {
      // WAS:
      self.log.append_line(&line);
      // NEW:
      if !is_noise_line(&line) {
          self.log.append_line(&line);
      }
  }
  ```

- [ ] Task 4. **Add `-O 64bit` to both `mkfs.ext4` steps in `planner.rs`**

  There are two `mkfs.ext4` step argv arrays in
  `crates/installer-core/src/planner.rs` (one in `plan_usb` around line 102,
  one in `plan_alongside` around line 280). In each, add `"-O".into(),
  "64bit".into(),` immediately after `"-F".into(),`:

  ```rust
  // Before:
  argv: vec!["mkfs.ext4".into(), "-F".into(), "-L".into(), "NIMBLEX_ROOT".into(), part.clone()],

  // After:
  argv: vec!["mkfs.ext4".into(), "-F".into(), "-O".into(), "64bit".into(), "-L".into(), "NIMBLEX_ROOT".into(), part.clone()],
  ```

- [ ] Task 5. **Build, test, relaunch**

  ```
  cargo build --release
  cargo test --workspace --lib
  pkill -9 -f release/nimblex-installer
  ./target/release/nimblex-installer >/tmp/nimblex.log 2>&1 &
  ```

## Verification Criteria

- `cargo build --release` is clean with no new warnings.
- `cargo test --workspace --lib` passes all existing tests.
- On a fresh install run, the log pane shows only meaningful lines:
  - `[1/7] Partition disk (GPT + EFI + Root)` → `The operation has completed successfully.`
  - `[2/7] Settle new partitions` → `Informing kernel...`, `Partitions ready:...`, `Unmounting stale mount:...` (if applicable)
  - `[3/7] Format ESP as FAT32` → `mkfs.fat 4.2 (2021-01-31)` (one line)
  - `[4/7] Format Nimblex root as ext4` → `mke2fs 1.47.4...`, `Creating filesystem...`, UUID line, inodes line — but **not** the "64-bit support not enabled" warning
- The `Superblock backups stored on blocks:` multi-line output is suppressed.
- No `/etc/udev/udev.conf:` lines appear in the log.

## Potential Risks and Mitigations

1. **`run_cmd_silent` masks a real failure** from `partx` or `blockdev`.
   Mitigation: we already have the partition-node polling loop as the authoritative success check — if nodes don't appear within 15 s, we fail with a clear error. `partx`/`blockdev` failure is always gracefully handled by the fallback chain.

2. **Noise filter hides a real error** that happens to match a noise pattern.
   Mitigation: the patterns are very specific multi-word strings unlikely to appear in genuine error messages. The full raw output is still captured in `/tmp/nimblex.log` for debugging.

3. **`-O 64bit` causes compatibility issues** on old kernels.
   Mitigation: kernel 6.19 (confirmed in the workspace) fully supports 64-bit ext4. This flag has been the default in distributions since kernel 4.x.
