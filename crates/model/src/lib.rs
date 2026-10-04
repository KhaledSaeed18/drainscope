//! Core domain: types, identity rules and the attribution model. Performs no I/O.

pub mod activity;
pub mod attribution;
pub mod calibration;
pub mod cgroup;
pub mod consumer;
pub mod delta;
pub mod snapshot;
pub mod unit_name;
pub mod units;
pub mod window;

pub use activity::{Activity, Resolver};
pub use attribution::{EnergySplit, Ledger, MODEL_VERSION, TickAttribution, attribute};
pub use calibration::{FloorEstimator, IdleFloor, Part, PowerHistogram, PsysCheck, PsysVerdict};
pub use cgroup::{CgroupIdentity, CgroupPath, classify};
pub use consumer::{ConsumerKey, ParseConsumerKeyError};
pub use delta::{CpuDelta, DiffError, IntervalDelta, diff};
pub use snapshot::{RaplDomain, Snapshot};
pub use units::{Joules, Microjoules, Watts};
pub use window::{ClosedWindow, Measurement, PowerSource, Window};
