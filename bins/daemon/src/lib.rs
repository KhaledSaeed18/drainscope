//! The drainscope user daemon: collects readings, attributes energy, stores history, and
//! serves `Monitor1` on the session bus.

pub mod collector;
pub mod engine;
pub mod monitor;
pub mod power;
pub mod rapl;
