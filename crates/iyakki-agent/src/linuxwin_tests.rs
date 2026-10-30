// #[cfg(test)] -- this whole file is test code (lib.rs includes it under #[cfg(test)]); the marker keeps the BOUNDARY check from counting it.
//! Tests of Linux windows: the session script and command, the launcher (`.desktop` files), the install steps with a fake runner,
//! starting and stopping a window with a fake spawner (golden argument array), cpu and memory from a fake `/proc`. No network, no namespaces.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use crate::linuxapps::{LinuxApps, Sandbox};
use crate::linuxwin::*;
use crate::rootfs;
use crate::wsexec::FakeRunner;

fn tmp(tag: &str) -> PathBuf {
    // the window's socket lives below this folder: a short path (the code refuses more than 100 bytes)
    crate::testdir::short("iyakki-linwin", tag)
}

/// A `gui` sandbox made by hand (no download): a root with the display server's file, optionally the script.
fn sandbox(data: &Path, runner: Arc<FakeRunner>, network: bool, with_xvnc: bool) -> LinuxApps {
    let mut la = LinuxApps::new(data, "admin", Path::new("/usr/bin/iyakki"), runner);
    la.assume(false, Some(50 << 30));
    let own = la.dir_of(SANDBOX);
    let root = la.rootfs_of(SANDBOX);
    for sub in ["usr/bin", "usr/local/bin", "usr/share/applications", "home/user", "mnt", "tmp", "etc"] {
        std::fs::create_dir_all(root.join(sub)).unwrap();
    }
    if with_xvnc {
        std::fs::write(root.join("usr/bin/Xvnc"), "x").unwrap();
        std::fs::write(root.join(SCRIPT_REL), session_script()).unwrap();
    }
    let s = Sandbox { name: SANDBOX.into(), owner: "admin".into(), release: RELEASE.into(), created: 1, network, mounts: vec![], id_base: 0, mem_mb: 2048, cpu_pct: 0, pids: 1024, verified: "test".into(), source: "test".into(), arch: "x86_64".into() };
    std::fs::write(own.join("sandbox.json"), serde_json::to_string(&s).unwrap()).unwrap();
    la
}

struct FakeProc {
    alive: Arc<AtomicBool>,
    killed: Arc<AtomicBool>,
}
impl Proc for FakeProc {
    fn pid(&self) -> Option<u32> {
        Some(4242)
    }
    fn alive(&mut self) -> bool {
        self.alive.load(Ordering::SeqCst)
    }
    fn kill(&mut self) {
        self.killed.store(true, Ordering::SeqCst);
        self.alive.store(false, Ordering::SeqCst);
    }
}

#[derive(Default)]
struct FakeSpawn {
    calls: Mutex<Vec<Vec<String>>>,
    alive: Arc<AtomicBool>,
    killed: Arc<AtomicBool>,
    make_socket: Option<PathBuf>,
    dies: bool,
}
impl Spawn for FakeSpawn {
    fn spawn(&self, argv: &[String], _cwd: &Path, log: &Path) -> Result<Box<dyn Proc>, String> {
        self.calls.lock().unwrap().push(argv.to_vec());
        self.alive.store(!self.dies, Ordering::SeqCst);
        std::fs::write(log, "Xvnc: no display\n").unwrap();
        if let Some(s) = &self.make_socket {
            std::fs::write(s, "").unwrap();
        }
        Ok(Box::new(FakeProc { alive: self.alive.clone(), killed: self.killed.clone() }))
    }
}

fn xterm() -> Program {
    Program { id: "xterm".into(), name: "XTerm".into(), exec: vec!["xterm".into()] }
}

#[test]
fn the_session_script_is_fixed_text_with_validated_sizes_and_no_eval() {
    let s = session_script();
    assert!(s.starts_with("#!/bin/sh\n"));
    assert!(s.contains("case \"$W$H\" in \"\"|*[!0-9]*) exit 2;; esac"), "sizes must be digits");
    assert!(s.contains("Xvnc :73 -geometry \"${W}x${H}\" -depth 24 -rfbunixpath \"$SOCK\" -rfbport -1 -SecurityTypes None -nolisten tcp -nolisten local -auth \"$XAUTHORITY\""), "{s}");
    assert!(s.contains("SOCK=/mnt/iyakki-display/display.sock"));
    assert!(s.contains("\"$@\" &"), "the program's words are passed on as they are");
    assert!(!s.contains("eval") && !s.contains("sh -c") && !s.contains("$*"));
    assert!(s.contains("matchbox-window-manager -use_titlebar no"));
    assert!(s.contains("DISPLAY=:73") && !s.contains(":1 ") && !s.contains("DISPLAY=:1"), "not a display number a desktop uses");
    assert!(s.contains("xauth -q -f \"$XAUTHORITY\" add :73 ."), "the display needs a secret key");
    assert!(s.contains("/dev/urandom") && !s.contains("-AcceptSetDesktopSize=0"));
}

#[test]
fn the_window_command_is_the_script_the_size_and_the_words() {
    assert_eq!(session_command(800, 600, &["xterm".into(), "-e".into(), "top".into()]).unwrap(), vec!["/usr/local/bin/iyakki-window-session", "800", "600", "xterm", "-e", "top"]);
    assert_eq!(session_command(1, 99999, &["x".into()]).unwrap()[1..3], ["64", "8192"], "sizes are bounded");
    assert!(session_command(10, 10, &[]).is_err());
    assert!(session_command(10, 10, &["".into()]).is_err());
    assert!(session_command(10, 10, &["a\0b".into()]).is_err());
}

#[test]
fn exec_lines_split_into_words_without_a_shell_and_without_field_codes() {
    assert_eq!(split_exec("xterm").unwrap(), ["xterm"]);
    assert_eq!(split_exec("mousepad %F").unwrap(), ["mousepad"]);
    assert_eq!(split_exec("env FOO=bar \"/usr/bin/my app\" --x %u %%").unwrap(), ["env", "FOO=bar", "/usr/bin/my app", "--x", "%"]);
    assert_eq!(split_exec(r#"prog "a \"quoted\" word""#).unwrap(), ["prog", "a \"quoted\" word"]);
    assert_eq!(split_exec("a;b|c && d $HOME `x`").unwrap(), ["a;b|c", "&&", "d", "$HOME", "`x`"], "nothing is expanded or split by shell characters");
    assert!(split_exec("").is_none() && split_exec("%f").is_none() && split_exec("\"unclosed").is_none() && split_exec("a\u{7}b").is_none());
}

#[test]
fn desktop_files_give_programs_that_are_shown_and_need_no_terminal() {
    let t = "# c\n[Desktop Entry]\nType=Application\nName=XTerm\nName[de]=Anders\nExec=xterm\nIcon=xterm\n\n[Desktop Action x]\nName=Other\nExec=evil\n";
    assert_eq!(parse_desktop("xterm", t), Some(Program { id: "xterm".into(), name: "XTerm".into(), exec: vec!["xterm".into()] }));
    assert!(parse_desktop("a", "[Desktop Entry]\nType=Application\nName=A\nExec=a\nNoDisplay=true\n").is_none());
    assert!(parse_desktop("a", "[Desktop Entry]\nType=Application\nName=A\nExec=a\nTerminal=true\n").is_none());
    assert!(parse_desktop("a", "[Desktop Entry]\nType=Link\nName=A\nURL=x\n").is_none());
    assert!(parse_desktop("a", "[Desktop Entry]\nType=Application\nName=A\n").is_none(), "no command");
    assert!(parse_desktop("a", "[Other]\nType=Application\nExec=a\n").is_none());
    assert_eq!(parse_desktop("fallback", "[Desktop Entry]\nType=Application\nExec=a\n").unwrap().name, "fallback");
}

#[test]
fn the_launcher_reads_only_plain_files_of_the_sandbox_root() {
    let d = tmp("launcher");
    let root = d.join("rootfs");
    let apps = root.join("usr/share/applications");
    std::fs::create_dir_all(&apps).unwrap();
    std::fs::write(apps.join("b.desktop"), "[Desktop Entry]\nType=Application\nName=B\nExec=b %f\n").unwrap();
    std::fs::write(apps.join("a.desktop"), "[Desktop Entry]\nType=Application\nName=A\nExec=a\n").unwrap();
    std::fs::write(apps.join("note.txt"), "x").unwrap();
    // a link to a host file is not followed
    std::fs::write(d.join("host.desktop"), "[Desktop Entry]\nType=Application\nName=Host\nExec=steal\n").unwrap();
    std::os::unix::fs::symlink(d.join("host.desktop"), apps.join("c.desktop")).unwrap();
    // too large
    std::fs::write(apps.join("big.desktop"), format!("[Desktop Entry]\nType=Application\nName=Big\nExec=big\n#{}\n", "x".repeat(70_000))).unwrap();
    let p = programs(&root);
    assert_eq!(p.iter().map(|p| p.id.as_str()).collect::<Vec<_>>(), ["a", "b"]);
    // the applications folder itself being a link
    let d2 = tmp("launcher2");
    let root2 = d2.join("rootfs");
    std::fs::create_dir_all(root2.join("usr/share")).unwrap();
    std::os::unix::fs::symlink(&apps, root2.join("usr/share/applications")).unwrap();
    assert!(programs(&root2).is_empty());
    assert!(programs(&d.join("missing")).is_empty());
    let _ = rootfs::remove_tree(&d);
    let _ = rootfs::remove_tree(&d2);
}

#[test]
fn the_install_runs_apt_with_the_display_packages_writes_the_script_and_restores_the_network_switch() {
    let d = tmp("install");
    let r = Arc::new(FakeRunner::new(&[]));
    r.answer("apt-get update", "ok").answer("--no-install-recommends", "installed").answer("dpkg-query", "2:1.12.0-9");
    // a sandbox without the script yet (Xvnc present, as apt would leave it) and its network off
    let la = sandbox(&d, r.clone(), false, false);
    std::fs::write(la.rootfs_of(SANDBOX).join("usr/bin/Xvnc"), "x").unwrap();
    assert!(!ready(&la), "no script yet");
    let st = iyakki_proxy::https::UreqStreamer::default();
    let mut said = Vec::new();
    let line = install(&la, &st, None, &mut |l| said.push(l.to_string())).unwrap();
    assert!(line.contains("ready") && line.contains("2:1.12.0-9"), "{line}");
    assert!(said.iter().any(|l| l.contains("never shipped")), "the licence note is said");
    assert!(ready(&la));
    assert_eq!(std::fs::read_to_string(la.rootfs_of(SANDBOX).join(SCRIPT_REL)).unwrap(), session_script());
    let lines = r.lines();
    let inst = lines.iter().find(|l| l.contains("apt-get install -y --no-install-recommends")).expect("apt install");
    for p in DISPLAY_PACKAGES {
        assert!(inst.contains(p), "{p}");
    }
    assert!(inst.contains("--net on"), "the apt run had the network: {inst}");
    assert!(!la.get(SANDBOX).unwrap().network, "the switch is back as it was");
    let log = std::fs::read_to_string(la.audit_path()).unwrap();
    assert!(log.contains("window.install.start") && log.contains("\twindow.install\t"), "{log}");
    // a failing apt is reported with its last lines and nothing is marked ready
    let r2 = Arc::new(FakeRunner::new(&[]));
    r2.answer("apt-get update", "ok").fail("--no-install-recommends", 100, "E: Unable to locate package tigervnc-standalone-server");
    let d2 = tmp("install-fail");
    let la2 = sandbox(&d2, r2, true, false);
    let e = install(&la2, &st, None, &mut |_| {}).unwrap_err();
    assert!(e.contains("Unable to locate"), "{e}");
    assert!(la2.audit_tail(5).iter().any(|a| a.2 == "window.install.failed"));
    let _ = rootfs::remove_tree(&d);
    let _ = rootfs::remove_tree(&d2);
}

#[test]
fn only_the_offered_test_programs_can_be_installed_and_only_after_the_base() {
    let d = tmp("prog");
    let r = Arc::new(FakeRunner::new(&[]));
    r.answer("apt-get update", "ok").answer("--no-install-recommends", "ok");
    let la = sandbox(&d, r.clone(), false, true);
    assert!(install_program(&la, "evil-pkg", None).unwrap_err().contains("not one of the offered"));
    assert!(install_program(&la, "--help", None).is_err());
    install_program(&la, "x11-apps", None).unwrap();
    assert!(r.lines().iter().any(|l| l.ends_with("apt-get install -y --no-install-recommends x11-apps")));
    assert!(!la.get(SANDBOX).unwrap().network);
    let d2 = tmp("prog2");
    let la2 = sandbox(&d2, r, true, false);
    assert!(install_program(&la2, "xterm", None).unwrap_err().contains("install Linux windows first"));
    let _ = rootfs::remove_tree(&d);
    let _ = rootfs::remove_tree(&d2);
}

#[test]
fn starting_a_window_builds_the_golden_argument_array() {
    let d = tmp("start");
    let la = sandbox(&d, Arc::new(FakeRunner::new(&[])), false, true);
    let sp = FakeSpawn::default();
    let w = Window::start(&la, &sp, &xterm(), 1280, 720).unwrap();
    let argv = sp.calls.lock().unwrap()[0].clone();
    let own = std::fs::canonicalize(la.dir_of(SANDBOX)).unwrap();
    let run = own.join("run");
    let root = own.join("rootfs");
    assert_eq!(argv[0], "/usr/bin/iyakki");
    assert_eq!(argv[1], "linuxapp-init");
    let at = |flag: &str| argv.iter().position(|a| a == flag).unwrap_or_else(|| panic!("{flag} in {argv:?}"));
    assert_eq!(argv[at("--root") + 1], root.display().to_string());
    assert_eq!(argv[at("--uid") + 1], "1000", "not root inside");
    assert_eq!(argv[at("--net") + 1], "off", "the network switch of the sandbox is kept");
    let b = at("--bind-rw");
    assert_eq!((argv[b + 1].as_str(), argv[b + 2].as_str()), (run.to_str().unwrap(), "/mnt/iyakki-display"));
    let dd = at("--");
    assert_eq!(&argv[dd + 1..], ["/usr/local/bin/iyakki-window-session", "1280", "720", "xterm"]);
    // host paths in the array: only the sandbox folder's own root and the runtime folder
    for a in &argv {
        if a.starts_with('/') && a != "/usr/bin/iyakki" && !a.starts_with("/usr/local/bin/") && !a.starts_with("/mnt/") && !a.starts_with("/home/") && !a.starts_with("/root") && !a.starts_with("/tmp") {
            assert!(a.starts_with(own.to_str().unwrap()), "unexpected host path {a}");
        }
    }
    assert_eq!(argv.iter().filter(|a| a.as_str() == "--bind-rw" || a.as_str() == "--bind-ro").count(), 1, "one folder only");
    assert_eq!(w.socket(), la.dir_of(SANDBOX).join("run").join("display.sock"));
    let log = std::fs::read_to_string(la.audit_path()).unwrap();
    assert!(log.contains("window.start") && log.contains("xterm") && log.contains("0 argument(s)") && log.contains("1280x720"), "{log}");
    drop(w);
    assert!(sp.killed.load(Ordering::SeqCst), "closing the window kills the sandbox's process, and with it the pid namespace");
    assert!(std::fs::read_to_string(la.audit_path()).unwrap().contains("window.stop"));
    // sandbox in the data folder: the runtime folder is private
    use std::os::unix::fs::PermissionsExt;
    assert_eq!(std::fs::metadata(la.dir_of(SANDBOX).join("run")).unwrap().permissions().mode() & 0o777, 0o700);
    let _ = rootfs::remove_tree(&d);
}

#[test]
fn a_window_needs_the_install_and_only_a_runtime_folder_of_the_sandbox_itself() {
    let d = tmp("needs");
    let la = sandbox(&d, Arc::new(FakeRunner::new(&[])), true, false);
    assert!(Window::start(&la, &FakeSpawn::default(), &xterm(), 100, 100).unwrap_err().contains("not installed"));
    std::fs::write(la.rootfs_of(SANDBOX).join("usr/bin/Xvnc"), "x").unwrap();
    std::fs::write(la.rootfs_of(SANDBOX).join(SCRIPT_REL), "x").unwrap();
    // another folder as the runtime folder is refused by the manager
    let other = tmp("needs-other");
    let cmd = session_command(100, 100, &["xterm".into()]).unwrap();
    let e = la.exec_argv_with_run(SANDBOX, &cmd, &Default::default(), &other, RUN_DST).unwrap_err();
    assert!(e.contains("folder of the sandbox itself"), "{e}");
    let _ = rootfs::remove_tree(&d);
    let _ = rootfs::remove_tree(&other);
}

#[test]
fn waiting_for_the_display_reports_a_program_that_ended_or_a_slow_start() {
    let d = tmp("wait");
    let la = sandbox(&d, Arc::new(FakeRunner::new(&[])), false, true);
    // ends at once: the log tail is in the message
    let sp = FakeSpawn { dies: true, ..Default::default() };
    let mut w = Window::start(&la, &sp, &xterm(), 100, 100).unwrap();
    let e = w.wait_socket(std::time::Duration::from_secs(5), &|_| {}).unwrap_err();
    assert!(e.contains("ended before") && e.contains("Xvnc: no display"), "{e}");
    // the socket appears
    let sock = la.dir_of(SANDBOX).join("run/display.sock");
    let sp = FakeSpawn { make_socket: Some(sock), ..Default::default() };
    let mut w2 = Window::start(&la, &sp, &xterm(), 100, 100).unwrap();
    w2.wait_socket(std::time::Duration::from_secs(5), &|_| {}).unwrap();
    assert!(w2.alive() && w2.pid() == Some(4242));
    // alive but no socket in time
    let d2 = tmp("wait2");
    let la2 = sandbox(&d2, Arc::new(FakeRunner::new(&[])), false, true);
    let sp = FakeSpawn::default();
    let mut w3 = Window::start(&la2, &sp, &xterm(), 100, 100).unwrap();
    let e = w3.wait_socket(std::time::Duration::from_millis(0), &|_| {}).unwrap_err();
    assert!(e.contains("did not appear"), "{e}");
    w.stop();
    w.stop();
    let _ = rootfs::remove_tree(&d);
    let _ = rootfs::remove_tree(&d2);
}

fn fake_proc(root: &Path, pid: u32, ppid: u32, utime: u64, stime: u64, rss_kb: u64, name: &str) {
    let d = root.join(pid.to_string());
    std::fs::create_dir_all(&d).unwrap();
    // pid (name) state ppid pgrp session tty tpgid flags minflt cminflt majflt cmajflt utime stime
    std::fs::write(d.join("stat"), format!("{pid} ({name}) S {ppid} 1 1 0 -1 0 0 0 0 0 {utime} {stime} 0 0 20 0 1 0 1 1 1\n")).unwrap();
    std::fs::write(d.join("status"), format!("Name:\t{name}\nVmRSS:\t{rss_kb} kB\n")).unwrap();
}

#[test]
fn cpu_and_memory_are_summed_over_the_processes_below_the_started_one() {
    let d = tmp("proc");
    fake_proc(&d, 100, 1, 10, 5, 1000, "iyakki");
    fake_proc(&d, 101, 100, 100, 20, 20_000, "Xvnc (odd) name");
    fake_proc(&d, 102, 101, 7, 3, 5000, "xterm");
    fake_proc(&d, 103, 102, 1, 1, 300, "bash");
    fake_proc(&d, 200, 1, 9999, 9999, 999_999, "unrelated");
    std::fs::create_dir_all(d.join("self")).unwrap();
    std::fs::write(d.join("junk"), "x").unwrap();
    let u = usage_of(&d, 100);
    assert_eq!(u, Usage { procs: 4, rss_kb: 26_300, ticks: 15 + 120 + 10 + 2 });
    assert_eq!(usage_of(&d, 555).procs, 0, "an unknown process has nothing below it");
    assert_eq!(usage_of(&d.join("nothing"), 100), Usage::default());
    assert!((cpu_percent(Usage { ticks: 100, ..Default::default() }, Usage { ticks: 150, ..Default::default() }, 1.0) - 50.0).abs() < 0.01);
    assert_eq!(cpu_percent(Usage { ticks: 100, ..Default::default() }, Usage { ticks: 50, ..Default::default() }, 1.0), 0.0);
    assert_eq!(cpu_percent(Usage::default(), Usage::default(), 0.0), 0.0);
    let _ = rootfs::remove_tree(&d);
}

#[test]
fn deleting_the_sandbox_needs_its_name() {
    let d = tmp("remove");
    let la = sandbox(&d, Arc::new(FakeRunner::new(&[])), false, true);
    assert!(remove(&la, "yes").unwrap_err().contains("type the name"));
    assert!(la.get(SANDBOX).is_ok());
    remove(&la, "gui").unwrap();
    assert!(la.get(SANDBOX).is_err());
    assert_eq!(count_ready(&d), 0);
    let _ = rootfs::remove_tree(&d);
}

#[test]
fn the_ready_count_needs_the_sandbox_the_server_and_the_script() {
    let d = tmp("count");
    let la = sandbox(&d, Arc::new(FakeRunner::new(&[])), false, true);
    assert_eq!(count_ready(&d), 1);
    std::fs::remove_file(la.rootfs_of(SANDBOX).join(SCRIPT_REL)).unwrap();
    assert_eq!(count_ready(&d), 0);
    let _ = rootfs::remove_tree(&d);
}

#[test]
fn the_setup_item_is_listed_under_linux_apps_and_marks_itself_done() {
    use crate::quickinstall::{items_of, Group, Item};
    assert_eq!(Item::parse("linux-windows"), Some(Item::LinuxWindows));
    assert_eq!(Item::LinuxWindows.group(), Group::Linux);
    assert!(items_of(Group::Linux).contains(&Item::LinuxWindows));
    assert!(Item::LinuxWindows.licence().contains("never shipped") && Item::LinuxWindows.licence().contains("does not rebrand"));
    assert!(Item::LinuxWindows.about().contains("never part of an image"));
    assert!(Item::LinuxWindows.runs_as_job() && Item::LinuxWindows.needs_now().is_empty());
}

#[test]
fn a_debian_alternatives_link_counts_as_the_display_server() {
    // in Debian /usr/bin/Xvnc is an absolute link to /etc/alternatives/Xvnc: it dangles when seen from the host, and must still count
    let d = tmp("alt-link");
    let la = sandbox(&d, Arc::new(FakeRunner::new(&[])), false, true);
    let x = la.rootfs_of(SANDBOX).join("usr/bin/Xvnc");
    std::fs::remove_file(&x).unwrap();
    std::os::unix::fs::symlink("/etc/alternatives/Xvnc", &x).unwrap();
    assert!(!x.exists(), "dangling on the host");
    assert!(ready(&la) && count_ready(&d) == 1);
    std::fs::remove_file(&x).unwrap();
    assert!(!ready(&la));
    let _ = rootfs::remove_tree(&d);
}

#[test]
fn a_sandbox_that_cannot_start_is_named_first_and_whole() {
    let out = "linuxapp-init: note: no cgroup v2 here\nlinuxapp-init: entering the sandbox failed: pivot_root: Invalid argument (os error 22)\n";
    let m = apt_failure("the packages", out);
    assert_eq!(m, "the sandbox could not start: entering the sandbox failed: pivot_root: Invalid argument (os error 22) (apt did not run)");
    assert!(!m.contains("..."), "{m}");
    let plain = apt_failure("curl", "Reading package lists...\nE: Unable to locate package curl\n");
    assert!(plain.starts_with("apt could not install curl: ") && plain.contains("Unable to locate"), "{plain}");
}
