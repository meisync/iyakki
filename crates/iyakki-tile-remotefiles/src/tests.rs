use super::*;
use iyakki_tile_api::remote::RemoteSource;
use std::cell::{Cell, RefCell};
use std::collections::HashMap;

/// A scripted source: folders in memory, a server identity, answers at once (or held back).
struct Fake {
    conns: RefCell<Vec<Conn>>,
    passwords: RefCell<Vec<(String, Option<String>)>>,
    presents: RefCell<String>,
    pinned: RefCell<Option<String>>,
    dirs: RefCell<HashMap<String, Vec<Entry>>>,
    jobs: RefCell<Vec<Job>>,
    results: RefCell<HashMap<u64, JobResult>>,
    next: Cell<u64>,
    hold: Cell<bool>,
    started: RefCell<Vec<TransferReq>>,
    transfers: RefCell<Vec<TransferInfo>>,
    cancelled: RefCell<Vec<u64>>,
    retried: RefCell<Vec<u64>>,
    cleared: Cell<bool>,
    forgotten: RefCell<Vec<String>>,
    closed: RefCell<Vec<u64>>,
}

fn e(name: &str, dir: bool, size: u64, modified: i64) -> Entry {
    Entry { name: name.into(), is_dir: dir, is_link: false, size, modified, perms: String::new() }
}

impl Fake {
    fn new() -> Fake {
        let f = Fake {
            conns: RefCell::new(vec![]),
            passwords: RefCell::new(vec![]),
            presents: RefCell::new("SHA256:abc".into()),
            pinned: RefCell::new(None),
            dirs: RefCell::new(HashMap::new()),
            jobs: RefCell::new(vec![]),
            results: RefCell::new(HashMap::new()),
            next: Cell::new(1),
            hold: Cell::new(false),
            started: RefCell::new(vec![]),
            transfers: RefCell::new(vec![]),
            cancelled: RefCell::new(vec![]),
            retried: RefCell::new(vec![]),
            cleared: Cell::new(false),
            forgotten: RefCell::new(vec![]),
            closed: RefCell::new(vec![]),
        };
        f.dirs.borrow_mut().insert("/".into(), vec![e("zeta.txt", false, 10, 300), e("docs", true, 0, 100), e("alpha.bin", false, 5000, 200), e("Beta.txt", false, 70, 50)]);
        f.dirs.borrow_mut().insert("/docs".into(), vec![e("readme.md", false, 12, 1)]);
        f
    }
    fn card(&self) -> HostCard {
        HostCard { host: "nas.lan".into(), port: 22, kind: "ssh-ed25519".into(), fingerprint: self.presents.borrow().clone(), pinned: self.pinned.borrow().clone().filter(|p| *p != *self.presents.borrow()) }
    }
}

impl RemoteSource for Fake {
    fn connections(&self) -> Vec<Conn> {
        self.conns.borrow().clone()
    }
    fn save(&self, c: &Conn, pw: Option<&str>, replacing: Option<&str>) -> Result<(), String> {
        if c.name.is_empty() {
            return Err("the name is required".into());
        }
        let mut v = self.conns.borrow_mut();
        v.retain(|x| Some(x.name.as_str()) != replacing);
        v.push(Conn { has_password: pw.is_some(), ..c.clone() });
        self.passwords.borrow_mut().push((c.name.clone(), pw.map(str::to_string)));
        Ok(())
    }
    fn remove(&self, name: &str) -> Result<(), String> {
        self.conns.borrow_mut().retain(|c| c.name != name);
        Ok(())
    }
    fn key_names(&self) -> Vec<String> {
        vec!["ssh.laptop".into()]
    }
    fn submit(&self, job: Job) -> u64 {
        let id = self.next.get();
        self.next.set(id + 1);
        let r = match &job {
            Job::Open(_) => {
                let card = self.card();
                match self.pinned.borrow().as_ref() {
                    None => JobResult::Opened(Opened::Unknown(card)),
                    Some(p) if *p == card.fingerprint => JobResult::Opened(Opened::Ready(7)),
                    Some(_) => JobResult::Opened(Opened::Changed(card)),
                }
            }
            Job::List(_, p) => match self.dirs.borrow().get(p) {
                Some(v) => JobResult::Listing(p.clone(), v.clone()),
                None => JobResult::Failed(format!("{p}: no such folder")),
            },
            Job::Mkdir(_, p) => {
                let (d, n) = (parent(p), p.rsplit('/').next().unwrap().to_string());
                self.dirs.borrow_mut().entry(d).or_default().push(e(&n, true, 0, 0));
                JobResult::Done(format!("made {p}"))
            }
            Job::Rename(_, a, b) => {
                let d = parent(a);
                if let Some(v) = self.dirs.borrow_mut().get_mut(&d) {
                    for x in v.iter_mut().filter(|x| x.name == a.rsplit('/').next().unwrap()) {
                        x.name = b.rsplit('/').next().unwrap().into();
                    }
                }
                JobResult::Done("renamed".into())
            }
            Job::Delete(_, p, _) => {
                let d = parent(p);
                if let Some(v) = self.dirs.borrow_mut().get_mut(&d) {
                    v.retain(|x| x.name != p.rsplit('/').next().unwrap());
                }
                JobResult::Done("deleted".into())
            }
        };
        self.jobs.borrow_mut().push(job);
        self.results.borrow_mut().insert(id, r);
        id
    }
    fn take(&self, t: u64) -> Option<JobResult> {
        if self.hold.get() { None } else { self.results.borrow_mut().remove(&t) }
    }
    fn trust(&self, _: &str, fp: &str) -> Result<(), String> {
        if fp != *self.presents.borrow() {
            return Err("the server presents another identity now".into());
        }
        *self.pinned.borrow_mut() = Some(fp.into());
        Ok(())
    }
    fn forget_host(&self, name: &str) -> Result<(), String> {
        *self.pinned.borrow_mut() = None;
        self.forgotten.borrow_mut().push(name.into());
        Ok(())
    }
    fn close(&self, s: u64) {
        self.closed.borrow_mut().push(s);
    }
    fn start_transfer(&self, r: &TransferReq) -> Result<u64, String> {
        self.started.borrow_mut().push(r.clone());
        Ok(self.started.borrow().len() as u64)
    }
    fn transfers(&self) -> Vec<TransferInfo> {
        self.transfers.borrow().clone()
    }
    fn cancel_transfer(&self, id: u64) {
        self.cancelled.borrow_mut().push(id);
    }
    fn retry_transfer(&self, id: u64) -> Result<u64, String> {
        self.retried.borrow_mut().push(id);
        Ok(id + 100)
    }
    fn clear_finished(&self) {
        self.cleared.set(true);
    }
    fn download_dir(&self) -> String {
        "/home/amy/Downloads".into()
    }
}

struct NoUse;
impl iyakki_tile_api::usage::UsageSource for NoUse {
    fn now_unix(&self) -> i64 {
        0
    }
    fn series(&self, _: iyakki_tile_api::usage::Range, _: i64, _: Option<&str>) -> iyakki_tile_api::usage::Series {
        iyakki_tile_api::usage::Series { models: vec![], buckets: vec![], totals: vec![], input: 0, output: 0, calls: 0 }
    }
}

fn cx_of(f: &Fake) -> TileCx<'_> {
    TileCx { user: "amy", manage_settings: false, usage: &NoUse, calendar: &iyakki_tile_api::calendar::NoCalendar, audit: &iyakki_tile_api::NoAudit, remote: f, cameras: &iyakki_tile_api::cameras::NoCameras }
}

fn key(a: &mut RemoteFilesApp, cx: &TileCx, c: char) {
    a.handle(&Event::Key(Key::Char(c)), cx);
}

fn type_in(a: &mut RemoteFilesApp, cx: &TileCx, text: &str) {
    for c in text.chars() {
        a.handle(&Event::Key(Key::Char(c)), cx);
    }
}

fn draw(a: &mut RemoteFilesApp, cx: &TileCx) -> Gfx {
    let mut g = Gfx::new(1200, 700);
    g.clear(rgb(9, 11, 20));
    g.clear_hits();
    a.render(&mut g, R::new(0.0, 46.0, 1200.0, 654.0), 1.0, cx, 0);
    g
}

fn saved(f: &Fake) {
    f.conns.borrow_mut().push(Conn { name: "nas".into(), host: "nas.lan".into(), port: 22, user: "amy".into(), proto: Proto::Sftp, auth: Auth::Password, has_password: true, ..Conn::default() });
}

fn browsing(f: &Fake) -> RemoteFilesApp {
    saved(f);
    *f.pinned.borrow_mut() = Some("SHA256:abc".into());
    let cx = cx_of(f);
    let mut a = RemoteFilesApp::new();
    a.handle(&Event::Key(Key::Enter), &cx);
    assert!(a.in_browser() && a.connected());
    a
}

#[test]
fn a_connection_is_added_through_the_form_and_the_password_goes_only_to_the_source() {
    let f = Fake::new();
    let cx = cx_of(&f);
    let mut a = RemoteFilesApp::new();
    draw(&mut a, &cx);
    key(&mut a, &cx, 'n');
    assert!(matches!(a.screen, Screen::ConnForm(..)));
    draw(&mut a, &cx);
    if let Screen::ConnForm(form, _) = &mut a.screen {
        form.set_value(0, "office");
        form.set_value(2, "files.example.org");
        form.set_value(4, "amy");
        form.set_value(6, "s3cret-pw");
    }
    a.handle(&Event::Key(Key::Enter), &cx);
    // Enter in the last field submits; the form is submitted through Form::handle on the last field, so drive it directly if still open
    if matches!(a.screen, Screen::ConnForm(..)) {
        if let Screen::ConnForm(form, _) = &mut a.screen {
            form.focus = form.fields.len() - 1;
        }
        a.handle(&Event::Key(Key::Enter), &cx);
    }
    assert!(matches!(a.screen, Screen::List), "saved");
    let c = f.conns.borrow()[0].clone();
    assert_eq!((c.name.as_str(), c.host.as_str(), c.port, c.proto, c.auth), ("office", "files.example.org", 22, Proto::Sftp, Auth::Password));
    assert_eq!(f.passwords.borrow()[0], ("office".to_string(), Some("s3cret-pw".to_string())));
    // the connection record itself holds no secret anywhere
    assert!(!format!("{c:?}").contains("s3cret"));
    let g = draw(&mut a, &cx);
    assert!(g.hits.iter().any(|h| h.label == "office"));
}

#[test]
fn plain_ftp_needs_the_warning_toggle_and_keys_are_for_sftp_only() {
    let f = Fake::new();
    let cx = cx_of(&f);
    let a = RemoteFilesApp::new();
    let mut form = a.conn_form(&cx, None);
    form.set_value(0, "old");
    form.set_value(2, "ftp.example.org");
    form.set_choice(1, 2);
    let keys = cx.remote.key_names();
    let err = RemoteFilesApp::conn_from_form(&form, &keys).unwrap_err();
    assert!(err.contains("unencrypted"), "{err}");
    form.set_choice(8, 1);
    let (c, _) = RemoteFilesApp::conn_from_form(&form, &keys).unwrap();
    assert_eq!((c.proto, c.plain_ok, c.port), (Proto::Ftp, true, 21));
    // FTPS and SFTP never carry the plain flag
    form.set_choice(1, 1);
    let (c, _) = RemoteFilesApp::conn_from_form(&form, &keys).unwrap();
    assert_eq!((c.proto, c.plain_ok, c.port), (Proto::Ftps, false, 21));
    // a key with FTPS is refused; with SFTP it needs a key chosen
    form.set_choice(5, 1);
    assert!(RemoteFilesApp::conn_from_form(&form, &keys).is_err());
    form.set_choice(1, 0);
    assert!(RemoteFilesApp::conn_from_form(&form, &keys).unwrap_err().contains("choose a key"));
    form.set_choice(7, 1);
    let (c, _) = RemoteFilesApp::conn_from_form(&form, &keys).unwrap();
    assert_eq!((c.auth, c.key.as_str()), (Auth::Key, "ssh.laptop"));
    form.set_value(3, "70000");
    assert!(RemoteFilesApp::conn_from_form(&form, &keys).is_err(), "port out of range");
}

#[test]
fn first_contact_shows_the_fingerprint_and_connects_only_after_the_person_trusts_it() {
    let f = Fake::new();
    saved(&f);
    let cx = cx_of(&f);
    let mut a = RemoteFilesApp::new();
    a.handle(&Event::Key(Key::Enter), &cx);
    assert_eq!(a.on_card().map(|c| c.fingerprint.as_str()), Some("SHA256:abc"));
    assert!(!a.connected());
    let g = draw(&mut a, &cx);
    assert!(g.hits.iter().any(|h| h.label.contains("Trust")), "a trust button");
    // other keys do nothing; Esc cancels and nothing was pinned
    key(&mut a, &cx, 'x');
    assert!(a.on_card().is_some());
    a.handle(&Event::Key(Key::Esc), &cx);
    assert!(matches!(a.screen, Screen::List) && f.pinned.borrow().is_none());
    a.handle(&Event::Key(Key::Enter), &cx);
    key(&mut a, &cx, 'y');
    assert_eq!(f.pinned.borrow().as_deref(), Some("SHA256:abc"));
    assert!(a.in_browser() && a.connected());
    assert_eq!(a.path(), "/");
    assert_eq!(a.names(), vec!["docs", "alpha.bin", "Beta.txt", "zeta.txt"]);
}

#[test]
fn a_changed_identity_is_refused_loudly_with_no_way_to_accept_it_in_one_step() {
    let f = Fake::new();
    saved(&f);
    *f.pinned.borrow_mut() = Some("SHA256:old".into());
    let cx = cx_of(&f);
    let mut a = RemoteFilesApp::new();
    a.handle(&Event::Key(Key::Enter), &cx);
    let card = a.on_card().cloned().expect("a card");
    assert_eq!(card.pinned.as_deref(), Some("SHA256:old"));
    assert!(!a.connected());
    let g = draw(&mut a, &cx);
    assert!(g.hits.iter().all(|h| !h.label.contains("Trust")), "no trust button on a changed identity");
    key(&mut a, &cx, 'y');
    assert!(!a.connected() && a.on_card().is_some(), "y does nothing here");
    assert_eq!(f.pinned.borrow().as_deref(), Some("SHA256:old"), "the pin is untouched");
    // forgetting is a separate, confirmed step
    key(&mut a, &cx, 'f');
    assert!(matches!(a.screen, Screen::Confirm(Ask::ForgetPin(_))));
    key(&mut a, &cx, 'n');
    assert!(f.forgotten.borrow().is_empty());
    a.handle(&Event::Key(Key::Enter), &cx);
    key(&mut a, &cx, 'f');
    key(&mut a, &cx, 'y');
    assert_eq!(*f.forgotten.borrow(), vec!["nas".to_string()]);
    // now it is a first contact again, which still asks
    a.handle(&Event::Key(Key::Enter), &cx);
    assert!(a.on_card().is_some_and(|c| c.pinned.is_none()));
}

#[test]
fn the_listing_sorts_by_name_size_and_date_with_folders_first_and_flips_on_a_second_press() {
    let f = Fake::new();
    let cx = cx_of(&f);
    let mut a = browsing(&f);
    assert_eq!(a.names(), vec!["docs", "alpha.bin", "Beta.txt", "zeta.txt"], "by name, case-insensitive, folder first");
    key(&mut a, &cx, '2');
    assert_eq!(a.names(), vec!["docs", "zeta.txt", "Beta.txt", "alpha.bin"], "smallest first");
    key(&mut a, &cx, '2');
    assert_eq!(a.names(), vec!["docs", "alpha.bin", "Beta.txt", "zeta.txt"], "biggest first");
    assert_eq!(a.sort_state(), (SortBy::Size, false));
    a.handle(&Event::Tab(43), &cx);
    assert_eq!(a.names(), vec!["docs", "Beta.txt", "alpha.bin", "zeta.txt"], "oldest first");
    a.handle(&Event::Tab(41), &cx);
    assert_eq!(a.sort_state(), (SortBy::Name, true));
    draw(&mut a, &cx);
}

#[test]
fn folders_open_and_the_way_back_up_never_goes_above_the_root() {
    let f = Fake::new();
    let cx = cx_of(&f);
    let mut a = browsing(&f);
    a.handle(&Event::Key(Key::Enter), &cx);
    assert_eq!((a.path(), a.names()), ("/docs", vec!["readme.md".to_string()]));
    a.handle(&Event::Key(Key::Backspace), &cx);
    assert_eq!(a.path(), "/");
    let before = f.jobs.borrow().len();
    a.handle(&Event::Key(Key::Left), &cx);
    assert_eq!(f.jobs.borrow().len(), before, "already at the root: nothing asked");
    // a failing listing is told, and the old one stays
    f.dirs.borrow_mut().remove("/docs");
    a.handle(&Event::Key(Key::Enter), &cx);
    assert_eq!(a.path(), "/");
    assert!(a.note.as_ref().is_some_and(|n| !n.1 && n.0.contains("no such folder")));
    draw(&mut a, &cx);
}

#[test]
fn a_hostile_name_from_the_server_cannot_become_a_local_path() {
    let f = Fake::new();
    f.dirs.borrow_mut().insert("/".into(), vec![e("../../.ssh/authorized_keys", false, 3, 0), e("ok.txt", false, 3, 0), e("evil\u{1b}[31m", false, 1, 0)]);
    let cx = cx_of(&f);
    let mut a = browsing(&f);
    assert_eq!(a.names().len(), 3);
    let g = draw(&mut a, &cx);
    assert!(!g.hits.is_empty());
    for i in 0..3 {
        a.sel = i;
        a.handle(&Event::Key(Key::Enter), &cx);
    }
    let started = f.started.borrow().clone();
    assert_eq!(started.len(), 3);
    for r in &started {
        assert!(r.local.starts_with("/home/amy/Downloads/"), "{}", r.local);
        assert!(!r.local["/home/amy/Downloads/".len()..].contains('/'), "no folder tricks: {}", r.local);
        assert!(!r.local.contains('\u{1b}'));
    }
    assert!(started.iter().any(|r| r.local.ends_with("ok.txt")));
    assert!(started.iter().all(|r| !r.overwrite && r.download && r.session == 7));
}

#[test]
fn an_upload_into_an_existing_name_asks_before_replacing() {
    let f = Fake::new();
    let cx = cx_of(&f);
    let mut a = browsing(&f);
    a.start_upload(&cx, "/home/amy/new.txt");
    assert_eq!(f.started.borrow().len(), 1);
    assert_eq!((f.started.borrow()[0].remote.as_str(), f.started.borrow()[0].overwrite, f.started.borrow()[0].download), ("/new.txt", false, false));
    a.start_upload(&cx, "/home/amy/zeta.txt");
    assert!(matches!(a.screen, Screen::Confirm(Ask::Overwrite(_))));
    assert_eq!(f.started.borrow().len(), 1, "nothing queued before the answer");
    draw(&mut a, &cx);
    key(&mut a, &cx, 'n');
    assert_eq!(f.started.borrow().len(), 1);
    a.start_upload(&cx, "/home/amy/zeta.txt");
    key(&mut a, &cx, 'y');
    assert_eq!(f.started.borrow().len(), 2);
    assert!(f.started.borrow()[1].overwrite);
}

#[test]
fn delete_asks_first_and_rename_and_new_folder_take_a_plain_name_only() {
    let f = Fake::new();
    let cx = cx_of(&f);
    let mut a = browsing(&f);
    a.sel = a.names().iter().position(|n| n == "zeta.txt").unwrap();
    key(&mut a, &cx, 'x');
    assert!(matches!(a.screen, Screen::Confirm(Ask::Delete(_))));
    draw(&mut a, &cx);
    a.handle(&Event::Key(Key::Esc), &cx);
    assert!(!f.jobs.borrow().iter().any(|j| matches!(j, Job::Delete(..))), "cancelled: nothing deleted");
    key(&mut a, &cx, 'x');
    key(&mut a, &cx, 'y');
    assert!(f.jobs.borrow().iter().any(|j| matches!(j, Job::Delete(7, p, false) if p == "/zeta.txt")));
    assert!(!a.names().contains(&"zeta.txt".to_string()), "the listing was reloaded");
    // new folder
    key(&mut a, &cx, 'n');
    type_in(&mut a, &cx, "../up");
    a.handle(&Event::Key(Key::Enter), &cx);
    assert!(matches!(a.screen, Screen::Name(..)), "a path is refused");
    if let Screen::Name(form, _) = &mut a.screen {
        form.set_value(0, "photos");
    }
    a.handle(&Event::Key(Key::Enter), &cx);
    assert!(f.jobs.borrow().iter().any(|j| matches!(j, Job::Mkdir(7, p) if p == "/photos")));
    assert!(a.names().contains(&"photos".to_string()));
    // rename
    a.sel = a.names().iter().position(|n| n == "alpha.bin").unwrap();
    key(&mut a, &cx, 'm');
    if let Screen::Name(form, _) = &mut a.screen {
        form.set_value(0, "omega.bin");
    }
    a.handle(&Event::Key(Key::Enter), &cx);
    assert!(f.jobs.borrow().iter().any(|j| matches!(j, Job::Rename(7, a, b) if a == "/alpha.bin" && b == "/omega.bin")));
    assert!(a.names().contains(&"omega.bin".to_string()));
}

#[test]
fn the_queue_shows_progress_and_cancels_resumes_and_clears() {
    let f = Fake::new();
    let cx = cx_of(&f);
    let mut a = browsing(&f);
    let t = |id, state, done, total, note: &str| TransferInfo { id, download: true, remote: format!("/f{id}.bin"), local: format!("/x/f{id}.bin"), done, total, state, note: note.into(), rate: 1_500_000, resumed: false };
    *f.transfers.borrow_mut() = vec![t(1, State::Running, 5_000_000, 10_000_000, ""), t(2, State::Failed, 100, 1000, "connection lost"), t(3, State::Done, 9, 9, "")];
    key(&mut a, &cx, 't');
    assert!(matches!(a.screen, Screen::Transfers));
    let g = draw(&mut a, &cx);
    assert_eq!(g.hits.iter().filter(|h| matches!(h.ev, Event::Row(_))).count(), 3);
    a.handle(&Event::Key(Key::Char('c')), &cx);
    assert_eq!(*f.cancelled.borrow(), vec![1]);
    a.handle(&Event::Key(Key::Down), &cx);
    key(&mut a, &cx, 'r');
    assert_eq!(*f.retried.borrow(), vec![2]);
    key(&mut a, &cx, 'o');
    assert!(matches!(a.screen, Screen::Confirm(Ask::Overwrite(ref r)) if r.overwrite && r.remote == "/f2.bin"));
    a.handle(&Event::Key(Key::Esc), &cx);
    assert!(matches!(a.screen, Screen::Browse));
    key(&mut a, &cx, 't');
    key(&mut a, &cx, 'x');
    assert!(f.cleared.get());
    a.handle(&Event::Key(Key::Esc), &cx);
    assert!(matches!(a.screen, Screen::Browse));
}

#[test]
fn leaving_closes_the_session_and_waiting_can_be_stopped() {
    let f = Fake::new();
    let cx = cx_of(&f);
    let mut a = browsing(&f);
    f.hold.set(true);
    key(&mut a, &cx, 'u');
    a.handle(&Event::Key(Key::Esc), &cx);
    a.handle(&Event::Key(Key::Enter), &cx);
    assert!(a.pending.is_some(), "listing held back");
    draw(&mut a, &cx);
    a.handle(&Event::Key(Key::Char('x')), &cx);
    assert!(a.pending.is_some(), "keys wait while the server is working");
    a.handle(&Event::Key(Key::Esc), &cx);
    assert!(a.pending.is_none());
    f.hold.set(false);
    a.handle(&Event::Key(Key::Esc), &cx);
    assert!(matches!(a.screen, Screen::List) && !a.connected());
    assert_eq!(*f.closed.borrow(), vec![7]);
    assert!(matches!(a.handle(&Event::Key(Key::Esc), &cx), Flow::Back));
    assert!(!a.help().is_empty());
}

#[test]
fn removing_a_connection_asks_first() {
    let f = Fake::new();
    saved(&f);
    let cx = cx_of(&f);
    let mut a = RemoteFilesApp::new();
    key(&mut a, &cx, 'd');
    assert!(matches!(a.screen, Screen::Confirm(Ask::RemoveConn(_))));
    draw(&mut a, &cx);
    a.handle(&Event::Key(Key::Esc), &cx);
    assert_eq!(f.conns.borrow().len(), 1);
    key(&mut a, &cx, 'd');
    key(&mut a, &cx, 'y');
    assert!(f.conns.borrow().is_empty());
    let g = draw(&mut a, &cx);
    assert!(g.hits.iter().any(|h| matches!(h.ev, Event::Tab(30))));
}

/// `IYAKKI_SHOT=/path.png cargo test -p iyakki-tile-remotefiles remotefiles_picture -- --ignored`
#[test]
#[ignore]
fn remotefiles_picture() {
    let Ok(out) = std::env::var("IYAKKI_SHOT") else { return };
    let f = Fake::new();
    let cx = cx_of(&f);
    let mut a = browsing(&f);
    let g = draw(&mut a, &cx);
    g.save_png(std::path::Path::new(&out)).unwrap();
}
