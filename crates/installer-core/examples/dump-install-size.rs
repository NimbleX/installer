fn main() {
    let v = installer_core::install_size::min_install_size();
    let r = installer_core::resize::min_reclaim();
    println!("min_install_size  = {} ({} bytes)", v, v.0);
    println!("min_reclaim       = {} ({} bytes)", r, r.0);
}
