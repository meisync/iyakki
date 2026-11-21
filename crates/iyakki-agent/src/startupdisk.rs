//! The startup disk of this computer (F-ARCH-06): the firmware work is `iyakki_core_local::startupdisk` (re-exported here, with the core's
//! [`iyakki_core_api::startupdisk::StartupDisk`] over it); this file keeps the once-per-boot call of the boot step and the `iyakki startup-disk` command.

pub use iyakki_core_api::startupdisk::StartupDisk;
pub use iyakki_core_local::startupdisk::*;

use std::sync::atomic::AtomicBool;

static DONE: AtomicBool = AtomicBool::new(false);

/// This computer's variables and data folder; a test of this crate sees a computer without firmware variables and never the real one.
pub fn env_real() -> Env {
    #[cfg(test)]
    return Env::nowhere();
    #[cfg(not(test))]
    Env::real()
}

/// The core's startup-disk handle for this computer, the old boot order kept in `backup` (`None`: the machine's own data folder). A test of this
/// crate gets a computer without firmware variables, never the real one.
pub fn local(backup: Option<std::path::PathBuf>) -> LocalStartupDisk {
    #[cfg(test)]
    {
        let _ = backup;
        return LocalStartupDisk::with_env(Env::nowhere);
    }
    #[cfg(not(test))]
    LocalStartupDisk::system(backup)
}

/// Called every poll; acts once, at 12 s of uptime.
pub fn step(uptime_secs: u64) {
    if cfg!(test) {
        return;
    }
    act_real(uptime_secs, &DONE, &mut |level, m| match level {
        Level::Warn => log::warn!("{m}"),
        Level::Info => log::info!("{m}"),
    });
}


/// `iyakki startup-disk status | register | first | next | own-first | revert`: what the firmware starts first, and (deliberately, needs root) make
/// IYAKKI's own boot entry, put it first, or start it at the next restart only. `own-first` is the old name of `first` for a computer that already
/// has the entry; `revert` puts the earlier order back.
pub fn cli(args: &[String]) -> i32 {
    let env = env_real();
    let disk = local(Some(env.backup.clone()));
    let reboot_setting = std::fs::read_to_string(env.backup.join("settings.json")).ok().and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok()).and_then(|v| v["startup_reboot_into_iyakki"].as_bool()).unwrap_or(false);
    let done = |r: Result<String, String>| match r {
        Ok(m) => {
            crate::qprintln!("{m}");
            0
        }
        Err(e) => {
            crate::qeprintln!("{e}");
            1
        }
    };
    match args.first().map(String::as_str) {
        None | Some("status") => {
            let l = status_lines(&env, reboot_setting);
            l.iter().for_each(|x| crate::qprintln!("{x}"));
            if disk.firmware_present() { 0 } else { 1 }
        }
        Some("register") => done(register(&env)),
        Some("first") => done(first(&env)),
        Some("own-first") => match disk.own_entry_first(true) {
            Ok(Some(m)) => {
                crate::qprintln!("{m}. `iyakki startup-disk revert` puts the old order back.");
                0
            }
            Ok(None) => {
                crate::qprintln!("Nothing to change: IYAKKI's own entry is already first, or this computer has no IYAKKI entry (`iyakki startup-disk register` makes one).");
                0
            }
            Err(e) => {
                crate::qeprintln!("{e}");
                1
            }
        },
        Some("next") => done(next(&env)),
        Some("revert") => done(revert(&env)),
        _ => {
            crate::qeprintln!("usage: iyakki startup-disk status | register | first | next | own-first | revert");
            2
        }
    }
}
