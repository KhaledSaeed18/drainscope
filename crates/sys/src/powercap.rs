//! RAPL energy counters from `/sys/class/powercap`. `energy_uj` is root-only (CVE-2020-8694),
//! so only the sandboxed sampler reads these; see PLAN.md's privilege model.

use std::fs;
use std::io;

use drainscope_model::Microjoules;
use drainscope_model::snapshot::{EnergyCounter, RaplDomain};

use crate::error::SysError;
use crate::root::SysRoot;

const POWERCAP: &str = "sys/class/powercap";

/// One RAPL zone, e.g. `intel-rapl:0` (package) or `intel-rapl:0:0` (core).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PowercapZone {
    pub zone: String,
    pub domain: RaplDomain,
    pub counter: EnergyCounter,
}

/// Every RAPL zone with a known domain, sorted by zone. The MMIO interface is skipped because
/// it duplicates the package zone on recent Intel parts.
///
/// # Errors
/// Any error reading a zone's counter, permission errors included: a sampler that silently
/// reads nothing would hide a misconfiguration.
pub fn read_zones(root: &SysRoot) -> Result<Vec<PowercapZone>, SysError> {
    let dir = root.path(POWERCAP);
    let entries = match fs::read_dir(&dir) {
        Ok(entries) => entries,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(err) => return Err(SysError::io(&dir, err)),
    };
    let mut zones = Vec::new();
    for entry in entries.filter_map(Result::ok) {
        let zone = entry.file_name().to_string_lossy().into_owned();
        if zone.starts_with("intel-rapl-mmio") {
            continue;
        }
        let path = entry.path();
        // Control-type directories (`intel-rapl`) have no name or counter.
        let Some(name) = root.read_optional(&path.join("name"))? else {
            continue;
        };
        let Some(domain) = RaplDomain::from_zone_name(&name) else {
            continue;
        };
        let value = read_counter(&path.join("energy_uj"))?;
        let range = read_counter(&path.join("max_energy_range_uj"))?;
        zones.push(PowercapZone {
            zone,
            domain,
            counter: EnergyCounter { value, range },
        });
    }
    zones.sort_by(|a, b| a.zone.cmp(&b.zone));
    Ok(zones)
}

fn read_counter(path: &std::path::Path) -> Result<Microjoules, SysError> {
    let text = fs::read_to_string(path).map_err(|e| SysError::io(path, e))?;
    text.trim()
        .parse()
        .map(Microjoules)
        .map_err(|_| SysError::parse(path, format!("not a counter: {:?}", text.trim())))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn zone(root: &SysRoot, zone: &str, name: &str, energy: u64) {
        let dir = root.path(&format!("{POWERCAP}/{zone}"));
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("name"), format!("{name}\n")).unwrap();
        fs::write(dir.join("energy_uj"), format!("{energy}\n")).unwrap();
        fs::write(dir.join("max_energy_range_uj"), "262143328850\n").unwrap();
    }

    #[test]
    fn reads_rapl_zones() {
        let dir = tempfile::tempdir().unwrap();
        let root = SysRoot::at(dir.path());
        fs::create_dir_all(root.path(&format!("{POWERCAP}/intel-rapl"))).unwrap();
        zone(&root, "intel-rapl:0", "package-0", 1_000);
        zone(&root, "intel-rapl:0:0", "core", 400);
        zone(&root, "intel-rapl:1", "psys", 9_000);
        zone(&root, "intel-rapl-mmio:0", "package-0", 1_000);
        zone(&root, "intel-rapl:0:3", "something-new", 5);

        let zones = read_zones(&root).unwrap();
        let summary: Vec<(&str, RaplDomain, u64)> = zones
            .iter()
            .map(|z| (z.zone.as_str(), z.domain, z.counter.value.0))
            .collect();
        assert_eq!(
            summary,
            [
                ("intel-rapl:0", RaplDomain::Package, 1_000),
                ("intel-rapl:0:0", RaplDomain::Core, 400),
                ("intel-rapl:1", RaplDomain::Psys, 9_000),
            ]
        );
        assert_eq!(zones[0].counter.range, Microjoules(262_143_328_850));
    }

    #[test]
    fn reads_amd_zones() {
        // AMD Zen through intel_rapl_msr: package and core only, with a smaller range.
        let dir = tempfile::tempdir().unwrap();
        let root = SysRoot::at(dir.path());
        zone(&root, "intel-rapl:0", "package-0", 70_000);
        zone(&root, "intel-rapl:0:0", "core", 30_000);
        let domains: Vec<RaplDomain> = read_zones(&root)
            .unwrap()
            .iter()
            .map(|z| z.domain)
            .collect();
        assert_eq!(domains, [RaplDomain::Package, RaplDomain::Core]);
    }

    #[test]
    fn no_powercap_means_no_zones() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(read_zones(&SysRoot::at(dir.path())).unwrap(), Vec::new());
    }
}
