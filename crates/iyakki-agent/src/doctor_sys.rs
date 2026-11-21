//! The parts of `doctor` that read the machine through the agent's `sysinfo` and `audioout` (the findings and their rules are `iyakki_shared::doctor`).

pub use iyakki_core_local::doctorsys::uptime_secs;

/// The sound outputs the kernel lists now.
pub fn sound_outputs() -> Vec<crate::audioout::Output> {
    crate::audioout::scan(std::path::Path::new(iyakki_core_local::kernelfacts::PROC), &|_| None)
}
