//! Core domain model for the Nimblex installer.
//!
//! This crate is UI-agnostic and side-effect-free except for read-only
//! disk probing through external commands. The GUI consumes the types
//! and planners exposed here; the helper consumes [`Plan`] over JSON.

pub mod bootloader;
pub mod disk;
pub mod install_size;
pub mod ntfs_raw;
pub mod plan;
pub mod planner;
pub mod resize;
pub mod scan;
pub mod scenario;
pub mod size;
pub mod usage_probe;

pub use bootloader::{Bootloader, Firmware};
pub use disk::{Disk, Partition, PartitionRole, TableType};
pub use plan::{Plan, Step, StepCategory};
pub use planner::{InstallMode, InstallPlanner};
pub use resize::{NtfsInfo, ResizePlanner};
pub use install_size::{live_source_dirs, min_install_size};
pub use scan::DiskScanner;
pub use scenario::Scenario;
pub use size::Bytes;
