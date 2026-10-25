use super::*;
use crate::gitsite::{parse, ParseOptions};
use std::process::Command;

fn temp(tag: &str) -> PathBuf {
    let base = std::env::var("TMPDIR").unwrap_or_else(|_| "/tmp".into());
    let mut r = [0u8; 3];
    iyakki_sec::random_fill(&mut r).unwrap();
    let d = Path::new(&base).join(format!("gk-{tag}-{:02x}{:02x}{:02x}", r[0], r[1], r[2]));
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn keys(tag: &str) -> (GitKeys, Arc<MemStore>, Arc<Mutex<Vec<String>>>, PathBuf) {
    let d = temp(tag);
    let store = Arc::new(MemStore::default());
    let log = Arc::new(Mutex::new(Vec::new()));
    let l2 = log.clone();
    let g = GitKeys::new(&d, store.clone(), Arc::new(move |a, b| l2.lock().unwrap().push(format!("{a} {b}"))), Arc::new(Session::default()));
    (g, store, log, d)
}

fn url(s: &str) -> RepoUrl {
    parse(s, ParseOptions::default()).unwrap()
}

#[test]
fn a_generated_key_lives_encrypted_in_the_vault_and_only_its_public_half_is_in_the_table() {
    let (g, store, log, d) = keys("gen");
    let info = g.generate("work", "me@iyakki").unwrap();
    assert!(info.public.starts_with("ssh-ed25519 ") && info.public.ends_with(" me@iyakki") && info.fingerprint.starts_with("SHA256:"));
    // the vault has the private text, under the name the CLI tile uses
    assert!(store.get("ssh.work").unwrap().starts_with("-----BEGIN OPENSSH PRIVATE KEY-----"));
    // the folder has the table and no private key anywhere
    for e in walk(&d) {
        let text = std::fs::read_to_string(&e).unwrap_or_default();
        assert!(!text.contains("BEGIN OPENSSH PRIVATE KEY"), "private key found in {}", e.display());
    }
    assert!(std::fs::read_to_string(d.join("git.json")).unwrap().contains(&info.fingerprint));
    // it can be used in memory, a second one of the same name is refused
    assert_eq!(g.pair("work").unwrap().public_line(), info.public);
    assert!(g.generate("work", "x").unwrap_err().contains("already exists"));
    for bad in ["", "Work", "a b", "../x", "x.passphrase", &"a".repeat(41)] {
        assert!(g.generate(bad, "x").is_err(), "{bad}");
    }
    // the audit names the key and its fingerprint, never the key
    let l = log.lock().unwrap().join("\n");
    assert!(l.contains("git.key.generate work SHA256:") && !l.contains("PRIVATE"), "{l}");
    let _ = std::fs::remove_dir_all(&d);
}

fn walk(d: &Path) -> Vec<PathBuf> {
    let mut out = vec![];
    for e in std::fs::read_dir(d).unwrap().flatten() {
        if e.path().is_dir() {
            out.extend(walk(&e.path()));
        } else {
            out.push(e.path());
        }
    }
    out
}

#[test]
fn a_locked_vault_gives_a_plain_message_and_nothing_is_half_written() {
    let (g, store, _, d) = keys("locked");
    *store.locked.lock().unwrap() = true;
    let e = g.generate("a", "c").unwrap_err();
    assert!(e.contains("locked"), "{e}");
    assert!(g.table().keys.is_empty());
    assert!(matches!(g.pair("a"), Err(KeyError::Damaged(m)) if m.contains("locked")));
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn keys_made_by_the_cli_tile_show_up_in_the_list() {
    let (g, store, _, d) = keys("cli");
    let k = KeyPair::generate("cli@box").unwrap();
    store.set("ssh.fromcli", &k.to_openssh()).unwrap();
    store.set("ssh.fromcli.passphrase", "x").unwrap();
    store.set("ssh.broken", "b64:AAAA").unwrap(); // a Dropbear-format key is not listed
    let l = g.keys();
    assert_eq!(l.len(), 1);
    assert_eq!((l[0].id.as_str(), l[0].fingerprint.as_str()), ("fromcli", k.fingerprint().as_str()));
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn imported_keys_with_a_passphrase_work_from_the_vault_or_once_per_session() {
    if Command::new("ssh-keygen").arg("-?").output().is_err() {
        eprintln!("skipped: no ssh-keygen");
        return;
    }
    let (g, store, _, d) = keys("imp");
    let f = d.join("k");
    assert!(Command::new("ssh-keygen").args(["-q", "-t", "ed25519", "-N", "open sesame", "-C", "imp@x", "-f"]).arg(&f).status().unwrap().success());
    let text = std::fs::read_to_string(&f).unwrap();
    // wrong or missing passphrase: refused, nothing stored
    assert!(g.import("locked", &text, None, false).unwrap_err().contains("passphrase"));
    assert!(g.import("locked", &text, Some("nope"), false).unwrap_err().contains("does not open"));
    assert!(store.get("ssh.locked").is_err());
    // asked once per session
    let info = g.import("locked", &text, Some("open sesame"), false).unwrap();
    assert!(info.passphrase && !info.remembered);
    assert!(store.get("ssh.locked.passphrase").is_err(), "not remembered in the vault");
    assert_eq!(g.pair("locked").unwrap().public_line(), info.public);
    g.session.forget();
    assert_eq!(g.pair("locked").unwrap_err(), KeyError::NeedPassphrase);
    assert!(g.unlock_key("locked", "bad", false).is_err());
    g.unlock_key("locked", "open sesame", false).unwrap();
    assert!(g.pair("locked").is_ok());
    // remembered in the vault: works without the session
    g.session.forget();
    g.unlock_key("locked", "open sesame", true).unwrap();
    g.session.forget();
    assert!(g.pair("locked").is_ok());
    assert!(g.key("locked").unwrap().remembered);
    g.delete_key("locked").unwrap();
    assert!(store.get("ssh.locked").is_err() && store.get("ssh.locked.passphrase").is_err() && g.key("locked").is_none());
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn hosts_map_to_keys_with_a_workspace_override_and_tokens_for_https() {
    let (g, store, log, d) = keys("hosts");
    g.generate("a", "a").unwrap();
    g.generate("b", "b").unwrap();
    g.set_host_key("github.com", "a").unwrap();
    g.set_host_key("Gitea.LAN", "b").unwrap();
    assert!(g.set_host_key("github.com", "ghost").unwrap_err().contains("no key called"));
    assert!(g.set_host_key("bad host!", "a").is_err());
    let none = BTreeMap::new();
    let gh = url("git@github.com:o/r.git");
    assert_eq!(g.resolve(&gh, &none), Auth::Key { id: "a".into(), from_workspace: false });
    assert_eq!(g.resolve(&url("ssh://git@gitea.lan:3000/o/r"), &none), Auth::Key { id: "b".into(), from_workspace: false });
    assert_eq!(g.resolve(&url("git@gitlab.com:o/r.git"), &none), Auth::None, "a host without a rule uses no key");
    // the workspace overrides one host
    let ws: BTreeMap<String, String> = [("github.com".to_string(), "b".to_string())].into();
    assert_eq!(g.resolve(&gh, &ws), Auth::Key { id: "b".into(), from_workspace: true });
    assert_eq!(g.resolve(&url("git@gitlab.com:o/r.git"), &ws), Auth::None);
    // tokens: HTTPS only, in the vault, never in the table
    g.set_token("github.com", "octo", "ghp_SECRETVALUE").unwrap();
    assert_eq!(g.resolve(&url("https://github.com/o/r.git"), &ws), Auth::Token { host: "github.com".into() });
    assert_eq!(g.resolve(&gh, &none), Auth::Key { id: "a".into(), from_workspace: false }, "ssh still uses the key");
    assert_eq!(g.resolve(&url("https://gitlab.com/o/r"), &none), Auth::None);
    let (u, t) = g.token("github.com").unwrap();
    assert_eq!((u.as_str(), t.as_str()), ("octo", "ghp_SECRETVALUE"));
    assert!(store.names().unwrap().contains(&"git.token.github.com".to_string()));
    assert!(!std::fs::read_to_string(d.join("git.json")).unwrap().contains("SECRETVALUE"));
    assert!(!log.lock().unwrap().join("\n").contains("SECRETVALUE"));
    assert!(g.set_token("github.com", "x", "  ").is_err());
    g.clear_token("github.com").unwrap();
    assert_eq!(g.resolve(&url("https://github.com/o/r.git"), &none), Auth::None);
    // deleting a key empties the rules that pointed at it
    g.delete_key("a").unwrap();
    assert_eq!(g.resolve(&gh, &none), Auth::None);
    assert_eq!(Auth::Token { host: "x".into() }.label(), "token");
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn an_assistant_asks_once_per_key_per_session_and_the_person_never_does() {
    let s = Session::default();
    assert!(!s.needs_approval(Who::Person, "a"));
    assert!(s.needs_approval(Who::Assistant, "a"));
    s.approve("a");
    assert!(!s.needs_approval(Who::Assistant, "a"));
    assert!(s.needs_approval(Who::Assistant, "b"));
    s.forget();
    assert!(s.needs_approval(Who::Assistant, "a"));
}

fn hk(host: &str, port: u16, seed: u8) -> HostKey {
    let mut blob = Vec::new();
    crate::sshkey::put_str(&mut blob, b"ssh-ed25519");
    crate::sshkey::put_str(&mut blob, &[seed; 32]);
    HostKey { pattern: crate::ssh::pattern(host, port), kind: "ssh-ed25519".into(), b64: crate::sshkey::b64(&blob) }
}

#[test]
fn a_new_host_is_confirmed_once_and_a_changed_key_is_refused_with_an_explanation() {
    let (g, _, _, d) = keys("kh");
    let kh = g.known_hosts();
    let probe_a = || Ok(vec![hk("git.example.org", 22, 1)]);
    match check_ssh_host(&kh, "git.example.org", 22, &probe_a).unwrap() {
        HostCheck::New(k) => assert!(k[0].fingerprint().starts_with("SHA256:")),
        other => panic!("{other:?}"),
    }
    kh.trust("git.example.org", 22, &[hk("git.example.org", 22, 1)]).unwrap();
    assert_eq!(check_ssh_host(&kh, "git.example.org", 22, &probe_a).unwrap(), HostCheck::Trusted);
    // another port is another host
    assert!(matches!(check_ssh_host(&kh, "git.example.org", 2222, &|| Ok(vec![hk("git.example.org", 2222, 1)])).unwrap(), HostCheck::New(_)));
    // the key changed
    let probe_b = || Ok(vec![hk("git.example.org", 22, 2)]);
    match check_ssh_host(&kh, "git.example.org", 22, &probe_b).unwrap() {
        HostCheck::Changed { recorded, presented } => {
            let t = changed_text("git.example.org", 22, &recorded, &presented);
            assert!(t.contains("CHANGED") && t.contains(&recorded[0].fingerprint()) && t.contains(&presented[0].fingerprint()) && t.contains("Nothing was sent"), "{t}");
        }
        other => panic!("{other:?}"),
    }
    assert!(check_ssh_host(&kh, "x.example.org", 22, &|| Err("no route".into())).is_err());
    // the file is in the SSH client's format, one line per key, mode 0600
    let text = std::fs::read_to_string(g.known_hosts_path()).unwrap();
    assert!(text.starts_with("git.example.org ssh-ed25519 "), "{text}");
    use std::os::unix::fs::PermissionsExt;
    assert_eq!(std::fs::metadata(g.known_hosts_path()).unwrap().permissions().mode() & 0o777, 0o600);
    let _ = std::fs::remove_dir_all(&d);
}
