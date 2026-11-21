//! The kernel read of the DOCTOR facts that does not need the agent (F-ARCH-07): seconds since the machine started.

/// Seconds since the machine started (a huge value when unknown, so "just booted" is never claimed by mistake).
pub fn uptime_secs() -> u64 {
    crate::sysinfo::uptime_f64(crate::sysinfo::proc_root()).map_or(u64::MAX / 2, |v| v as u64)
}
