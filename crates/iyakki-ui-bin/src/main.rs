//! `iyakki-ui`: the shell and apps as their own program, supervised by the core (docs/development/UI-BOUNDARY.md section 14).

use iyakki_ui_bin::{parse, Mode, USAGE};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let a = match parse(&args) {
        Ok(a) => a,
        Err(e) => {
            eprintln!("iyakki-ui: {e}\n{USAGE}");
            std::process::exit(2);
        }
    };
    match a.mode {
        Mode::Socket(path) => socket(&path, &a.fb),
        Mode::InProcess => in_process(&a.fb),
    }
}

/// The shell over the core's socket (feature `socket-shell`): exit status 0 after a normal stop, 1 when the core cannot be reached or the screen
/// fails, 4 when the core serves no object calls.
#[cfg(feature = "socket-shell")]
fn socket(path: &std::path::Path, fb: &str) -> ! {
    use iyakki_ui_bin::socket_shell::{environment, Refused};
    use std::sync::atomic::{AtomicBool, Ordering};
    static STOP: AtomicBool = AtomicBool::new(false);
    extern "C" fn on_signal(_: libc::c_int) {
        STOP.store(true, Ordering::SeqCst);
    }
    // SAFETY: the handler only stores into an atomic.
    unsafe {
        libc::signal(libc::SIGTERM, on_signal as *const () as usize);
        libc::signal(libc::SIGINT, on_signal as *const () as usize);
    }
    // the core opens its socket while it starts: wait for it, a few seconds at most
    let env = match environment(path, false, 40, &|| std::thread::sleep(std::time::Duration::from_millis(250))) {
        Ok(e) => e,
        Err(Refused::NoObjectCalls) => {
            eprintln!("iyakki-ui: the core does not serve the object calls (turn on `rpc` for the UI socket); use --in-process");
            std::process::exit(4);
        }
        Err(Refused::Core(e)) => {
            eprintln!("iyakki-ui: {e}");
            std::process::exit(1);
        }
    };
    // the screen stops when the core stops answering (every 2 s a look, 5 misses in a row): the supervisor starts the program again
    {
        let svc = env.services.clone();
        let _ = std::thread::Builder::new().name("iyakki-ui-watch".into()).spawn(move || {
            let mut w = iyakki_ui_bin::CoreWatch::new(5);
            loop {
                std::thread::sleep(std::time::Duration::from_secs(2));
                if STOP.load(Ordering::SeqCst) {
                    return;
                }
                if w.note(svc.is_real()) {
                    eprintln!("iyakki-ui: the core does not answer any more");
                    std::process::exit(iyakki_ui_bin::EXIT_CORE_LOST);
                }
            }
        });
    }
    let mut sh = iyakki_shell::shell::Shell::new(env);
    match iyakki_shell::backend::run_fb(&mut sh, fb, &STOP) {
        Ok(()) => std::process::exit(0),
        Err(e) => {
            eprintln!("iyakki-ui: {e}");
            std::process::exit(1);
        }
    }
}

/// Without the shell linked in, the program connects, says what the core serves and ends with a status the supervisor reads.
#[cfg(not(feature = "socket-shell"))]
fn socket(path: &std::path::Path, _fb: &str) -> ! {
    match iyakki_ui_bin::connect(path) {
        Ok(Ok(c)) => {
            eprintln!("iyakki-ui: core {} serves {}; the object calls answer", c.info.machine, c.info.domains.join(", "));
            eprintln!("iyakki-ui: this build has no shell (build with --features socket-shell); use --in-process");
            std::process::exit(3);
        }
        Ok(Err(iyakki_ui_bin::NotReady::NoObjectCalls)) => {
            eprintln!("iyakki-ui: the core does not serve the object calls (turn on `rpc` for the UI socket); use --in-process");
            std::process::exit(4);
        }
        Err(e) => {
            eprintln!("iyakki-ui: {e}");
            std::process::exit(1);
        }
    }
}

#[cfg(feature = "in-process")]
fn in_process(fb: &str) -> ! {
    use std::sync::atomic::{AtomicBool, Ordering};
    static STOP: AtomicBool = AtomicBool::new(false);
    extern "C" fn on_signal(_: libc::c_int) {
        STOP.store(true, Ordering::SeqCst);
    }
    // SAFETY: the handler only stores into an atomic.
    unsafe {
        libc::signal(libc::SIGTERM, on_signal as *const () as usize);
        libc::signal(libc::SIGINT, on_signal as *const () as usize);
    }
    iyakki_compose::wire();
    let host = std::sync::Arc::new(iyakki_core::host::CoreHost::new());
    let paths = iyakki_shell::shell::ShellPaths::default();
    let env = match iyakki_shell::shell::ShellEnv::from_paths(&paths, host) {
        Ok(e) => e,
        Err(e) => {
            eprintln!("iyakki-ui: {e}");
            std::process::exit(1);
        }
    };
    let mut sh = iyakki_shell::shell::Shell::new(env);
    match iyakki_shell::backend::run_fb(&mut sh, fb, &STOP) {
        Ok(()) => std::process::exit(0),
        Err(e) => {
            eprintln!("iyakki-ui: {e}");
            std::process::exit(1);
        }
    }
}

#[cfg(not(feature = "in-process"))]
fn in_process(_fb: &str) -> ! {
    eprintln!("iyakki-ui: this build has no in-process shell (build with --features in-process)");
    std::process::exit(2);
}
