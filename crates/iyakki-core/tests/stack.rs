//! The stack sizes IYAKKI runs on: the Linux norm (8 MiB) for every thread, more for the supervisor and the shell.

use iyakki_core::console::{run_with_big_stack, MAIN_STACK, SHELL_STACK, THREAD_STACK};

fn deep(n: usize) -> usize {
    let pad = [n as u8; 512];
    if n == 0 { pad[0] as usize } else { deep(n - 1) + pad[(n % 512) as usize] as usize % 2 }
}

#[test]
fn the_stack_sizes_follow_the_linux_norm() {
    assert_eq!(THREAD_STACK, 8 * 1024 * 1024, "glibc-based Linux: ulimit -s 8192");
    assert!(MAIN_STACK >= THREAD_STACK && SHELL_STACK >= THREAD_STACK);
    // starting the supervisor makes 8 MiB the default for threads started afterwards (the standard library
    // reads RUST_MIN_STACK when it starts a thread; the test harness has already started its own threads, so
    // here only the setting itself can be checked)
    assert_eq!(run_with_big_stack(|| std::env::var("RUST_MIN_STACK").ok()).unwrap().as_deref(), Some("8388608"));
}

#[test]
fn threads_with_the_standard_stack_survive_deep_recursion_and_the_main_thread_survives_much_deeper() {
    // about 8 000 frames of >512 bytes is 4 MB (debug builds use more per frame): inside 8 MiB
    let ok = std::thread::Builder::new().stack_size(THREAD_STACK).spawn(|| deep(6_000)).unwrap().join().is_ok();
    assert!(ok);
    // the supervisor thread has room for far more
    assert!(run_with_big_stack(|| deep(60_000)).is_ok());
}
