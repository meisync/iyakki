//! The CLI tile's SSH against a real OpenSSH server (F-CLI-06, F-CLI-07, F-CLI-08): the system `sshd`, started as the current user on a
//! high port of 127.0.0.1 in a temporary folder (see `sshtunnel_real_tests`), and the real `ssh`, `ssh-keygen` and `ssh-keyscan` the
//! module drives. A key is made by `generate_key` (the machine's own `ssh-keygen`, the private half going to a fake vault), authorised on
//! the server, and used by `run` and by the interactive session. The tests skip (and say so) where a tool is missing or the server cannot
//! start here.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use iyakki_vt::terminal::{Link, LinkState};
use zeroize::Zeroizing;

use crate::ssh::*;
use crate::sshtunnel_real_tests::{find, start_sshd, temp, Server};

fn allow_all(p: &str, _write: bool) -> Result<PathBuf, String> {
    Ok(PathBuf::from(p))
}

fn v(a: &[&str]) -> Vec<String> {
    a.iter().map(|s| s.to_string()).collect()
}

struct Rig {
    dir: PathBuf,
    srv: Server,
    env: Env,
    work: PathBuf,
}

/// A server that accepts a key `generate_key` made, and an `Env` whose vault holds that key's private half; `None` (with the reason) where it cannot be done here.
fn rig(tag: &str) -> Option<Rig> {
    for t in ["ssh", "ssh-keygen", "sshd"] {
        if find(t).is_none() {
            eprintln!("SKIP: no {t} here");
            return None;
        }
    }
    let dir = temp(tag);
    let data = dir.join("data");
    let vault: Arc<Mutex<std::collections::HashMap<String, String>>> = Arc::default();
    let v2 = vault.clone();
    let env = Env::new(&data, "amy", &std::env::var("PATH").unwrap_or_else(|_| "/usr/bin:/bin".into()), Arc::new(move |n| v2.lock().unwrap().get(n).map(|s| Zeroizing::new(s.clone())).ok_or_else(|| format!("no secret {n}"))));
    let store = {
        let v3 = vault.clone();
        move |name: &str, text: &str| -> Result<(), String> {
            v3.lock().unwrap().insert(name.to_string(), text.to_string());
            Ok(())
        }
    };
    let public = generate_key(&env, "ssh.real", "golden test key", &store).expect("the machine's ssh-keygen makes a key");
    let Some(srv) = start_sshd(&dir, &public) else {
        eprintln!("SKIP: no sshd, or it cannot be started here as this user");
        return None;
    };
    let work = dir.join("work");
    std::fs::create_dir_all(&work).unwrap();
    Some(Rig { dir, srv, env, work })
}

fn target(r: &Rig) -> String {
    format!("{}@127.0.0.1", r.srv.user)
}

fn trust_server(r: &Rig) {
    let c = r.env.client().expect("a client");
    let keys = probe(&c, "127.0.0.1", r.srv.port).expect("the host key is fetched");
    r.env.known_hosts().trust("127.0.0.1", r.srv.port, &keys).unwrap();
}

#[test]
fn the_host_key_is_fetched_from_a_real_server_trusted_by_the_person_and_a_login_with_the_made_key_runs_a_command() {
    let Some(r) = rig("cli-run") else { return };
    let c = r.env.client().unwrap();
    let port = r.srv.port.to_string();
    // before the host key is trusted nothing is started
    let e = run(&r.env, Mode::Assistant, "ssh", &v(&["-p", &port, "-i", "uvault:ssh.real", &target(&r), "echo", "x"]), &r.work, 20, &allow_all, None).unwrap_err();
    assert!(e.contains("has not been trusted"), "{e}");
    // what the real ssh-keyscan fetched is the key the server really has
    let keys = probe(&c, "127.0.0.1", r.srv.port).unwrap();
    assert!(keys.iter().any(|k| format!("{} {}", k.kind, k.b64) == r.srv.host_pub), "{keys:?} vs {}", r.srv.host_pub);
    r.env.known_hosts().trust("127.0.0.1", r.srv.port, &keys).unwrap();
    // the login: the key comes from the vault into a private temporary file, the command runs on the real server
    let out = run(&r.env, Mode::Assistant, "ssh", &v(&["-p", &port, "-i", "uvault:ssh.real", &target(&r), "echo", "real-$((20+22))"]), &r.work, 30, &allow_all, None);
    let out = out.unwrap_or_else(|e| panic!("the login did not work: {e}"));
    assert!(out.contains("real-42"), "{out}");
    // output that looks like a secret is redacted, whatever the server printed
    let out = run(&r.env, Mode::Assistant, "ssh", &v(&["-p", &port, "-i", "uvault:ssh.real", &target(&r), "echo", "password: hunter2hunter2; echo done"]), &r.work, 30, &allow_all, None).unwrap();
    assert!(out.contains("done") && !out.contains("hunter2hunter2"), "{out}");
    // the temporary key is gone and nothing of it reached the workspace
    assert!(std::fs::read_dir(&r.work).unwrap().next().is_none());
    let _ = std::fs::remove_dir_all(&r.dir);
}

#[test]
fn a_changed_host_key_is_a_hard_refusal_on_a_real_connection_and_forgetting_it_asks_again() {
    let Some(r) = rig("cli-changed") else { return };
    trust_server(&r);
    let port = r.srv.port.to_string();
    let good = r.env.known_hosts().for_host("127.0.0.1", r.srv.port);
    assert!(!good.is_empty());
    // the person had trusted another key for this server: the real client stops, and the words say so
    r.env.known_hosts().forget("127.0.0.1", r.srv.port).unwrap();
    let other = HostKey { b64: "AAAAC3NzaC1lZDI1NTE5AAAAIAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA".into(), ..good[0].clone() };
    let other = HostKey { kind: "ssh-ed25519".into(), ..other };
    r.env.known_hosts().trust("127.0.0.1", r.srv.port, &[other]).unwrap();
    let e = run(&r.env, Mode::Assistant, "ssh", &v(&["-p", &port, "-i", "uvault:ssh.real", &target(&r), "true"]), &r.work, 30, &allow_all, None).unwrap_err();
    assert!(e.contains("HOST KEY CHANGED") || e.to_lowercase().contains("host key verification failed"), "{e}");
    // forgetting the host and trusting what it really presents makes the same login work again
    r.env.known_hosts().forget("127.0.0.1", r.srv.port).unwrap();
    trust_server(&r);
    let out = run(&r.env, Mode::Assistant, "ssh", &v(&["-p", &port, "-i", "uvault:ssh.real", &target(&r), "echo", "again"]), &r.work, 30, &allow_all, None).unwrap();
    assert!(out.contains("again"), "{out}");
    let _ = std::fs::remove_dir_all(&r.dir);
}

#[test]
fn an_interactive_session_logs_in_on_a_terminal_runs_what_is_typed_and_ends_with_the_shell() {
    let Some(r) = rig("cli-pty") else { return };
    trust_server(&r);
    let (mut link, _keep) = interactive(&r.env, &v(&["-p", &r.srv.port.to_string(), "-i", "uvault:ssh.real", &target(&r)]), &r.work, &allow_all, 100, 30).expect("the client starts on a terminal");
    let mut seen: Vec<u8> = Vec::new();
    let mut wait_for = |link: &mut dyn Link, mark: &str, secs: u64| -> bool {
        let t = Instant::now();
        while t.elapsed() < Duration::from_secs(secs) {
            let mut chunk = Vec::new();
            let st = link.read(&mut chunk);
            seen.extend_from_slice(&chunk);
            if String::from_utf8_lossy(&seen).contains(mark) {
                return true;
            }
            if st == LinkState::Closed {
                return false;
            }
            std::thread::sleep(Duration::from_millis(30));
        }
        false
    };
    // the shell is up once it answers; the answer is computed on the server, so typing alone cannot fake it
    std::thread::sleep(Duration::from_millis(500));
    link.write(b"echo pty-$((6*7))-ok\n");
    assert!(wait_for(&mut link, "pty-42-ok", 20), "{}", String::from_utf8_lossy(&seen));
    link.write(b"exit\n");
    let t = Instant::now();
    let mut code = None;
    while t.elapsed() < Duration::from_secs(20) && code.is_none() {
        let mut chunk = Vec::new();
        let _ = link.read(&mut chunk);
        code = link.exit_code();
        std::thread::sleep(Duration::from_millis(30));
    }
    assert_eq!(code, Some(0), "the client ended with the shell's exit code");
    let _ = std::fs::remove_dir_all(&r.dir);
}

#[test]
fn a_saved_connection_connects_with_its_own_arguments_and_a_missing_key_is_said_in_words() {
    let Some(r) = rig("cli-saved") else { return };
    trust_server(&r);
    let mut c = Connection { name: "home".into(), host: "127.0.0.1".into(), port: r.srv.port, user: r.srv.user.clone(), key: "ssh.real".into(), command: String::new() };
    c.validate().unwrap();
    // the quick check ends in `true`: it connects and leaves
    run(&r.env, Mode::Person, "ssh", &c.args(), &r.work, 30, &allow_all, None).unwrap_or_else(|e| panic!("the quick check failed: {e}"));
    // a saved default command runs on the server
    c.command = "echo saved-$((3+4))".into();
    c.validate().unwrap();
    let out = run(&r.env, Mode::Person, "ssh", &c.args(), &r.work, 30, &allow_all, None).unwrap();
    assert!(out.contains("saved-7"), "{out}");
    // a key that is not in the vault is named, nothing is started
    c.key = "ssh.nope".into();
    let e = run(&r.env, Mode::Person, "ssh", &c.args(), &r.work, 30, &allow_all, None).unwrap_err();
    assert!(e.contains("ssh.nope"), "{e}");
    let _ = std::fs::remove_dir_all(&r.dir);
}
