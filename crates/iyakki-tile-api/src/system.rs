//! The plain data of the SYSTEM tile that has no pure leaf crate (alpha.9 P6, D-S1-E4-01 option (b)): the agent runs going on, the WebAssembly
//! tasks, the dev cluster's health and the memory figures are plain mirrors of `iyakki_shared::runs::LiveRun`, `iyakki_core_api::WasmTask`,
//! `iyakki_shared::k3sdev::Health` and `iyakki_zram::MemStats`; the shell converts one to the other (a test keeps them in step). A mirror keeps
//! only what the page reads. The processor, memory, network and disk figures and the process rows are the leaf crate `iyakki-sysinfo-model`.

/// An agent run that is going on now.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LiveRun {
    pub id: String,
    pub user: String,
    /// The start of what was asked.
    pub prompt: String,
    pub started: i64,
    /// Tool calls waiting for an answer.
    pub pending: usize,
    /// Events so far.
    pub steps: usize,
    /// Tokens used so far, when the run has reported them.
    pub tokens: u64,
    /// What happened last, in a few words.
    pub last: String,
}

/// A WebAssembly task the supervisor is running right now.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WasmTask {
    pub id: u64,
    pub user: String,
    pub entry: String,
    /// Size of the module in bytes.
    pub bytes: usize,
    /// When it started (unix seconds).
    pub started: i64,
}

/// What the Overview shows of the dev cluster IYAKKI manages on this machine. `line` is the one line the page prints (`Health::line`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ClusterHealth {
    pub installed: bool,
    /// `not installed`, `stopped`, `starting`, `running`, `stopping`, `failed`.
    pub state: String,
    pub line: String,
    pub failed_reason: String,
    pub data_bytes: Option<u64>,
    pub disk_pct: Option<u8>,
    pub restarts: u32,
    pub last_restart_reason: String,
    /// When it was read last (unix seconds).
    pub checked_at: i64,
}

/// Compressed memory (zram) as the memory card shows it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Zram {
    pub algorithm: String,
    /// The size of the device; zero means the compressed memory is off.
    pub disksize: u64,
    /// Bytes held, before and after compression.
    pub orig: u64,
    pub compressed: u64,
}

impl Zram {
    /// Compression ratio (original / compressed), `None` while empty.
    pub fn ratio(&self) -> Option<f64> {
        (self.compressed > 0).then(|| self.orig as f64 / self.compressed as f64)
    }
}

/// The memory facts beyond installed and free: compressed memory and pressure.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct MemStats {
    /// `None`: no zram on this machine.
    pub zram: Option<Zram>,
    /// Memory pressure "some" averages over 10 / 60 / 300 s, percent.
    pub pressure: Option<[f32; 3]>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_ratio_is_empty_until_something_is_compressed() {
        let mut z = Zram { algorithm: "zstd".into(), disksize: 4 << 30, orig: 3 << 30, compressed: 1 << 30 };
        assert_eq!(z.ratio(), Some(3.0));
        z.compressed = 0;
        assert_eq!(z.ratio(), None);
    }
}
