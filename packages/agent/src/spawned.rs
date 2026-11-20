//! A record of the processes this daemon starts to do a tool's work, so the daemon
//! can tell its own tool programs from the user's own applications when
//! something connects to its socket.
//!
//! Every program started for a `tool-cli` call, a `tool-mcp` stdio server, an
//! ACP agent or a PTY session is recorded here with its process id and its
//! process group. [`classify_peer`] then says whether a connecting process is
//! one of them: it is when its process group is a recorded one (which a
//! background process, an orphan whose parent exited, and any grandchild all
//! keep) or when a recorded process is among its ancestors.
//!
//! A process the daemon starts that is NOT recorded (the tray's "Open
//! NodeSpace" starts the app as the daemon's child) is not a tool's, and is not
//! classified as one.
//!
//! What this does not cover: a recorded program that leaves its process group
//! (`setpgid`, `setsid`) and whose parent has gone is indistinguishable from any
//! other program of the same user. Closing that needs the peer's audit token and
//! a code-signature check; it is not done here.
//!
//! Identity is read from the kernel (`/proc` on Linux, `proc_pidinfo` on macOS);
//! a process whose own identity cannot be read is `None`, never "not a tool". On
//! other platforms nothing can be read, so every peer is `None`. A process id
//! that was reused is told apart by its start time.

use std::collections::{HashMap, HashSet};
use std::sync::Mutex;

/// What the kernel says about one process.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProcInfo {
    pub ppid: u32,
    pub pgid: u32,
    /// A start time that differs for two processes that had the same id.
    pub start: u64,
}

/// The recorded programs: pid -> start time, and the process groups they lead.
#[derive(Default)]
struct Registry {
    pids: HashMap<u32, u64>,
    groups: HashSet<u32>,
}

/// Most entries kept; dead ones are dropped first.
const MAX_RECORDED: usize = 65_536;

static REGISTRY: Mutex<Option<Registry>> = Mutex::new(None);

fn with_registry<T>(f: impl FnOnce(&mut Registry) -> T) -> T {
    let mut guard = REGISTRY.lock().unwrap_or_else(|p| p.into_inner());
    f(guard.get_or_insert_with(Registry::default))
}

/// Records a program the daemon has just started. A program that is already
/// gone (it cannot be read) is not recorded: nothing of it can connect.
pub fn record(pid: u32) {
    let Some(info) = proc_info(pid) else {
        return;
    };
    with_registry(|r| {
        prune(r, &proc_info, &group_alive);
        if r.pids.len() >= MAX_RECORDED {
            // Full of live programs: keep the new one, drop an old pid entry
            // (its group stays, which is what identifies the orphans).
            if let Some(oldest) = r.pids.keys().next().copied() {
                r.pids.remove(&oldest);
            }
        }
        r.pids.insert(pid, info.start);
        // A tool program leads its own group (started with `process_group(0)`, or
        // as a session leader on a PTY); a group is recorded only when it does.
        if info.pgid == pid {
            r.groups.insert(pid);
        }
    });
}

fn prune(
    r: &mut Registry,
    lookup: &dyn Fn(u32) -> Option<ProcInfo>,
    group_alive: &dyn Fn(u32) -> bool,
) {
    r.pids
        .retain(|pid, start| lookup(*pid).is_some_and(|i| i.start == *start));
    r.groups.retain(|g| group_alive(*g));
}

/// Whether `peer` is a program started for a tool (`Some(true)`), is not
/// (`Some(false)`), or cannot be told (`None`: its own identity is unreadable, or
/// its process id is 0 or 1).
pub fn classify_peer(peer: u32) -> Option<bool> {
    with_registry(|r| {
        prune(r, &proc_info, &group_alive);
        classify_with(peer, &proc_info, &r.pids, &r.groups)
    })
}

fn classify_with(
    peer: u32,
    lookup: &dyn Fn(u32) -> Option<ProcInfo>,
    pids: &HashMap<u32, u64>,
    groups: &HashSet<u32>,
) -> Option<bool> {
    // 0 is what the kernel reports for a peer in another pid namespace; 1 is
    // init. Neither is an application that can be named.
    if peer <= 1 {
        return None;
    }
    let info = lookup(peer)?;
    if groups.contains(&info.pgid) {
        return Some(true);
    }
    if pids.get(&peer) == Some(&info.start) {
        return Some(true);
    }
    // Its ancestors: a recorded program among them makes it a tool's.
    let mut current = info.ppid;
    for _ in 0..256 {
        if current <= 1 {
            return Some(false);
        }
        // An ancestor that cannot be read ends the walk: a recorded program
        // among the ancestors would have been readable when it was recorded, and
        // the peer's own process group was already checked.
        let Some(up) = lookup(current) else {
            return Some(false);
        };
        if pids.get(&current) == Some(&up.start) || groups.contains(&up.pgid) {
            return Some(true);
        }
        current = up.ppid;
    }
    None
}

#[cfg(target_os = "linux")]
pub fn proc_info(pid: u32) -> Option<ProcInfo> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    // "pid (comm) state ppid pgrp session ... starttime(22)": comm may hold
    // spaces and parentheses, so the fields are read after the LAST ')'.
    let rest = &stat[stat.rfind(')')? + 1..];
    let fields: Vec<&str> = rest.split_whitespace().collect();
    Some(ProcInfo {
        ppid: fields.get(1)?.parse().ok()?,
        pgid: fields.get(2)?.parse().ok()?,
        start: fields.get(19)?.parse().ok()?,
    })
}

#[cfg(target_os = "macos")]
pub fn proc_info(pid: u32) -> Option<ProcInfo> {
    // SAFETY: proc_pidinfo writes at most `size` bytes into the struct we own.
    unsafe {
        let mut info: libc::proc_bsdinfo = std::mem::zeroed();
        let size = std::mem::size_of::<libc::proc_bsdinfo>() as libc::c_int;
        let written = libc::proc_pidinfo(
            pid as libc::c_int,
            libc::PROC_PIDTBSDINFO,
            0,
            &mut info as *mut _ as *mut libc::c_void,
            size,
        );
        (written == size).then_some(ProcInfo {
            ppid: info.pbi_ppid,
            pgid: info.pbi_pgid,
            start: info
                .pbi_start_tvsec
                .wrapping_mul(1_000_000)
                .wrapping_add(info.pbi_start_tvusec),
        })
    }
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
pub fn proc_info(_pid: u32) -> Option<ProcInfo> {
    None
}

/// Whether any process still belongs to group `pgid`.
#[cfg(unix)]
fn group_alive(pgid: u32) -> bool {
    // SAFETY: kill with signal 0 sends nothing; it only reports whether the
    // group exists.
    let rc = unsafe { libc::kill(-(pgid as libc::pid_t), 0) };
    rc == 0 || std::io::Error::last_os_error().raw_os_error() != Some(libc::ESRCH)
}

#[cfg(not(unix))]
fn group_alive(_pgid: u32) -> bool {
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    fn table(rows: &[(u32, u32, u32, u64)]) -> impl Fn(u32) -> Option<ProcInfo> + '_ {
        move |pid| {
            rows.iter()
                .find(|r| r.0 == pid)
                .map(|&(_, ppid, pgid, start)| ProcInfo { ppid, pgid, start })
        }
    }

    #[test]
    fn a_peer_is_classified_by_group_by_recorded_pid_and_by_ancestry() {
        // 100 (recorded, leads group 100); 101 its child (same group); 102 an
        // orphan of 100 (parent gone: ppid 1) still in group 100; 103 a child of
        // 101 that left the group but whose parent is alive; 200 an application;
        // 201 the tray-started UI: a child of the daemon 50, not recorded.
        let rows = [
            (50, 1, 50, 5),
            (100, 50, 100, 10),
            (101, 100, 100, 11),
            (102, 1, 100, 12),
            (103, 101, 103, 13),
            (200, 1, 200, 20),
            (201, 50, 50, 21),
        ];
        let lookup = table(&rows);
        let pids: HashMap<u32, u64> = [(100, 10)].into();
        let groups: HashSet<u32> = [100].into();
        let c = |p| classify_with(p, &lookup, &pids, &groups);
        assert_eq!(c(100), Some(true), "the recorded program");
        assert_eq!(c(101), Some(true), "its child, by group");
        assert_eq!(c(102), Some(true), "an orphan keeps its group");
        assert_eq!(
            c(103),
            Some(true),
            "left the group, parent chain reaches 100"
        );
        assert_eq!(c(200), Some(false), "an application outside the tree");
        assert_eq!(
            c(201),
            Some(false),
            "a program the daemon started and did not record"
        );
        assert_eq!(
            c(50),
            Some(false),
            "the daemon is not itself a tool program"
        );
    }

    #[test]
    fn what_cannot_be_identified_is_none_never_not_a_tool() {
        // A row for pid 0 as a lookup could return it: still not a peer to judge.
        let rows = [(0, 1, 0, 9), (300, 301, 300, 1), (302, 1, 302, 2)];
        let lookup = table(&rows);
        let none = HashMap::new();
        let nogroups = HashSet::new();
        // 0 and 1 are not applications.
        assert_eq!(classify_with(0, &lookup, &none, &nogroups), None);
        assert_eq!(classify_with(1, &lookup, &none, &nogroups), None);
        // A process that is not there (dead, or hidden).
        assert_eq!(classify_with(999, &lookup, &none, &nogroups), None);
        // An ancestor that cannot be read only ends the walk: the peer is known,
        // and no recorded program was found above it or in its group.
        assert_eq!(classify_with(300, &lookup, &none, &nogroups), Some(false));
        assert_eq!(classify_with(302, &lookup, &none, &nogroups), Some(false));
    }

    #[test]
    fn a_reused_process_id_is_not_the_recorded_program() {
        // 100 was recorded with start 10; the id now belongs to a new process
        // (start 99) in a group of its own.
        let rows = [(100, 1, 100, 99)];
        let lookup = table(&rows);
        let pids: HashMap<u32, u64> = [(100, 10)].into();
        let groups: HashSet<u32> = HashSet::new();
        assert_eq!(classify_with(100, &lookup, &pids, &groups), Some(false));
    }

    #[test]
    fn pruning_forgets_dead_programs_and_empty_groups() {
        let rows = [(100, 1, 100, 10), (101, 1, 100, 11)];
        let lookup = table(&rows);
        let mut r = Registry::default();
        r.pids.insert(100, 10);
        r.pids.insert(555, 1); // gone
        r.pids.insert(101, 7); // reused
        r.groups.insert(100);
        r.groups.insert(777); // empty
        prune(&mut r, &lookup, &|g| g == 100);
        assert_eq!(r.pids.keys().copied().collect::<Vec<_>>(), [100]);
        assert_eq!(r.groups, [100].into());
    }

    #[cfg(unix)]
    #[test]
    fn this_process_can_be_read_and_a_dead_one_cannot() {
        let me = proc_info(std::process::id()).expect("readable");
        assert!(me.ppid >= 1 && me.pgid >= 1);
        let mut child = std::process::Command::new("true").spawn().unwrap();
        let pid = child.id();
        child.wait().unwrap();
        assert_eq!(proc_info(pid), None, "a reaped process is not readable");
    }
}
