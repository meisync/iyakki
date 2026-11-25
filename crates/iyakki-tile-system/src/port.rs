//! What the SYSTEM tile asks of the rest of the system (the app standard, docs/development/APP-DEVELOPMENT-PROCEDURE.md section 10).
//!
//! The tile names no `Services`, no session, no audit log and no core: it asks a [`SystemPort`] for the signed-in person, whether they manage
//! the system, the kernel's figures ([`SysRead`], read from the roots the tile owns), the hardware and memory facts, the graphics cards, the
//! dev cluster, the agent runs and the programs started for the person, the audio line, and the audit line. The real port (`CxPort` in the shell
//! crate, `system_port.rs`) is the only place that reads the session and calls the core and the services; a fake answers with canned figures.

use std::collections::HashMap;
use std::sync::Arc;

use iyakki_sysinfo_model::kubectl::Kubectl;
use iyakki_sysinfo_model::{CpuTimes, DiskCounters, DiskUse, Memory, NetCounters, NetUse, Proc, SysRoots};
use iyakki_tile_api::system::{ClusterHealth, LiveRun, MemStats, WasmTask};

/// A process with its owner and its network and disk use (the "My processes" page).
#[derive(Clone, Debug)]
pub struct OwnedProc {
    pub proc: Proc,
    pub owner: String,
    /// What the claimed program at the top of it was started as.
    pub what: String,
    pub net: NetUse,
    pub disk: DiskUse,
}

/// The kernel's figures the page shows, read from the given roots (`SysRoots::real()` on the device, a made-up machine in a test).
pub trait SysRead {
    fn cpu_times(&self, r: &SysRoots) -> CpuTimes;
    fn cpu_count(&self, r: &SysRoots) -> usize;
    fn memory_of(&self, r: &SysRoots) -> Memory;
    fn load_average(&self, r: &SysRoots) -> (f32, f32, f32);
    fn uptime_of(&self, r: &SysRoots) -> u64;
    fn hottest_temp(&self, r: &SysRoots) -> Option<f32>;
    fn net_counters(&self, r: &SysRoots) -> Vec<NetCounters>;
    fn disk_counters(&self, r: &SysRoots) -> Vec<DiskCounters>;
    fn processes(&self, r: &SysRoots) -> Vec<Proc>;
    fn net_use(&self, r: &SysRoots, pids: &[u32]) -> HashMap<u32, NetUse>;
    /// Stop (SIGTERM) or kill (SIGKILL) a process; process 1 and the program itself are refused.
    fn signal(&self, pid: u32, hard: bool) -> Result<(), String>;
}

impl<T: SysRead + ?Sized> SysRead for &T {
    fn cpu_times(&self, r: &SysRoots) -> CpuTimes {
        (**self).cpu_times(r)
    }
    fn cpu_count(&self, r: &SysRoots) -> usize {
        (**self).cpu_count(r)
    }
    fn memory_of(&self, r: &SysRoots) -> Memory {
        (**self).memory_of(r)
    }
    fn load_average(&self, r: &SysRoots) -> (f32, f32, f32) {
        (**self).load_average(r)
    }
    fn uptime_of(&self, r: &SysRoots) -> u64 {
        (**self).uptime_of(r)
    }
    fn hottest_temp(&self, r: &SysRoots) -> Option<f32> {
        (**self).hottest_temp(r)
    }
    fn net_counters(&self, r: &SysRoots) -> Vec<NetCounters> {
        (**self).net_counters(r)
    }
    fn disk_counters(&self, r: &SysRoots) -> Vec<DiskCounters> {
        (**self).disk_counters(r)
    }
    fn processes(&self, r: &SysRoots) -> Vec<Proc> {
        (**self).processes(r)
    }
    fn net_use(&self, r: &SysRoots, pids: &[u32]) -> HashMap<u32, NetUse> {
        (**self).net_use(r, pids)
    }
    fn signal(&self, pid: u32, hard: bool) -> Result<(), String> {
        (**self).signal(pid, hard)
    }
}

pub trait SystemPort {
    /// The signed-in person.
    fn user(&self) -> String;
    /// The person manages the system (administrators): sees every process and every run, may stop them, may read the cluster log.
    fn is_admin(&self) -> bool;
    /// A line in the audit log for the signed-in person.
    fn audit(&self, action: &str, subject: &str);
    /// The kernel's figures (processor, memory, load, network, disks, temperature, uptime, signals).
    fn host(&self) -> Box<dyn SysRead + '_>;
    /// The compressed-memory statistics.
    fn memory_stats(&self) -> MemStats;
    /// "MAC: aa:bb:cc:dd:ee:ff (private)" for the adapter in use.
    fn mac_summary(&self) -> Option<String>;
    /// The hardware page's lines.
    fn hardware_report(&self) -> Vec<String>;
    /// Free and total bytes of the volume holding `path`.
    fn disk_space(&self, path: &str) -> Result<(u64, u64), String>;
    /// Where sound plays and why (Settings -> Sound), read from the given `/proc` root.
    fn audio_summary(&self, proc_root: &std::path::Path) -> String;
    /// One line per graphics card ("AMD Radeon Graphics (Raphael) · driver amdgpu"), read from the folder that holds `sys` and `proc`.
    fn gpu_lines(&self, root: &std::path::Path) -> Vec<String>;
    /// The machine's name as the kernel has it, and the one line about the machine.
    fn hostname(&self) -> String;
    fn machine_about(&self) -> String;
    /// The dev cluster IYAKKI manages on this machine: it is installed.
    fn cluster_installed(&self) -> bool;
    /// The dev cluster's service is running.
    fn cluster_running(&self) -> bool;
    fn cluster_health(&self) -> ClusterHealth;
    /// The job that reads the cluster's health again (a few kubectl calls: run it on a worker thread).
    fn cluster_refresh_health(&self) -> Arc<dyn Fn() + Send + Sync>;
    /// A reader of the cluster's pods.
    fn cluster_kubectl(&self) -> Arc<dyn Kubectl>;
    /// The last lines of the cluster's service log.
    fn cluster_log_tail(&self, lines: usize) -> String;
    /// The agent runs going on now.
    fn live_runs(&self) -> Vec<LiveRun>;
    /// Stop one run; false when it was not found.
    fn stop_live(&self, id: &str) -> bool;
    /// Stop every run (of one person, or of everyone with `None`); how many were stopped.
    fn stop_all_live(&self, only: Option<&str>) -> usize;
    /// WebAssembly tasks of the supervisor running now.
    fn wasm_tasks(&self) -> Vec<WasmTask>;
    /// The programs started for the person (everyone's for an administrator).
    fn programs_mine(&self, roots: &SysRoots) -> Vec<OwnedProc>;
    /// Stop (or kill) a program that was started for the person.
    fn program_stop(&self, roots: &SysRoots, pid: u32, hard: bool) -> Result<String, String>;
    /// A command line with credentials replaced.
    fn redact(&self, line: &str) -> String;
    /// The person's flows scheduler is running.
    fn scheduler_running(&self) -> bool;
}
