use std::path::PathBuf;
fn main() {
    let dev: PathBuf = std::env::args().nth(1).expect("usage: probe-ntfs <device>").into();
    let t = std::time::Instant::now();
    match installer_core::ntfs_raw::probe_used(&dev) {
        Some(b) => println!("used = {} bytes ({}) in {:?}", b.0, b, t.elapsed()),
        None => println!("probe failed"),
    }
}
