//! Moved out of `iyakki_agent::quiet` (F-SPLIT-03, bucket b0); the agent re-exports it.


use std::time::Duration;

/// Where the kernel lists the sound cards.
pub const ASOUND: &str = "/proc/asound";

/// What the shutdown needs from the machine.
pub trait Env: Send + Sync {
    /// Run `amixer` with these arguments.
    fn amixer(&self, args: &[String]) -> Result<(), String>;
    /// Send a signal (15 or 9) to a process.
    fn signal(&self, pid: u32, sig: i32);
    /// Wait.
    fn sleep(&self, d: Duration);
    /// Play a short period of silence through the `iyakki` device, taking at most `max`.
    fn silence(&self, max: Duration);
    /// This process (never signalled).
    fn own_pid(&self) -> u32;
}
