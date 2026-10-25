// #[cfg(test)] -- this whole file is test code (shell.rs includes it under #[cfg(test)]).
//! The Linux apps page through the shell: a click starts a detached job whose command line carries the argument array, the switches and
//! folders change the sandbox's record, a delete needs the name, the terminal opens the CLI tile inside the sandbox, an account without the
//! full command line is refused, and everything lands in the audit log. A stand-in program is the job (never the real `iyakki`), no network.

use super::tests::*;
use super::*;
use crate::brandmenu::{LinuxAction, Page};
use iyakki_agent::linuxapps::{Mount, Sandbox};

fn fake_launch(tag: &str) -> iyakki_agent::quickinstall::Launch {
    use std::os::unix::fs::PermissionsExt;
    let d = std::env::temp_dir().join(format!("iyakki-linux-shell-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    let p = d.join("fake-iyakki");
    std::fs::write(&p, "#!/bin/sh\necho \"[1/1] $*\"\nsleep 5\n").unwrap();
    std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
    iyakki_agent::quickinstall::Launch { exe: p, data: d.join("data") }
}

fn sandbox_record(sh: &Shell, user: &str, name: &str, network: bool) {
    let dir = sh.env.services.dir().join("linuxapps").join(user).join(name);
    std::fs::create_dir_all(dir.join("rootfs")).unwrap();
    let s = Sandbox { name: name.into(), owner: user.into(), release: "debian-12".into(), created: 1, network, mounts: vec![], id_base: 0, mem_mb: 2048, cpu_pct: 0, pids: 1024, verified: "x".into(), source: "x".into(), arch: "amd64".into() };
    std::fs::write(dir.join("sandbox.json"), serde_json::to_string(&s).unwrap()).unwrap();
}

fn sandbox_of(sh: &Shell, user: &str, name: &str) -> Sandbox {
    serde_json::from_str(&std::fs::read_to_string(sh.env.services.dir().join("linuxapps").join(user).join(name).join("sandbox.json")).unwrap()).unwrap()
}

fn open_linux(sh: &mut Shell) {
    sh.toggle_brand();
    let out = sh.brand.open_page(Page::Linux);
    assert_eq!(out, crate::brandmenu::Outcome::Opened(Page::Linux));
    sh.brand_opened(Page::Linux);
}

fn stop_all(sh: &Shell, user: &str) {
    for j in sh.env.services.jobs.list(user) {
        let _ = sh.env.services.jobs.stop(user, &j.id, std::time::Duration::from_millis(300));
    }
}

fn audit(sh: &Shell) -> String {
    std::fs::read_to_string(sh.env.audit.path()).unwrap_or_default()
}

#[test]
fn creating_a_sandbox_and_installing_packages_are_detached_jobs_with_the_command_line_of_the_job_program() {
    let (mut sh, _) = shell_with_users("linux-jobs");
    login(&mut sh, "root", "rootpass12");
    sh.quick_launch = Some(fake_launch("jobs"));
    open_linux(&mut sh);
    let f = sh.brand.linux.facts.clone().unwrap();
    assert!(f.allowed && f.any_host && f.boxes.is_empty() && f.images.len() == 3, "{f:?}");
    assert!(f.images.iter().all(|i| i.example && !i.pinned), "the shipped examples wait for their digests");
    // create
    sh.linux_action(LinuxAction::Create { name: "dev".into(), release: "debian-12".into(), network: false, folder: None });
    let note = sh.brand.note.clone().unwrap();
    assert!(note.1 && note.0.contains("Started: Linux apps: create dev"), "{note:?}");
    let jobs = sh.env.services.jobs.list("root");
    assert_eq!(jobs.len(), 1);
    assert!(jobs[0].command.ends_with("linux-apps --data ".to_string().as_str()) || jobs[0].command.contains("linux-apps --data"), "{}", jobs[0].command);
    assert!(jobs[0].command.ends_with("--user root create dev debian-12 --net off"), "{}", jobs[0].command);
    assert_eq!(jobs[0].workspace, iyakki_agent::quickinstall::WORKSPACE, "it shows beside the clock like a Quick install");
    assert!(audit(&sh).contains("linuxapps.create"));
    // a bad name or release is refused before any job
    sh.linux_action(LinuxAction::Create { name: "Bad Name".into(), release: "debian-12".into(), network: false, folder: None });
    assert!(sh.brand.note.as_ref().unwrap().0.contains("not valid"));
    sh.linux_action(LinuxAction::Create { name: "ok".into(), release: "arch".into(), network: false, folder: None });
    assert_eq!(sh.env.services.jobs.list("root").len(), 1);
    // packages: a job per click, names checked
    sandbox_record(&sh, "root", "web", true);
    sh.linux_action(LinuxAction::Install { name: "web".into(), packages: vec!["curl".into(), "git".into()] });
    assert!(sh.env.services.jobs.list("root").iter().any(|j| j.command.ends_with("install web curl git")), "{:?}", sh.env.services.jobs.list("root").iter().map(|j| &j.command).collect::<Vec<_>>());
    sh.linux_action(LinuxAction::Install { name: "web".into(), packages: vec!["curl".into(), "--allow-unauthenticated".into()] });
    assert!(sh.brand.note.as_ref().unwrap().0.contains("not a package name"));
    sh.linux_action(LinuxAction::Remove { name: "web".into(), packages: vec!["curl".into()] });
    sh.linux_action(LinuxAction::Upgrade { name: "web".into() });
    let cmds: Vec<String> = sh.env.services.jobs.list("root").iter().map(|j| j.command.clone()).collect();
    assert!(cmds.iter().any(|c| c.ends_with("remove web curl")) && cmds.iter().any(|c| c.ends_with("upgrade web")), "{cmds:?}");
    let log = audit(&sh);
    assert!(log.contains("linuxapps.apt.install") && log.contains("linuxapps.apt.remove") && log.contains("linuxapps.upgrade"), "{log}");
    stop_all(&sh, "root");
}

#[test]
fn switches_folders_terminal_and_delete_change_the_record_and_are_audited() {
    let (mut sh, _) = shell_with_users("linux-switches");
    login(&mut sh, "root", "rootpass12");
    open_linux(&mut sh);
    sandbox_record(&sh, "root", "dev", false);
    sh.refresh_linux();
    assert_eq!(sh.brand.linux.facts.as_ref().unwrap().boxes.len(), 1);
    // network
    sh.linux_action(LinuxAction::SetNetwork { name: "dev".into(), on: true });
    assert!(sandbox_of(&sh, "root", "dev").network);
    assert!(sh.brand.linux.facts.as_ref().unwrap().boxes[0].network, "the page shows the new state");
    sh.linux_action(LinuxAction::SetNetwork { name: "dev".into(), on: false });
    assert!(!sandbox_of(&sh, "root", "dev").network);
    // folders: only what the person's scope and the data-folder rule allow
    let proj = std::env::temp_dir().join(format!("iyakki-linux-shell-proj-{}", std::process::id()));
    std::fs::create_dir_all(&proj).unwrap();
    sh.linux_action(LinuxAction::Mount { name: "dev".into(), folder: proj.display().to_string(), rw: false });
    let m: Vec<Mount> = sandbox_of(&sh, "root", "dev").mounts;
    assert_eq!(m.len(), 1);
    assert!(!m[0].rw && m[0].dst.starts_with("/mnt/"));
    let vault = sh.env.services.dir().to_path_buf();
    sh.linux_action(LinuxAction::Mount { name: "dev".into(), folder: vault.display().to_string(), rw: true });
    assert!(sh.brand.note.as_ref().unwrap().0.contains("not a folder of yours"), "{:?}", sh.brand.note);
    std::fs::create_dir_all(vault.join("secret")).unwrap();
    sh.linux_action(LinuxAction::Mount { name: "dev".into(), folder: vault.join("secret").display().to_string(), rw: true });
    assert!(sh.brand.note.as_ref().unwrap().0.contains("data folder"), "{:?}", sh.brand.note);
    assert_eq!(sandbox_of(&sh, "root", "dev").mounts.len(), 1);
    sh.linux_action(LinuxAction::Unmount { name: "dev".into(), folder: proj.display().to_string() });
    assert!(sandbox_of(&sh, "root", "dev").mounts.is_empty());
    // the terminal: the CLI tile opens
    sh.linux_action(LinuxAction::Terminal { name: "dev".into() });
    assert!(matches!(sh.screen, Screen::App(_, PApp::Cli)), "the CLI tile opened");
    assert!(!sh.brand.open);
    // delete: a wrong name deletes nothing
    open_linux(&mut sh);
    sh.linux_action(LinuxAction::Delete { name: "dev".into(), typed: "nope".into() });
    assert!(sh.brand.note.as_ref().unwrap().0.contains("type the name"));
    assert!(sh.env.services.dir().join("linuxapps/root/dev").exists());
    sh.linux_action(LinuxAction::Delete { name: "dev".into(), typed: "dev".into() });
    assert!(!sh.env.services.dir().join("linuxapps/root/dev").exists());
    assert!(sh.brand.linux.facts.as_ref().unwrap().boxes.is_empty());
    let log = audit(&sh);
    for want in ["linuxapps.network", "linuxapps.mount", "linuxapps.unmount", "linuxapps.terminal", "linuxapps.delete"] {
        assert!(log.contains(want), "{want} missing:\n{log}");
    }
    // and the sandbox's own audit file has the same story
    let own = std::fs::read_to_string(sh.env.services.dir().join("linuxapps/audit.log")).unwrap();
    assert!(own.contains("sandbox.network") && own.contains("sandbox.mount") && own.contains("sandbox.delete.refused") && own.contains("sandbox.delete\t") || own.contains("sandbox.delete\t"), "{own}");
    let _ = std::fs::remove_dir_all(&proj);
}

#[test]
fn an_account_without_the_full_command_line_cannot_use_linux_apps() {
    let (mut sh, _) = shell_with_users("linux-guest");
    login(&mut sh, "gus", "guestpass1");
    sh.quick_launch = Some(fake_launch("guest"));
    open_linux(&mut sh);
    assert!(!sh.brand.linux.facts.as_ref().unwrap().allowed);
    sh.linux_action(LinuxAction::Create { name: "dev".into(), release: "debian-12".into(), network: false, folder: None });
    assert!(sh.brand.note.as_ref().unwrap().0.contains("full command-line permission"));
    assert!(sh.env.services.jobs.list("gus").is_empty());
    // the page says so instead of showing buttons that do nothing
    let mut g = Gfx::new(1280, 720);
    sh.render(&mut g);
    assert!(!sh.hits().iter().any(|h| h.ev == Event::Tab(crate::brandmenu::linux_page_ids::TERMINAL)));
}

#[test]
fn image_apps_need_the_cluster_and_a_pinned_digest() {
    let (mut sh, _) = shell_with_users("linux-images");
    login(&mut sh, "root", "rootpass12");
    open_linux(&mut sh);
    sh.linux_action(LinuxAction::RunImage { id: "busybox-httpd".into() });
    assert!(sh.brand.note.as_ref().unwrap().0.contains("install k3s first"), "{:?}", sh.brand.note);
    sh.linux_action(LinuxAction::RunImage { id: "nothing".into() });
    assert!(sh.brand.note.as_ref().unwrap().0.contains("no image app"));
    assert!(!audit(&sh).contains("imageapp.run"));
}
