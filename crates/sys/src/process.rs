//! Per-process facts from `/proc/<pid>`.

use std::fs;

use drainscope_model::CgroupPath;

use crate::error::SysError;
use crate::root::{SysRoot, is_gone};

/// The fields of `/proc/<pid>/stat` drainscope needs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProcStat {
    /// Process name (field 2), at most 15 bytes.
    pub comm: String,
    /// User plus system CPU time over the process's life, in clock ticks (fields 14 + 15).
    pub cpu_ticks: u64,
    /// Start time after boot in clock ticks (field 22). Together with the pid it identifies
    /// a process across pid reuse.
    pub start_ticks: u64,
}

/// Parses `/proc/<pid>/stat`. The name can contain spaces and parentheses, so fields are
/// counted from the last `)`.
#[must_use]
pub fn parse_stat(text: &str) -> Option<ProcStat> {
    let open = text.find('(')?;
    let close = text.rfind(')')?;
    let comm = text.get(open + 1..close)?.to_owned();
    // After the name, fields start at 3 (state), so field N is at index N − 3.
    let fields: Vec<&str> = text.get(close + 1..)?.split_whitespace().collect();
    let field = |n: usize| fields.get(n - 3)?.parse::<u64>().ok();
    Some(ProcStat {
        comm,
        cpu_ticks: field(14)?.checked_add(field(15)?)?,
        start_ticks: field(22)?,
    })
}

/// # Errors
/// Unexpected I/O errors or a malformed stat line.
pub fn read_stat(root: &SysRoot, pid: u32) -> Result<Option<ProcStat>, SysError> {
    let path = root.path(&format!("proc/{pid}/stat"));
    let Some(text) = root.read_optional(&path)? else {
        return Ok(None);
    };
    parse_stat(&text)
        .map(Some)
        .ok_or_else(|| SysError::parse(&path, "unexpected stat format"))
}

/// The process's cgroup v2 path, from the `0::<path>` line of `/proc/<pid>/cgroup`.
///
/// # Errors
/// Unexpected I/O errors.
pub fn read_cgroup(root: &SysRoot, pid: u32) -> Result<Option<CgroupPath>, SysError> {
    let path = root.path(&format!("proc/{pid}/cgroup"));
    Ok(root.read_optional(&path)?.and_then(|text| {
        text.lines()
            .find_map(|line| line.strip_prefix("0::"))
            .map(CgroupPath::new)
    }))
}

/// Every numeric entry of `/proc`.
///
/// # Errors
/// If `/proc` itself can't be listed.
pub fn list_pids(root: &SysRoot) -> Result<Vec<u32>, SysError> {
    let dir = root.path("proc");
    let entries = fs::read_dir(&dir).map_err(|e| SysError::io(&dir, e))?;
    let mut pids: Vec<u32> = entries
        .filter_map(Result::ok)
        .filter_map(|entry| entry.file_name().to_str()?.parse().ok())
        .collect();
    pids.sort_unstable();
    Ok(pids)
}

/// Process ids in a cgroup, from `cgroup.procs`. Empty if the cgroup is gone.
///
/// # Errors
/// Unexpected I/O errors.
pub fn cgroup_pids(root: &SysRoot, cgroup: &CgroupPath) -> Result<Vec<u32>, SysError> {
    let path = root
        .path(crate::cgroup::CGROUPFS)
        .join(cgroup.as_str())
        .join("cgroup.procs");
    match fs::read_to_string(&path) {
        Ok(text) => Ok(text.lines().filter_map(|l| l.trim().parse().ok()).collect()),
        Err(err) if is_gone(&err) => Ok(Vec::new()),
        Err(err) => Err(SysError::io(&path, err)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SYSTEMD: &str = "1 (systemd) S 0 1 1 0 -1 4194560 96060 1972686 643 5431 446 332 13765 6631 20 0 1 0 8 37998592 5245 18446744073709551615";

    #[test]
    fn parses_stat() {
        let stat = parse_stat(SYSTEMD).unwrap();
        assert_eq!(stat.comm, "systemd");
        assert_eq!(stat.cpu_ticks, 446 + 332);
        assert_eq!(stat.start_ticks, 8);
    }

    #[test]
    fn names_with_spaces_and_parens_parse() {
        let tricky = SYSTEMD.replacen("(systemd)", "(Web Content (x))", 1);
        let stat = parse_stat(&tricky).unwrap();
        assert_eq!(stat.comm, "Web Content (x)");
        assert_eq!(stat.start_ticks, 8);
    }

    #[test]
    fn truncated_stat_is_rejected() {
        assert_eq!(parse_stat("1 (systemd) S 0 1"), None);
    }

    #[test]
    fn reads_cgroup_and_pids() {
        let dir = tempfile::tempdir().unwrap();
        let root = SysRoot::at(dir.path());
        fs::create_dir_all(root.path("proc/42")).unwrap();
        fs::create_dir_all(root.path("proc/self")).unwrap();
        fs::write(
            root.path("proc/42/cgroup"),
            "0::/system.slice/cups.service\n",
        )
        .unwrap();
        assert_eq!(
            read_cgroup(&root, 42).unwrap(),
            Some(CgroupPath::new("system.slice/cups.service"))
        );
        assert_eq!(read_cgroup(&root, 7).unwrap(), None);
        assert_eq!(list_pids(&root).unwrap(), vec![42]);
    }
}
