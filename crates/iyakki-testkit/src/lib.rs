//! Test support that removes the usual causes of flaky tests (owner order 2026-11-18: "add smartness to the tests and make them pass always").
//!
//! The class of flake and its cure:
//! * a test and another test touch the same process-wide thing (input state, environment, current folder, the cluster, a fixed port,
//!   the audio mixer, the clock): hold a NAMED LOCK ([`lock`]) for the short critical section; it works across threads and across test
//!   processes (an `flock` on a file under `/tmp`), has a timeout, and names the holder when it times out;
//! * two tests get the same port, or a "nothing listens" port is taken by another test: [`port`] reserves a port until the test takes
//!   it, [`closed_port`] hands out a port nobody else of this machine can claim;
//! * a test waits a fixed time (`sleep(50ms)`, `for _ in 0..600`) for something in the background: [`wait_until`] polls against a generous WALL-CLOCK
//!   budget and says the last state it saw when the budget ends;
//! * a stub child lives for a fixed time (`sleep 2`) and the test is slower than that: [`GatedChild`] lives until the test lets it go;
//! * a test asserts "took less than X ms": assert behaviour; where speed is the point inject a [`ManualClock`].
//!
//! Nothing here is shipped: add the crate as a `[dev-dependencies]` entry only.

pub mod clock;
pub mod gate;
pub mod lock;
pub mod ports;
pub mod wait;

pub use clock::ManualClock;
pub use gate::{wait_gate_snippet, ChildGuard, GatedChild};
pub use lock::{lock, lock_timeout, names, EnvGuard, Lock};
pub use ports::{closed_port, port, Port};
pub use wait::{wait_for_file, wait_for_line, wait_for_state, wait_until, BUDGET};
