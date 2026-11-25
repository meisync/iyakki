//! What the SYSTEM tile asks of the rest of the system: the trait is `iyakki_tile_system::SystemPort` (the crate `iyakki-tile-system`). This
//! file is the real port ([`CxPort`]), the only place that reads the session and calls the core and the services, and the conversions from the
//! core's and the agent's types to the tile's plain mirrors (`iyakki_tile_api::system`; a test keeps them in step).

use std::sync::Arc;

use iyakki_core_api::memory::MemStats as CoreMem;
use iyakki_core_api::sysinfo::SysRoots;
use iyakki_core_api::HostFacts;
use iyakki_tile_api::system::{ClusterHealth, LiveRun, MemStats, WasmTask, Zram};
pub use iyakki_tile_system::{OwnedProc, SysRead, SystemPort};

use super::AppCx;

/// The core's host facts as the tile's read trait.
pub struct HostRead<'a>(pub &'a dyn HostFacts);

impl SysRead for HostRead<'_> {
    fn cpu_times(&self, r: &SysRoots) -> iyakki_core_api::sysinfo::CpuTimes {
        self.0.cpu_times(r)
    }
    fn cpu_count(&self, r: &SysRoots) -> usize {
        self.0.cpu_count(r)
    }
    fn memory_of(&self, r: &SysRoots) -> iyakki_core_api::sysinfo::Memory {
        self.0.memory_of(r)
    }
    fn load_average(&self, r: &SysRoots) -> (f32, f32, f32) {
        self.0.load_average(r)
    }
    fn uptime_of(&self, r: &SysRoots) -> u64 {
        self.0.uptime_of(r)
    }
    fn hottest_temp(&self, r: &SysRoots) -> Option<f32> {
        self.0.hottest_temp(r)
    }
    fn net_counters(&self, r: &SysRoots) -> Vec<iyakki_core_api::sysinfo::NetCounters> {
        self.0.net_counters(r)
    }
    fn disk_counters(&self, r: &SysRoots) -> Vec<iyakki_core_api::sysinfo::DiskCounters> {
        self.0.disk_counters(r)
    }
    fn processes(&self, r: &SysRoots) -> Vec<iyakki_core_api::sysinfo::Proc> {
        self.0.processes(r)
    }
    fn net_use(&self, r: &SysRoots, pids: &[u32]) -> std::collections::HashMap<u32, iyakki_core_api::sysinfo::NetUse> {
        self.0.net_use(r, pids)
    }
    fn signal(&self, pid: u32, hard: bool) -> Result<(), String> {
        self.0.signal(pid, hard)
    }
}

pub fn mem_stats(m: &CoreMem) -> MemStats {
    MemStats { zram: m.zram.as_ref().map(|z| Zram { algorithm: z.algorithm.clone(), disksize: z.disksize, orig: z.orig, compressed: z.compressed }), pressure: m.pressure }
}

pub fn live_run(r: iyakki_shared::runs::LiveRun) -> LiveRun {
    LiveRun { id: r.id, user: r.user, prompt: r.prompt, started: r.started, pending: r.pending, steps: r.steps, tokens: r.tokens, last: r.last }
}

pub fn wasm_task(t: iyakki_core_api::WasmTask) -> WasmTask {
    WasmTask { id: t.id, user: t.user, entry: t.entry, bytes: t.bytes, started: t.started }
}

pub fn cluster_health(h: &iyakki_shared::k3sdev::Health) -> ClusterHealth {
    ClusterHealth { installed: h.installed, state: h.state.clone(), line: h.line(), failed_reason: h.failed_reason.clone(), data_bytes: h.data_bytes, disk_pct: h.disk_pct, restarts: h.restarts, last_restart_reason: h.last_restart_reason.clone(), checked_at: h.checked_at }
}

pub fn owned_proc(o: iyakki_agent_api::OwnedProc) -> OwnedProc {
    OwnedProc { proc: o.proc, owner: o.owner, what: o.what, net: o.net, disk: o.disk }
}

/// One line per graphics card, as the Overview shows them: "AMD Radeon Graphics (Raphael) · driver amdgpu". Virtual display devices are left
/// out; a card with no driver says so (the NVIDIA driver is a download).
pub fn gpu_summary(gpus: &[iyakki_shared::gpu::Gpu]) -> Vec<String> {
    gpus.iter()
        .filter(|g| !matches!(g.vendor, iyakki_shared::gpu::Vendor::Other(_)))
        .map(|g| {
            let name = if g.name.to_ascii_lowercase().contains(&g.vendor.name().to_ascii_lowercase()) { g.name.clone() } else { format!("{} {}", g.vendor.name(), g.name) };
            let driver = g.driver.clone().map(|d| format!("driver {d}")).unwrap_or_else(|| "no driver loaded".to_string());
            let mem = g.vram.filter(|v| *v > 0).map(|v| format!(" · {}", iyakki_core_api::sysinfo::human_bytes(v))).unwrap_or_default();
            format!("{name} · {driver}{mem}")
        })
        .collect()
}

/// The real port, over the shell's context.
pub struct CxPort<'a>(pub &'a AppCx);

impl SystemPort for CxPort<'_> {
    fn user(&self) -> String {
        self.0.session.user.clone()
    }
    fn is_admin(&self) -> bool {
        self.0.session.policy.manage_settings
    }
    fn audit(&self, action: &str, subject: &str) {
        self.0.audit.record(&self.0.session.user, action, subject);
    }
    fn host(&self) -> Box<dyn SysRead + '_> {
        Box::new(HostRead(self.0.core.host()))
    }
    fn memory_stats(&self) -> MemStats {
        mem_stats(&self.0.os.memory().stats())
    }
    fn mac_summary(&self) -> Option<String> {
        self.0.os.hardware().mac_summary()
    }
    fn hardware_report(&self) -> Vec<String> {
        self.0.os.hardware().hardware_report()
    }
    fn disk_space(&self, path: &str) -> Result<(u64, u64), String> {
        self.0.os.disks().space(path).map_err(|e| e.to_string())
    }
    fn audio_summary(&self, proc_root: &std::path::Path) -> String {
        self.0.agent.audioout_summary_at(proc_root)
    }
    fn gpu_lines(&self, root: &std::path::Path) -> Vec<String> {
        gpu_summary(&iyakki_shared::gpu::detect(root))
    }
    fn hostname(&self) -> String {
        crate::hostname::current("iyakki")
    }
    fn machine_about(&self) -> String {
        iyakki_shared::relinfo::machine_about().to_string()
    }
    fn cluster_installed(&self) -> bool {
        self.0.k3sdev().installed().is_some()
    }
    fn cluster_running(&self) -> bool {
        self.0.k3sdev().state() == iyakki_shared::k3sdev::Svc::Running
    }
    fn cluster_health(&self) -> ClusterHealth {
        cluster_health(&self.0.k3sdev().health())
    }
    fn cluster_refresh_health(&self) -> Arc<dyn Fn() + Send + Sync> {
        let k = self.0.k3sdev();
        Arc::new(move || {
            k.refresh_health();
        })
    }
    fn cluster_kubectl(&self) -> Arc<dyn iyakki_sysinfo_model::kubectl::Kubectl> {
        self.0.k3sdev().kubectl_reader()
    }
    fn cluster_log_tail(&self, lines: usize) -> String {
        self.0.k3sdev().log_tail(lines)
    }
    fn live_runs(&self) -> Vec<LiveRun> {
        self.0.runs().live().into_iter().map(live_run).collect()
    }
    fn stop_live(&self, id: &str) -> bool {
        self.0.runs().stop_live(id)
    }
    fn stop_all_live(&self, only: Option<&str>) -> usize {
        self.0.runs().stop_all_live(only)
    }
    fn wasm_tasks(&self) -> Vec<WasmTask> {
        self.0.host.wasm_tasks().into_iter().map(wasm_task).collect()
    }
    fn programs_mine(&self, roots: &SysRoots) -> Vec<OwnedProc> {
        self.0.agent.proc_owner().mine(roots, &self.0.session.user, self.0.session.policy.manage_settings).into_iter().map(owned_proc).collect()
    }
    fn program_stop(&self, roots: &SysRoots, pid: u32, hard: bool) -> Result<String, String> {
        self.0.agent.proc_owner().stop(roots, &self.0.session.user, self.0.session.policy.manage_settings, pid, hard)
    }
    fn redact(&self, line: &str) -> String {
        self.0.agent.sysprocs_redact(line)
    }
    fn scheduler_running(&self) -> bool {
        self.0.core.flows_view().scheduler_running(&self.0.session.user)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_memory_mirror_keeps_the_zram_figures_and_the_pressure() {
        let mut m = CoreMem::default();
        assert_eq!(mem_stats(&m), MemStats::default());
        m.zram = Some(iyakki_core_api::device_types::zram::ZramStats { algorithm: "lz4".into(), disksize: 8, orig: 6, compressed: 3, ..Default::default() });
        m.pressure = Some([1.0, 2.0, 3.0]);
        let t = mem_stats(&m);
        assert_eq!(t.zram, Some(Zram { algorithm: "lz4".into(), disksize: 8, orig: 6, compressed: 3 }));
        assert_eq!((t.pressure, t.zram.unwrap().ratio()), (Some([1.0, 2.0, 3.0]), Some(2.0)));
    }

    #[test]
    fn a_run_and_a_wasm_task_keep_every_field() {
        let r = iyakki_shared::runs::LiveRun { id: "r".into(), user: "u".into(), prompt: "p".into(), started: 5, pending: 1, steps: 2, tokens: 3, last: "l".into() };
        assert_eq!(live_run(r), LiveRun { id: "r".into(), user: "u".into(), prompt: "p".into(), started: 5, pending: 1, steps: 2, tokens: 3, last: "l".into() });
        let t = iyakki_core_api::WasmTask { id: 9, user: "u".into(), entry: "main".into(), bytes: 100, started: 7 };
        assert_eq!(wasm_task(t), WasmTask { id: 9, user: "u".into(), entry: "main".into(), bytes: 100, started: 7 });
    }

    #[test]
    fn the_cluster_mirror_carries_the_line_the_page_prints() {
        let h = iyakki_shared::k3sdev::Health { installed: true, state: "running".into(), version: "v1.30".into(), restarts: 2, last_restart_reason: "oom".into(), data_bytes: Some(5), disk_pct: Some(90), checked_at: 42, ..Default::default() };
        let c = cluster_health(&h);
        assert_eq!((c.line.as_str(), c.state.as_str(), c.restarts, c.disk_pct, c.data_bytes, c.checked_at), (h.line().as_str(), "running", 2, Some(90), Some(5), 42));
        assert!(c.line.starts_with("running"));
        assert_eq!(cluster_health(&iyakki_shared::k3sdev::Health::default()).line, "not installed");
    }

    #[test]
    fn graphics_cards_are_named_with_their_driver_and_virtual_ones_are_left_out() {
        use iyakki_shared::gpu::{Gpu, Vendor};
        let card = |vendor: Vendor, name: &str, driver: Option<&str>, vram: Option<u64>| Gpu { pci: "0000:01:00.0".into(), vendor_id: 0, device_id: 0, vendor, name: name.into(), driver: driver.map(str::to_string), card: None, render: None, boot_vga: false, vram };
        let lines = gpu_summary(&[card(Vendor::Amd, "Radeon Graphics (Raphael)", Some("amdgpu"), None), card(Vendor::Nvidia, "GeForce RTX 4070", None, Some(8_000_000_000)), card(Vendor::Other(0x1234), "Virtual", None, None)]);
        assert_eq!(lines.len(), 2, "{lines:?}");
        assert!(lines[0].ends_with("driver amdgpu"), "{lines:?}");
        assert!(lines[1].contains("no driver loaded") && lines[1].contains("8.0 GB"), "{lines:?}");
    }
}
