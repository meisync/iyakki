//! The CLI tile's Connections tab and the `ssh` entry of the CLIs tab: saved hosts, the host-key question, Connect, the
//! terminal's `ssh` and `sshtrust`, key generation, and geometry at the four screen sizes. A stand-in `ssh` on a temporary search
//! path answers; nothing connects to any machine.

use std::os::unix::fs::PermissionsExt;

use super::*;
use crate::apps::tests::cx_with_backend;

const ED: &str = "AAAAC3NzaC1lZDI1NTE5AAAAIKBc6HPCQiTnZ0RTiREqU3BRULCRSFwDp8N5+hGLuS4H";
const ED_FP: &str = "SHA256:3XJFuuO18/NM/NTpVFPkp7Fp35hDb42XVWDdJjfIXFo";

struct Fake {
    dir: std::path::PathBuf,
}

impl Fake {
    fn new(tag: &str) -> Fake {
        let dir = std::env::temp_dir().join(format!("iyakki-conntest-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("bin")).unwrap();
        std::fs::create_dir_all(dir.join("ctl")).unwrap();
        let f = Fake { dir };
        let (log, ctl) = (f.log().display().to_string(), f.dir.join("ctl").display().to_string());
        f.script("ssh", &format!(
            r#"LOG="{log}"; CTL="{ctl}"
if [ "$1" = "-V" ]; then echo "OpenSSH_9.6p1 Ubuntu-3, OpenSSL 3.0.13" >&2; exit 0; fi
echo "ARGV: $*" >> "$LOG"
for a in "$@"; do
  case "$a" in StrictHostKeyChecking=accept-new) probe=1;; esac
  case "$a" in UserKnownHostsFile=*) kh="${{a#UserKnownHostsFile=}}";; esac
done
if [ -n "$probe" ] && [ -f "$CTL/probekey" ]; then echo "127.0.0.1 $(cat "$CTL/probekey")" > "$kh"; exit 255; fi
[ -f "$CTL/out" ] && cat "$CTL/out"
[ -f "$CTL/err" ] && cat "$CTL/err" >&2
exit $(cat "$CTL/rc" 2>/dev/null || echo 0)
"#
        ));
        f.script("ssh-keygen", "for last; do :; done\nprintf 'PRIVATE-KEY-TEXT\\n' > \"$last\"\nprintf 'ssh-ed25519 AAAAPUB me@iyakki\\n' > \"$last.pub\"\nexit 0\n");
        f.ctl("probekey", &format!("ssh-ed25519 {ED}"));
        f.ctl("out", "remote output line\n");
        f
    }

    fn script(&self, name: &str, body: &str) {
        let p = self.dir.join("bin").join(name);
        let _ = std::fs::remove_file(&p);
        std::fs::write(&p, format!("#!/bin/sh\nPATH=/usr/bin:/bin; export PATH\n{body}")).unwrap();
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
    }

    fn ctl(&self, name: &str, v: &str) {
        std::fs::write(self.dir.join("ctl").join(name), v).unwrap();
    }

    fn log(&self) -> std::path::PathBuf {
        self.dir.join("log")
    }

    fn logged(&self) -> String {
        std::fs::read_to_string(self.log()).unwrap_or_default()
    }
}

impl Drop for Fake {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn setup(tag: &str) -> (AppCx, Fake) {
    let (cx, _d) = cx_with_backend(tag, iyakki_agent::chat::Unconfigured);
    let fake = Fake::new(tag);
    cx.services.clis.set_search_path(Some(&fake.dir.join("bin").display().to_string()));
    cx.services.on_login(&cx.session.user, "pass-word-12", true);
    (cx, fake)
}

fn draw(app: &mut CliApp, cx: &AppCx, w: u32, h: u32) -> Gfx {
    let mut g = Gfx::new(w, h);
    g.clear(rgb(9, 11, 20));
    app.render(&mut g, R::new(0.0, 46.0, w as f32, h as f32 - 46.0), 1.0, cx, 3);
    g
}

fn typ(app: &mut CliApp, cx: &AppCx, s: &str) {
    for c in s.chars() {
        app.handle(&Event::Key(Key::Char(c)), cx);
    }
}

fn key(app: &mut CliApp, cx: &AppCx, k: Key) {
    app.handle(&Event::Key(k), cx);
}

/// Ticks until the background call is over.
fn settle(app: &mut CliApp, cx: &AppCx) {
    for _ in 0..600 {
        app.handle(&Event::Tick, cx);
        if !app.conns.is_waiting() {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    panic!("the call did not finish");
}

fn add(app: &mut CliApp, cx: &AppCx, fields: &[&str]) {
    app.handle(&Event::Tab(2), cx);
    key(app, cx, Key::Char('n'));
    for (i, f) in fields.iter().enumerate() {
        if i > 0 {
            key(app, cx, Key::Tab);
        }
        typ(app, cx, f);
    }
    app.handle(&Event::Tab(conn::SAVE), cx);
}

impl CliApp {
    fn conns_status(&self) -> String {
        self.conns.status_text().unwrap_or("").to_string()
    }
}

fn lines(app: &CliApp) -> String {
    app.output_lines().join("\n")
}

fn audit(cx: &AppCx) -> String {
    std::fs::read_to_string(cx.audit.path()).unwrap_or_default()
}

#[test]
fn the_connections_tab_lists_adds_edits_and_removes_hosts_through_a_form_and_keeps_no_secret_in_the_file() {
    let (cx, _f) = setup("conn1");
    let mut app = CliApp::new(&cx);
    let g = draw(&mut app, &cx, 1100, 650);
    assert!(g.hits.iter().any(|h| h.ev == Event::Tab(2) && h.label == "Connections"), "the third tab is a button");
    app.handle(&Event::Tab(2), &cx);
    assert_eq!(app.tab(), Tab::Connections);
    let g = draw(&mut app, &cx, 1100, 650);
    assert!(g.hits.iter().any(|h| h.label == "New (n)") && !g.hits.iter().any(|h| h.label == "Connect (c)"), "an empty list offers New only");
    // a form with a bad port stays open and says so
    add(&mut app, &cx, &["web server", "Web.Example.com", "99999", "deploy", "ssh.deploy", "uptime -p"]);
    assert!(app.conns.in_form(), "the form stays open");
    // fix the port: three Up presses reach it from the command field... or click the field
    app.handle(&Event::FocusAt(2, 1000.0), &cx);
    for _ in 0..5 {
        key(&mut app, &cx, Key::Backspace);
    }
    typ(&mut app, &cx, "2222");
    app.handle(&Event::Tab(conn::SAVE), &cx);
    assert!(!app.conns.in_form());
    let file = std::fs::read_to_string(cx.services.dir().join("ssh/tester/connections.json")).unwrap();
    assert!(file.contains("web.example.com") && file.contains("2222") && file.contains("ssh.deploy") && !file.contains("PRIVATE"), "{file}");
    let g = draw(&mut app, &cx, 1100, 650);
    for want in ["web server", "Connect (c)", "Edit (e)", "Remove (x)", "Trust key (t)", "Forget key (f)", "Make key (g)"] {
        assert!(g.hits.iter().any(|h| h.label == want), "{want}");
    }
    // a second one, then edit the first
    add(&mut app, &cx, &["db", "10.0.0.7", "22"]);
    assert_eq!(iyakki_agent::ssh::Env::for_services(&cx.services, "tester").connections().list().len(), 2);
    key(&mut app, &cx, Key::Up);
    key(&mut app, &cx, Key::Up);
    key(&mut app, &cx, Key::Char('e'));
    assert!(app.conns.in_form());
    key(&mut app, &cx, Key::Tab);
    key(&mut app, &cx, Key::Tab);
    typ(&mut app, &cx, "0");
    app.handle(&Event::Tab(conn::SAVE), &cx);
    let list = iyakki_agent::ssh::Env::for_services(&cx.services, "tester").connections().list();
    assert_eq!(list.iter().map(|c| (c.name.as_str(), c.port)).collect::<Vec<_>>(), vec![("db", 220), ("web server", 2222)]);
    // remove asks first
    key(&mut app, &cx, Key::Char('x'));
    draw(&mut app, &cx, 1100, 650);
    key(&mut app, &cx, Key::Char('n'));
    assert_eq!(iyakki_agent::ssh::Env::for_services(&cx.services, "tester").connections().list().len(), 2, "declined");
    key(&mut app, &cx, Key::Char('x'));
    key(&mut app, &cx, Key::Char('y'));
    assert_eq!(iyakki_agent::ssh::Env::for_services(&cx.services, "tester").connections().list().len(), 1);
    let a = audit(&cx);
    assert!(a.contains("ssh.connection.save") && a.contains("ssh.connection.remove"), "{a}");
}

#[test]
fn connect_asks_about_an_unknown_host_key_first_then_runs_and_streams_the_output_into_the_terminal() {
    let (cx, f) = setup("conn2");
    let mut app = CliApp::new(&cx);
    add(&mut app, &cx, &["web", "10.0.0.5", "22", "deploy", "", "uptime"]);
    key(&mut app, &cx, Key::Enter);
    assert!(app.conns.is_waiting(), "the host key is being read");
    settle(&mut app, &cx);
    let ask = app.conns.trust_ask().expect("the question is asked before anything else happens");
    assert_eq!((ask.0.as_str(), ask.1, ask.3), ("10.0.0.5", 22, false), "{ask:?}");
    assert_eq!(ask.2, [format!("ssh-ed25519 {ED_FP}")]);
    let g = draw(&mut app, &cx, 1100, 650);
    assert!(g.hits.iter().any(|h| h.label == "Trust this key") && g.hits.iter().any(|h| h.label == "Cancel"));
    assert!(!f.logged().contains("uptime"), "nothing ran yet");
    // anything but y declines
    key(&mut app, &cx, Key::Char('n'));
    assert!(app.conns.trust_ask().is_none());
    let env = iyakki_agent::ssh::Env::for_services(&cx.services, "tester");
    assert!(env.known_hosts().read().is_empty(), "nothing was trusted");
    // again, and confirm: the key is recorded for the host and the command runs
    key(&mut app, &cx, Key::Enter);
    settle(&mut app, &cx);
    assert!(app.conns.trust_ask().is_some());
    key(&mut app, &cx, Key::Char('y'));
    settle(&mut app, &cx);
    assert_eq!(env.known_hosts().for_host("10.0.0.5", 22).len(), 1, "{:?}", env.known_hosts().read());
    let log = lines(&app);
    assert!(log.contains("remote output line"), "{log}");
    assert!(f.logged().contains("deploy@10.0.0.5 uptime") || f.logged().contains("deploy@10.0.0.5 true"), "{}", f.logged());
    let a = audit(&cx);
    assert!(a.contains("ssh.trust") && a.contains("ssh.connect") && !a.contains("uptime"), "{a}");
}

#[test]
fn a_changed_host_key_is_refused_with_a_clear_message_and_trust_shows_the_warning() {
    let (cx, f) = setup("conn3");
    let mut app = CliApp::new(&cx);
    add(&mut app, &cx, &["web", "10.0.0.5", "22"]);
    let env = iyakki_agent::ssh::Env::for_services(&cx.services, "tester");
    let old = iyakki_agent::ssh::HostKey { pattern: "10.0.0.5".into(), kind: "ssh-ed25519".into(), b64: "AAAAC3NzaC1lZDI1NTE5AAAAIAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA".into() };
    env.known_hosts().trust("10.0.0.5", 22, &[old]).unwrap();
    f.ctl("err", "@@@ WARNING: REMOTE HOST IDENTIFICATION HAS CHANGED! @@@\nHost key verification failed.\n");
    f.ctl("rc", "255");
    let _ = std::fs::remove_file(f.dir.join("ctl/out"));
    key(&mut app, &cx, Key::Enter);
    settle(&mut app, &cx);
    let log = lines(&app);
    assert!(log.contains("HOST KEY CHANGED") && log.contains("intercepting"), "{log}");
    // T reads the key again; the screen says it is a different one
    key(&mut app, &cx, Key::Char('t'));
    settle(&mut app, &cx);
    let ask = app.conns.trust_ask().expect("asked");
    assert!(ask.3, "marked as changed");
    let g = draw(&mut app, &cx, 1100, 650);
    assert!(g.hits.iter().any(|h| h.label == "Trust this key"));
    key(&mut app, &cx, Key::Esc);
    assert_eq!(env.known_hosts().for_host("10.0.0.5", 22)[0].b64, "AAAAC3NzaC1lZDI1NTE5AAAAIAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA", "the old key was not replaced");
    // F forgets, which is the person's explicit step
    key(&mut app, &cx, Key::Char('f'));
    assert!(env.known_hosts().for_host("10.0.0.5", 22).is_empty());
    assert!(audit(&cx).contains("ssh.forget"));
}

#[test]
fn ssh_typed_in_the_terminal_runs_in_the_background_refuses_unknown_hosts_and_sshtrust_needs_the_fingerprint() {
    let (cx, f) = setup("conn4");
    let mut app = CliApp::new(&cx);
    let run = |app: &mut CliApp, line: &str| {
        typ(app, &cx, line);
        key(app, &cx, Key::Enter);
        settle(app, &cx);
    };
    run(&mut app, "ssh -p 22 bob@10.0.0.5 uptime");
    assert!(lines(&app).contains("has not been trusted yet"), "{}", lines(&app));
    assert!(!f.logged().contains("uptime"));
    run(&mut app, "sshtrust 10.0.0.5");
    assert!(lines(&app).contains(&format!("sshtrust 10.0.0.5 {ED_FP}")), "{}", lines(&app));
    run(&mut app, "sshtrust 10.0.0.5 SHA256:nope");
    assert!(lines(&app).contains("nothing was trusted"));
    run(&mut app, &format!("sshtrust 10.0.0.5 {ED_FP}"));
    assert!(lines(&app).contains("trusted ssh-ed25519"), "{}", lines(&app));
    run(&mut app, "sshhosts");
    assert!(lines(&app).contains(ED_FP));
    run(&mut app, "ssh -p 22 bob@10.0.0.5 uptime");
    assert!(lines(&app).contains("remote output line"), "{}", lines(&app));
    // options that run programs are refused before anything starts
    let before = f.logged().lines().count();
    run(&mut app, "ssh -o ProxyCommand=id bob@10.0.0.5");
    run(&mut app, "ssh bob@10.0.0.5 -J jump");
    assert!(lines(&app).contains("can run a program on this machine") && f.logged().lines().count() == before);
    // arguments are not written to the audit log
    let a = audit(&cx);
    assert!(a.contains("cli.exec") && !a.contains("ProxyCommand") && !a.contains("uptime"), "{a}");
}

#[test]
fn an_account_that_may_not_reach_the_host_cannot_connect_or_probe_it() {
    let (mut cx, f) = setup("conn5");
    cx.session.policy.net_hosts = vec!["example.com".into()];
    let mut app = CliApp::new(&cx);
    add(&mut app, &cx, &["box", "10.0.0.5", "22"]);
    key(&mut app, &cx, Key::Enter);
    assert!(!app.conns.is_waiting());
    let g = draw(&mut app, &cx, 1100, 650);
    assert!(g.hits.iter().any(|h| h.label == "New (n)"));
    assert!(app.conns_status().contains("may not reach 10.0.0.5"), "{}", app.conns_status());
    app.handle(&Event::Tab(0), &cx);
    typ(&mut app, &cx, "ssh 10.0.0.5 id");
    key(&mut app, &cx, Key::Enter);
    settle(&mut app, &cx);
    assert!(lines(&app).contains("may not reach 10.0.0.5"), "{}", lines(&app));
    assert!(f.logged().is_empty(), "no client was ever started");
    // a restricted terminal does not have it at all
    let (mut cx2, _f2) = setup("conn5b");
    cx2.session.policy.cli = iyakki_sec::policy::CliLevel::Restricted;
    let mut app2 = CliApp::new(&cx2);
    typ(&mut app2, &cx2, "ssh 10.0.0.5 id");
    key(&mut app2, &cx2, Key::Enter);
    assert!(lines(&app2).contains("command not found"), "{}", lines(&app2));
    app2.handle(&Event::Tab(2), &cx2);
    key(&mut app2, &cx2, Key::Char('n'));
    typ(&mut app2, &cx2, "b");
    key(&mut app2, &cx2, Key::Tab);
    typ(&mut app2, &cx2, "10.0.0.5");
    app2.handle(&Event::Tab(conn::SAVE), &cx2);
    key(&mut app2, &cx2, Key::Enter);
    assert!(app2.conns_status().contains("full terminal"), "{}", app2.conns_status());
}

#[test]
fn a_key_is_made_into_the_vault_and_the_public_half_is_shown() {
    let (cx, _f) = setup("conn6");
    let mut app = CliApp::new(&cx);
    add(&mut app, &cx, &["web", "10.0.0.5", "22", "", "ssh.web"]);
    key(&mut app, &cx, Key::Char('g'));
    let g = draw(&mut app, &cx, 1100, 650);
    assert!(g.hits.iter().any(|h| h.label == "Make key"));
    key(&mut app, &cx, Key::Enter);
    let names = cx.services.user_names("tester").unwrap();
    assert!(names.contains(&"ssh.web".to_string()), "{names:?}");
    assert_eq!(cx.services.user_get("tester", "ssh.web").unwrap().as_str(), "PRIVATE-KEY-TEXT\n");
    let g = draw(&mut app, &cx, 1100, 650);
    assert!(g.hits.iter().any(|h| h.label == "web"));
    assert!(audit(&cx).contains("ssh.keygen"));
    // the same name again is refused, and nothing private is on disk in the data folder
    key(&mut app, &cx, Key::Char('g'));
    key(&mut app, &cx, Key::Enter);
    assert!(app.conns_status().contains("already exists"), "{}", app.conns_status());
    fn scan(d: &std::path::Path, hit: &mut Vec<String>) {
        for e in std::fs::read_dir(d).into_iter().flatten().flatten() {
            let p = e.path();
            if p.is_dir() {
                scan(&p, hit);
            } else if std::fs::read(&p).is_ok_and(|b| b.windows(11).any(|w| w == b"PRIVATE-KEY")) {
                hit.push(p.display().to_string());
            }
        }
    }
    let mut hit = vec![];
    scan(cx.services.dir(), &mut hit);
    assert!(hit.is_empty(), "the private key is only in the vault: {hit:?}");
}

#[test]
fn the_ssh_entry_of_the_clis_tab_shows_what_is_on_the_machine_and_how_to_get_it_and_cannot_be_downloaded() {
    let (cx, f) = setup("conn7");
    let mut app = CliApp::new(&cx);
    app.handle(&Event::Tab(1), &cx);
    let i = CATALOG.iter().position(|d| d.id == "ssh").expect("ssh is in the list");
    app.handle(&Event::Row(i), &cx);
    let g = draw(&mut app, &cx, 1200, 720);
    assert!(g.hits.iter().any(|h| h.label == "SSH client (OpenSSH)") && g.hits.iter().any(|h| h.label == "Saved hosts (c)"));
    assert!(!g.hits.iter().any(|h| h.label.starts_with("Install") || h.label.starts_with("Remove")), "nothing to install or remove");
    let st = cx.services.clis.system_status("ssh").unwrap();
    assert!(st.found && st.label == "OpenSSH 9.6p1" && st.commands.contains(&"ssh-keygen".to_string()), "{st:?}");
    key(&mut app, &cx, Key::Char('i'));
    assert!(app.status_text().unwrap().contains("comes with the system"), "{:?}", app.status_text());
    assert!(cx.services.clis.install("ssh", "tester").unwrap_err().contains("comes with the system"));
    assert!(cx.services.clis.remove("ssh").unwrap_err().contains("nothing here to remove") && cx.services.clis.check_latest("ssh").is_err());
    // the C key goes to the saved hosts
    key(&mut app, &cx, Key::Char('c'));
    assert_eq!(app.tab(), Tab::Connections);
    // with the client gone it says so and how to install
    std::fs::remove_file(f.dir.join("bin/ssh")).unwrap();
    cx.services.clis.recheck_system();
    let st = cx.services.clis.system_status("ssh").unwrap();
    assert!(!st.found && st.how_to.contains("openssh-client"), "{st:?}");
    app.handle(&Event::Tab(1), &cx);
    draw(&mut app, &cx, 1200, 720);
    app.handle(&Event::Tab(2), &cx);
    draw(&mut app, &cx, 1200, 720);
}

fn inside(g: &Gfx, w: u32, h: u32, what: &str) {
    for hit in &g.hits {
        let r = hit.rect;
        assert!(r.x >= -0.5 && r.y >= -0.5 && r.x + r.w <= w as f32 + 0.5 && r.y + r.h <= h as f32 + 0.5, "{what}: {} is outside {w}x{h}: {r:?}", hit.label);
    }
}

#[test]
fn geometry_and_clicks_hold_at_every_screen_size_for_the_list_the_form_and_the_dialogs() {
    for (w, h) in [(1280u32, 720u32), (1920, 1080), (1024, 600), (800, 480)] {
        let (cx, _f) = setup(&format!("conn8-{w}"));
        let mut app = CliApp::new(&cx);
        let what = format!("{w}x{h}");
        add(&mut app, &cx, &["long connection name for the list view", "a-very-long-host-name.subdomain.example.com", "2222", "deploy", "ssh.deploy", "uptime -p && df -h && free -m && who -a"]);
        add(&mut app, &cx, &["db", "10.0.0.7", "22"]);
        app.handle(&Event::Tab(2), &cx);
        let g = draw(&mut app, &cx, w, h);
        inside(&g, w, h, &format!("{what} list"));
        for want in ["Connections", "New (n)", "Connect (c)", "Remove (x)", "Make key (g)"] {
            assert!(g.hits.iter().any(|x| x.label == want), "{what}: {want}");
        }
        // the buttons are inside the panel, in at most two rows, and do not overlap each other
        let buttons: Vec<_> = g.hits.iter().filter(|x| matches!(x.ev, Event::Key(Key::Char(_))) && x.label.contains('(')).collect();
        assert_eq!(buttons.len(), 7, "{what}");
        for (i, a) in buttons.iter().enumerate() {
            for b in buttons.iter().skip(i + 1) {
                let apart = a.rect.x + a.rect.w <= b.rect.x + 0.5 || b.rect.x + b.rect.w <= a.rect.x + 0.5 || a.rect.y + a.rect.h <= b.rect.y + 0.5 || b.rect.y + b.rect.h <= a.rect.y + 0.5;
                assert!(apart, "{what}: {} overlaps {}", a.label, b.label);
            }
        }
        // a click on the second row selects it
        let row = g.hits.iter().find(|x| x.label == "db").expect("row");
        assert!(matches!(row.ev, Event::Row(_)));
        app.handle(&row.ev.clone(), &cx);
        // the form: six fields that do not overlap, with Save and Cancel below them
        key(&mut app, &cx, Key::Char('e'));
        let g = draw(&mut app, &cx, w, h);
        inside(&g, w, h, &format!("{what} form"));
        let fields: Vec<_> = g.hits.iter().filter(|x| x.label == "field").collect();
        assert_eq!(fields.len(), 6, "{what}");
        for (i, a) in fields.iter().enumerate() {
            assert!(a.rect.h >= 24.0 && a.rect.w >= 150.0, "{what}: field {i} is usable: {:?}", a.rect);
            for b in fields.iter().skip(i + 1) {
                assert!(a.rect.y + a.rect.h <= b.rect.y + 0.5 || b.rect.y + b.rect.h <= a.rect.y + 0.5, "{what}: fields overlap");
            }
        }
        let save = g.hits.iter().find(|x| x.label == "Save").expect("Save");
        let last = fields.iter().map(|f| f.rect.y + f.rect.h).fold(0.0, f32::max);
        assert!(save.rect.y >= last, "{what}: Save is below the fields");
        // clicking a field focuses it and the click path saves
        let f3 = fields[3].ev.clone();
        assert!(matches!(f3, Event::Focus(3)));
        app.handle(&Event::FocusAt(3, 4.0), &cx);
        app.handle(&save.ev.clone(), &cx);
        assert!(!app.conns.in_form(), "{what}: Save closes the form");
        // the dialogs
        key(&mut app, &cx, Key::Char('x'));
        inside(&draw(&mut app, &cx, w, h), w, h, &format!("{what} remove"));
        key(&mut app, &cx, Key::Esc);
        key(&mut app, &cx, Key::Char('g'));
        let g = draw(&mut app, &cx, w, h);
        inside(&g, w, h, &format!("{what} key"));
        assert!(g.hits.iter().any(|x| x.label == "Make key") && g.hits.iter().any(|x| x.label == "field"));
        key(&mut app, &cx, Key::Esc);
        key(&mut app, &cx, Key::Enter);
        settle(&mut app, &cx);
        let g = draw(&mut app, &cx, w, h);
        inside(&g, w, h, &format!("{what} trust"));
        let (yes, no) = (g.hits.iter().find(|x| x.label == "Trust this key").expect("yes"), g.hits.iter().find(|x| x.label == "Cancel").expect("no"));
        assert!(yes.rect.x + yes.rect.w <= no.rect.x + 0.5, "{what}: the buttons are side by side");
        app.handle(&no.ev.clone(), &cx);
        // the CLIs tab entry
        app.handle(&Event::Tab(1), &cx);
        let i = CATALOG.iter().position(|d| d.id == "ssh").unwrap();
        app.handle(&Event::Row(i), &cx);
        inside(&draw(&mut app, &cx, w, h), w, h, &format!("{what} clis"));
    }
}
