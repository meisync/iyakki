//! What the kernel says about one process or a group of processes (F-ARCH-07): the state letter and start time from `/proc/<pid>/stat`, the
//! program name from `comm`, and the CPU ticks a process group has used. The job runner of the agent asks these; this is the one reader of
//! those files. Every function has a `_in` form that takes the `/proc` root so a test uses a made-up tree.

use std::path::Path;

/// `(state letter, start time in clock ticks since boot)` of `pid` from `<proc_>/<pid>/stat`, `None` when there is no such process.
pub fn stat_in(proc_: &Path, pid: u32) -> Option<(char, u64)> {
    let s = std::fs::read_to_string(proc_.join(pid.to_string()).join("stat")).ok()?;
    let rest = &s[s.rfind(')')? + 1..];
    let mut t = rest.split_whitespace();
    let state = t.next()?.chars().next()?;
    let start = t.nth(18)?.parse().ok()?;
    Some((state, start))
}

/// [`stat_in`] on this machine.
pub fn stat(pid: u32) -> Option<(char, u64)> {
    stat_in(Path::new("/proc"), pid)
}

/// The program name of `pid` (`<proc_>/<pid>/comm`, trimmed), `None` when there is no such process.
pub fn name_in(proc_: &Path, pid: u32) -> Option<String> {
    std::fs::read_to_string(proc_.join(pid.to_string()).join("comm")).ok().map(|c| c.trim().to_string())
}

/// [`name_in`] on this machine.
pub fn name(pid: u32) -> Option<String> {
    name_in(Path::new("/proc"), pid)
}

/// CPU time (user + system, in clock ticks) used by every process of the process group `pgid` under `proc_`. `None` when the directory cannot
/// be listed.
pub fn group_cpu_ticks_in(proc_: &Path, pgid: u32) -> Option<u64> {
    let mut sum = 0u64;
    for e in std::fs::read_dir(proc_).ok()?.flatten() {
        let name = e.file_name();
        let Some(pid) = name.to_str().filter(|n| n.bytes().all(|b| b.is_ascii_digit())) else { continue };
        let Ok(text) = std::fs::read_to_string(proc_.join(pid).join("stat")) else { continue };
        let Some(rest) = text.rfind(')').map(|i| &text[i + 1..]) else { continue };
        let f: Vec<&str> = rest.split_whitespace().collect();
        if f.get(2).and_then(|g| g.parse::<u32>().ok()) == Some(pgid) {
            sum += f.get(11).and_then(|v| v.parse::<u64>().ok()).unwrap_or(0) + f.get(12).and_then(|v| v.parse::<u64>().ok()).unwrap_or(0);
        }
    }
    Some(sum)
}

/// [`group_cpu_ticks_in`] on this machine.
pub fn group_cpu_ticks(pgid: u32) -> Option<u64> {
    group_cpu_ticks_in(Path::new("/proc"), pgid)
}

/// The direct children of process `parent` under `proc_` whose program name is `name` (read from `<pid>/stat` and `<pid>/comm`).
pub fn children_named_in(proc_: &Path, parent: u32, name: &str) -> Vec<u32> {
    let mut out = Vec::new();
    for e in std::fs::read_dir(proc_).into_iter().flatten().flatten() {
        let Ok(pid) = e.file_name().to_string_lossy().parse::<u32>() else { continue };
        let Ok(stat) = std::fs::read_to_string(e.path().join("stat")) else { continue };
        // "pid (comm) S ppid ...": comm may hold spaces and brackets, so cut after the last ')'
        let ppid = stat.rsplit_once(')').and_then(|(_, r)| r.split_whitespace().nth(1)).and_then(|p| p.parse::<u32>().ok());
        let comm = std::fs::read_to_string(e.path().join("comm")).unwrap_or_default();
        if ppid == Some(parent) && comm.trim() == name {
            out.push(pid);
        }
    }
    out.sort_unstable();
    out
}

/// [`children_named_in`] for the direct children of this process on this machine.
pub fn my_children_named(name: &str) -> Vec<u32> {
    children_named_in(Path::new("/proc"), std::process::id(), name)
}

/// The program file `pid` runs (`<proc_>/<pid>/exe`), `None` when there is no such process or it may not be looked at.
pub fn exe_in(proc_: &Path, pid: u32) -> Option<std::path::PathBuf> {
    std::fs::read_link(proc_.join(pid.to_string()).join("exe")).ok()
}

/// [`exe_in`] on this machine.
pub fn exe(pid: u32) -> Option<std::path::PathBuf> {
    exe_in(Path::new("/proc"), pid)
}

/// The resident memory in bytes of this process (`<proc_>/self/status`, `VmRSS`), `None` where the system does not say.
pub fn resident_bytes_in(proc_: &Path) -> Option<u64> {
    let t = std::fs::read_to_string(proc_.join("self").join("status")).ok()?;
    let kb: u64 = t.lines().find_map(|l| l.strip_prefix("VmRSS:"))?.split_whitespace().next()?.parse().ok()?;
    Some(kb * 1024)
}

/// [`resident_bytes_in`] on this machine.
pub fn resident_bytes() -> Option<u64> {
    resident_bytes_in(Path::new("/proc"))
}

/// Every process under `proc_` whose program name (`comm`) is exactly `name`, in order.
pub fn pids_named_in(proc_: &Path, name: &str) -> Vec<u32> {
    let mut out: Vec<u32> = std::fs::read_dir(proc_)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| {
            let pid = e.file_name().to_string_lossy().parse::<u32>().ok()?;
            std::fs::read_to_string(e.path().join("comm")).ok().filter(|n| n.trim() == name).map(|_| pid)
        })
        .collect();
    out.sort_unstable();
    out
}

/// [`pids_named_in`] on this machine; `None` when the machine has no `/proc` (the listing cannot be told from "nothing runs").
pub fn pids_named(name: &str) -> Option<Vec<u32>> {
    Path::new("/proc/self").exists().then(|| pids_named_in(Path::new("/proc"), name))
}

/// Whether process `pid` exists under `proc_`; `None` when `proc_` does not look like a process tree (no `self` entry: macOS, a made-up
/// folder), so the caller must not guess.
pub fn is_alive_in(proc_: &Path, pid: u32) -> Option<bool> {
    proc_.join("self").exists().then(|| proc_.join(pid.to_string()).exists())
}

/// [`is_alive_in`] on this machine.
pub fn is_alive(pid: u32) -> Option<bool> {
    is_alive_in(Path::new("/proc"), pid)
}

/// One socket on a local port: the table it was listed in (`tcp`, `tcp6`, `udp`, `udp6`), its state code and the process that owns it
/// (id and program name) when that can be seen.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SocketUse {
    pub table: String,
    pub state: String,
    pub owner: Option<(u32, String)>,
}

/// The sockets on local `port` under `proc_` (`net/{tcp,tcp6,udp,udp6}`) with their owners (`<pid>/fd` links `socket:[inode]`).
pub fn socket_uses_in(proc_: &Path, port: u16) -> Vec<SocketUse> {
    let hex = format!(":{port:04X}");
    let mut found: Vec<(String, String, String)> = Vec::new();
    for table in ["tcp", "tcp6", "udp", "udp6"] {
        let Ok(text) = std::fs::read_to_string(proc_.join("net").join(table)) else { continue };
        for line in text.lines().skip(1) {
            let f: Vec<&str> = line.split_whitespace().collect();
            if f.len() > 9 && f[1].ends_with(&hex) {
                found.push((table.to_string(), f[3].to_string(), f[9].to_string()));
            }
        }
    }
    found
        .into_iter()
        .map(|(table, state, inode)| {
            let want = format!("socket:[{inode}]");
            let owner = std::fs::read_dir(proc_).into_iter().flatten().flatten().find_map(|e| {
                let pid = e.file_name().to_string_lossy().parse::<u32>().ok()?;
                let held = std::fs::read_dir(e.path().join("fd")).ok()?.flatten().any(|fd| std::fs::read_link(fd.path()).is_ok_and(|l| l.to_string_lossy() == want));
                held.then(|| (pid, std::fs::read_to_string(e.path().join("comm")).unwrap_or_default().trim().to_string()))
            });
            SocketUse { table, state, owner }
        })
        .collect()
}

/// [`socket_uses_in`] on this machine.
pub fn socket_uses(port: u16) -> Vec<SocketUse> {
    socket_uses_in(Path::new("/proc"), port)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A stat line: pid, (comm), state, ppid, pgrp, then filler up to utime (field 14), stime (15) and start time (field 22).
    fn line(comm: &str, state: char, pgrp: u32, utime: u64, stime: u64, start: u64) -> String {
        let mut f = vec!["0".to_string(); 20];
        f[0] = state.to_string();
        f[1] = "1".into();
        f[2] = pgrp.to_string();
        f[11] = utime.to_string();
        f[12] = stime.to_string();
        f[19] = start.to_string();
        format!("42 ({comm}) {}\n", f.join(" "))
    }

    fn tree(tag: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("iyakki-procfacts-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn put(d: &Path, pid: u32, stat: &str, comm: &str) {
        std::fs::create_dir_all(d.join(pid.to_string())).unwrap();
        std::fs::write(d.join(pid.to_string()).join("stat"), stat).unwrap();
        std::fs::write(d.join(pid.to_string()).join("comm"), comm).unwrap();
    }

    #[test]
    fn state_start_and_name_come_from_the_files_even_with_brackets_in_the_name() {
        let d = tree("one");
        put(&d, 42, &line("we ) ird", 'S', 7, 1, 2, 9999), "we ) ird\n");
        assert_eq!(stat_in(&d, 42), Some(('S', 9999)));
        assert_eq!(name_in(&d, 42).as_deref(), Some("we ) ird"));
        assert_eq!(stat_in(&d, 43), None);
        assert_eq!(name_in(&d, 43), None);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn group_ticks_add_user_and_system_of_the_group_only() {
        let d = tree("grp");
        put(&d, 10, &line("a", 'R', 7, 100, 20, 1), "a\n");
        put(&d, 11, &line("b", 'S', 7, 3, 4, 2), "b\n");
        put(&d, 12, &line("c", 'S', 8, 1000, 1000, 3), "c\n");
        std::fs::create_dir_all(d.join("self")).unwrap();
        assert_eq!(group_cpu_ticks_in(&d, 7), Some(127));
        assert_eq!(group_cpu_ticks_in(&d, 99), Some(0));
        assert_eq!(group_cpu_ticks_in(&d.join("nothing"), 7), None);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn children_are_those_of_the_parent_with_the_name_even_with_brackets_in_it() {
        let d = tree("kids");
        let kid = |pid: u32, comm: &str, ppid: u32| format!("{pid} ({comm}) S {ppid} 0 0 0");
        put(&d, 20, &kid(20, "aplay", 5), "aplay\n");
        put(&d, 21, &kid(21, "we ) ird", 5), "we ) ird\n");
        put(&d, 22, &kid(22, "aplay", 6), "aplay\n");
        std::fs::create_dir_all(d.join("self")).unwrap();
        assert_eq!(children_named_in(&d, 5, "aplay"), vec![20]);
        assert_eq!(children_named_in(&d, 5, "we ) ird"), vec![21]);
        assert!(children_named_in(&d, 9, "aplay").is_empty());
        assert_eq!(exe_in(&d, 20), None);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn resident_memory_names_and_liveness_come_from_a_made_up_tree() {
        let d = tree("misc");
        std::fs::create_dir_all(d.join("self")).unwrap();
        std::fs::write(d.join("self/status"), "Name:\tx\nVmRSS:\t  2048 kB\n").unwrap();
        assert_eq!(resident_bytes_in(&d), Some(2048 * 1024));
        std::fs::write(d.join("self/status"), "Name:\tx\n").unwrap();
        assert_eq!(resident_bytes_in(&d), None, "no VmRSS line: the system does not say");
        assert_eq!(resident_bytes_in(&d.join("none")), None);
        put(&d, 31, "", "mixer\n");
        put(&d, 30, "", "mixer\n");
        put(&d, 32, "", "other\n");
        assert_eq!(pids_named_in(&d, "mixer"), vec![30, 31]);
        assert!(pids_named_in(&d, "nothing").is_empty());
        assert_eq!(is_alive_in(&d, 30), Some(true));
        assert_eq!(is_alive_in(&d, 77), Some(false));
        assert_eq!(is_alive_in(&d.join("none"), 30), None, "not a process tree: no guess");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[cfg(unix)]
    #[test]
    fn a_socket_on_a_port_is_matched_to_its_owner_through_the_fd_links() {
        let d = tree("sock");
        std::fs::create_dir_all(d.join("net")).unwrap();
        let head = "  sl  local_address rem_address   st tx_queue rx_queue tr tm->when retrnsmt   uid  timeout inode\n";
        std::fs::write(d.join("net/tcp"), format!("{head}   0: 0100007F:1F90 00000000:0000 0A 00000000:00000000 00:00000000 00000000  1000        0 5555 1 0\n   1: 0100007F:1F91 00000000:0000 0A 00000000:00000000 00:00000000 00000000  1000        0 6666 1 0\n")).unwrap();
        put(&d, 50, "", "server\n");
        std::fs::create_dir_all(d.join("50/fd")).unwrap();
        std::os::unix::fs::symlink("socket:[5555]", d.join("50/fd/3")).unwrap();
        let u = socket_uses_in(&d, 8080);
        assert_eq!(u, vec![SocketUse { table: "tcp".into(), state: "0A".into(), owner: Some((50, "server".into())) }]);
        assert!(socket_uses_in(&d, 9999).is_empty());
        let other = socket_uses_in(&d, 0x1F91);
        assert_eq!(other.len(), 1);
        assert_eq!(other[0].owner, None, "nobody holds inode 6666 that can be seen");
        let _ = std::fs::remove_dir_all(&d);
    }
}
