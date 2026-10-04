//! Core domain: types, identity rules and the attribution model. Performs no I/O.

pub mod cgroup;
pub mod consumer;
pub mod delta;
pub mod snapshot;
pub mod unit_name;
pub mod units;

pub use cgroup::{CgroupIdentity, CgroupPath, classify};
pub use consumer::{ConsumerKey, ParseConsumerKeyError};
pub use delta::{CpuDelta, DiffError, IntervalDelta, diff};
pub use snapshot::{RaplDomain, Snapshot};
pub use units::{Joules, Microjoules, Watts};
