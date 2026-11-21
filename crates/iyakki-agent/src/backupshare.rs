//! The real mounter of a backup share (F-BACKUP-04): runs the machine's own `mount` and `umount` with a fixed argument list and no shell.
//! The flow, the address parsing and the credentials handling are `iyakki_shared::backup_share`.

pub use iyakki_shared::backup_share::RealMounter;

#[cfg(test)]
mod tests {
    use super::*;
    use iyakki_shared::backup_share::Mounter;

    /// With no program to run the real mounter gives an error, never a panic, and a failing mount says why.
    #[test]
    fn a_mount_that_fails_says_why_and_is_never_a_panic() {
        let e = RealMounter.mount(&["-t".into(), "nosuchfs".into(), "//x/y".into(), "/nonexistent-iyakki-mp".into()]).unwrap_err();
        assert!(!e.is_empty());
    }
}
