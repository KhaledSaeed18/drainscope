//! Core domain: types, identity rules and the attribution model. Performs no I/O.

pub mod cgroup;
pub mod consumer;
pub mod unit_name;
pub mod units;

pub use cgroup::{CgroupIdentity, CgroupPath, classify};
pub use consumer::{ConsumerKey, ParseConsumerKeyError};
pub use units::{Joules, Microjoules, Watts};
