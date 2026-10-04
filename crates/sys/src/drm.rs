//! GPU busy time per DRM client, from `/proc/<pid>/fdinfo`.
//!
//! Scanning every fd of every process each tick cost the M0 recorder 8% of a CPU (ADR 0001).
//! [`DrmScanner`] discovers a process's DRM fds once, remembers processes without any, and
//! re-reads only the known fds. A full rediscovery every [`FULL_RESCAN`] catches processes
//! that open the GPU later.

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;
use std::time::Duration;

use drainscope_model::snapshot::{DrmClient, DrmHolder, ProcessId};

use crate::error::SysError;
use crate::process::{list_pids, read_cgroup, read_stat};
use crate::root::{SysRoot, is_gone};

pub const FULL_RESCAN: Duration = Duration::from_secs(60);

const DRM_DEVICES: &str = "/dev/dri";

/// One fdinfo's DRM fields.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FdInfo {
    pub client_id: u64,
    /// Sum of `drm-engine-*` busy times (capacities excluded), nanoseconds.
    pub engine_ns: u64,
}

/// Parses DRM fields from an fdinfo; `None` if it isn't a DRM client.
#[must_use]
pub fn parse_fdinfo(text: &str) -> Option<FdInfo> {
    let mut client_id = None;
    let mut engine_ns: u64 = 0;
    for line in text.lines() {
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        let value = value.trim();
        if key == "drm-client-id" {
            client_id = value.parse().ok();
        } else if key.starts_with("drm-engine-") && !key.starts_with("drm-engine-capacity") {
            let ns = value
                .trim_end_matches("ns")
                .trim()
                .parse::<u64>()
                .unwrap_or(0);
            engine_ns = engine_ns.saturating_add(ns);
        }
    }
    Some(FdInfo {
        client_id: client_id?,
        engine_ns,
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct KnownProcess {
    start_ticks: u64,
    /// DRM fd numbers; empty for processes known not to use the GPU.
    drm_fds: Vec<String>,
}

#[derive(Debug, Default)]
pub struct DrmScanner {
    known: BTreeMap<u32, KnownProcess>,
    last_full_scan: Option<Duration>,
}

impl DrmScanner {
    /// DRM clients visible to this process at monotonic time `now`.
    ///
    /// # Errors
    /// If `/proc` can't be listed, or on unexpected I/O errors.
    pub fn scan(
        &mut self,
        root: &SysRoot,
        now: Duration,
    ) -> Result<BTreeMap<u64, DrmClient>, SysError> {
        if self
            .last_full_scan
            .is_none_or(|last| now.saturating_sub(last) >= FULL_RESCAN)
        {
            self.known.clear();
            self.last_full_scan = Some(now);
        }
        let pids = list_pids(root)?;
        self.known.retain(|pid, _| pids.binary_search(pid).is_ok());

        let mut clients: BTreeMap<u64, DrmClient> = BTreeMap::new();
        for pid in pids {
            // Known processes without DRM fds cost nothing per tick. A pid reused within
            // FULL_RESCAN by a process that opens the GPU is picked up at the next rescan.
            if self.known.get(&pid).is_some_and(|k| k.drm_fds.is_empty()) {
                continue;
            }
            let Some(stat) = read_stat(root, pid)? else {
                self.known.remove(&pid);
                continue;
            };
            let reused = self
                .known
                .get(&pid)
                .is_some_and(|k| k.start_ticks != stat.start_ticks);
            if reused || !self.known.contains_key(&pid) {
                let drm_fds = discover_drm_fds(root, pid)?;
                self.known.insert(
                    pid,
                    KnownProcess {
                        start_ticks: stat.start_ticks,
                        drm_fds,
                    },
                );
            }
            let Some(known) = self.known.get(&pid) else {
                continue;
            };
            if known.drm_fds.is_empty() {
                continue;
            }
            let fds = known.drm_fds.clone();
            let process = ProcessId {
                pid,
                start_ticks: stat.start_ticks,
            };
            if !read_process_clients(root, process, &fds, &mut clients)? {
                // An fd closed or was reused; rediscover next tick.
                self.known.remove(&pid);
            }
        }
        Ok(clients)
    }
}

/// Fd numbers of `pid` pointing into `/dev/dri`. Empty if unreadable (another user's
/// process) or gone.
fn discover_drm_fds(root: &SysRoot, pid: u32) -> Result<Vec<String>, SysError> {
    let dir = root.path(&format!("proc/{pid}/fd"));
    let entries = match fs::read_dir(&dir) {
        Ok(entries) => entries,
        Err(err) if is_gone(&err) => return Ok(Vec::new()),
        Err(err) => return Err(SysError::io(&dir, err)),
    };
    let mut fds: Vec<String> = entries
        .filter_map(Result::ok)
        .filter(|fd| fs::read_link(fd.path()).is_ok_and(|t| t.starts_with(Path::new(DRM_DEVICES))))
        .filter_map(|fd| fd.file_name().into_string().ok())
        .collect();
    fds.sort();
    Ok(fds)
}

/// Adds this process's clients to `clients`. Returns `false` if any cached fd no longer
/// reads as a DRM client.
fn read_process_clients(
    root: &SysRoot,
    process: ProcessId,
    fds: &[String],
    clients: &mut BTreeMap<u64, DrmClient>,
) -> Result<bool, SysError> {
    let Some(cgroup) = read_cgroup(root, process.pid)? else {
        return Ok(false);
    };
    let mut all_valid = true;
    for fd in fds {
        let path = root.path(&format!("proc/{}/fdinfo/{fd}", process.pid));
        let Some(info) = root.read_optional(&path)?.as_deref().and_then(parse_fdinfo) else {
            all_valid = false;
            continue;
        };
        let client = clients.entry(info.client_id).or_insert_with(|| DrmClient {
            engine_ns: info.engine_ns,
            holders: Vec::new(),
        });
        client.engine_ns = client.engine_ns.max(info.engine_ns);
        if !client.holders.iter().any(|h| h.process == process) {
            client.holders.push(DrmHolder {
                process,
                cgroup: cgroup.clone(),
            });
        }
    }
    Ok(all_valid)
}

#[cfg(test)]
mod tests {
    use super::*;
    use drainscope_model::CgroupPath;
    use std::os::unix::fs::symlink;

    const I915: &str = "pos:\t0\nflags:\t02100002\ndrm-driver:\ti915\ndrm-client-id:\t33\ndrm-engine-render:\t20462333122 ns\ndrm-engine-copy:\t5 ns\ndrm-engine-capacity-video:\t2\n";

    #[test]
    fn parses_i915_fdinfo() {
        assert_eq!(
            parse_fdinfo(I915),
            Some(FdInfo {
                client_id: 33,
                engine_ns: 20_462_333_127
            })
        );
        assert_eq!(parse_fdinfo("pos:\t0\nflags:\t02\n"), None);
    }

    struct Fixture {
        _dir: tempfile::TempDir,
        root: SysRoot,
    }

    impl Fixture {
        fn new() -> Self {
            let dir = tempfile::tempdir().unwrap();
            let root = SysRoot::at(dir.path());
            fs::create_dir_all(root.path("proc")).unwrap();
            Self { _dir: dir, root }
        }

        fn process(&self, pid: u32, start: u64, cgroup: &str) {
            let base = self.root.path(&format!("proc/{pid}"));
            fs::create_dir_all(base.join("fd")).unwrap();
            fs::create_dir_all(base.join("fdinfo")).unwrap();
            let stat_line =
                format!("{pid} (p{pid}) S 1 1 1 0 -1 0 0 0 0 0 1 1 0 0 20 0 1 0 {start} 0 0");
            fs::write(base.join("stat"), stat_line).unwrap();
            fs::write(base.join("cgroup"), format!("0::/{cgroup}\n")).unwrap();
        }

        fn drm_fd(&self, pid: u32, fd: u32, client: u64, engine_ns: u64) {
            let base = self.root.path(&format!("proc/{pid}"));
            let link = base.join(format!("fd/{fd}"));
            if !link.exists() && fs::symlink_metadata(&link).is_err() {
                symlink("/dev/dri/renderD128", &link).unwrap();
            }
            fs::write(
                base.join(format!("fdinfo/{fd}")),
                format!("drm-client-id:\t{client}\ndrm-engine-render:\t{engine_ns} ns\n"),
            )
            .unwrap();
        }

        fn other_fd(&self, pid: u32, fd: u32) {
            symlink("/dev/null", self.root.path(&format!("proc/{pid}/fd/{fd}"))).unwrap();
        }
    }

    const SHELL: &str =
        "user.slice/user-1000.slice/user@1000.service/session.slice/org.gnome.Shell@user.service";

    #[test]
    fn collects_shared_clients_with_all_holders() {
        let fx = Fixture::new();
        fx.process(1, 8, "init.scope");
        fx.process(3416, 900, SHELL);
        fx.process(5000, 950, "system.slice/cups.service");
        fx.drm_fd(1, 604, 16, 1_000);
        fx.drm_fd(3416, 12, 16, 1_000);
        fx.drm_fd(3416, 13, 16, 1_000); // same client through a dup'd fd
        fx.other_fd(5000, 3);

        let mut scanner = DrmScanner::default();
        let clients = scanner.scan(&fx.root, Duration::ZERO).unwrap();
        let client = &clients[&16];
        assert_eq!(client.engine_ns, 1_000);
        let holders: Vec<u32> = client.holders.iter().map(|h| h.process.pid).collect();
        assert_eq!(holders, vec![1, 3416]);
        assert_eq!(client.holders[1].cgroup, CgroupPath::new(SHELL));
    }

    #[test]
    fn rereads_cached_fds_and_skips_known_non_gpu_processes() {
        let fx = Fixture::new();
        fx.process(10, 5, "a.scope");
        fx.process(11, 6, "b.scope");
        fx.drm_fd(10, 4, 7, 100);
        fx.other_fd(11, 3);
        let mut scanner = DrmScanner::default();
        scanner.scan(&fx.root, Duration::ZERO).unwrap();

        // Process 11 opens the GPU after discovery; it's only seen after a full rescan.
        fx.drm_fd(11, 5, 8, 50);
        fx.drm_fd(10, 4, 7, 400);
        let clients = scanner.scan(&fx.root, Duration::from_secs(2)).unwrap();
        assert_eq!(clients[&7].engine_ns, 400);
        assert!(!clients.contains_key(&8));

        let clients = scanner.scan(&fx.root, FULL_RESCAN).unwrap();
        assert_eq!(clients[&8].engine_ns, 50);
    }

    #[test]
    fn reused_pids_are_rediscovered() {
        let fx = Fixture::new();
        fx.process(10, 5, "a.scope");
        fx.drm_fd(10, 4, 7, 100);
        let mut scanner = DrmScanner::default();
        scanner.scan(&fx.root, Duration::ZERO).unwrap();

        // Same pid, new process (different start time), different client on another fd.
        fs::remove_file(fx.root.path("proc/10/fd/4")).unwrap();
        fs::remove_file(fx.root.path("proc/10/fdinfo/4")).unwrap();
        fx.process(10, 99, "b.scope");
        fx.drm_fd(10, 9, 21, 5);
        let clients = scanner.scan(&fx.root, Duration::from_secs(2)).unwrap();
        assert_eq!(clients.keys().copied().collect::<Vec<_>>(), vec![21]);
        assert_eq!(clients[&21].holders[0].process.start_ticks, 99);
    }

    #[test]
    fn exited_processes_are_forgotten() {
        let fx = Fixture::new();
        fx.process(10, 5, "a.scope");
        fx.drm_fd(10, 4, 7, 100);
        let mut scanner = DrmScanner::default();
        scanner.scan(&fx.root, Duration::ZERO).unwrap();
        fs::remove_dir_all(fx.root.path("proc/10")).unwrap();
        assert!(
            scanner
                .scan(&fx.root, Duration::from_secs(2))
                .unwrap()
                .is_empty()
        );
        assert!(scanner.known.is_empty());
    }
}
