//! Where idle-exit counts come from: the eBPF program in production, fixtures in tests.

use std::collections::BTreeMap;
use std::sync::{Mutex, PoisonError};

use anyhow::Context;
use aya::maps::HashMap;
use aya::programs::BtfTracePoint;
use aya::{Btf, Ebpf, include_bytes_aligned};

const OBJECT: &[u8] = include_bytes_aligned!(concat!(env!("OUT_DIR"), "/wakeups.bpf.o"));

pub trait WakeupSource: Send + Sync {
    /// Cumulative idle exits by cgroup ID since the source started.
    ///
    /// # Errors
    /// Reading the counters failed.
    fn read(&self) -> anyhow::Result<BTreeMap<u64, u64>>;
}

/// `bpf/wakeups.bpf.c`, attached to the `sched_switch` tracepoint for as long as this lives.
pub struct Bpf {
    ebpf: Mutex<Ebpf>,
}

impl Bpf {
    /// Loads and attaches the program. Needs `CAP_BPF` and `CAP_PERFMON`.
    ///
    /// # Errors
    /// If the kernel lacks BTF or refuses the program.
    pub fn load() -> anyhow::Result<Self> {
        let mut ebpf = Ebpf::load(OBJECT).context("loading the eBPF object")?;
        let btf = Btf::from_sys_fs().context("reading kernel BTF")?;
        let program: &mut BtfTracePoint = ebpf
            .program_mut("count_idle_exits")
            .context("program count_idle_exits is missing")?
            .try_into()?;
        program
            .load("sched_switch", &btf)
            .context("loading count_idle_exits")?;
        program.attach().context("attaching to sched_switch")?;
        Ok(Self {
            ebpf: Mutex::new(ebpf),
        })
    }
}

impl WakeupSource for Bpf {
    fn read(&self) -> anyhow::Result<BTreeMap<u64, u64>> {
        let ebpf = self.ebpf.lock().unwrap_or_else(PoisonError::into_inner);
        let map: HashMap<_, u64, u64> =
            HashMap::try_from(ebpf.map("wakeups").context("map wakeups is missing")?)?;
        // Entries evicted between listing and lookup fail; they're skipped.
        Ok(map.iter().flatten().collect())
    }
}
