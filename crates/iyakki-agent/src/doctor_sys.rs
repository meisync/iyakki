//! The parts of `doctor` that read the machine through the agent's `sysinfo` and `audioout` (the findings and their rules are `iyakki_shared::doctor`).

/// Seconds since the machine started (a huge value when unknown, so "just booted" is never claimed by mistake).
pub fn uptime_secs() -> u64 {
    crate::sysinfo::uptime_f64(crate::sysinfo::proc_root()).map_or(u64::MAX / 2, |v| v as u64)
}

/// The sound outputs the kernel lists now.
pub fn sound_outputs() -> Vec<crate::audioout::Output> {
    crate::audioout::scan(std::path::Path::new("/proc"), &|_| None)
}
