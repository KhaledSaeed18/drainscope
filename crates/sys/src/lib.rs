//! System integration: sysfs, procfs and cgroupfs readers and D-Bus clients.
//!
//! Every reader takes a [`SysRoot`], so tests run against fixture trees laid out like `/`.

pub mod cgroup;
pub mod drm;
pub mod error;
pub mod power_supply;
pub mod powercap;
pub mod process;
pub mod root;

pub use cgroup::{read_cpu_usage, terminal_labels};
pub use drm::DrmScanner;
pub use error::SysError;
pub use power_supply::read_batteries;
pub use powercap::{PowercapZone, read_zones};
pub use root::SysRoot;
