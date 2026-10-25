use super::*;
use crate::gitkeys::{MemStore, SecretStore, Session};
use std::os::unix::fs::PermissionsExt;
use std::process::Command;
use std::sync::Mutex;
use std::time::Instant;

fn temp(tag: &str) -> PathBuf {
    let base = std::env::var("TMPDIR").unwrap_or_else(|_| "/tmp".into());
    let mut r = [0u8; 3];
    iyakki_sec::random_fill(&mut r).unwrap();
    let d = Path::new(&base).join(format!("gj-{tag}-{:02x}{:02x}{:02x}", r[0], r[1], r[2]));
    std::fs::create_dir_all(&d).unwrap();
    d
}

struct Fx {
    base: PathBuf,
    root: PathBuf,
    ctx: Ctx,
    jobs: Arc<Jobs>,
    store: Arc<MemStore>,
    audit: Arc<Mutex<Vec<String>>>,
}

fn fx(tag: &str, fake: bool) -> Fx {
    let base = temp(tag);
    let git = if fake { fake_git(&base) } else { PathBuf::from("git") };
    let root = base.join("ws");
    std::fs::create_dir_all(&root).unwrap();
    let store = Arc::new(MemStore::default());
    let audit = Arc::new(Mutex::new(Vec::new()));
    let a2 = audit.clone();
    let keys = Arc::new(GitKeys::new(&base.join("ssh"), store.clone(), Arc::new(move |a, b| a2.lock().unwrap().push(format!("{a} {b}"))), Arc::new(Session::default())));
    let ctx = Ctx {
        keys,
        tools: Tools { git, ssh: Ok(PathBuf::from("/usr/libexec/iyakki/git-ssh")), allow_local: true },
        owner: "alice".into(),
        workspace: "demo".into(),
        root: std::fs::canonicalize(&root).unwrap(),
        ws_keys: BTreeMap::new(),
        who: Who::Person,
        run_dir: base.join("run"),
        probe: Arc::new(|_, _| Err("no network in tests".into())),
        scope: Arc::new(|_| Ok(())),
    };
    Fx { base: base.clone(), root: ctx.root.clone(), ctx, jobs: Arc::new(Jobs::new(&base.join("jobs"))), store, audit }
}

fn fake_git(dir: &Path) -> PathBuf {
    let log = dir.join("calls.log");
    let p = dir.join("fakegit");
    std::fs::write(
        &p,
        format!(
            r#"#!/bin/sh
{{
echo "ARGV: $*"
echo "CWD: $(pwd)"
echo "SSH_AUTH_SOCK=$SSH_AUTH_SOCK"
echo "GIT_ASKPASS=$GIT_ASKPASS"
echo "ASKSOCK=$IYAKKI_GIT_ASKPASS_SOCK"
echo "GIT_SSH_COMMAND=$GIT_SSH_COMMAND"
echo "TERMINAL_PROMPT=$GIT_TERMINAL_PROMPT"
echo "SOCK_EXISTS=$([ -S "$SSH_AUTH_SOCK" ] && echo yes || echo no)"
}} >> '{log}'
case "$*" in *failme*) echo "fatal: repository 'https://x/failme' not found" >&2; exit 128;; esac
case "$1" in
  clone) for a; do last=$a; done; mkdir -p "$last" && echo hi > "$last/README"; echo "Receiving objects: 100% (3/3), done." >&2 ;;
  fetch|pull) echo "Already up to date." ;;
  push) echo "To origin" >&2 ;;
esac
sleep 0.3
exit 0
"#,
            log = log.display()
        ),
    )
    .unwrap();
    std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
    p
}

fn wait(b: &mut Batch, jobs: &Jobs, secs: u64) {
    let t = Instant::now();
    while b.pending() && t.elapsed() < Duration::from_secs(secs) {
        b.tick(jobs);
        std::thread::sleep(Duration::from_millis(40));
    }
    b.tick(jobs);
}

fn spec(url: &str) -> CloneSpec {
    CloneSpec { url: url.into(), ..Default::default() }
}

fn log_of(f: &Fx) -> String {
    std::fs::read_to_string(f.base.join("calls.log")).unwrap_or_default()
}

fn cleanup(f: Fx) {
    let _ = std::fs::remove_dir_all(&f.base);
}

#[test]
fn several_repositories_land_in_their_own_folders_two_at_a_time_and_project_json_lists_them() {
    let f = fx("multi", true);
    let specs = vec![spec("https://git.example.org/a/alpha.git"), CloneSpec { url: "https://git.example.org/b/beta".into(), branch: "dev".into(), depth: 1, folder: "second".into() }, spec("https://other.example.org/c/alpha.git"), spec("https://git.example.org/d/delta.git")];
    let mut b = Batch::plan_clone(f.ctx.clone(), &specs).unwrap();
    let folders: Vec<&str> = b.items.iter().map(|i| i.folder.as_str()).collect();
    assert_eq!(folders, ["alpha", "second", "alpha-2", "delta"], "names from the addresses, de-duplicated, or the one chosen");
    assert_eq!(b.phase, Phase::Running, "https only: no host keys to check");
    b.tick(&f.jobs);
    assert_eq!(b.running_count(), 2, "at most two at a time");
    assert_eq!(b.items.iter().filter(|i| i.state == ItemState::Queued).count(), 2);
    let mut max_running = 0;
    let t = Instant::now();
    while b.pending() && t.elapsed() < Duration::from_secs(30) {
        b.tick(&f.jobs);
        max_running = max_running.max(b.running_count());
        std::thread::sleep(Duration::from_millis(30));
    }
    assert!(max_running <= MAX_PARALLEL);
    assert_eq!(b.done(), 4, "{:?}", b.items.iter().map(|i| &i.state).collect::<Vec<_>>());
    assert_eq!(b.phase, Phase::Done);
    for d in ["alpha", "second", "alpha-2", "delta"] {
        assert!(f.root.join(d).join("README").is_file(), "{d}");
    }
    let c = crate::workspace::ProjectConfig::load(&f.root).unwrap();
    assert_eq!(c.repos.len(), 4);
    let second = c.repos.iter().find(|r| r.path == "second").unwrap();
    assert_eq!((second.url.as_str(), second.branch.as_str(), second.depth, second.state.as_str()), ("https://git.example.org/b/beta", "dev", 1, "cloned"));
    // the command: an array, `--` before the address, branch and depth as options, a cut-down environment, the folder last
    let log = log_of(&f);
    assert!(log.contains("ARGV: clone --progress --branch dev --depth 1 -- https://git.example.org/b/beta second"), "{log}");
    assert!(log.contains("TERMINAL_PROMPT=0") && log.contains("CWD: ") && log.contains("ASKSOCK="), "{log}");
    // jobs exist and belong to the workspace (the installer indicator shows them)
    let jobs = f.jobs.visible("alice", "demo");
    assert_eq!(jobs.len(), 4);
    assert!(jobs.iter().any(|j| j.label == "git clone a/alpha"), "{:?}", jobs.iter().map(|j| &j.label).collect::<Vec<_>>());
    // the key service is gone when the batch is done
    assert!(b.agent_dir().is_none());
    assert!(std::fs::read_dir(f.base.join("run")).map(|d| d.count() == 0).unwrap_or(true), "agent folder removed");
    cleanup(f);
}

#[test]
fn one_failure_does_not_stop_the_others_and_a_retry_runs_again() {
    let f = fx("partial", true);
    let mut b = Batch::plan_clone(f.ctx.clone(), &[spec("https://git.example.org/a/one.git"), spec("https://git.example.org/a/failme.git"), spec("https://git.example.org/a/three.git")]).unwrap();
    wait(&mut b, &f.jobs, 30);
    assert_eq!((b.done(), b.failed()), (2, 1));
    let bad = &b.items[1];
    assert!(matches!(&bad.state, ItemState::Failed(m) if m.contains("not found")), "{:?}", bad.state);
    assert_eq!(bad.last, "fatal: repository 'https://x/failme' not found");
    assert!(f.root.join("one/README").is_file() && f.root.join("three/README").is_file() && !f.root.join("failme").exists());
    assert_eq!(crate::workspace::ProjectConfig::load(&f.root).unwrap().repos.len(), 2, "only cloned repositories are listed");
    assert_eq!(b.phase, Phase::Done);
    // retry: the same failure again, counted; the finished ones stay finished
    assert!(b.retry(1) && !b.retry(0));
    assert_eq!(b.phase, Phase::Running);
    wait(&mut b, &f.jobs, 30);
    assert_eq!((b.done(), b.failed(), b.items[1].tries), (2, 1, 1));
    assert_eq!(log_of(&f).matches("failme").count() >= 2, true);
    cleanup(f);
}

#[test]
fn a_bad_line_is_reported_by_number_and_nothing_starts() {
    let f = fx("lines", true);
    std::fs::create_dir_all(f.root.join("taken")).unwrap();
    std::fs::write(f.root.join("taken/x"), "x").unwrap();
    let e = Batch::plan_clone(f.ctx.clone(), &[spec("https://ok.example/a/b.git"), spec("not a url"), spec("https://u:pw@h/o/r.git"), CloneSpec { url: "https://h/o/r".into(), folder: "taken".into(), ..Default::default() }, CloneSpec { url: "https://h/o/r".into(), branch: "-x".into(), ..Default::default() }, CloneSpec { url: "https://h/o/r".into(), folder: "../out".into(), ..Default::default() }]).err().unwrap();
    let nums: Vec<usize> = e.iter().map(|x| x.0).collect();
    assert_eq!(nums, [2, 3, 4, 5, 6], "{e:?}");
    assert!(e[1].1.contains("Settings") && !e[1].1.contains("pw"));
    assert!(e[2].1.contains("already exists"));
    assert!(log_of(&f).is_empty() && f.jobs.list("alice").is_empty());
    assert!(Batch::plan_clone(f.ctx.clone(), &[]).is_err());
    let many: Vec<CloneSpec> = (0..MAX_ITEMS + 1).map(|i| spec(&format!("https://h/o/r{i}"))).collect();
    assert!(Batch::plan_clone(f.ctx.clone(), &many).is_err());
    cleanup(f);
}

fn hk(host: &str, seed: u8) -> HostKey {
    let mut blob = Vec::new();
    crate::sshkey::put_str(&mut blob, b"ssh-ed25519");
    crate::sshkey::put_str(&mut blob, &[seed; 32]);
    HostKey { pattern: host.into(), kind: "ssh-ed25519".into(), b64: crate::sshkey::b64(&blob) }
}

#[test]
fn ssh_repositories_wait_for_the_host_key_get_the_key_through_the_agent_and_leave_no_key_behind() {
    let mut f = fx("ssh", true);
    f.ctx.probe = Arc::new(|h, _| Ok(vec![hk(h, 1)]));
    f.ctx.keys.generate("work", "w@x").unwrap();
    f.ctx.keys.set_host_key("git.example.org", "work").unwrap();
    let mut b = Batch::plan_clone(f.ctx.clone(), &[spec("git@git.example.org:team/app.git"), spec("git@nokey.example.org:team/other.git"), spec("https://git.example.org/team/web.git")]).unwrap();
    assert_eq!(b.phase, Phase::Probing);
    let t = Instant::now();
    while b.phase == Phase::Probing && t.elapsed() < Duration::from_secs(10) {
        b.tick(&f.jobs);
        std::thread::sleep(Duration::from_millis(20));
    }
    assert_eq!(b.phase, Phase::Confirm, "a host never seen before waits for the person");
    assert_eq!(b.prompts.len(), 2);
    let p = b.prompts.iter().find(|p| p.host == "git.example.org").unwrap().clone();
    assert!(p.keys[0].fingerprint().starts_with("SHA256:"));
    assert_eq!(log_of(&f), "", "nothing ran before the decision");
    b.tick(&f.jobs);
    assert_eq!(b.running_count(), 0);
    b.trust_host("git.example.org", 22).unwrap();
    b.refuse_host("nokey.example.org", 22);
    assert_eq!(b.phase, Phase::Running);
    assert!(matches!(&b.items[1].state, ItemState::Failed(m) if m.contains("did not trust")));
    assert!(!b.retry(1), "a refused host is not retried behind the person's back");
    // the host is now recorded in the person's known_hosts
    assert!(std::fs::read_to_string(f.ctx.keys.known_hosts_path()).unwrap().starts_with("git.example.org ssh-ed25519 "));
    b.tick(&f.jobs);
    assert!(b.agent_dir().is_some());
    let dir = b.agent_dir().unwrap().to_path_buf();
    assert!(dir.starts_with(f.base.join("run")), "the key service lives on the data volume, not in the workspace or /tmp");
    wait(&mut b, &f.jobs, 30);
    assert_eq!(b.done(), 2, "{:?}", b.items.iter().map(|i| &i.state).collect::<Vec<_>>());
    let log = log_of(&f);
    // the ssh item had the agent socket, a strict ssh command with the person's known hosts, and the socket existed while git ran
    let ssh_call = log.split("ARGV: ").find(|c| c.contains("git@git.example.org:team/app.git")).unwrap();
    assert!(ssh_call.contains("SOCK_EXISTS=yes") && ssh_call.contains("StrictHostKeyChecking=yes") && ssh_call.contains("IdentityAgent=") && ssh_call.contains("BatchMode=yes") && ssh_call.contains("-F /dev/null"), "{ssh_call}");
    assert!(ssh_call.contains(&f.ctx.keys.known_hosts_path().display().to_string()));
    let https_call = log.split("ARGV: ").find(|c| c.contains("team/web.git")).unwrap();
    assert!(https_call.contains("SSH_AUTH_SOCK=\n") && https_call.contains("GIT_SSH_COMMAND=\n"), "an https clone gets no ssh settings: {https_call}");
    // the audit names host and key, never key material
    let audit = f.audit.lock().unwrap().join("\n");
    assert!(audit.contains("git.use clone git.example.org via key work") && audit.contains("git.host.trust git.example.org:22 SHA256:"), "{audit}");
    assert!(!audit.contains("PRIVATE KEY"));
    // the agent folder and the private key are gone from the disk, and were never in it
    assert!(!dir.exists());
    for p in walk(&f.base) {
        let t = std::fs::read(&p).unwrap_or_default();
        assert!(!String::from_utf8_lossy(&t).contains("BEGIN OPENSSH PRIVATE KEY"), "private key text on disk: {}", p.display());
    }
    cleanup(f);
}

fn walk(d: &Path) -> Vec<PathBuf> {
    let mut out = vec![];
    for e in std::fs::read_dir(d).into_iter().flatten().flatten() {
        let p = e.path();
        if p.is_dir() && !p.is_symlink() {
            out.extend(walk(&p));
        } else {
            out.push(p);
        }
    }
    out
}

#[test]
fn a_changed_host_key_is_refused_with_the_explanation_and_nothing_is_sent() {
    let mut f = fx("changed", true);
    f.ctx.keys.generate("k", "k").unwrap();
    f.ctx.keys.set_host_key("git.example.org", "k").unwrap();
    f.ctx.keys.known_hosts().trust("git.example.org", 22, &[hk("git.example.org", 1)]).unwrap();
    f.ctx.probe = Arc::new(|h, _| Ok(vec![hk(h, 9)]));
    let mut b = Batch::plan_clone(f.ctx.clone(), &[spec("git@git.example.org:o/r.git")]).unwrap();
    wait(&mut b, &f.jobs, 10);
    assert!(matches!(&b.items[0].state, ItemState::Failed(m) if m.contains("CHANGED") && m.contains("Nothing was sent")), "{:?}", b.items[0].state);
    assert_eq!(log_of(&f), "");
    assert!(f.audit.lock().unwrap().iter().any(|l| l.contains("git.host.changed")));
    cleanup(f);
}

#[test]
fn an_ssh_address_without_a_key_or_with_a_locked_key_says_what_to_do() {
    let mut f = fx("nokey", true);
    f.ctx.probe = Arc::new(|h, _| Ok(vec![hk(h, 1)]));
    f.ctx.keys.known_hosts().trust("h.example.org", 22, &[hk("h.example.org", 1)]).unwrap();
    let mut b = Batch::plan_clone(f.ctx.clone(), &[spec("git@h.example.org:o/r.git")]).unwrap();
    wait(&mut b, &f.jobs, 10);
    assert!(matches!(&b.items[0].state, ItemState::Failed(m) if m.contains("no SSH key is set for h.example.org") && m.contains("Settings")), "{:?}", b.items[0].state);
    // a Dropbear-only system
    let mut g = fx("dropbear", true);
    g.ctx.tools.ssh = Err("this system has only Dropbear".into());
    g.ctx.probe = Arc::new(|h, _| Ok(vec![hk(h, 1)]));
    g.ctx.keys.known_hosts().trust("h.example.org", 22, &[hk("h.example.org", 1)]).unwrap();
    g.ctx.keys.generate("k", "k").unwrap();
    g.ctx.keys.set_host_key("h.example.org", "k").unwrap();
    let mut b = Batch::plan_clone(g.ctx.clone(), &[spec("git@h.example.org:o/r.git")]).unwrap();
    wait(&mut b, &g.jobs, 10);
    assert!(matches!(&b.items[0].state, ItemState::Failed(m) if m.contains("Dropbear")));
    // an assistant must be approved first
    let mut h = fx("assist", true);
    h.ctx.who = Who::Assistant;
    h.ctx.probe = Arc::new(|h, _| Ok(vec![hk(h, 1)]));
    h.ctx.keys.known_hosts().trust("h.example.org", 22, &[hk("h.example.org", 1)]).unwrap();
    h.ctx.keys.generate("k", "k").unwrap();
    h.ctx.keys.set_host_key("h.example.org", "k").unwrap();
    let mut b = Batch::plan_clone(h.ctx.clone(), &[spec("git@h.example.org:o/r.git")]).unwrap();
    wait(&mut b, &h.jobs, 10);
    assert!(matches!(&b.items[0].state, ItemState::Failed(m) if m.contains("needs your approval")), "{:?}", b.items[0].state);
    h.ctx.keys.session.approve("k");
    assert!(b.retry(0));
    wait(&mut b, &h.jobs, 20);
    assert_eq!(b.done(), 1);
    cleanup(f);
    cleanup(g);
    cleanup(h);
}

#[test]
fn a_token_reaches_git_only_through_the_askpass_socket_and_never_through_the_command_the_log_or_the_job_files() {
    let f = fx("token", true);
    f.ctx.keys.set_token("git.example.org", "octo", "ghp_TOPSECRETTOKEN").unwrap();
    let mut b = Batch::plan_clone(f.ctx.clone(), &[spec("https://git.example.org/team/private.git")]).unwrap();
    assert_eq!(b.items[0].auth, Auth::Token { host: "git.example.org".into() });
    wait(&mut b, &f.jobs, 20);
    assert_eq!(b.done(), 1);
    let log = log_of(&f);
    let asksock = log.lines().find_map(|l| l.strip_prefix("ASKSOCK=")).unwrap().to_string();
    assert!(!asksock.is_empty());
    // everything that was written anywhere: the token is not in it
    for p in walk(&f.base) {
        let t = std::fs::read(&p).unwrap_or_default();
        assert!(!String::from_utf8_lossy(&t).contains("TOPSECRETTOKEN"), "token found in {}", p.display());
    }
    let audit = f.audit.lock().unwrap().join("\n");
    assert!(!audit.contains("TOPSECRETTOKEN") && audit.contains("git.use clone git.example.org via token"));
    // (while the batch was running the socket answered; here we check the answer logic through a second batch that is kept alive)
    let mut b2 = Batch::plan_clone(f.ctx.clone(), &[spec("https://git.example.org/team/private2.git")]).unwrap();
    b2.tick(&f.jobs);
    let sock = b2.agent_dir().map(|d| d.join("askpass.sock")).unwrap();
    assert_eq!(crate::gitagent::askpass_client(sock.to_str().unwrap(), "Password for 'https://octo@git.example.org': ").unwrap(), "ghp_TOPSECRETTOKEN");
    assert_eq!(crate::gitagent::askpass_client(sock.to_str().unwrap(), "Password for 'https://octo@evil.example.org': ").unwrap(), "");
    wait(&mut b2, &f.jobs, 20);
    cleanup(f);
}

#[test]
fn the_ssh_command_quotes_paths_and_the_status_text_is_parsed() {
    let c = ssh_command(Path::new("/usr/libexec/iyakki/git-ssh"), Path::new("/data/a b/known_hosts"), Path::new("/data/it's/agent.sock"));
    assert!(c.contains("UserKnownHostsFile='/data/a b/known_hosts'") && c.contains("IdentityAgent='/data/it'\\''s/agent.sock'") && c.starts_with("'/usr/libexec/iyakki/git-ssh' -F /dev/null"), "{c}");
    let s = parse_status("# branch.oid abc\n# branch.head main\n# branch.upstream origin/main\n# branch.ab +2 -3\n1 .M N... 100644 100644 100644 a b src/x.rs\n1 M. N... 100644 100644 100644 a b y\n2 R. N... 100644 100644 100644 a b R100 new\told\nu UU N... 1 2 3 4 a b c z\n? new.txt\n? other\n");
    assert_eq!((s.branch.as_str(), s.upstream.as_str(), s.ahead, s.behind, s.changed, s.conflicted, s.untracked), ("main", "origin/main", 2, 3, 3, 1, 2));
    assert_eq!(s.summary(), "main ↑2 ↓3 ✎4 +2");
    assert!(s.dirty());
    let d = parse_status("# branch.oid abc\n# branch.head (detached)\n");
    assert!(d.detached && d.summary() == "detached" && !d.dirty());
    assert_eq!(parse_status("").summary(), "no commits");
}

// ---- with the real git (skipped when it is not installed)

fn have_git() -> bool {
    Command::new("git").arg("--version").output().is_ok()
}

fn git(dir: &Path, args: &[&str]) -> String {
    let o = Command::new("git").args(["-c", "user.name=t", "-c", "user.email=t@t", "-c", "init.defaultBranch=main", "-c", "protocol.file.allow=always"]).args(args).current_dir(dir).env("GIT_CONFIG_NOSYSTEM", "1").output().unwrap();
    assert!(o.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&o.stderr));
    String::from_utf8_lossy(&o.stdout).to_string()
}

/// A bare repository with one commit (and a second branch), made with the real git.
fn bare(base: &Path, name: &str) -> PathBuf {
    let work = base.join(format!("{name}-work"));
    std::fs::create_dir_all(&work).unwrap();
    git(&work, &["init", "-q"]);
    std::fs::write(work.join("README.md"), format!("# {name}\n")).unwrap();
    git(&work, &["add", "."]);
    git(&work, &["commit", "-q", "-m", "first"]);
    git(&work, &["branch", "dev"]);
    let bare = base.join(format!("{name}.git"));
    git(base, &["clone", "-q", "--bare", work.to_str().unwrap(), bare.to_str().unwrap()]);
    bare
}

#[test]
fn real_git_clones_two_local_repositories_a_missing_one_fails_alone_and_status_fetch_and_pull_work() {
    if !have_git() {
        eprintln!("skipped: no git");
        return;
    }
    let mut f = fx("real", false);
    f.ctx.tools.git = crate::ssh::find_program("git", &std::env::var("PATH").unwrap()).unwrap();
    let one = bare(&f.base, "one");
    let two = bare(&f.base, "two");
    let missing = f.base.join("nothing-here.git");
    let specs = vec![spec(one.to_str().unwrap()), CloneSpec { url: two.to_str().unwrap().into(), branch: "dev".into(), ..Default::default() }, spec(missing.to_str().unwrap())];
    let mut b = Batch::plan_clone(f.ctx.clone(), &specs).unwrap();
    wait(&mut b, &f.jobs, 60);
    assert_eq!((b.done(), b.failed()), (2, 1), "{:?}", b.items.iter().map(|i| (&i.state, &i.last)).collect::<Vec<_>>());
    assert!(f.root.join("one/README.md").is_file() && f.root.join("two/README.md").is_file());
    assert!(matches!(&b.items[2].state, ItemState::Failed(m) if !m.is_empty()));
    assert!(!f.root.join("nothing-here").exists(), "a failed clone leaves no folder");
    let c = crate::workspace::ProjectConfig::load(&f.root).unwrap();
    assert_eq!(c.repos.iter().map(|r| r.path.as_str()).collect::<Vec<_>>(), ["one", "two"]);
    // the branch asked for is the one checked out
    let st = status(&f.ctx.tools.git, &f.root.join("two")).unwrap();
    assert_eq!((st.branch.as_str(), st.ahead, st.behind, st.dirty()), ("dev", 0, 0, false));
    // a local commit is "ahead"; a file is "changed"; a new file is "untracked"
    std::fs::write(f.root.join("one/new.txt"), "x").unwrap();
    git(&f.root.join("one"), &["add", "."]);
    git(&f.root.join("one"), &["commit", "-q", "-m", "local"]);
    std::fs::write(f.root.join("one/README.md"), "changed\n").unwrap();
    std::fs::write(f.root.join("one/untracked.txt"), "u").unwrap();
    let st = status(&f.ctx.tools.git, &f.root.join("one")).unwrap();
    assert_eq!((st.branch.as_str(), st.ahead, st.behind, st.changed, st.untracked), ("main", 1, 0, 1, 1), "{st:?}");
    assert!(removal_risks(&f.ctx.tools.git, &f.root.join("one")).contains("1 commit(s) not pushed"));
    // upstream moves on: fetch shows "behind", pull is fast-forward only
    let pusher = f.base.join("pusher");
    git(&f.base, &["clone", "-q", two.to_str().unwrap(), pusher.to_str().unwrap()]);
    git(&pusher, &["checkout", "-q", "dev"]);
    std::fs::write(pusher.join("up.txt"), "up").unwrap();
    git(&pusher, &["add", "."]);
    git(&pusher, &["commit", "-q", "-m", "upstream"]);
    git(&pusher, &["push", "-q", "origin", "dev"]);
    let repos = crate::workspace::ProjectConfig::load(&f.root).unwrap().repos;
    let mut fb = Batch::plan_update(f.ctx.clone(), &repos, Op::Fetch).unwrap();
    wait(&mut fb, &f.jobs, 60);
    assert_eq!(fb.done(), 2, "{:?}", fb.items.iter().map(|i| (&i.state, &i.last)).collect::<Vec<_>>());
    let st = status(&f.ctx.tools.git, &f.root.join("two")).unwrap();
    assert_eq!((st.ahead, st.behind), (0, 1));
    assert!(!f.root.join("two/up.txt").exists(), "fetch does not change files");
    let clean: Vec<RepoEntry> = repos.iter().filter(|r| r.path == "two").cloned().collect();
    let mut pb = Batch::plan_update(f.ctx.clone(), &clean, Op::Pull).unwrap();
    wait(&mut pb, &f.jobs, 60);
    assert_eq!(pb.done(), 1);
    assert!(f.root.join("two/up.txt").is_file());
    assert!(log_args(&f.jobs).contains("--ff-only"));
    // a diverged branch cannot be pulled (fast-forward only): it fails and keeps the local commit
    git(&f.root.join("two"), &["reset", "-q", "--hard", "HEAD~1"]);
    std::fs::write(f.root.join("two/local.txt"), "l").unwrap();
    git(&f.root.join("two"), &["add", "."]);
    git(&f.root.join("two"), &["commit", "-q", "-m", "diverge"]);
    let mut pb = Batch::plan_update(f.ctx.clone(), &clean, Op::Pull).unwrap();
    wait(&mut pb, &f.jobs, 60);
    assert_eq!(pb.failed(), 1, "{:?}", pb.items[0].state);
    assert!(f.root.join("two/local.txt").is_file());
    // removal needs the typed name, stays inside, and updates project.json
    assert!(remove_repo(&f.root, "two", "tw").unwrap_err().contains("exactly"));
    assert!(remove_repo(&f.root, "..", "..").is_err());
    assert!(f.root.join("two").exists());
    remove_repo(&f.root, "two", "two").unwrap();
    assert!(!f.root.join("two").exists());
    assert_eq!(crate::workspace::ProjectConfig::load(&f.root).unwrap().repos.len(), 1);
    cleanup(f);
}

fn log_args(jobs: &Jobs) -> String {
    jobs.list("alice").iter().map(|j| j.command.clone()).collect::<Vec<_>>().join("\n")
}

#[test]
fn removing_a_folder_never_follows_a_link_out_of_the_workspace() {
    let f = fx("rm", false);
    let outside = f.base.join("outside");
    std::fs::create_dir_all(&outside).unwrap();
    std::fs::write(outside.join("keep"), "k").unwrap();
    std::os::unix::fs::symlink(&outside, f.root.join("lnk")).unwrap();
    assert!(remove_repo(&f.root, "lnk", "lnk").is_err());
    assert!(outside.join("keep").is_file());
    cleanup(f);
}

#[test]
fn git_missing_is_said_plainly() {
    match check_git("/nonexistent-folder") {
        GitCheck::Missing { message } => assert!(message.contains("git is not installed")),
        other => panic!("{other:?}"),
    }
    assert!(Tools::detect("/nonexistent-folder").is_err());
}

#[test]
fn a_platform_gitea_is_named_by_its_instance_and_its_token_is_adopted_once() {
    use crate::platform::installed::{self, Installed};
    let f = fx("inst", true);
    let dir = f.base.join("svc");
    let rec = Installed {
        id: "gitea-iyakki-gitea".into(),
        service: "gitea".into(),
        urls: vec![crate::platform::UrlInfo { label: "Gitea".into(), url: "http://gitea.lan.example:30300".into(), kind: "web".into() }],
        vault: vec!["svc.gitea-iyakki-gitea.token".into(), "svc.gitea-iyakki-gitea.admin".into()],
        ..Default::default()
    };
    installed::save(&dir, "alice", &[rec]).unwrap();
    let r = expand_instance(&dir, "alice", "@gitea-iyakki-gitea/team/app").unwrap().unwrap();
    assert_eq!((r.url.as_str(), r.host.as_str(), r.token_vault.as_deref()), ("http://gitea.lan.example:30300/team/app.git", "gitea.lan.example", Some("svc.gitea-iyakki-gitea.token")));
    assert_eq!(expand_instance(&dir, "alice", "gitea:gitea-iyakki-gitea/team/app.git").unwrap().unwrap().url, r.url);
    assert!(expand_instance(&dir, "alice", "https://github.com/a/b").is_none(), "an ordinary address is not an instance reference");
    let e = expand_instance(&dir, "alice", "@other/team/app").unwrap().unwrap_err();
    assert!(e.contains("gitea-iyakki-gitea"), "the known instances are named: {e}");
    assert!(expand_instance(&dir, "alice", "@gitea-iyakki-gitea/justone").unwrap().is_err());
    assert!(expand_instance(&dir, "bob", "@gitea-iyakki-gitea/team/app").unwrap().is_err(), "another person's services are not visible");
    // IYAKKI's token for the service becomes the host's token, once, without being shown
    f.store.set("svc.gitea-iyakki-gitea.token", "tok_abcdefghijklmnopqrstuv").unwrap();
    assert!(f.ctx.keys.adopt_token("gitea.lan.example", "git", "svc.gitea-iyakki-gitea.token").unwrap());
    assert!(!f.ctx.keys.adopt_token("gitea.lan.example", "git", "svc.gitea-iyakki-gitea.token").unwrap(), "a token the host already has is not replaced");
    let u = gitsite::parse(&r.url, ParseOptions::default()).unwrap();
    assert_eq!(f.ctx.keys.resolve(&u, &BTreeMap::new()), Auth::Token { host: "gitea.lan.example".into() });
    assert!(!f.audit.lock().unwrap().join("\n").contains("tok_abcdef"));
    cleanup(f);
}

#[test]
fn a_push_goes_through_the_job_with_the_connections_token_and_trusted_certificate_but_not_in_the_command() {
    use crate::gitconn::{Connection, Kind};
    let f = fx("push", true);
    let conns = f.ctx.keys.conns();
    let c = conns.add(Connection { name: "G".into(), kind: Kind::Gitea, url: "https://git.example.org".into(), account: "octo".into(), ..Default::default() }, Some("tok_FAKEFAKEFAKEFAKE1234")).unwrap();
    conns.trust_cert(&c.id, &crate::gitconn::cert_info("-----BEGIN CERTIFICATE-----\nAQID\n-----END CERTIFICATE-----\n").unwrap()).unwrap();
    // the host table has no token: the connection supplies it, read from its own vault entry
    let u = gitsite::parse("https://git.example.org/octo/app.git", ParseOptions::default()).unwrap();
    assert_eq!(f.ctx.keys.resolve(&u, &BTreeMap::new()), Auth::Token { host: "git.example.org".into() });
    assert!(f.ctx.keys.table().hosts.is_empty(), "nothing was copied into the host table");
    assert!(Batch::plan_push(f.ctx.clone(), "https://git.example.org/octo/app.git", "..", "main").is_err());
    assert!(Batch::plan_push(f.ctx.clone(), "https://git.example.org/octo/app.git", ".", "--force").is_err());
    assert!(Batch::plan_push(f.ctx.clone(), "https://octo:pw@git.example.org/octo/app.git", ".", "main").is_err());
    let mut b = Batch::plan_push(f.ctx.clone(), "https://git.example.org/octo/app.git", ".", "main").unwrap();
    assert_eq!((b.items[0].op, b.items[0].title().as_str()), (Op::Push, "git push octo/app"));
    wait(&mut b, &f.jobs, 20);
    assert_eq!(b.done(), 1, "{:?}", b.items[0].state);
    let log = log_of(&f);
    assert!(log.contains("ARGV: push --set-upstream --progress origin main"), "{log}");
    assert!(log.contains(&format!("CWD: {}", f.root.display())));
    for p in walk(&f.base) {
        if p.ends_with("pem") {
            continue;
        }
        let t = std::fs::read(&p).unwrap_or_default();
        assert!(!String::from_utf8_lossy(&t).contains("FAKEFAKEFAKE"), "token found in {}", p.display());
    }
    let audit = f.audit.lock().unwrap().join("\n");
    assert!(audit.contains("git.use push git.example.org via token") && !audit.contains("FAKEFAKE"), "{audit}");
    let (user, tok) = f.ctx.keys.token("git.example.org").unwrap();
    assert_eq!((user.as_str(), tok.as_str()), ("octo", "tok_FAKEFAKEFAKEFAKE1234"));
    // the job's argument array names the certificate file, never a secret
    let it = &b.items[0];
    let agent = crate::gitagent::GitAgent::start(&f.base.join("agent-x"), vec![], vec![], Arc::new(|_, _| {}), Arc::new(|_| {})).unwrap();
    let argv = command(&f.ctx, it, &agent).unwrap();
    assert!(argv.iter().any(|a| a.starts_with("GIT_SSL_CAINFO=") && a.ends_with("g.pem")), "{argv:?}");
    assert!(!argv.join(" ").contains("FAKEFAKEFAKE"));
    cleanup(f);
}
