//! The drainscope sampler: RAPL counters over D-Bus for authorized callers only.

pub mod accumulator;
pub mod service;

pub use service::{Config, CounterSource, LastCall, Powercap, Sampler};
