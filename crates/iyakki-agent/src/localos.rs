//! The local implementation of the core-API read traits (`Sysinfo`, `Disks`, `Net`) for code in the agent (F-ARCH-03). Since F-ARCH-05 this is a
//! thin wrapper: the reading is done by `iyakki_core_local::LocalOs`, the one implementation of the core API on this machine; the agent's callers
//! keep their `real()` and `at(SysRoots)` constructors. Nothing here writes.

use iyakki_core_api::devices::{BlockDev, Disks, Mount, Net, NetCounters};
use iyakki_core_api::error::ApiResult;
use iyakki_core_api::sysinfo::{LoadAvg, MemInfo, Sysinfo, Uptime};

use crate::sysinfo::SysRoots;

pub struct LocalOs(iyakki_core_local::LocalOs);

impl LocalOs {
    pub fn real() -> LocalOs {
        LocalOs(iyakki_core_local::LocalOs::system())
    }

    pub fn at(roots: SysRoots) -> LocalOs {
        LocalOs(iyakki_core_local::LocalOs::with_roots(roots.proc, roots.sys))
    }
}

impl Sysinfo for LocalOs {
    fn cmdline(&self) -> ApiResult<String> {
        self.0.cmdline()
    }
    fn meminfo(&self) -> ApiResult<MemInfo> {
        self.0.meminfo()
    }
    fn uptime(&self) -> ApiResult<Uptime> {
        self.0.uptime()
    }
    fn loadavg(&self) -> ApiResult<LoadAvg> {
        self.0.loadavg()
    }
    fn product_name(&self) -> ApiResult<String> {
        self.0.product_name()
    }
    fn cpu_count(&self) -> ApiResult<u32> {
        self.0.cpu_count()
    }
}

impl Disks for LocalOs {
    fn block_devices(&self) -> ApiResult<Vec<BlockDev>> {
        self.0.block_devices()
    }
    fn mounts(&self) -> ApiResult<Vec<Mount>> {
        self.0.mounts()
    }
    fn space(&self, path: &str) -> ApiResult<(u64, u64)> {
        self.0.space(path)
    }
    fn reword_failure(&self, msg: &str) -> String {
        self.0.reword_failure(msg)
    }
}

impl Net for LocalOs {
    fn interfaces(&self) -> ApiResult<Vec<NetCounters>> {
        self.0.interfaces()
    }
    fn ipv4_addresses(&self) -> ApiResult<Vec<(String, String)>> {
        self.0.ipv4_addresses()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use iyakki_core_api::error::ApiError;

    fn fake_roots(tag: &str) -> SysRoots {
        let d = std::env::temp_dir().join(format!("iyakki-localos-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        let (p, s) = (d.join("proc"), d.join("sys"));
        std::fs::create_dir_all(p.join("net")).unwrap();
        std::fs::create_dir_all(s.join("class/dmi/id")).unwrap();
        std::fs::write(p.join("cmdline"), "quiet root=/dev/sda1\n").unwrap();
        std::fs::write(p.join("meminfo"), "MemTotal: 8000 kB\nMemFree: 1000 kB\nMemAvailable: 4000 kB\nSwapTotal: 2000 kB\nSwapFree: 500 kB\n").unwrap();
        std::fs::write(p.join("uptime"), "100.50 300.25\n").unwrap();
        std::fs::write(p.join("loadavg"), "0.10 0.20 0.30 1/100 999\n").unwrap();
        std::fs::write(p.join("stat"), "cpu  1 2 3 4\ncpu0 1 2 3 4\ncpu1 1 2 3 4\nintr 5\n").unwrap();
        std::fs::write(p.join("partitions"), "major minor  #blocks  name\n\n   8        0  1000000 sda\n").unwrap();
        std::fs::write(p.join("mounts"), "/dev/sda1 / ext4 rw,relatime 0 0\n").unwrap();
        std::fs::write(p.join("net/dev"), "Inter-|   Receive |  Transmit\n face |bytes packets errs drop fifo frame compressed multicast|bytes packets errs drop fifo colls carrier compressed\n  eth0: 100 1 0 0 0 0 0 0 200 2 0 0 0 0 0 0\n").unwrap();
        std::fs::write(s.join("class/dmi/id/product_name"), "Test Box\n").unwrap();
        SysRoots { proc: p, sys: s, passwd: d.join("passwd") }
    }

    #[test]
    fn the_agents_local_os_is_the_core_local_one_on_a_made_up_kernel() {
        let os = LocalOs::at(fake_roots("ok"));
        assert_eq!(os.cmdline().unwrap(), "quiet root=/dev/sda1");
        let m = os.meminfo().unwrap();
        assert_eq!((m.total_kb, m.available_kb, m.swap_free_kb), (8000, 4000, 500));
        assert_eq!(os.uptime().unwrap().up_secs, 100.5);
        assert_eq!(os.loadavg().unwrap().fifteen, 0.30);
        assert_eq!(os.product_name().unwrap(), "Test Box");
        assert_eq!(os.cpu_count().unwrap(), 2, "the summary line `cpu` is not a processor");
        assert_eq!(os.block_devices().unwrap()[0].name, "sda");
        assert_eq!(os.mounts().unwrap()[0].target, "/");
        let n = os.interfaces().unwrap();
        assert_eq!((n[0].name.as_str(), n[0].rx_bytes, n[0].tx_bytes), ("eth0", 100, 200));
    }

    #[test]
    fn a_missing_file_is_unsupported_and_an_empty_product_name_too() {
        let os = LocalOs::at(SysRoots { proc: "/nonexistent-proc".into(), sys: "/nonexistent-sys".into(), passwd: "/nonexistent".into() });
        assert!(matches!(os.meminfo(), Err(ApiError::Unsupported(_))));
        assert!(matches!(os.product_name(), Err(ApiError::Unsupported(_))));
        let r = fake_roots("empty");
        std::fs::write(r.sys.join("class/dmi/id/product_name"), "\n").unwrap();
        assert!(matches!(LocalOs::at(r).product_name(), Err(ApiError::Unsupported(_))));
    }

    #[test]
    fn the_real_implementation_answers_on_linux_and_says_unsupported_elsewhere() {
        let os = LocalOs::real();
        match os.meminfo() {
            Ok(m) => assert!(m.total_kb > 0),
            Err(e) => assert!(matches!(e, ApiError::Unsupported(_)), "{e}"),
        }
    }
}
