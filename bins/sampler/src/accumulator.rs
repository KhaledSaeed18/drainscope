//! Turns raw, wrapping powercap counters into per-domain totals that only grow.

use std::collections::BTreeMap;

use drainscope_model::delta::counter_delta;
use drainscope_model::snapshot::{EnergyCounter, RaplDomain};
use drainscope_sys::PowercapZone;

#[derive(Debug, Default)]
pub struct Accumulator {
    last: BTreeMap<String, EnergyCounter>,
    totals: BTreeMap<RaplDomain, u64>,
}

impl Accumulator {
    /// Adds each zone's energy since its previous reading and returns the per-domain totals
    /// (microjoules since the first reading). Zones of the same domain (multiple packages) are
    /// summed. A counter that reset rather than wrapped restarts from its new value.
    pub fn update(&mut self, zones: &[PowercapZone]) -> &BTreeMap<RaplDomain, u64> {
        for zone in zones {
            let total = self.totals.entry(zone.domain).or_default();
            if let Some(previous) = self.last.get(&zone.zone)
                && let Some(delta) = counter_delta(*previous, zone.counter)
            {
                *total = total.saturating_add(delta.0);
            }
            self.last.insert(zone.zone.clone(), zone.counter);
        }
        &self.totals
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use drainscope_model::Microjoules;

    fn zone(name: &str, domain: RaplDomain, value: u64) -> PowercapZone {
        PowercapZone {
            zone: name.into(),
            domain,
            counter: EnergyCounter {
                value: Microjoules(value),
                range: Microjoules(1_000),
            },
        }
    }

    #[test]
    fn totals_start_at_zero_and_survive_wraparound() {
        let mut acc = Accumulator::default();
        let first = acc
            .update(&[zone("intel-rapl:0", RaplDomain::Package, 900)])
            .clone();
        assert_eq!(first, BTreeMap::from([(RaplDomain::Package, 0)]));
        acc.update(&[zone("intel-rapl:0", RaplDomain::Package, 950)]);
        let wrapped = acc.update(&[zone("intel-rapl:0", RaplDomain::Package, 20)]);
        assert_eq!(wrapped[&RaplDomain::Package], 50 + 70);
    }

    #[test]
    fn packages_of_one_domain_are_summed() {
        let mut acc = Accumulator::default();
        acc.update(&[
            zone("intel-rapl:0", RaplDomain::Package, 0),
            zone("intel-rapl:1", RaplDomain::Package, 0),
        ]);
        let totals = acc.update(&[
            zone("intel-rapl:0", RaplDomain::Package, 10),
            zone("intel-rapl:1", RaplDomain::Package, 5),
        ]);
        assert_eq!(totals[&RaplDomain::Package], 15);
    }
}
