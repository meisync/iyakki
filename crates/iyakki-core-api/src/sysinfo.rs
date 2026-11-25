//! Read-only system figures: the kernel command line, memory, uptime, load, the machine's model and processor count.
//! The parsers are pure so the local implementation and the tests share them.

use crate::error::{ApiError, ApiResult};
use serde::{Deserialize, Serialize};

/// Memory figures in kibibytes (the unit of `/proc/meminfo`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemInfo {
    pub total_kb: u64,
    pub free_kb: u64,
    pub available_kb: u64,
    pub swap_total_kb: u64,
    pub swap_free_kb: u64,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Uptime {
    pub up_secs: f64,
    /// Summed idle time of all processors, as the kernel reports it.
    pub idle_secs: f64,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct LoadAvg {
    pub one: f64,
    pub five: f64,
    pub fifteen: f64,
}

/// Read-only system information. Method names follow what the tiles read today (docs/development/UI-OS-SPLIT-PLAN.md, step 5).
pub trait Sysinfo: Send + Sync {
    /// The kernel command line, one line, without the trailing newline.
    fn cmdline(&self) -> ApiResult<String>;
    fn meminfo(&self) -> ApiResult<MemInfo>;
    fn uptime(&self) -> ApiResult<Uptime>;
    fn loadavg(&self) -> ApiResult<LoadAvg>;
    /// The product name from the firmware (`Mac mini`, `ThinkPad X1`); `Unsupported` where there is none.
    fn product_name(&self) -> ApiResult<String>;
    fn cpu_count(&self) -> ApiResult<u32>;
}

/// The machine the program runs on, as one handle (docs/development/UI-OS-SPLIT-PLAN.md step 5). Tiles ask `cx.os` instead of reading `/proc` and `/sys`;
/// the device gives them `iyakki_core_local::LocalOs`, tests give them `iyakki_core_fake::FakeOs`. Later slices add accessors (audio, disks, net).
pub trait Os: Send + Sync {
    fn sysinfo(&self) -> &dyn Sysinfo;
    fn audio(&self) -> &dyn crate::audio::Audio;
    /// The kernel facts sound routing reads (`/proc/asound`, who holds a playback device, the mixer's controls). A machine that does not say otherwise knows none.
    fn audio_route(&self) -> std::sync::Arc<dyn crate::audioroute::AudioRoute> {
        std::sync::Arc::new(crate::audioroute::NoAudioRoute)
    }
    fn disks(&self) -> &dyn crate::devices::Disks;
    fn net(&self) -> &dyn crate::devices::Net;
    /// Compressed memory, swap and `system.conf` (a shared handle: the Memory page runs its changes on a thread).
    #[cfg(feature = "native")]
    fn memory(&self) -> std::sync::Arc<dyn crate::memory::Memory>;
    /// Links, radios, brightness, sound cards, private addresses and Bluetooth hardware.
    #[cfg(feature = "native")]
    fn hardware(&self) -> std::sync::Arc<dyn crate::hardware::Hardware>;
    /// The kept kernel logs of earlier sessions.
    #[cfg(feature = "native")]
    fn crashlog(&self) -> std::sync::Arc<dyn crate::crashlog::CrashLog>;
    /// What the kernel says about the screens (connectors, modes, the firmware framebuffer's signature, which card to drive). A machine that does not say otherwise has none.
    fn screen_facts(&self) -> std::sync::Arc<dyn crate::screenfacts::ScreenFacts> {
        std::sync::Arc::new(crate::screenfacts::NoScreenFacts)
    }
    /// Where sound plays now: the device, how loud and a change counter (read only). A machine that does not say otherwise has no selector.
    fn output_state(&self) -> std::sync::Arc<dyn crate::soundctl::OutputState> {
        std::sync::Arc::new(crate::soundctl::NoOutputState)
    }
    /// Bluetooth audio, both directions (reads, and the changes behind `bluetooth.use`). A machine that does not say otherwise has none.
    #[cfg(feature = "native")]
    fn bt_audio(&self) -> std::sync::Arc<dyn crate::btaudio::BtAudio> {
        std::sync::Arc::new(crate::btaudio::NoBtAudio)
    }
    /// The firmware's startup entry of this computer (read, and the four changes behind `system.startup.set`). A machine that does not say otherwise has no firmware variables.
    fn startup_disk(&self) -> std::sync::Arc<dyn crate::startupdisk::StartupDisk> {
        std::sync::Arc::new(crate::startupdisk::NoStartupDisk)
    }
    /// Install to disk: the disks, the source, the writing. A machine that does not say otherwise has none (nothing to install from).
    #[cfg(feature = "native")]
    fn installer(&self) -> std::sync::Arc<dyn crate::installer::Installer> {
        std::sync::Arc::new(crate::installer::NoInstaller)
    }
    /// The drivers of this machine (reads only). A machine that does not say otherwise has none.
    #[cfg(feature = "native")]
    fn drivers(&self) -> std::sync::Arc<dyn crate::drivers::Drivers> {
        std::sync::Arc::new(crate::drivers::NoDrivers)
    }
    /// The drive engine: what is on each drive, and mounting, unmounting and ejecting (reads and checked changes). A machine that does not say otherwise has none.
    #[cfg(feature = "native")]
    fn disk_engine(&self) -> std::sync::Arc<dyn crate::diskmodel::DiskEngine> {
        std::sync::Arc::new(crate::diskmodel::NoDiskEngine)
    }
    /// Finding the machines on the local network.
    #[cfg(feature = "native")]
    fn netscan(&self) -> std::sync::Arc<dyn crate::netscan::NetScan>;
    /// The model router's permissions and access log. A machine that does not say otherwise has none (it refuses).
    #[cfg(feature = "native")]
    fn router(&self) -> std::sync::Arc<dyn crate::router::Router> {
        std::sync::Arc::new(crate::router::NoRouter)
    }
    /// The pair group (fleet) permissions and access log. A machine that does not say otherwise has none (it refuses).
    #[cfg(feature = "native")]
    fn fleet(&self) -> std::sync::Arc<dyn crate::fleet::Fleet> {
        std::sync::Arc::new(crate::fleet::NoFleet)
    }
    /// Long-lived `kubectl port-forward`s to a cluster (start, list, stop; one list for the tile, the chat tool and `dev forward`). A machine that does not say otherwise has none.
    #[cfg(feature = "native")]
    fn port_forwards(&self) -> std::sync::Arc<dyn crate::portforward::PortForwards> {
        std::sync::Arc::new(crate::portforward::NoPortForwards)
    }
}

/// Parse the text of `/proc/meminfo`. `MemTotal` is required; a missing `MemAvailable` falls back to `MemFree`.
pub fn parse_meminfo(text: &str) -> ApiResult<MemInfo> {
    let field = |name: &str| -> Option<u64> {
        text.lines().find_map(|l| l.strip_prefix(name)?.strip_prefix(':')?.split_whitespace().next()?.parse().ok())
    };
    let total_kb = field("MemTotal").ok_or_else(|| ApiError::Io("meminfo has no MemTotal".into()))?;
    let free_kb = field("MemFree").unwrap_or(0);
    Ok(MemInfo { total_kb, free_kb, available_kb: field("MemAvailable").unwrap_or(free_kb), swap_total_kb: field("SwapTotal").unwrap_or(0), swap_free_kb: field("SwapFree").unwrap_or(0) })
}

/// Parse the text of `/proc/uptime` (`12345.67 23456.78`).
pub fn parse_uptime(text: &str) -> ApiResult<Uptime> {
    let mut it = text.split_whitespace();
    let up = it.next().and_then(|v| v.parse::<f64>().ok());
    let idle = it.next().and_then(|v| v.parse::<f64>().ok());
    match (up, idle) {
        (Some(up_secs), Some(idle_secs)) => Ok(Uptime { up_secs, idle_secs }),
        _ => Err(ApiError::Io("uptime is not two numbers".into())),
    }
}

/// Parse the text of `/proc/loadavg` (`0.52 0.58 0.59 1/467 12345`).
pub fn parse_loadavg(text: &str) -> ApiResult<LoadAvg> {
    let v: Vec<f64> = text.split_whitespace().take(3).filter_map(|t| t.parse().ok()).collect();
    match v[..] {
        [one, five, fifteen] => Ok(LoadAvg { one, five, fifteen }),
        _ => Err(ApiError::Io("loadavg is not three numbers".into())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MEM: &str = "MemTotal:       16384000 kB\nMemFree:         1000000 kB\nMemAvailable:    8000000 kB\nSwapTotal:       2048000 kB\nSwapFree:        2000000 kB\n";

    #[test]
    fn meminfo_is_read_and_falls_back_to_free_without_available() {
        let m = parse_meminfo(MEM).unwrap();
        assert_eq!((m.total_kb, m.free_kb, m.available_kb, m.swap_total_kb, m.swap_free_kb), (16384000, 1000000, 8000000, 2048000, 2000000));
        let m = parse_meminfo("MemTotal: 100 kB\nMemFree: 40 kB\n").unwrap();
        assert_eq!((m.available_kb, m.swap_total_kb), (40, 0));
    }

    #[test]
    fn meminfo_without_a_total_is_an_io_error_with_a_stable_code() {
        assert_eq!(parse_meminfo("MemFree: 1 kB").unwrap_err().code(), "io");
        assert_eq!(parse_meminfo("").unwrap_err().code(), "io");
    }

    #[test]
    fn uptime_and_loadavg_parse_and_refuse_junk() {
        assert_eq!(parse_uptime("123.5 400.25\n").unwrap(), Uptime { up_secs: 123.5, idle_secs: 400.25 });
        assert!(parse_uptime("x y").is_err());
        assert!(parse_uptime("5").is_err());
        assert_eq!(parse_loadavg("0.52 0.58 0.59 1/467 12345\n").unwrap(), LoadAvg { one: 0.52, five: 0.58, fifteen: 0.59 });
        assert!(parse_loadavg("0.5 0.5").is_err());
    }
}

// The memory figures, the byte and time words, the roots and the counters of the System page live in the pure leaf crate `iyakki-sysinfo-model`
// (alpha.9 P6, D-S2-01: the SYSTEM tile crate names the leaf crate and not this one); every item is re-exported here under the old path.
pub use iyakki_sysinfo_model::{cpu_percent, human_bytes, human_duration, proc_cpu, protected, CpuTimes, DiskCounters, DiskUse, Memory, NetCounters, NetUse, Proc, SysRoots, TICKS_PER_SEC};
