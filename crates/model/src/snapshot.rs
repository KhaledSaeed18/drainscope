//! Raw readings taken at one instant. The `sys` layer fills these from sysfs, procfs and
//! cgroupfs; the model only ever sees these values.

use std::collections::BTreeMap;
use std::time::Duration;

use crate::cgroup::CgroupPath;
use crate::units::{Joules, Microjoules, Watts};

/// A RAPL energy domain (powercap `name` attribute).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum RaplDomain {
    /// The whole CPU package: cores, caches, memory controller, integrated GPU.
    Package,
    /// The CPU cores.
    Core,
    /// The rest of the package's compute, on Intel client parts the integrated GPU.
    Uncore,
    /// DRAM.
    Dram,
    /// The platform as reported by firmware. Needs a plausibility check before use.
    Psys,
}

impl RaplDomain {
    /// Parses a powercap zone name. Multi-socket `package-N` zones all map to `Package`;
    /// the reader sums them.
    #[must_use]
    pub fn from_zone_name(name: &str) -> Option<Self> {
        match name.trim() {
            "core" => Some(Self::Core),
            "uncore" => Some(Self::Uncore),
            "dram" => Some(Self::Dram),
            "psys" => Some(Self::Psys),
            other if other.starts_with("package-") => Some(Self::Package),
            _ => None,
        }
    }

    /// Name used on D-Bus and in storage.
    #[must_use]
    pub fn wire_name(self) -> &'static str {
        match self {
            Self::Package => "package",
            Self::Core => "core",
            Self::Uncore => "uncore",
            Self::Dram => "dram",
            Self::Psys => "psys",
        }
    }

    /// Inverse of [`Self::wire_name`].
    #[must_use]
    pub fn from_wire_name(name: &str) -> Option<Self> {
        [
            Self::Package,
            Self::Core,
            Self::Uncore,
            Self::Dram,
            Self::Psys,
        ]
        .into_iter()
        .find(|d| d.wire_name() == name)
    }
}

/// A cumulative energy counter and the value at which it wraps to zero.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EnergyCounter {
    pub value: Microjoules,
    pub range: Microjoules,
}

/// A process, distinguishing PID reuse by start time (`/proc/<pid>/stat` field 22).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ProcessId {
    pub pid: u32,
    pub start_ticks: u64,
}

/// One process's view of a DRM client (an open DRM file), from `/proc/<pid>/fdinfo`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DrmHolder {
    pub process: ProcessId,
    pub cgroup: CgroupPath,
}

/// A DRM client: cumulative engine busy time and every process holding it. Several
/// processes can hold the same client (fd passing, systemd's fd store).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DrmClient {
    /// Sum of all `drm-engine-*` busy times, in nanoseconds.
    pub engine_ns: u64,
    pub holders: Vec<DrmHolder>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BatteryStatus {
    Charging,
    Discharging,
    NotCharging,
    Full,
    Unknown,
}

impl BatteryStatus {
    /// Parses the `power_supply` `status` attribute.
    #[must_use]
    pub fn from_sysfs(value: &str) -> Self {
        match value.trim() {
            "Charging" => Self::Charging,
            "Discharging" => Self::Discharging,
            "Not charging" => Self::NotCharging,
            "Full" => Self::Full,
            _ => Self::Unknown,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct BatteryReading {
    /// Supply name, e.g. `BAT0`.
    pub name: String,
    pub status: BatteryStatus,
    /// Instantaneous power (`power_now`, or `current_now × voltage_now`).
    pub power: Option<Watts>,
    /// Remaining energy (`energy_now`, or `charge_now × voltage_now`).
    pub energy: Option<Joules>,
    /// Capacity when full (`energy_full`, or `charge_full × voltage_now`).
    pub energy_full: Option<Joules>,
}

/// Everything read at one instant.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Snapshot {
    /// `CLOCK_MONOTONIC` when the snapshot was taken (excludes suspend).
    pub taken_at: Duration,
    /// Empty when the sampler is unavailable.
    pub rapl: BTreeMap<RaplDomain, EnergyCounter>,
    /// Cumulative `usage_usec` from every cgroup's `cpu.stat`, the root included.
    pub cgroup_cpu_usec: BTreeMap<CgroupPath, u64>,
    /// DRM clients by `drm-client-id`.
    pub drm_clients: BTreeMap<u64, DrmClient>,
    pub batteries: Vec<BatteryReading>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_zone_names() {
        assert_eq!(
            RaplDomain::from_zone_name("package-0\n"),
            Some(RaplDomain::Package)
        );
        assert_eq!(
            RaplDomain::from_zone_name("package-1"),
            Some(RaplDomain::Package)
        );
        assert_eq!(RaplDomain::from_zone_name("psys"), Some(RaplDomain::Psys));
        assert_eq!(RaplDomain::from_zone_name("gpu"), None);
    }

    #[test]
    fn wire_names_round_trip() {
        for domain in [
            RaplDomain::Package,
            RaplDomain::Core,
            RaplDomain::Uncore,
            RaplDomain::Dram,
            RaplDomain::Psys,
        ] {
            assert_eq!(RaplDomain::from_wire_name(domain.wire_name()), Some(domain));
        }
        assert_eq!(RaplDomain::from_wire_name("package-0"), None);
    }

    #[test]
    fn parses_battery_status() {
        assert_eq!(
            BatteryStatus::from_sysfs("Not charging\n"),
            BatteryStatus::NotCharging
        );
        assert_eq!(
            BatteryStatus::from_sysfs("Discharging"),
            BatteryStatus::Discharging
        );
        assert_eq!(BatteryStatus::from_sysfs("weird"), BatteryStatus::Unknown);
    }
}
