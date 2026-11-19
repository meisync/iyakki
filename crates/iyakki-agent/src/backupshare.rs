//! The real mounter of a backup share (F-BACKUP-04): runs the machine's own `mount` and `umount` with a fixed argument list and no shell.
//! The flow, the address parsing and the credentials handling are `iyakki_shared::backup_share`.

use std::path::Path;
use std::process::Command;

use iyakki_shared::backup_share::Mounter;

pub struct RealMounter;

fn run(prog: &str, args: &[String]) -> Result<(), String> {
    let out = Command::new(prog).args(args).output().map_err(|e| format!("cannot run {prog}: {e}"))?;
    if out.status.success() {
        Ok(())
    } else {
        Err(String::from_utf8_lossy(&out.stderr).trim().chars().take(300).collect())
    }
}

impl Mounter for RealMounter {
    fn mount(&self, args: &[String]) -> Result<(), String> {
        run("/bin/mount", args)
    }
    fn unmount(&self, mountpoint: &Path) -> Result<(), String> {
        run("/bin/umount", &[mountpoint.display().to_string()])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// With no program to run the real mounter gives an error, never a panic, and a failing mount says why.
    #[test]
    fn a_mount_that_fails_says_why_and_is_never_a_panic() {
        let e = RealMounter.mount(&["-t".into(), "nosuchfs".into(), "//x/y".into(), "/nonexistent-iyakki-mp".into()]).unwrap_err();
        assert!(!e.is_empty());
    }
}
