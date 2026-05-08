fn main() {
    use installer_core::{Bootloader, Bytes, Disk, InstallPlanner, TableType};
    let usb = Disk {
        path: "/dev/sdb".into(), size: Bytes::from_gib(16),
        removable: true, model: "Test Stick".into(), transport: "usb".into(),
        table_type: TableType::Mbr, partitions: vec![],
    };
    let plan = InstallPlanner::plan_usb(&usb, Bootloader::SystemdBoot).unwrap();
    println!("{}", serde_json::to_string(&plan).unwrap());
}
