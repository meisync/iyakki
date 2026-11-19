//! Tests for the SSH client support. A stand-in `ssh` (a shell script that records what it was given) sits on a temporary search
//! path; nothing ever connects to a real machine.

use super::*;
use std::sync::Mutex;

struct Tmp(PathBuf);
impl Tmp {
    fn new(tag: &str) -> Tmp {
        let d = std::env::temp_dir().join(format!("iyakki-sshtest-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(d.join("bin")).unwrap();
        std::fs::create_dir_all(d.join("ctl")).unwrap();
        std::fs::create_dir_all(d.join("work")).unwrap();
        Tmp(d)
    }
    fn log(&self) -> PathBuf {
        self.0.join("log")
    }
    fn ctl(&self, name: &str, v: &str) {
        std::fs::write(self.0.join("ctl").join(name), v).unwrap();
    }
    fn script(&self, name: &str, body: &str) {
        let p = self.0.join("bin").join(name);
        let _ = std::fs::remove_file(&p);
        std::fs::write(&p, format!("#!/bin/sh\nPATH=/usr/bin:/bin; export PATH\n{body}")).unwrap();
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    /// An OpenSSH stand-in: answers -V, records its arguments and the key files it was given, prints a line.
    fn fake_openssh(&self) {
        let (log, ctl) = (self.log().display().to_string(), self.0.join("ctl").display().to_string());
        self.script("ssh", &format!(r#"LOG="{log}"; CTL="{ctl}"
if [ "$1" = "-V" ]; then echo "OpenSSH_9.6p1 Ubuntu-3, OpenSSL 3.0.13 30 Jan 2024" >&2; exit 0; fi
{{ echo "ARGV: $*"; echo "HOME=$HOME"; echo "AUTH=${{SSH_AUTH_SOCK:-none}}"; echo "ENVCOUNT=$(env | wc -l)"; }} >> "$LOG"
prev=""
for a in "$@"; do
  case "$a" in StrictHostKeyChecking=accept-new) probe=1;; esac
  case "$a" in UserKnownHostsFile=*) kh="${{a#UserKnownHostsFile=}}";; esac
  if [ "$prev" = "-i" ]; then echo "KEYMODE=$(stat -c %a "$a") DIRMODE=$(stat -c %a "$(dirname "$a")")" >> "$LOG"; head -c 12 "$a" >> "$LOG"; echo >> "$LOG"; fi
  prev="$a"
done
if [ -n "$probe" ] && [ -f "$CTL/probekey" ]; then echo "127.0.0.1 $(cat "$CTL/probekey")" > "$kh"; fi
[ -f "$CTL/out" ] && cat "$CTL/out"
[ -f "$CTL/err" ] && cat "$CTL/err" >&2
exit $(cat "$CTL/rc" 2>/dev/null || echo 0)
"#));
        let (log, _) = (self.log().display().to_string(), 0);
        for n in ["scp", "sftp"] {
            self.script(n, &format!("echo \"{n} $*\" >> \"{log}\"\nexit 0\n"));
        }
    }
    fn env(&self, secrets: &[(&str, &str)]) -> Env {
        let m: std::collections::HashMap<String, String> = secrets.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
        let m = Arc::new(Mutex::new(m));
        Env::new(&self.0.join("data"), "amy", &self.0.join("bin").display().to_string(), Arc::new(move |n| m.lock().unwrap().get(n).map(|v| Zeroizing::new(v.clone())).ok_or_else(|| format!("no secret {n}"))))
    }
    fn logged(&self) -> String {
        std::fs::read_to_string(self.log()).unwrap_or_default()
    }
}
impl Drop for Tmp {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn v(a: &[&str]) -> Vec<String> {
    a.iter().map(|s| s.to_string()).collect()
}

const ED: &str = "AAAAC3NzaC1lZDI1NTE5AAAAIKBc6HPCQiTnZ0RTiREqU3BRULCRSFwDp8N5+hGLuS4H";
const ED_FP: &str = "SHA256:3XJFuuO18/NM/NTpVFPkp7Fp35hDb42XVWDdJjfIXFo";

fn key(host: &str, port: u16) -> HostKey {
    HostKey { pattern: pattern(host, port), kind: "ssh-ed25519".into(), b64: ED.into() }
}

fn allow_all(path: &str, _write: bool) -> Result<PathBuf, String> {
    Ok(PathBuf::from(format!("/scoped/{}", path.trim_start_matches("./"))))
}

#[test]
fn the_client_is_found_by_its_version_banner_openssh_or_dropbear_and_missing_is_none() {
    assert_eq!(parse_banner("OpenSSH_9.6p1 Ubuntu-3ubuntu13.5, OpenSSL 3.0.13 30 Jan 2024"), Some((Flavor::OpenSsh, "9.6p1".into())));
    assert_eq!(parse_banner("OpenSSH_for_Windows_9.5p1, LibreSSL 3.8.2"), Some((Flavor::OpenSsh, "9.5p1".into())));
    assert_eq!(parse_banner("Dropbear v2026.91"), Some((Flavor::Dropbear, "2026.91".into())));
    assert_eq!(parse_banner("Dropbear SSH client v2022.83 https://matt.ucc.asn.au/dropbear/dropbear.html"), Some((Flavor::Dropbear, "2022.83".into())));
    assert_eq!(parse_banner("something else 1.0"), None);
    let t = Tmp::new("detect");
    assert!(detect(&format!("{}", t.0.join("bin").display())).is_none(), "an empty folder has no client");
    t.fake_openssh();
    t.script("ssh-keygen", "exit 0\n");
    let c = detect(&t.0.join("bin").display().to_string()).expect("found");
    assert_eq!((c.flavor, c.version.as_str()), (Flavor::OpenSsh, "9.6p1"));
    assert_eq!(c.commands(), ["ssh", "scp", "sftp", "ssh-keygen"]);
    // Dropbear: only dbclient and the multi-call names, no sftp
    let d = Tmp::new("detect2");
    d.script("dbclient", "echo 'Dropbear v2026.91' >&2\nexit 0\n");
    d.script("scp", "exit 0\n");
    d.script("dropbearkey", "exit 0\n");
    let c = detect(&d.0.join("bin").display().to_string()).expect("dbclient is a client too");
    assert_eq!((c.flavor, c.version.as_str()), (Flavor::Dropbear, "2026.91"));
    assert_eq!(c.commands(), ["ssh", "scp", "dropbearkey"]);
    assert!(install_hint("linux").contains("openssh-client") && install_hint("macos").contains("built in") && install_hint("windows").contains("OpenSSH.Client"));
}

#[test]
fn commands_that_run_programs_here_forward_ports_or_hide_a_hop_are_refused_before_anything_starts() {
    let bad: &[&[&str]] = &[
        &["-o", "ProxyCommand=nc %h %p", "h"],
        &["-oProxyCommand=id", "h"],
        &["-o", "proxycommand id", "h"],
        &["h", "-oProxyCommand=id"],
        &["h", "-o", "LocalCommand=id"],
        &["-o", "PermitLocalCommand=yes", "h"],
        &["-o", "KnownHostsCommand=id", "h"],
        &["-o", "ProxyJump=j", "h"],
        &["-o", "Include=/etc/ssh/x", "h"],
        &["-o", "RemoteCommand=id", "h"],
        &["-o", "IdentityAgent=/tmp/s", "h"],
        &["-o", "UserKnownHostsFile=/dev/null", "h"],
        &["-o", "StrictHostKeyChecking=no", "h"],
        &["-o", "StrictHostKeyChecking=accept-new", "h"],
        &["-o", "BatchMode=no", "h"],
        &["-o", "PreferredAuthentications=password", "h"],
        &["-o", "ForwardAgent=yes", "h"],
        &["-o", "SendEnv=*", "h"],
        &["-F", "/tmp/cfg", "h"],
        &["-F/tmp/cfg", "h"],
        &["-J", "jump", "h"],
        &["-Jjump", "h"],
        &["-S", "/tmp/sock", "h"],
        &["-L", "8080:localhost:80", "h"],
        &["-R", "8080:localhost:80", "h"],
        &["-D", "1080", "h"],
        &["-W", "a:1", "h"],
        &["-w", "0:0", "h"],
        &["-f", "h"],
        &["-N", "h"],
        &["-y", "h"],
        &["-E", "/tmp/log", "h"],
        &["-A", "h"],
        &["-4A", "h"],
        &["-p", "0", "h"],
        &["-p", "22", "-o", "Port=2222", "h"],
        &["-p", "x", "h"],
        &["ssh://h"],
        &["-i", "/home/amy/key", "h"],
        &["-i", "uvault:svc.openai", "h"],
        &["-i", "uvault:ssh.k.passphrase", "h"],
        &["-o", "IdentityFile=/etc/shadow", "h"],
        &["-o", "Ciphers=a;b", "h"],
        &["--"],
        &[],
        &["-p"],
        &["bad host"],
        &["-bad", "h"],
        &["us er@h"],
    ];
    for b in bad {
        assert!(plan("ssh", &v(b), Mode::Assistant).is_err(), "{b:?} should be refused");
    }
    let bad_scp: &[&[&str]] = &[
        &["-S", "evil", "a", "h:b"],
        &["-F", "x", "a", "h:b"],
        &["-J", "x", "a", "h:b"],
        &["-o", "ProxyCommand=id", "a", "h:b"],
        &["-oLocalCommand=id", "a", "h:b"],
        &["a", "h:b", "-S", "evil"],
        &["a", "b"],
        &["h1:a", "h2:b"],
        &["a"],
        &["scp://h/x", "y"],
        &["a", "-x"],
        &["-3", "h1:a", "h2:b"],
    ];
    for b in bad_scp {
        assert!(plan("scp", &v(b), Mode::Assistant).is_err(), "scp {b:?} should be refused");
    }
    let bad_sftp: &[&[&str]] = &[&["-s", "x", "h"], &["-S", "x", "h"], &["-D", "/bin/sh", "h"], &["-F", "x", "h"], &["-J", "x", "h"], &["-b", "-", "h"], &["sftp://h/x"], &["a", "b"], &[], &["-o", "ProxyCommand=id", "h"]];
    for b in bad_sftp {
        assert!(plan("sftp", &v(b), Mode::Assistant).is_err(), "sftp {b:?} should be refused");
    }
    assert!(plan("rm", &v(&["x"]), Mode::Person).is_err());
}

#[test]
fn ordinary_commands_are_understood_for_ssh_scp_and_sftp_and_the_person_may_forward_an_agent_but_the_assistant_may_not() {
    let p = plan("ssh", &v(&["-p", "2222", "-i", "uvault:ssh.deploy", "bob@Box.Example.com", "uptime", "-a"]), Mode::Assistant).unwrap();
    assert_eq!((p.host.as_str(), p.port, p.user.as_deref(), p.keys.clone()), ("box.example.com", 2222, Some("bob".into()), vec!["ssh.deploy".to_string()]));
    assert_eq!(p.operands, ["bob@Box.Example.com", "uptime", "-a"], "everything after the first word of the remote command belongs to the other machine");
    assert_eq!(plan("ssh", &v(&["-4q", "-l", "bob", "10.0.0.5", "ls"]), Mode::Assistant).unwrap().user.as_deref(), Some("bob"));
    assert_eq!(plan("ssh", &v(&["-o", "Port=2200", "-o", "ConnectTimeout=5", "[::1]"]), Mode::Assistant).unwrap().port, 2200);
    assert_eq!(plan("ssh", &v(&["-p22", "h"]), Mode::Assistant).unwrap().port, 22);
    let p = plan("ssh", &v(&["h", "--", "-x"]), Mode::Assistant).unwrap();
    assert_eq!(p.operands, ["h", "-x"]);
    assert!(plan("ssh", &v(&["-A", "h"]), Mode::Person).unwrap().agent);
    assert!(plan("ssh", &v(&["-o", "ForwardAgent=yes", "h"]), Mode::Person).unwrap().agent);
    assert!(!plan("ssh", &v(&["-o", "ForwardAgent=no", "h"]), Mode::Assistant).unwrap().agent);
    // a person typing a key file name is trusted with it; the assistant is not
    assert!(plan("ssh", &v(&["-i", "mykey", "h"]), Mode::Person).is_ok());
    let p = plan("scp", &v(&["-P", "2222", "-r", "./a dir", "bob@h.example.com:/srv/in"]), Mode::Assistant).unwrap();
    assert_eq!((p.host.as_str(), p.port, p.user.as_deref()), ("h.example.com", 2222, Some("bob")));
    assert_eq!(p.locals, [Local { path: "./a dir".into(), write: false }]);
    let p = plan("scp", &v(&["h:/etc/hosts", "h:/etc/motd", "out/"]), Mode::Assistant).unwrap();
    assert_eq!(p.locals, [Local { path: "out/".into(), write: true }]);
    let p = plan("scp", &v(&["./a:b", "[::1]:/x"]), Mode::Assistant).unwrap();
    assert_eq!((p.host.as_str(), p.locals.len()), ("::1", 1), "a name with ./ before the colon is a local file");
    let p = plan("sftp", &v(&["-b", "steps.txt", "-P", "2200", "amy@host.example.com"]), Mode::Assistant).unwrap();
    assert_eq!((p.host.as_str(), p.port, p.batch.as_deref()), ("host.example.com", 2200, Some("steps.txt")));
    assert_eq!(plan("sftp", &v(&["host.example.com:/srv"]), Mode::Person).unwrap().host, "host.example.com");
}

#[test]
fn a_run_adds_the_safe_options_the_managed_known_hosts_and_a_private_key_file_that_is_gone_afterwards() {
    let t = Tmp::new("run");
    t.fake_openssh();
    t.ctl("out", "remote output\n");
    let env = t.env(&[("ssh.deploy", "-----BEGIN OPENSSH PRIVATE KEY-----\nAAAA\n-----END OPENSSH PRIVATE KEY-----")]);
    let work = t.0.join("work");
    // an unknown host is refused before the client is started
    let e = run(&env, Mode::Assistant, "ssh", &v(&["-i", "uvault:ssh.deploy", "bob@10.0.0.5", "uptime"]), &work, 20, &allow_all, None).unwrap_err();
    assert!(e.contains("has not been trusted yet") && e.contains("sshtrust 10.0.0.5"), "{e}");
    assert!(!t.log().exists(), "the client was not started for an unknown host");
    env.known_hosts().trust("10.0.0.5", 22, &[key("10.0.0.5", 22)]).unwrap();
    let out = run(&env, Mode::Assistant, "ssh", &v(&["-i", "uvault:ssh.deploy", "-o", "ConnectTimeout=5", "bob@10.0.0.5", "uptime"]), &work, 20, &allow_all, None).unwrap();
    assert!(out.contains("remote output"), "{out}");
    let log = t.logged();
    let argv = log.lines().find(|l| l.starts_with("ARGV: ")).unwrap();
    for want in ["-F /dev/null", "BatchMode=yes", "StrictHostKeyChecking=yes", "GlobalKnownHostsFile=/dev/null", "PreferredAuthentications=publickey", "ProxyCommand=none", "ClearAllForwardings=yes", "ForwardAgent=no", "IdentitiesOnly=yes", "ConnectTimeout=5", "-- bob@10.0.0.5 uptime"] {
        assert!(argv.contains(want), "{want} in {argv}");
    }
    assert!(argv.contains(&format!("UserKnownHostsFile={}", env.home().join(".ssh/known_hosts").display())));
    assert!(argv.find("BatchMode=yes").unwrap() < argv.find("ConnectTimeout=5").unwrap(), "the forced options come first, so they win");
    assert!(!argv.contains("uvault:"), "the vault reference was replaced by the temporary file: {argv}");
    assert!(log.contains("KEYMODE=600 DIRMODE=700"), "the key file is private while the client runs: {log}");
    assert!(log.contains("-----BEGIN O"), "the client could read the key");
    assert!(log.contains(&format!("HOME={}", env.home().display())) && log.contains("AUTH=none"));
    let keyfile = argv.split_whitespace().skip_while(|w| *w != "-i").nth(1).unwrap().to_string();
    assert!(!Path::new(&keyfile).exists() && !Path::new(&keyfile).parent().unwrap().exists(), "the temporary key and its folder are removed: {keyfile}");
    // the key never reached the workspace
    assert!(std::fs::read_dir(&work).unwrap().next().is_none());
    // a key that is not in the vault says so
    let e = run(&env, Mode::Assistant, "ssh", &v(&["-i", "uvault:ssh.nope", "bob@10.0.0.5", "id"]), &work, 20, &allow_all, None).unwrap_err();
    assert!(e.contains("no secret ssh.nope") || e.contains("ssh.nope"), "{e}");
    // the person's own run can forward an agent with -A, and the assistant's never does
    let out = run(&env, Mode::Person, "ssh", &v(&["-A", "bob@10.0.0.5", "id"]), &work, 20, &allow_all, None).unwrap();
    assert!(out.contains("remote output"));
    assert_eq!(t.logged().lines().filter(|l| l.starts_with("ARGV: ")).count(), 2);
    assert!(!t.logged().lines().filter(|l| l.starts_with("ARGV: ")).last().unwrap().contains("ForwardAgent=no"));
}

#[test]
fn the_output_is_redacted_and_streamed_and_a_changed_host_key_is_a_hard_refusal() {
    let t = Tmp::new("changed");
    t.fake_openssh();
    let env = t.env(&[]);
    let work = t.0.join("work");
    env.known_hosts().trust("h.example.com", 22, &[key("h.example.com", 22)]).unwrap();
    t.ctl("out", "uptime 3 days\npassword: hunter2hunter2\n-----BEGIN OPENSSH PRIVATE KEY-----\nb3BlbnNzaC1rZXktdjEAAAAABG5vbmU=\n-----END OPENSSH PRIVATE KEY-----\n");
    let seen = Arc::new(Mutex::new(String::new()));
    let s2 = seen.clone();
    let out = run(&env, Mode::Person, "ssh", &v(&["h.example.com", "uptime"]), &work, 20, &allow_all, Some(Arc::new(move |t: &str| s2.lock().unwrap().push_str(t)))).unwrap();
    assert!(out.contains("uptime 3 days") && !out.contains("hunter2hunter2") && !out.contains("b3BlbnNzaC1rZXktdjE"), "{out}");
    let streamed = seen.lock().unwrap().clone();
    assert!(streamed.contains("uptime 3 days") && !streamed.contains("b3BlbnNzaC1rZXktdjE"), "{streamed}");
    // the client says the key changed
    let _ = std::fs::remove_file(t.0.join("ctl/out"));
    for msg in ["@@@@ WARNING: REMOTE HOST IDENTIFICATION HAS CHANGED! @@@@\nHost key verification failed.\n", "ssh-ed25519 host key mismatch for h.example.com !\n"] {
        t.ctl("err", msg);
        t.ctl("rc", "255");
        let e = run(&env, Mode::Assistant, "ssh", &v(&["h.example.com", "id"]), &work, 20, &allow_all, None).unwrap_err();
        assert!(e.contains("HOST KEY CHANGED") && e.contains("intercepting") && !e.contains("@@@@"), "{e}");
    }
    // another failure is passed on, not mistaken for it
    t.ctl("err", "Permission denied (publickey).\n");
    let e = run(&env, Mode::Assistant, "ssh", &v(&["h.example.com", "id"]), &work, 20, &allow_all, None).unwrap_err();
    assert!(e.contains("Permission denied") && !e.contains("CHANGED"), "{e}");
}

#[test]
fn host_keys_are_recorded_per_person_privately_with_ports_and_fingerprints_and_replaced_or_forgotten_on_request() {
    let t = Tmp::new("kh");
    let env = t.env(&[]);
    let kh = env.known_hosts();
    assert!(kh.read().is_empty());
    assert_eq!(key("h", 22).fingerprint(), ED_FP, "the fingerprint is the one OpenSSH and Dropbear print");
    kh.trust("Box.Example.com", 22, &[key("x", 22)]).unwrap();
    kh.trust("box.example.com", 2222, &[key("x", 22)]).unwrap();
    assert_eq!(kh.for_host("box.example.com", 22).len(), 1);
    assert_eq!(kh.for_host("BOX.example.com", 2222)[0].pattern, "[box.example.com]:2222");
    assert!(kh.for_host("other", 22).is_empty());
    let path = env.home().join(".ssh/known_hosts");
    assert_eq!(std::fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);
    assert_eq!(std::fs::metadata(path.parent().unwrap()).unwrap().permissions().mode() & 0o777, 0o700);
    // trusting again replaces (the person confirmed the new key); a second key for the same host is added
    let other = HostKey { b64: "AAAAC3NzaC1lZDI1NTE5AAAAIAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA".into(), ..key("x", 22) };
    kh.trust("box.example.com", 22, &[other.clone()]).unwrap();
    assert_eq!(kh.for_host("box.example.com", 22).len(), 1);
    assert_eq!(kh.for_host("box.example.com", 22)[0].b64, other.b64);
    assert_eq!(kh.forget("box.example.com", 22).unwrap(), 1);
    assert_eq!(kh.forget("box.example.com", 22).unwrap(), 0);
    assert_eq!(kh.read().len(), 1);
    assert!(kh.trust("h", 22, &[]).is_err());
    // lines of other shapes are skipped, not trusted
    std::fs::write(&path, "# comment\n|1|abc|def ssh-ed25519 AAAA\nh ssh-ed25519 not*base64\nbad line\nok.example.com ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIKBc6HPCQiTnZ0RTiREqU3BRULCRSFwDp8N5+hGLuS4H me\n").unwrap();
    assert_eq!(kh.read().len(), 1);
    // each person has a folder of their own
    let other_env = Env::new(&t.0.join("data2"), "bob", "", Arc::new(|_| Err("none".into())));
    assert!(other_env.known_hosts().read().is_empty());
}

#[test]
fn the_host_key_is_fetched_for_the_person_to_check_and_trusted_only_by_its_fingerprint() {
    let t = Tmp::new("trust");
    t.fake_openssh();
    let env = t.env(&[]);
    // no ssh-keyscan: the client itself is asked, in a throw-away home, with no key and no way to log in
    t.ctl("probekey", &format!("ssh-ed25519 {ED}"));
    let c = env.client().unwrap();
    let keys = probe(&c, "Box.Example.com", 2222).unwrap();
    assert_eq!(keys.len(), 1);
    assert_eq!(keys[0].pattern, "[box.example.com]:2222");
    assert_eq!(keys[0].fingerprint(), ED_FP);
    assert!(env.known_hosts().read().is_empty(), "probing records nothing");
    let log = t.logged();
    assert!(log.contains("StrictHostKeyChecking=accept-new") && log.contains("IdentityFile=/dev/null") && log.contains("AUTH=none"), "{log}");
    // ssh-keyscan, when there is one
    t.script("ssh-keyscan", &format!("echo '# box:22 SSH-2.0-OpenSSH_9'\necho 'box ssh-ed25519 {ED}'\nexit 0\n"));
    let keys = probe(&env.client().unwrap(), "box.example.com", 22).unwrap();
    assert_eq!(keys[0].pattern, "box.example.com");
    // nothing answered
    t.script("ssh-keyscan", "exit 0\n");
    assert!(probe(&env.client().unwrap(), "box.example.com", 22).unwrap_err().contains("sent no host key"));
    t.script("ssh-keyscan", &format!("echo 'box ssh-ed25519 {ED}'\n"));
    // without a fingerprint nothing is trusted and the fingerprint is shown
    let shown = trust_flow(&env, "box.example.com", 22, None).unwrap();
    assert!(shown.contains(ED_FP) && shown.contains(&format!("sshtrust box.example.com {ED_FP}")), "{shown}");
    assert!(env.known_hosts().read().is_empty());
    // a wrong fingerprint trusts nothing
    let e = trust_flow(&env, "box.example.com", 22, Some("SHA256:wrong")).unwrap_err();
    assert!(e.contains("nothing was trusted") && e.contains(ED_FP), "{e}");
    assert!(env.known_hosts().read().is_empty());
    // the right one trusts exactly that key
    assert!(trust_flow(&env, "box.example.com", 22, Some(ED_FP)).unwrap().contains("trusted"));
    assert_eq!(env.known_hosts().for_host("box.example.com", 22).len(), 1);
    // a key that differs from the trusted one is shown with a warning
    env.known_hosts().trust("box.example.com", 22, &[HostKey { b64: "AAAAC3NzaC1lZDI1NTE5AAAAIAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA".into(), ..key("x", 22) }]).unwrap();
    assert!(trust_flow(&env, "box.example.com", 22, None).unwrap().contains("NOT the key you trusted"));
    assert_eq!(host_port(&v(&["Box.example.com", "2200", "SHA256:x"])).unwrap(), ("box.example.com".to_string(), 2200));
    assert!(host_port(&v(&[])).is_err() && host_port(&v(&["a b"])).is_err() && host_port(&v(&["h", "0"])).is_err());
}

#[test]
fn private_keys_come_only_from_the_vault_in_private_files_and_encrypted_keys_use_a_passphrase_from_the_vault_not_an_argument() {
    // what counts as encrypted
    let plain = format!("-----BEGIN OPENSSH PRIVATE KEY-----\n{}\n-----END OPENSSH PRIVATE KEY-----", crate::story::base64_encode(&[b"openssh-key-v1\0".as_slice(), &[0, 0, 0, 4], b"none"].concat()));
    let enc = format!("-----BEGIN OPENSSH PRIVATE KEY-----\n{}\n-----END OPENSSH PRIVATE KEY-----", crate::story::base64_encode(&[b"openssh-key-v1\0".as_slice(), &[0, 0, 0, 10], b"aes256-ctr"].concat()));
    assert!(!key_is_encrypted(&plain) && key_is_encrypted(&enc));
    assert!(key_is_encrypted("-----BEGIN RSA PRIVATE KEY-----\nProc-Type: 4,ENCRYPTED\n") && key_is_encrypted("-----BEGIN ENCRYPTED PRIVATE KEY-----"));
    assert!(!key_is_encrypted("not a key"));
    assert!(valid_key_name("ssh.deploy") && valid_key_name("ssh.a-b_c.d") && !valid_key_name("ssh.") && !valid_key_name("deploy") && !valid_key_name("ssh.x.passphrase") && !valid_key_name("ssh.UPPER"));

    let t = Tmp::new("enc");
    t.fake_openssh();
    let log = t.log().display().to_string();
    // ssh-keygen stand-in: records that the passphrase came through the askpass helper, never an argument, and "decrypts" the key
    t.script("ssh-keygen", &format!("echo \"KEYGEN $* ASKPASS=$SSH_ASKPASS REQUIRE=$SSH_ASKPASS_REQUIRE PASS=$($SSH_ASKPASS)\" >> \"{log}\"\nfor last; do :; done\nprintf 'DECRYPTED\\n' > \"$last\"\nexit 0\n"));
    t.ctl("out", "ok\n");
    let work = t.0.join("work");
    let env = t.env(&[("ssh.locked", &enc)]);
    env.known_hosts().trust("h.example.com", 22, &[key("h.example.com", 22)]).unwrap();
    let e = run(&env, Mode::Assistant, "ssh", &v(&["-i", "uvault:ssh.locked", "h.example.com", "id"]), &work, 20, &allow_all, None).unwrap_err();
    assert!(e.contains("ssh.locked.passphrase") && e.contains("never typed on a command line"), "{e}");
    assert!(!t.logged().contains("ARGV"), "the client was not started");
    let env = t.env(&[("ssh.locked", &enc), ("ssh.locked.passphrase", "s3cret-pass")]);
    run(&env, Mode::Assistant, "ssh", &v(&["-i", "uvault:ssh.locked", "h.example.com", "id"]), &work, 20, &allow_all, None).unwrap();
    let l = t.logged();
    let kg = l.lines().find(|x| x.starts_with("KEYGEN")).unwrap();
    assert!(kg.contains("-p -N  -f") || kg.contains("-p -N"), "{kg}");
    assert!(kg.contains("REQUIRE=force") && kg.contains("PASS=s3cret-pass"), "{kg}");
    assert!(!kg.split("ASKPASS=").next().unwrap().contains("s3cret-pass"), "the passphrase is not an argument: {kg}");
    assert!(l.contains("DECRYPTED"), "the client got the opened key");
    // a Dropbear-made key is kept as b64: and written back as the original bytes
    let raw = [0u8, 1, 2, 3, 255, 10, 13, 7];
    let env = t.env(&[("ssh.db", &format!("b64:{}", crate::story::base64_encode(&raw)))]);
    let tmp = TempDir::new("k").unwrap();
    let c = env.client().unwrap();
    let files = write_keys(&env, &c, &["ssh.db".into()], tmp.path()).unwrap();
    assert_eq!(std::fs::read(&files[0].1).unwrap(), raw);
    assert_eq!(std::fs::metadata(&files[0].1).unwrap().permissions().mode() & 0o777, 0o600);
    let p = tmp.path().to_path_buf();
    drop(tmp);
    assert!(!p.exists());
}

#[test]
fn a_key_is_made_by_the_machines_own_tool_into_the_vault_and_never_left_in_a_folder() {
    let t = Tmp::new("gen");
    t.fake_openssh();
    // ssh-keygen stand-in writes <file> and <file>.pub
    t.script("ssh-keygen", "for last; do :; done\nprintf 'PRIVATE-KEY-TEXT\\n' > \"$last\"\nprintf 'ssh-ed25519 AAAAPUB me@iyakki\\n' > \"$last.pub\"\nexit 0\n");
    let env = t.env(&[]);
    let stored = Arc::new(Mutex::new(Vec::<(String, String)>::new()));
    let s2 = stored.clone();
    let pubkey = generate_key(&env, "ssh.deploy", "me@iyakki", &move |n, val| {
        s2.lock().unwrap().push((n.to_string(), val.to_string()));
        Ok(())
    })
    .unwrap();
    assert_eq!(pubkey, "ssh-ed25519 AAAAPUB me@iyakki");
    assert_eq!(stored.lock().unwrap()[0], ("ssh.deploy".to_string(), "PRIVATE-KEY-TEXT\n".to_string()));
    assert_eq!(env.public_key("ssh.deploy").as_deref(), Some("ssh-ed25519 AAAAPUB me@iyakki"));
    assert!(std::fs::read_dir(env.dir.join("keys")).unwrap().flatten().all(|e| e.file_name().to_string_lossy().ends_with(".pub")), "no private key in the folder");
    for bad in ["deploy", "ssh.", "ssh.x.passphrase"] {
        assert!(generate_key(&env, bad, "", &|_, _| Ok(())).is_err(), "{bad}");
    }
    let env2 = t.env(&[("ssh.deploy", "x")]);
    assert!(generate_key(&env2, "ssh.deploy", "", &|_, _| Ok(())).unwrap_err().contains("already exists"));
    // Dropbear: dropbearkey writes a binary file and prints the public key; the vault holds it as b64:
    let d = Tmp::new("gen2");
    d.script("dbclient", "echo 'Dropbear v2026.91' >&2\nexit 0\n");
    d.script("dropbearkey", "for last; do :; done\nprintf '\\000\\001BIN' > \"$last\"\necho 'Public key portion is:'\necho 'ssh-ed25519 AAAADB me@iyakki'\necho 'Fingerprint: SHA256:x'\nexit 0\n");
    let denv = d.env(&[]);
    let stored = Arc::new(Mutex::new(Vec::<(String, String)>::new()));
    let s2 = stored.clone();
    let pubkey = generate_key(&denv, "ssh.box", "me@iyakki", &move |n, val| {
        s2.lock().unwrap().push((n.to_string(), val.to_string()));
        Ok(())
    })
    .unwrap();
    assert_eq!(pubkey, "ssh-ed25519 AAAADB me@iyakki");
    let (_, val) = stored.lock().unwrap()[0].clone();
    assert_eq!(crate::models::b64_decode(val.strip_prefix("b64:").unwrap()).unwrap(), [0u8, 1, b'B', b'I', b'N']);
}

#[test]
fn saved_connections_are_added_changed_renamed_removed_and_validated_and_hold_no_secret() {
    let t = Tmp::new("conn");
    let env = t.env(&[]);
    let c = env.connections();
    assert!(c.list().is_empty());
    let web = Connection { name: "web box".into(), host: "Web.Example.com".into(), port: 2222, user: "deploy".into(), key: "ssh.deploy".into(), command: "uptime -p".into() };
    c.save(web.clone(), None).unwrap();
    c.save(Connection { name: "db".into(), host: "10.0.0.7".into(), port: 22, ..Default::default() }, None).unwrap();
    let all = c.list();
    assert_eq!(all.iter().map(|x| x.name.as_str()).collect::<Vec<_>>(), ["db", "web box"]);
    assert_eq!(c.get("web box").unwrap().host, "web.example.com");
    assert_eq!(web.args(), ["-p", "2222", "-i", "uvault:ssh.deploy", "deploy@Web.Example.com", "uptime", "-p"]);
    assert_eq!(c.get("db").unwrap().args(), ["-p", "22", "10.0.0.7", "true"], "no command means a quick check");
    // the file holds names, never a key
    let file = std::fs::read_to_string(env.dir.join("connections.json")).unwrap();
    assert!(file.contains("ssh.deploy") && !file.contains("PRIVATE") && !file.contains("password"));
    assert_eq!(std::fs::metadata(env.dir.join("connections.json")).unwrap().permissions().mode() & 0o777, 0o600);
    // duplicates, renames, bad values
    assert!(c.save(Connection { name: "DB".into(), host: "x".into(), port: 22, ..Default::default() }, None).unwrap_err().contains("already"));
    c.save(Connection { name: "database".into(), ..c.get("db").unwrap() }, Some("db")).unwrap();
    assert!(c.get("db").is_none() && c.get("database").is_some());
    c.save(Connection { port: 2200, ..c.get("database").unwrap() }, Some("database")).unwrap();
    assert_eq!(c.get("database").unwrap().port, 2200);
    for bad in [
        Connection { name: "".into(), host: "h".into(), port: 22, ..Default::default() },
        Connection { name: "x".into(), host: "a b".into(), port: 22, ..Default::default() },
        Connection { name: "x".into(), host: "h".into(), port: 0, ..Default::default() },
        Connection { name: "x".into(), host: "h".into(), port: 22, user: "-evil".into(), ..Default::default() },
        Connection { name: "x".into(), host: "h".into(), port: 22, key: "id_rsa".into(), ..Default::default() },
        Connection { name: "x".into(), host: "h".into(), port: 22, command: "a\nb".into(), ..Default::default() },
        Connection { name: "x".into(), host: "h".into(), port: 22, command: "echo 'open".into(), ..Default::default() },
        Connection { name: "x/y".into(), host: "h".into(), port: 22, ..Default::default() },
    ] {
        assert!(bad.validate().is_err(), "{bad:?}");
        assert!(c.save(bad, None).is_err());
    }
    c.remove("database").unwrap();
    assert!(c.remove("database").is_err());
    assert_eq!(c.list().len(), 1);
    // a saved connection becomes a command line the sanitiser accepts
    assert!(plan("ssh", &web.args(), Mode::Assistant).is_ok());
}

#[test]
fn files_copied_with_scp_and_sftp_batches_stay_inside_the_file_scope() {
    let t = Tmp::new("scp");
    t.fake_openssh();
    let env = t.env(&[]);
    let work = t.0.join("work");
    env.known_hosts().trust("h.example.com", 22, &[key("h.example.com", 22)]).unwrap();
    env.known_hosts().trust("h.example.com", 2200, &[key("h.example.com", 2200)]).unwrap();
    let asked = Mutex::new(Vec::<(String, bool)>::new());
    let scope = |p: &str, w: bool| -> Result<PathBuf, String> {
        asked.lock().unwrap().push((p.to_string(), w));
        if p.contains("secret") { Err("permission denied: outside your folders".into()) } else { Ok(PathBuf::from("/real").join(p.trim_start_matches("./"))) }
    };
    run(&env, Mode::Assistant, "scp", &v(&["-P", "2200", "report.txt", "bob@h.example.com:/srv/in/"]), &work, 20, &scope, None).unwrap();
    let l = t.logged();
    let line = l.lines().find(|x| x.starts_with("scp ")).unwrap();
    assert!(line.ends_with("/real/report.txt bob@h.example.com:/srv/in/"), "the checked, resolved path is the one used: {line}");
    assert_eq!(asked.lock().unwrap()[0], ("report.txt".to_string(), false));
    // downloads need the write scope
    run(&env, Mode::Assistant, "scp", &v(&["h.example.com:/etc/hosts", "got/hosts"]), &work, 20, &scope, None).unwrap();
    assert_eq!(asked.lock().unwrap().last().unwrap(), &("got/hosts".to_string(), true));
    let e = run(&env, Mode::Assistant, "scp", &v(&["secret.txt", "h.example.com:/tmp/"]), &work, 20, &scope, None).unwrap_err();
    assert!(e.contains("permission denied"), "{e}");
    // sftp batch file: remote commands and scoped transfers only
    let batch = |text: &str| {
        std::fs::write(work.join("b.txt"), text).unwrap();
        let real = work.join("b.txt");
        let sc = move |p: &str, w: bool| -> Result<PathBuf, String> {
            if p == "b.txt" { return Ok(real.clone()) }
            if p.contains("secret") { return Err("permission denied".into()) }
            let _ = w;
            Ok(PathBuf::from("/real").join(p))
        };
        run(&env, Mode::Assistant, "sftp", &v(&["-b", "b.txt", "h.example.com"]), &work, 20, &sc, None)
    };
    assert!(batch("cd /srv\nls -l\nput up.txt /srv/up.txt\nget /srv/down.txt down.txt\n").is_ok());
    for bad in ["!id\n", "lcd /etc\n", "lpwd\n", "put secret.txt\n", "get /etc/passwd\n", "get /etc/passwd secret.out\n", "-!sh\n", "lmkdir x\n", "shell\n"] {
        assert!(batch(bad).is_err(), "{bad:?}");
    }
    // the person's own batch file is theirs
    std::fs::write(work.join("p.txt"), "!echo hi\n").unwrap();
    let sc = |p: &str, _: bool| -> Result<PathBuf, String> { Ok(PathBuf::from(p)) };
    assert!(run(&env, Mode::Person, "sftp", &v(&["-b", "p.txt", "h.example.com"]), &work, 20, &sc, None).is_ok());
    // Dropbear has no sftp
    let d = Tmp::new("scp2");
    d.script("dbclient", "echo 'Dropbear v2026.91' >&2\nexit 0\n");
    let denv = d.env(&[]);
    denv.known_hosts().trust("h.example.com", 22, &[key("h.example.com", 22)]).unwrap();
    let e = run(&denv, Mode::Person, "sftp", &v(&["h.example.com"]), &work, 20, &sc, None).unwrap_err();
    assert!(e.contains("has no sftp") && e.contains("Dropbear"), "{e}");
}

#[test]
fn dropbear_gets_a_home_with_only_the_keys_trusted_for_that_host_and_no_options_it_does_not_know() {
    let t = Tmp::new("db");
    let (log, ctl) = (t.log().display().to_string(), t.0.join("ctl").display().to_string());
    // a Dropbear stand-in that records its arguments and the known_hosts it finds in its HOME
    t.script("dbclient", &format!("if [ \"$1\" = \"-V\" ]; then echo 'Dropbear v2026.91'; exit 0; fi\n{{ echo \"ARGV: $*\"; echo \"HOME=$HOME\"; cat \"$HOME/.ssh/known_hosts\"; }} >> \"{log}\"\n[ -f \"{ctl}/err\" ] && cat \"{ctl}/err\" >&2\nexit $(cat \"{ctl}/rc\" 2>/dev/null || echo 0)\n"));
    let env = t.env(&[]);
    let work = t.0.join("work");
    env.known_hosts().trust("h.example.com", 2200, &[key("h.example.com", 2200)]).unwrap();
    env.known_hosts().trust("other.example.com", 22, &[key("o", 22)]).unwrap();
    let sc = |p: &str, _: bool| -> Result<PathBuf, String> { Ok(PathBuf::from(p)) };
    run(&env, Mode::Assistant, "ssh", &v(&["-p", "2200", "h.example.com", "id"]), &work, 20, &sc, None).unwrap();
    let l = t.logged();
    let argv = l.lines().find(|x| x.starts_with("ARGV: ")).unwrap();
    assert_eq!(argv, "ARGV: -o BatchMode=yes -o StrictHostKeyChecking=yes -p 2200 h.example.com id", "no `--` and no OpenSSH-only options");
    assert!(l.contains(&format!("h.example.com ssh-ed25519 {ED}")) && !l.contains("other.example.com"), "{l}");
    assert!(!l.contains(&env.home().display().to_string()), "the managed file is not used directly");
    let home = l.lines().find(|x| x.starts_with("HOME=")).unwrap().trim_start_matches("HOME=").to_string();
    assert!(!Path::new(&home).exists(), "the per-run home is removed");
    // Dropbear's own words for a changed key
    t.ctl("err", "ssh-ed25519 host key mismatch for h.example.com !\nFingerprint is SHA256:aaa\n");
    t.ctl("rc", "1");
    assert!(run(&env, Mode::Assistant, "ssh", &v(&["-p", "2200", "h.example.com", "id"]), &work, 20, &sc, None).unwrap_err().contains("HOST KEY CHANGED"));
}

/// Against a real Dropbear server on 127.0.0.1 only. Set IYAKKI_SSH_LOOPBACK to `<port>:<folder with dropbear, dbclient, ssh,
/// dropbearkey links>` after starting `dropbear -F -E -s -r <hostkey> -p 127.0.0.1:<port>`. Not part of the normal run.
#[test]
#[ignore]
fn loopback_dropbear_probe_trust_and_changed_key_end_to_end() {
    let Ok(spec) = std::env::var("IYAKKI_SSH_LOOPBACK") else { return };
    let (port, dir) = spec.split_once(':').expect("port:folder");
    let port: u16 = port.parse().unwrap();
    let t = Tmp::new("loop");
    let env = Env::new(&t.0.join("data"), "amy", dir, Arc::new(|n| Err(format!("no secret {n}"))));
    let c = env.client().expect("a Dropbear client");
    assert_eq!(c.flavor, Flavor::Dropbear);
    let sc = |p: &str, _: bool| -> Result<PathBuf, String> { Ok(PathBuf::from(p)) };
    let work = t.0.join("work");
    assert!(run(&env, Mode::Assistant, "ssh", &v(&["-p", &port.to_string(), "127.0.0.1", "true"]), &work, 20, &sc, None).unwrap_err().contains("has not been trusted"));
    let keys = probe(&c, "127.0.0.1", port).unwrap();
    env.known_hosts().trust("127.0.0.1", port, &keys).unwrap();
    let r = run(&env, Mode::Assistant, "ssh", &v(&["-p", &port.to_string(), "127.0.0.1", "echo", "loopback-ok"]), &work, 30, &sc, None);
    // the server may refuse the login (no key offered); a host-key problem must not be what stops it
    if let Err(e) = &r {
        assert!(!e.contains("HOST KEY CHANGED") && !e.contains("not been trusted"), "{e}");
    }
    env.known_hosts().trust("127.0.0.1", port, &[HostKey { b64: "AAAAC3NzaC1lZDI1NTE5AAAAIAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA".into(), ..keys[0].clone() }]).unwrap();
    assert!(run(&env, Mode::Assistant, "ssh", &v(&["-p", &port.to_string(), "127.0.0.1", "true"]), &work, 20, &sc, None).unwrap_err().contains("HOST KEY CHANGED"));
}
