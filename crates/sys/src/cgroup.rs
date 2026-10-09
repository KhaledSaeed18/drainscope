//! cgroup v2 CPU accounting and terminal-tab labels.

use std::collections::BTreeMap;
use std::fs;
use std::io::Read as _;
use std::mem::MaybeUninit;
use std::os::fd::{AsFd, OwnedFd};
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

use rustix::fs::{AtFlags, CWD, FileType, Mode, OFlags, RawDir, openat, statat};
use rustix::io::Errno;

use drainscope_model::CgroupPath;
use drainscope_model::cgroup::TERMINAL_SCOPE_PREFIXES;

use crate::error::SysError;
use crate::process::{cgroup_pids, read_stat};
use crate::root::{SysRoot, is_gone};

pub const CGROUPFS: &str = "sys/fs/cgroup";

/// Cumulative `usage_usec` of every cgroup, the root included (system-wide busy time).
///
/// Children are read before their parents: counters only grow, so a parent read later is
/// never below the sum of its children (ADR 0001). Cgroups removed during the walk are
/// skipped.
///
/// # Errors
/// Unexpected I/O errors, or a `cpu.stat` without `usage_usec`.
pub fn read_cpu_usage(root: &SysRoot) -> Result<BTreeMap<CgroupPath, u64>, SysError> {
    let base = root.path(CGROUPFS);
    let mut usage = BTreeMap::new();
    let Some(dir) = open_dir(CWD, &base, &base)? else {
        return Ok(usage);
    };
    let mut walk = Walk {
        entries: vec![MaybeUninit::uninit(); 16 * 1024],
        text: String::new(),
        usage: &mut usage,
    };
    walk.cgroup(&dir, &base, "")?;
    Ok(usage)
}

/// The per-tick walk. Each cgroup directory holds dozens of control files, so it lists them
/// without allocating per entry (`RawDir`) and opens everything relative to the parent's
/// descriptor instead of resolving each full path again.
struct Walk<'a> {
    entries: Vec<MaybeUninit<u8>>,
    text: String,
    usage: &'a mut BTreeMap<CgroupPath, u64>,
}

impl Walk<'_> {
    /// Reads `dir`'s child cgroups, then its own `cpu.stat`. `path` is only for errors.
    fn cgroup(&mut self, dir: &OwnedFd, path: &Path, relative: &str) -> Result<(), SysError> {
        // List the children before descending, so one buffer serves every level.
        let mut children = Vec::new();
        let mut entries = RawDir::new(dir, &mut self.entries);
        while let Some(entry) = entries.next() {
            let entry = match entry {
                Ok(entry) => entry,
                // Removed while listing: skip it and its children.
                Err(err) if gone(err) => return Ok(()),
                Err(err) => return Err(SysError::io(path, err.into())),
            };
            let name = entry.file_name();
            if name == c"." || name == c".." {
                continue;
            }
            let is_dir = match entry.file_type() {
                FileType::Directory => true,
                // Filesystems without d_type (not cgroupfs); fixtures may live on one.
                FileType::Unknown => statat(dir, name, AtFlags::SYMLINK_NOFOLLOW)
                    .is_ok_and(|stat| FileType::from_raw_mode(stat.st_mode) == FileType::Directory),
                _ => false,
            };
            if is_dir {
                children.push(name.to_owned());
            }
        }
        for name in children {
            let name_text = name.to_string_lossy();
            let child_path = path.join(name_text.as_ref());
            let Some(child) = open_dir(dir, &name, &child_path)? else {
                continue;
            };
            let child_relative = if relative.is_empty() {
                name_text.into_owned()
            } else {
                format!("{relative}/{name_text}")
            };
            self.cgroup(&child, &child_path, &child_relative)?;
        }

        // This cgroup's own counter, after every child's.
        let stat_path = path.join("cpu.stat");
        let file = match openat(
            dir,
            c"cpu.stat",
            OFlags::RDONLY | OFlags::CLOEXEC,
            Mode::empty(),
        ) {
            Ok(file) => file,
            Err(err) if gone(err) => return Ok(()),
            Err(err) => return Err(SysError::io(&stat_path, err.into())),
        };
        self.text.clear();
        match fs::File::from(file).read_to_string(&mut self.text) {
            Ok(_) => {}
            Err(err) if is_gone(&err) => return Ok(()),
            Err(err) => return Err(SysError::io(&stat_path, err)),
        }
        let usec = parse_usage_usec(&self.text)
            .ok_or_else(|| SysError::parse(&stat_path, "missing usage_usec"))?;
        self.usage.insert(CgroupPath::new(relative), usec);
        Ok(())
    }
}

/// Opens a directory relative to `parent`; `Ok(None)` when it's gone.
fn open_dir(
    parent: impl AsFd,
    name: impl rustix::path::Arg,
    path: &Path,
) -> Result<Option<OwnedFd>, SysError> {
    match openat(
        parent,
        name,
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC,
        Mode::empty(),
    ) {
        Ok(fd) => Ok(Some(fd)),
        Err(err) if gone(err) => Ok(None),
        Err(err) => Err(SysError::io(path, err.into())),
    }
}

fn gone(err: Errno) -> bool {
    is_gone(&err.into())
}

/// Every cgroup directory under `base`, parents before their children.
fn cgroup_dirs(base: &Path) -> Result<Vec<PathBuf>, SysError> {
    let mut dirs: Vec<PathBuf> = Vec::new();
    let mut stack = vec![base.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let entries = match fs::read_dir(&dir) {
            Ok(entries) => entries,
            Err(err) if is_gone(&err) => continue,
            Err(err) => return Err(SysError::io(&dir, err)),
        };
        for entry in entries.filter_map(Result::ok) {
            if entry.file_type().is_ok_and(|t| t.is_dir()) {
                stack.push(entry.path());
            }
        }
        // Parents enter `dirs` before any of their children, so reversing reads leaves first.
        dirs.push(dir);
    }
    Ok(dirs)
}

/// Every cgroup by its ID: the inode number of its cgroupfs directory, which is what eBPF
/// programs see (`kernfs_node.id`, `bpf_get_current_cgroup_id`).
///
/// # Errors
/// If the cgroup hierarchy can't be listed.
pub fn cgroup_ids(root: &SysRoot) -> Result<BTreeMap<u64, CgroupPath>, SysError> {
    let base = root.path(CGROUPFS);
    let mut ids = BTreeMap::new();
    for dir in cgroup_dirs(&base)? {
        // Removed since listing: skip.
        let Ok(metadata) = fs::metadata(&dir) else {
            continue;
        };
        let relative = dir.strip_prefix(&base).unwrap_or(&dir);
        ids.insert(metadata.ino(), CgroupPath::new(&relative.to_string_lossy()));
    }
    Ok(ids)
}

fn parse_usage_usec(cpu_stat: &str) -> Option<u64> {
    cpu_stat
        .lines()
        .find_map(|line| line.strip_prefix("usage_usec "))
        .and_then(|value| value.trim().parse().ok())
}

/// Labels for terminal-tab scopes among `cgroups`: the name of the process that has used the
/// most CPU there, so a tab running `pnpm dev` reads as `pnpm`, not as its shell.
///
/// # Errors
/// Unexpected I/O errors.
pub fn terminal_labels<'a>(
    root: &SysRoot,
    cgroups: impl IntoIterator<Item = &'a CgroupPath>,
) -> Result<BTreeMap<CgroupPath, String>, SysError> {
    let mut labels = BTreeMap::new();
    for cgroup in cgroups {
        let is_terminal = cgroup
            .segments()
            .last()
            .is_some_and(|name| TERMINAL_SCOPE_PREFIXES.iter().any(|p| name.starts_with(p)));
        if !is_terminal {
            continue;
        }
        let mut busiest: Option<(u64, String)> = None;
        for pid in cgroup_pids(root, cgroup)? {
            let Some(stat) = read_stat(root, pid)? else {
                continue;
            };
            if busiest
                .as_ref()
                .is_none_or(|(ticks, _)| stat.cpu_ticks > *ticks)
            {
                busiest = Some((stat.cpu_ticks, stat.comm));
            }
        }
        if let Some((_, comm)) = busiest {
            labels.insert(cgroup.clone(), comm);
        }
    }
    Ok(labels)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(root: &SysRoot, path: &str, contents: &str) {
        let full = root.path(path);
        fs::create_dir_all(full.parent().unwrap()).unwrap();
        fs::write(full, contents).unwrap();
    }

    fn cpu_stat(usec: u64) -> String {
        format!("usage_usec {usec}\nuser_usec 1\nsystem_usec 1\n")
    }

    #[test]
    fn maps_cgroup_ids_to_paths() {
        let dir = tempfile::tempdir().unwrap();
        let root = SysRoot::at(dir.path());
        let base = root.path(CGROUPFS);
        fs::create_dir_all(base.join("user.slice/user-1000.slice")).unwrap();
        let ids = cgroup_ids(&root).unwrap();
        let inode = |p: &str| fs::metadata(base.join(p)).unwrap().ino();
        assert_eq!(ids.len(), 3);
        assert_eq!(ids[&inode("")], CgroupPath::root());
        assert_eq!(
            ids[&inode("user.slice/user-1000.slice")],
            CgroupPath::new("user.slice/user-1000.slice")
        );
    }

    #[test]
    fn reads_every_cgroup_including_root() {
        let dir = tempfile::tempdir().unwrap();
        let root = SysRoot::at(dir.path());
        write(&root, "sys/fs/cgroup/cpu.stat", &cpu_stat(900));
        write(&root, "sys/fs/cgroup/system.slice/cpu.stat", &cpu_stat(500));
        write(
            &root,
            r"sys/fs/cgroup/system.slice/a\x2db.service/cpu.stat",
            &cpu_stat(200),
        );
        write(
            &root,
            "sys/fs/cgroup/system.slice/cpu.stat.local",
            "throttled_usec 0\n",
        );
        // A directory without cpu.stat (e.g. removed mid-walk) is skipped.
        fs::create_dir_all(root.path("sys/fs/cgroup/gone.scope")).unwrap();

        let usage = read_cpu_usage(&root).unwrap();
        assert_eq!(
            usage,
            BTreeMap::from([
                (CgroupPath::root(), 900),
                (CgroupPath::new("system.slice"), 500),
                (CgroupPath::new(r"system.slice/a\x2db.service"), 200),
            ])
        );
    }

    #[test]
    fn missing_cgroupfs_reads_as_empty() {
        let dir = tempfile::tempdir().unwrap();
        assert!(read_cpu_usage(&SysRoot::at(dir.path())).unwrap().is_empty());
    }

    #[test]
    fn malformed_cpu_stat_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let root = SysRoot::at(dir.path());
        write(&root, "sys/fs/cgroup/cpu.stat", "garbage\n");
        assert!(matches!(read_cpu_usage(&root), Err(SysError::Parse { .. })));
    }

    fn stat_line(pid: u32, comm: &str, utime: u64) -> String {
        format!("{pid} ({comm}) S 1 1 1 0 -1 0 0 0 0 0 {utime} 0 0 0 20 0 1 0 100 0 0")
    }

    #[test]
    fn terminal_tabs_are_named_after_their_busiest_process() {
        let dir = tempfile::tempdir().unwrap();
        let root = SysRoot::at(dir.path());
        let tab = CgroupPath::new(
            "user.slice/user-1000.slice/user@1000.service/app.slice/ptyxis-spawn-1.scope",
        );
        let other = CgroupPath::new("system.slice/cups.service");
        write(
            &root,
            &format!("sys/fs/cgroup/{}/cgroup.procs", tab.as_str()),
            "10\n11\n12\n",
        );
        write(&root, "proc/10/stat", &stat_line(10, "bash", 50));
        write(&root, "proc/11/stat", &stat_line(11, "node", 9_000));
        // pid 12 exited between listing and reading.
        let labels = terminal_labels(&root, [&tab, &other]).unwrap();
        assert_eq!(labels, BTreeMap::from([(tab, "node".to_owned())]));
        assert!(!root.path("proc/12").exists());
    }
}
