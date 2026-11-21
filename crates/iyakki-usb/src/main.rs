use std::process::ExitCode;

#[cfg(not(windows))]
mod unix_main;

fn main() -> ExitCode {
    #[cfg(windows)]
    let r = {
        let args: Vec<String> = std::env::args().skip(1).collect();
        iyakki_usb::windows_cli::run_real(&args).map(|c| ExitCode::from(c.clamp(0, 255) as u8))
    };
    #[cfg(not(windows))]
    let r = unix_main::run();
    match r {
        Ok(code) => code,
        Err(e) => {
            eprintln!("\x1b[31miyakki-usb: {e}\x1b[0m");
            ExitCode::FAILURE
        }
    }
}
