//! Keeps the memory of the test process flat (KI-067). Every test runs on a thread of its own, and glibc gives each of those threads
//! a malloc arena that keeps what the test freed: 2,500 tests then hold 8 GB of freed pages although under 200 MB are in use.
//! Big buffers (screens, pictures) are always mapped on their own so they go back when freed, and a helper thread hands the freed
//! pages of every arena back to the system twice a second. Tests only; the program itself is not touched.

#[cfg(all(target_os = "linux", target_env = "gnu"))]
pub fn start() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        // a fixed limit also stops glibc from raising it after each freed big block (it would keep them in the arenas)
        unsafe { libc::mallopt(libc::M_MMAP_THRESHOLD, 128 * 1024) };
        let _ = std::thread::Builder::new().name("iyakki-test-trim".into()).spawn(|| loop {
            unsafe { libc::malloc_trim(0) };
            std::thread::sleep(std::time::Duration::from_millis(500));
        });
    });
}

#[cfg(not(all(target_os = "linux", target_env = "gnu")))]
pub fn start() {}
