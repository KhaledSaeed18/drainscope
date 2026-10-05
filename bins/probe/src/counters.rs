//! Where the counts come from: the eBPF programs in production, fixtures in tests.

use std::collections::BTreeMap;
use std::sync::{Mutex, PoisonError};

use anyhow::Context;
use aya::maps::{HashMap, MapData};
use aya::programs::links::CgroupAttachMode;
use aya::programs::{BtfTracePoint, CgroupSkb, CgroupSkbAttachType};
use aya::{Btf, Ebpf, include_bytes_aligned};

const WAKEUPS: &[u8] = include_bytes_aligned!(concat!(env!("OUT_DIR"), "/wakeups.bpf.o"));
const NETWORK: &[u8] = include_bytes_aligned!(concat!(env!("OUT_DIR"), "/network.bpf.o"));
const CGROUPFS: &str = "/sys/fs/cgroup";

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
        let mut ebpf = Ebpf::load(WAKEUPS).context("loading the wakeups object")?;
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
        read_counts(&ebpf, "wakeups")
    }
}

fn read_counts(ebpf: &Ebpf, name: &str) -> anyhow::Result<BTreeMap<u64, u64>> {
    let map: HashMap<&MapData, u64, u64> = HashMap::try_from(
        ebpf.map(name)
            .with_context(|| format!("map {name} is missing"))?,
    )?;
    // Entries evicted between listing and lookup fail; they're skipped.
    Ok(map.iter().flatten().collect())
}

/// Bytes received and sent, by cgroup ID, since the source started.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Traffic {
    pub received: BTreeMap<u64, u64>,
    pub sent: BTreeMap<u64, u64>,
}

pub trait NetworkSource: Send + Sync {
    /// # Errors
    /// Reading the counters failed.
    fn read(&self) -> anyhow::Result<Traffic>;
}

/// `bpf/network.bpf.c`, attached to the root cgroup (alongside other programs) for as long as
/// this lives.
pub struct NetworkBpf {
    ebpf: Mutex<Ebpf>,
}

impl NetworkBpf {
    /// Loads and attaches both programs. Needs `CAP_BPF` and `CAP_NET_ADMIN`.
    ///
    /// # Errors
    /// If the kernel refuses the programs or the attachment.
    pub fn load() -> anyhow::Result<Self> {
        let mut ebpf = Ebpf::load(NETWORK).context("loading the network object")?;
        let root = std::fs::File::open(CGROUPFS).context("opening the root cgroup")?;
        for (name, attach_type) in [
            ("count_received", CgroupSkbAttachType::Ingress),
            ("count_sent", CgroupSkbAttachType::Egress),
        ] {
            let program: &mut CgroupSkb = ebpf
                .program_mut(name)
                .with_context(|| format!("program {name} is missing"))?
                .try_into()?;
            program.load().with_context(|| format!("loading {name}"))?;
            program
                .attach(&root, attach_type, CgroupAttachMode::AllowMultiple)
                .with_context(|| format!("attaching {name} to the root cgroup"))?;
        }
        Ok(Self {
            ebpf: Mutex::new(ebpf),
        })
    }
}

impl NetworkSource for NetworkBpf {
    fn read(&self) -> anyhow::Result<Traffic> {
        let ebpf = self.ebpf.lock().unwrap_or_else(PoisonError::into_inner);
        Ok(Traffic {
            received: read_counts(&ebpf, "received")?,
            sent: read_counts(&ebpf, "sent")?,
        })
    }
}
