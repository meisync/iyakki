//! A backup destination on a network share of its own (F-BACKUP-04): `smb://host/share[/folder]` or `nfs://host/export[/folder]`, mounted by
//! IYAKKI for the backup and let go again afterwards. This module is the pure part and the flow; the program that does the mount is a
//! [`Mounter`] (the real one is `iyakki_agent::backupshare::RealMounter`, tests use [`FakeMounter`]).
//!
//! Decision (docs/development/ALPHA-7-DECISIONS.md, D-BACKUP-04): the credentials of an SMB share live in a VAULT entry of the person
//! (`backup-share/<name>`: first line the user name, second line the password), never in the settings or on a command line. They are written to
//! a credentials file with mode 0600 inside a private folder for the length of the mount only and removed after. NFS needs none.

use std::path::{Path, PathBuf};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Kind {
    Smb,
    Nfs,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ShareSpec {
    pub kind: Kind,
    pub host: String,
    /// The share (SMB) or the export path (NFS), no leading slash.
    pub share: String,
    /// A folder below the share, plain names only.
    pub folder: Vec<String>,
}

fn host_ok(h: &str) -> bool {
    !h.is_empty() && h.len() <= 253 && h.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-')) && !h.starts_with('-') && !h.starts_with('.')
}

fn name_ok(n: &str) -> bool {
    !n.is_empty() && n.len() <= 80 && n != "." && n != ".." && n.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_' | ' '))
}

impl ShareSpec {
    pub fn parse(text: &str) -> Result<ShareSpec, String> {
        let (kind, rest) = if let Some(r) = text.strip_prefix("smb://") {
            (Kind::Smb, r)
        } else if let Some(r) = text.strip_prefix("nfs://") {
            (Kind::Nfs, r)
        } else {
            return Err("A share address starts with smb:// or nfs://.".into());
        };
        let mut parts = rest.split('/');
        let host = parts.next().unwrap_or("");
        if !host_ok(host) {
            return Err("The host name of the share is not valid.".into());
        }
        let names: Vec<&str> = parts.filter(|p| !p.is_empty()).collect();
        if names.is_empty() {
            return Err("The address needs a share name after the host.".into());
        }
        if let Some(bad) = names.iter().find(|n| !name_ok(n)) {
            return Err(format!("\u{201C}{}\u{201D} is not a plain folder name.", bad.chars().take(40).collect::<String>()));
        }
        // an SMB share is one name; an NFS export may be a path, so its first names are the export and none are left for a folder
        let (share, folder) = match kind {
            Kind::Smb => (names[0].to_string(), names[1..].iter().map(|s| s.to_string()).collect()),
            Kind::Nfs => (names.join("/"), Vec::new()),
        };
        Ok(ShareSpec { kind, host: host.to_string(), share, folder })
    }

    /// The source argument of `mount`.
    pub fn source(&self) -> String {
        match self.kind {
            Kind::Smb => format!("//{}/{}", self.host, self.share),
            Kind::Nfs => format!("{}:/{}", self.host, self.share),
        }
    }
}

/// The arguments of `mount` (never a password: SMB reads a credentials file). Options keep the share from running programs on this machine.
pub fn mount_args(spec: &ShareSpec, mountpoint: &Path, creds_file: Option<&Path>) -> Vec<String> {
    let mut a = vec!["-t".to_string(), if spec.kind == Kind::Smb { "cifs".into() } else { "nfs".into() }, spec.source(), mountpoint.display().to_string(), "-o".into()];
    let mut o = vec!["nosuid".to_string(), "nodev".into(), "noexec".into()];
    match spec.kind {
        Kind::Smb => {
            o.push(match creds_file {
                Some(f) => format!("credentials={}", f.display()),
                None => "guest".into(),
            });
            o.push("vers=3.0".into());
        }
        Kind::Nfs => {
            o.push("vers=4".into());
            o.push("soft".into());
        }
    }
    a.push(o.join(","));
    a
}

/// What does the mounting. `unmount` is called even after a failed backup.
pub trait Mounter {
    fn mount(&self, args: &[String]) -> Result<(), String>;
    fn unmount(&self, mountpoint: &Path) -> Result<(), String>;
}

/// Credentials of an SMB share, from the vault entry (two lines).
pub fn parse_credentials(entry: &str) -> Result<(String, String), String> {
    let mut l = entry.lines();
    let (u, p) = (l.next().unwrap_or("").trim(), l.next().unwrap_or(""));
    if u.is_empty() || p.is_empty() || u.contains(['\n', ',', '=']) {
        return Err("The vault entry for the share needs a user name on the first line and a password on the second.".into());
    }
    Ok((u.to_string(), p.to_string()))
}

/// Mount the share under `work` (a private folder the caller made, mode 0700), run `body` on the folder to write into (the share's `folder` below
/// the mount, created if missing), and always unmount and remove the credentials file and the mountpoint.
pub fn with_share<T>(spec: &ShareSpec, creds: Option<(&str, &str)>, mounter: &dyn Mounter, work: &Path, body: impl FnOnce(&Path) -> Result<T, String>) -> Result<T, String> {
    use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
    let mp = work.join("mnt");
    std::fs::create_dir_all(&mp).map_err(|e| format!("cannot make the mount folder: {e}"))?;
    std::fs::set_permissions(&mp, std::fs::Permissions::from_mode(0o700)).map_err(|e| format!("cannot protect the mount folder: {e}"))?;
    let mut cred_file: Option<PathBuf> = None;
    if let (Kind::Smb, Some((u, p))) = (&spec.kind, creds) {
        let f = work.join("creds");
        let mut o = std::fs::OpenOptions::new();
        o.write(true).create_new(true).mode(0o600);
        let mut h = o.open(&f).map_err(|e| format!("cannot write the credentials file: {e}"))?;
        use std::io::Write;
        write!(h, "username={u}\npassword={p}\n").map_err(|e| format!("cannot write the credentials file: {e}"))?;
        cred_file = Some(f);
    }
    let args = mount_args(spec, &mp, cred_file.as_deref());
    let mounted = mounter.mount(&args);
    let result = match mounted {
        Err(e) => Err(format!("The share could not be mounted: {e}")),
        Ok(()) => {
            let mut dest = mp.clone();
            for n in &spec.folder {
                dest.push(n);
            }
            let r = std::fs::create_dir_all(&dest).map_err(|e| format!("cannot use the folder on the share: {e}")).and_then(|_| body(&dest));
            let un = mounter.unmount(&mp);
            match (r, un) {
                (Ok(v), Ok(())) => Ok(v),
                (Ok(_), Err(e)) => Err(format!("The backup was written but the share could not be let go: {e}")),
                (Err(e), _) => Err(e),
            }
        }
    };
    if let Some(f) = cred_file {
        let _ = std::fs::remove_file(f);
    }
    let _ = std::fs::remove_dir(&mp);
    result
}

/// Back up to a share: mount it, write the backup file into the share's folder, let the share go. Returns the name and size of the file written.
pub fn create_on_share(l: &crate::backup::Layout, o: &crate::backup::CreateOptions, spec: &ShareSpec, creds: Option<(&str, &str)>, mounter: &dyn Mounter, work: &Path) -> Result<(String, u64), String> {
    with_share(spec, creds, mounter, work, |dir| {
        let c = crate::backup::create(l, dir, o).map_err(|e| e.to_string())?;
        Ok((c.path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(), c.bytes))
    })
}

/// A mounter that makes the mountpoint look mounted by doing nothing, and remembers what it was asked.
#[derive(Default)]
pub struct FakeMounter {
    pub calls: std::sync::Mutex<Vec<String>>,
    pub fail_mount: bool,
    pub fail_unmount: bool,
    /// What the credentials file said at mount time (to prove it was there and 0600).
    pub seen_creds: std::sync::Mutex<Option<(String, u32)>>,
}

impl Mounter for FakeMounter {
    fn mount(&self, args: &[String]) -> Result<(), String> {
        use std::os::unix::fs::PermissionsExt;
        self.calls.lock().unwrap_or_else(std::sync::PoisonError::into_inner).push(format!("mount {}", args.join(" ")));
        if let Some(f) = args.last().and_then(|o| o.split(',').find_map(|x| x.strip_prefix("credentials="))) {
            let mode = std::fs::metadata(f).map(|m| m.permissions().mode() & 0o777).unwrap_or(0);
            *self.seen_creds.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = Some((std::fs::read_to_string(f).unwrap_or_default(), mode));
        }
        if self.fail_mount {
            Err("no route to host".into())
        } else {
            Ok(())
        }
    }
    fn unmount(&self, mp: &Path) -> Result<(), String> {
        self.calls.lock().unwrap_or_else(std::sync::PoisonError::into_inner).push(format!("umount {}", mp.display()));
        if self.fail_unmount {
            Err("busy".into())
        } else {
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn work(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("iyakki-share-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn share_addresses_are_parsed_strictly() {
        let s = ShareSpec::parse("smb://nas.local/backups/iyakki/amy").unwrap();
        assert_eq!((s.kind.clone(), s.share.as_str(), s.folder.clone()), (Kind::Smb, "backups", vec!["iyakki".to_string(), "amy".to_string()]));
        assert_eq!(s.source(), "//nas.local/backups");
        assert_eq!(ShareSpec::parse("nfs://10.0.0.5/export/bk").unwrap().source(), "10.0.0.5:/export/bk");
        for bad in ["http://x/y", "smb:///share", "smb://host", "smb://ho st/s", "smb://host/../x", "smb://-host/s", "smb://host/a;b", "smb://host/a,b"] {
            assert!(ShareSpec::parse(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn the_mount_line_never_holds_a_password_and_forbids_programs() {
        let s = ShareSpec::parse("smb://nas/bk").unwrap();
        let a = mount_args(&s, Path::new("/m"), Some(Path::new("/w/creds")));
        let line = a.join(" ");
        assert!(line.contains("credentials=/w/creds") && line.contains("noexec") && line.contains("nosuid") && line.contains("nodev"));
        assert!(!line.contains("password") && !line.contains("pass="));
        assert!(mount_args(&ShareSpec::parse("nfs://h/e").unwrap(), Path::new("/m"), None).join(" ").contains("vers=4"));
    }

    #[test]
    fn the_share_is_mounted_for_the_backup_only_the_credentials_are_private_and_everything_is_removed() {
        let w = work("flow");
        let m = FakeMounter::default();
        let s = ShareSpec::parse("smb://nas/bk/iyakki").unwrap();
        let out = with_share(&s, Some(("amy", "s3cret")), &m, &w, |dir| {
            assert!(dir.ends_with("mnt/iyakki") && dir.is_dir(), "the folder below the share is made");
            Ok(7)
        })
        .unwrap();
        assert_eq!(out, 7);
        let calls = m.calls.lock().unwrap().clone();
        assert!(calls[0].starts_with("mount -t cifs //nas/bk ") && calls[1].starts_with("umount "));
        let (text, mode) = m.seen_creds.lock().unwrap().clone().unwrap();
        assert_eq!((text.as_str(), mode), ("username=amy\npassword=s3cret\n", 0o600));
        // (a real unmount leaves the empty mountpoint, which is then removed; the fake leaves the folder the body made)
        assert!(!w.join("creds").exists(), "no credentials file is left");
    }

    #[test]
    fn a_failed_mount_says_so_and_a_failed_backup_still_unmounts() {
        let w = work("fail");
        let s = ShareSpec::parse("nfs://h/e").unwrap();
        let m = FakeMounter { fail_mount: true, ..Default::default() };
        let e = with_share(&s, None, &m, &w, |_| Ok(())).unwrap_err();
        assert!(e.contains("could not be mounted") && e.contains("no route"));
        let m = FakeMounter::default();
        let e = with_share::<()>(&s, None, &m, &w, |_| Err("disk full".into())).unwrap_err();
        assert_eq!(e, "disk full");
        assert_eq!(m.calls.lock().unwrap().len(), 2, "it was let go");
        let m = FakeMounter { fail_unmount: true, ..Default::default() };
        assert!(with_share(&s, None, &m, &w, |_| Ok(())).unwrap_err().contains("could not be let go"));
    }

    #[test]
    fn a_backup_is_written_into_the_shares_folder_through_the_flow() {
        use crate::backup::{CreateOptions, Layout};
        let w = work("backup");
        let l = Layout { data: w.join("data"), home: w.join("home"), user: "amy".into() };
        std::fs::create_dir_all(l.data.join("skills/user/amy")).unwrap();
        std::fs::write(l.data.join("skills/user/amy/s.md"), "skill").unwrap();
        std::fs::create_dir_all(&l.home).unwrap();
        let o = CreateOptions { areas: ["skills".to_string()].into_iter().collect(), passphrase: None, now: 1_000_000, release: "t".into() };
        let m = FakeMounter::default();
        let s = ShareSpec::parse("smb://nas/bk/amy").unwrap();
        let wk = w.join("work");
        std::fs::create_dir_all(&wk).unwrap();
        // the fake mount leaves the folder in place, so the file the backup wrote is still there to look at afterwards
        let (name, bytes) = create_on_share(&l, &o, &s, Some(("amy", "pw")), &m, &wk).unwrap();
        assert!(name.starts_with("iyakki-backup-amy-") && bytes > 0);
        assert!(wk.join("mnt/amy").join(&name).is_file());
    }

    #[test]
    fn the_vault_entry_needs_two_lines() {
        assert_eq!(parse_credentials("amy\nhunter2").unwrap(), ("amy".into(), "hunter2".into()));
        assert!(parse_credentials("amy").is_err() && parse_credentials("\nx").is_err() && parse_credentials("a,b\nx").is_err());
    }
}
