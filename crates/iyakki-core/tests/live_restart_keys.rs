//! F-SPLIT-06: a new program that fails before it reads the handed-over vault keys leaves the old program with its vaults open.
//!
//! The keys travel in a memory file (`liverestart::hand_over`); the old program releases nothing until the new one reports healthy. So when the
//! exec fails, or the new program cannot read the descriptor, the old program's vaults stay open and usable, the memory file still holds the
//! keys for another try, and no key is ever written to the data folder. Linux only (a memory file); no timing, no ports, no process-wide hook.
#![cfg(all(target_os = "linux", feature = "headless"))]

use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use iyakki_core::liverestart::{self, Exec, Paths};

struct Dead;
impl iyakki_proxy::Upstream for Dead {
    fn request(&self, _: &str, _: &str, _: &[(String, String)], _: &[u8]) -> Result<iyakki_proxy::UpstreamResponse, String> {
        Err("offline".into())
    }
}

/// An exec that fails at once, as a program that cannot be started does.
struct FailingExec;
impl Exec for FailingExec {
    fn exec(&self, _exe: &Path, _args: &[String]) -> io::Error {
        io::Error::new(io::ErrorKind::NotFound, "no such program")
    }
}

fn files_under(d: &Path, out: &mut Vec<PathBuf>) {
    for e in std::fs::read_dir(d).into_iter().flatten().flatten() {
        let p = e.path();
        if p.is_dir() { files_under(&p, out) } else { out.push(p) }
    }
}

#[test]
fn a_new_program_that_fails_before_reading_the_key_descriptor_leaves_the_old_programs_vaults_open_and_the_keys_off_the_disk() {
    let root = iyakki_agent::testdir::unique("live-restart-keys", "a");
    let _ = std::fs::remove_dir_all(&root);
    let data = root.join("data");
    let svc = iyakki_agent::services::Services::open(&data, Arc::new(Dead), Arc::new(Dead)).unwrap();
    svc.on_login("alice", "alice-pass-1", false);
    svc.user_set("alice", "token", "secret-value").unwrap_or_else(|e| panic!("{e:?}"));
    let before = svc.export_session_keys().len();
    assert!(before >= 1, "alice's vault is open");

    // the old program does what its before-exec hook does: puts the keys in a memory file and keeps it over the exec
    let packed = iyakki_agent::livekeys::pack(&svc);
    let fd = liverestart::hand_over("session-keys", &packed).unwrap();
    liverestart::keep_across_exec(fd).unwrap();

    // the new program cannot be started: the exec fails, no state is left, the old program is told it keeps running
    let paths = Paths { dir: root.join("lr"), proc_root: "/proc".into(), flag: root.join("lr.flag") };
    std::fs::create_dir_all(&paths.dir).unwrap();
    std::fs::write(&paths.flag, "on").unwrap();
    let program = root.join("new-iyakki");
    let mut elf = b"\x7fELF".to_vec();
    elf.extend(vec![7u8; 2000]);
    std::fs::write(&program, elf).unwrap();
    std::fs::set_permissions(&program, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
    let said = liverestart::perform(&paths, &program, Path::new("/proc/self/exe"), "0.0.0", 1_000, vec![fd], &[], &FailingExec);
    assert!(said.contains("the old one keeps running"), "{said}");

    // nobody read the descriptor, and the old program lost nothing
    assert!(svc.user_vault_unlocked("alice"));
    assert_eq!(&*svc.user_get("alice", "token").unwrap(), "secret-value");
    assert_eq!(svc.export_session_keys().len(), before);
    // the memory file still holds the keys, so a second try can read them
    assert_eq!(liverestart::take_over(fd).unwrap(), packed);

    // a new program that could not read the descriptor starts with locked vaults; the old one is not changed by that
    let second = iyakki_agent::services::Services::open(&data, Arc::new(Dead), Arc::new(Dead)).unwrap();
    assert!(liverestart::take_over(-1).is_err());
    assert!(!second.user_vault_unlocked("alice"), "nothing was handed over: locked until the person signs in");
    assert!(svc.user_vault_unlocked("alice"));
    liverestart::release(fd);

    // no key is written anywhere in the data folder or the restart folder
    let keys = svc.export_session_keys();
    let mut all = Vec::new();
    files_under(&root, &mut all);
    for f in all {
        let bytes = std::fs::read(&f).unwrap_or_default();
        assert!(!keys.iter().any(|k| bytes.windows(32).any(|w| w == &k.2[..])), "a key is in {}", f.display());
    }
    let _ = std::fs::remove_dir_all(&root);
}
