//! The drainscope user daemon: collects readings, attributes energy, stores history, and
//! serves `Monitor1` on the session bus.

pub mod collector;
pub mod database;
pub mod engine;
pub mod lock;
pub mod monitor;
pub mod power;
pub mod probe;
pub mod rapl;
pub mod ticker;
