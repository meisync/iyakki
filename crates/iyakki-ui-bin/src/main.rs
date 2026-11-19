//! `iyakki-ui`: the shell and apps as their own program, supervised by the core (docs/development/UI-BOUNDARY.md section 14).

use iyakki_ui_bin::{handshake, parse, Mode, USAGE};

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
        Mode::Socket(path) => match handshake(&path) {
            Ok(info) => {
                eprintln!("iyakki-ui: core {} serves {}", info.machine, info.domains.join(", "));
                // The screen over the socket needs every service of the shell to come over the contract (UI-BOUNDARY.md section 14: not
                // yet); until then the core runs the shell itself and this program ends with a plain status the supervisor reads.
                eprintln!("iyakki-ui: the shell services are not served over the socket yet; use --in-process");
                std::process::exit(3);
            }
            Err(e) => {
                eprintln!("iyakki-ui: {e}");
                std::process::exit(1);
            }
        },
        Mode::InProcess => in_process(&a.fb),
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
