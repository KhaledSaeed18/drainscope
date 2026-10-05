//! The drainscope probe: kernel activity counted with eBPF, over D-Bus for authorized callers
//! only (ADR 0006).

pub mod counters;
pub mod service;
pub mod visibility;

pub use counters::{Bpf, WakeupSource};
pub use service::{Config, LastCall, Probe};
