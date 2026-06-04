//! Debug helper: dump what the GUI sees (scan_with_usage) AND the exact mode
//! its destination screen would pick, plus the resulting plan transcript.
use installer_core::{
    live::LiveMedia, resize::min_install, Bootloader, Bytes, DiskScanner, InstallMode,
    InstallPlanner,
};

fn main() {
    let live = LiveMedia::detect();
    println!("LIVE MEDIA: {:?}", live);
    let disks = DiskScanner::scan_with_usage().expect("scan failed");
    for d in &disks {
        println!(
            "DISK {} size={} removable={} has_windows={} existing_nimblex={:?}",
            d.path.display(),
            d.size,
            d.removable,
            d.has_windows(),
            d.existing_nimblex_partition()
                .map(|p| p.path.display().to_string()),
        );
        for p in &d.partitions {
            println!(
                "  p{} role={:?} fs={:?} label={:?} start={} size={} used={:?}",
                p.number,
                p.role,
                p.fs,
                p.label,
                p.start.0,
                p.size.0,
                p.used.map(|b| b.0)
            );
        }
        if let Some(g) = d.largest_free_gap() {
            println!(
                "  largest_free_gap: start={} size={} after_number={:?}",
                g.start, g.size, g.after_number
            );
        }

        // Replicate screen_destination::refresh_layout mode decision exactly.
        let mode = if d.existing_nimblex_partition().is_some() {
            InstallMode::ReuseNimblex
        } else if d.has_windows() && !d.removable {
            let win_is_bitlocker = d
                .primary_windows_partition()
                .map(|p| p.fs.eq_ignore_ascii_case("bitlocker"))
                .unwrap_or(false);
            let largest_gap = d.largest_free_gap().map(|g| g.size).unwrap_or(0);
            if win_is_bitlocker && Bytes(largest_gap) >= min_install() {
                InstallMode::FreeSpace
            } else {
                InstallMode::AlongsideWindows
            }
        } else {
            InstallMode::EraseWholeDisk
        };
        println!(
            "  >>> GUI MODE = {:?}  (min_install={})",
            mode,
            min_install()
        );

        match InstallPlanner::plan_for(d, mode, None, Bootloader::Auto, false, false) {
            Ok(plan) => {
                println!("  OVERLAY SUMMARY: {}", plan.summary_one_line());
                println!("  --- plan transcript ---");
                for line in plan.shell_transcript().lines() {
                    println!("    {}", line);
                }
            }
            Err(e) => println!("  plan error: {}", e),
        }
        println!();
    }
}
