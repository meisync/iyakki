//! Hand-over of the screen and keyboard to a separate full-screen program (the optional full web engine).
//!
//! An app cannot let go of the framebuffer itself: the display loop in [`crate::backend`] owns it. So the app leaves
//! a [`Job`] here, the loop notices it, closes the framebuffer and the input devices, runs the program, takes the
//! screen back and reports how it went. Only the display loop of a real screen says it can do this
//! ([`set_supported`]); in the desktop test window or the browser front end the request is refused.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

/// What to run for whom.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Job {
    pub url: String,
    pub user: String,
    /// The data directory the profile folder lives under.
    pub data: PathBuf,
}

static SUPPORTED: AtomicBool = AtomicBool::new(false);
static ACTIVE: AtomicBool = AtomicBool::new(false);
static SLOT: Mutex<Option<Job>> = Mutex::new(None);
static RESULT: Mutex<Option<Result<i32, String>>> = Mutex::new(None);

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// The display loop of a real screen calls this when it starts (true) and ends (false).
pub fn set_supported(on: bool) {
    SUPPORTED.store(on, Ordering::SeqCst);
}

pub fn supported() -> bool {
    SUPPORTED.load(Ordering::SeqCst)
}

/// Ask for the screen. Fails when this display cannot hand it over or a job is already waiting or running.
pub fn request(job: Job) -> Result<(), String> {
    if !supported() {
        return Err("This screen cannot hand over to the full browser (it runs only on the machine's own display).".into());
    }
    if ACTIVE.load(Ordering::SeqCst) {
        return Err("The full browser is already open.".into());
    }
    let mut slot = lock(&SLOT);
    if slot.is_some() {
        return Err("The full browser is already starting.".into());
    }
    *slot = Some(job);
    Ok(())
}

/// The display loop: take the waiting job, if any.
pub fn take() -> Option<Job> {
    let job = lock(&SLOT).take();
    if job.is_some() {
        ACTIVE.store(true, Ordering::SeqCst);
    }
    job
}

/// The display loop: the job ended (exit status, or why it could not run) and the screen is back.
pub fn finish(result: Result<i32, String>) {
    *lock(&RESULT) = Some(result);
    ACTIVE.store(false, Ordering::SeqCst);
}

/// The app: how the last job went, once.
pub fn take_result() -> Option<Result<i32, String>> {
    lock(&RESULT).take()
}

/// A job is waiting or running.
pub fn busy() -> bool {
    ACTIVE.load(Ordering::SeqCst) || lock(&SLOT).is_some()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex as TestLock;

    // the state is process-wide: tests that touch it take turns
    static ONE_AT_A_TIME: TestLock<()> = TestLock::new(());

    fn job(n: &str) -> Job {
        Job { url: format!("https://{n}/"), user: "u".into(), data: "/tmp".into() }
    }

    #[test]
    fn a_job_is_handed_over_once_and_reported_once() {
        let _g = ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
        while take().is_some() {}
        finish(Ok(0));
        let _ = take_result();
        set_supported(false);
        assert!(request(job("a.org")).unwrap_err().contains("cannot hand over"), "refused on a screen that cannot do it");
        set_supported(true);
        assert!(request(job("a.org")).is_ok());
        assert!(busy());
        assert!(request(job("b.org")).unwrap_err().contains("already"), "one at a time");
        assert_eq!(take(), Some(job("a.org")));
        assert!(take().is_none());
        assert!(busy(), "running");
        assert!(request(job("c.org")).unwrap_err().contains("already open"));
        finish(Err("boom".into()));
        assert!(!busy());
        assert_eq!(take_result(), Some(Err("boom".into())));
        assert_eq!(take_result(), None);
        set_supported(false);
    }
}
