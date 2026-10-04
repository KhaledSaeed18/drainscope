//! What the daemon has learned about the machine: idle-floor histograms per power source and
//! the `psys` plausibility counts.
//!
//! Histograms are stored sparsely as little-endian `(bin: u16, count: u32)` pairs.

use std::collections::BTreeMap;

use drainscope_model::{FloorEstimator, Part, PowerHistogram, PowerSource, PsysCheck};
use rusqlite::{OptionalExtension, params};

use crate::{Store, StoreError};

/// The `part` value for the battery-level histogram.
const BATTERY: &str = "battery";
const PAIR_BYTES: usize = 6;

fn encode(histogram: &PowerHistogram) -> Result<Vec<u8>, StoreError> {
    let mut bytes = Vec::new();
    for (bin, count) in histogram.nonzero_bins() {
        let bin =
            u16::try_from(bin).map_err(|_| StoreError::Corrupt(format!("histogram bin {bin}")))?;
        bytes.extend_from_slice(&bin.to_le_bytes());
        bytes.extend_from_slice(&count.to_le_bytes());
    }
    Ok(bytes)
}

fn decode(bytes: &[u8]) -> Option<PowerHistogram> {
    let (pairs, rest) = bytes.as_chunks::<PAIR_BYTES>();
    if !rest.is_empty() {
        return None;
    }
    PowerHistogram::from_bins(pairs.iter().map(|&[b0, b1, c0, c1, c2, c3]| {
        (
            usize::from(u16::from_le_bytes([b0, b1])),
            u32::from_le_bytes([c0, c1, c2, c3]),
        )
    }))
}

impl Store {
    /// Replaces the stored idle-floor histograms for `source`.
    ///
    /// # Errors
    /// SQLite errors.
    pub fn save_floor(
        &mut self,
        source: PowerSource,
        estimator: &FloorEstimator,
    ) -> Result<(), StoreError> {
        let tx = self.conn.transaction()?;
        tx.execute(
            "DELETE FROM calibration WHERE power_source = ?1",
            params![source.wire_name()],
        )?;
        let histograms = estimator
            .part_histograms()
            .map(|(part, h)| (part.wire_name(), h))
            .chain(std::iter::once((BATTERY, estimator.battery_histogram())));
        for (part, histogram) in histograms {
            tx.execute(
                "INSERT INTO calibration (power_source, part, histogram) VALUES (?1, ?2, ?3)",
                params![source.wire_name(), part, encode(histogram)?],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    /// The stored estimator for `source`; empty if nothing was learned yet.
    ///
    /// # Errors
    /// SQLite errors or undecodable histograms.
    pub fn load_floor(&self, source: PowerSource) -> Result<FloorEstimator, StoreError> {
        let mut statement = self
            .conn
            .prepare_cached("SELECT part, histogram FROM calibration WHERE power_source = ?1")?;
        let rows = statement.query_map(params![source.wire_name()], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, Vec<u8>>(1)?))
        })?;
        let mut parts = BTreeMap::new();
        let mut battery = PowerHistogram::default();
        for row in rows {
            let (part, bytes) = row?;
            let histogram = decode(&bytes)
                .ok_or_else(|| StoreError::Corrupt(format!("histogram for {part}")))?;
            if part == BATTERY {
                battery = histogram;
            } else {
                let part = Part::from_wire_name(&part)
                    .ok_or_else(|| StoreError::Corrupt(format!("calibration part {part:?}")))?;
                parts.insert(part, histogram);
            }
        }
        Ok(FloorEstimator::from_histograms(parts, battery))
    }

    /// # Errors
    /// SQLite errors.
    pub fn save_psys_check(&self, check: &PsysCheck) -> Result<(), StoreError> {
        let (at_least, below) = check.counts();
        self.conn.execute(
            "INSERT INTO psys_check (id, at_least_package, below_package) VALUES (1, ?1, ?2)
             ON CONFLICT (id) DO UPDATE SET
               at_least_package = excluded.at_least_package, below_package = excluded.below_package",
            params![at_least, below],
        )?;
        Ok(())
    }

    /// # Errors
    /// SQLite errors.
    pub fn load_psys_check(&self) -> Result<PsysCheck, StoreError> {
        let counts = self
            .conn
            .query_row(
                "SELECT at_least_package, below_package FROM psys_check WHERE id = 1",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?;
        Ok(counts.map_or_else(PsysCheck::default, |(at_least, below)| {
            PsysCheck::from_counts(at_least, below)
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use drainscope_model::Watts;

    fn histogram(watts: &[f64]) -> PowerHistogram {
        let mut h = PowerHistogram::default();
        for w in watts {
            h.add(Watts(*w));
        }
        h
    }

    #[test]
    fn histograms_round_trip_sparsely() {
        let h = histogram(&[0.2, 0.2, 1.5, 19.99, 25.0]);
        let bytes = encode(&h).unwrap();
        assert_eq!(bytes.len(), 4 * PAIR_BYTES); // four distinct bins
        assert_eq!(decode(&bytes), Some(h));
        assert_eq!(decode(&bytes[..5]), None);
    }

    #[test]
    fn floors_are_stored_per_power_source() {
        let mut store = Store::open_in_memory().unwrap();
        let estimator = FloorEstimator::from_histograms(
            BTreeMap::from([
                (Part::Core, histogram(&[0.19; 40])),
                (Part::Dram, histogram(&[0.37; 40])),
            ]),
            histogram(&[5.1; 40]),
        );
        store.save_floor(PowerSource::Battery, &estimator).unwrap();
        assert_eq!(store.load_floor(PowerSource::Battery).unwrap(), estimator);
        assert_eq!(
            store.load_floor(PowerSource::Ac).unwrap(),
            FloorEstimator::default()
        );

        // Saving again replaces rather than duplicates.
        store
            .save_floor(PowerSource::Battery, &FloorEstimator::default())
            .unwrap();
        assert_eq!(
            store.load_floor(PowerSource::Battery).unwrap(),
            FloorEstimator::default()
        );
    }

    #[test]
    fn psys_counts_persist() {
        let store = Store::open_in_memory().unwrap();
        assert_eq!(store.load_psys_check().unwrap(), PsysCheck::default());
        store
            .save_psys_check(&PsysCheck::from_counts(2, 57))
            .unwrap();
        store
            .save_psys_check(&PsysCheck::from_counts(3, 60))
            .unwrap();
        assert_eq!(store.load_psys_check().unwrap().counts(), (3, 60));
    }
}
