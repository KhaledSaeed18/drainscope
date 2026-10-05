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
pub mod sleep;

pub use cgroup::{cgroup_ids, read_cpu_usage, terminal_labels};
pub use drm::{DrmScanner, EngineTime, gpu_drivers};
pub use error::SysError;
pub use power_supply::read_batteries;
pub use powercap::{PowercapZone, read_zones};
pub use root::SysRoot;
pub use sleep::{
    Login1ManagerProxy, on_battery, read_mem_sleep, read_wakeup_irq, read_wakeup_sources,
};
