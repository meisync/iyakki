//! The GIT tile against a scripted host (`FakeGitHttp`, with canned JSON for Gitea, GitHub and GitLab) and a scripted git
//! (`FakeRunner`). No host is asked, no real repository is read, and no job runs unless a test says so: a started clone, fetch, pull or
//! push is looked at as the plan the jobs would carry out. The audit log is a temp file.

use super::*;
use crate::apps::git_tile_repo::{Shown, Tab};
use crate::apps::git_tile_ui::{FIRST_RUN_HINT, FIRST_RUN_TITLE};
use crate::apps::tests::cx_with_admin_store;
use crate::gfx::{rgb, Gfx};
use crate::ui::{Event, Key, R};
use iyakki_agent::gitbrowse::EntryKind;
use iyakki_agent::gitconn::{FakeGitHttp, Kind, NetErr, Req, Resp};
use iyakki_agent::k8sretry::FakeTimer;
use iyakki_agent::wsexec::FakeRunner;
use iyakki_sec::policy::{Policy, Role};
use serde_json::{json, Value};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

pub(super) const TOK: &str = "tok_FAKEFAKEFAKEFAKE1234";

/// The host the tile talks to: a fake that a test can swap for another (a host that goes down and comes back), remembering every request.
struct Net {
    cur: Mutex<Arc<FakeGitHttp>>,
    all: Mutex<Vec<Req>>,
}

impl Net {
    fn set(&self, h: Arc<FakeGitHttp>) {
        *self.cur.lock().unwrap() = h;
    }
    fn requests(&self) -> Vec<Req> {
        self.all.lock().unwrap().clone()
    }
    fn lines(&self) -> Vec<String> {
        self.requests().iter().map(|r| format!("{} {}", r.method, r.url)).collect()
    }
    fn count(&self, needle: &str) -> usize {
        self.lines().iter().filter(|l| l.contains(needle)).count()
    }
}

impl GitHttp for Net {
    fn send(&self, r: &Req) -> Result<Resp, NetErr> {
        self.all.lock().unwrap().push(r.clone());
        let h = self.cur.lock().unwrap().clone();
        h.send(r)
    }
    fn peer_cert(&self, url: &str) -> Result<gitconn::CertInfo, String> {
        self.cur.lock().unwrap().peer_cert(url)
    }
}

struct Fx {
    app: GitApp,
    cx: AppCx,
    net: Arc<Net>,
    run: Arc<FakeRunner>,
    timer: Arc<FakeTimer>,
    root: PathBuf,
    g: Gfx,
    area: R,
    s: f32,
    tick: u64,
}

fn fixture_with(tag: &str, role: Role, w: u32, h: u32, s: f32) -> Fx {
    static N: AtomicUsize = AtomicUsize::new(0);
    let mut cx = cx_with_admin_store(&format!("{tag}-{}", N.fetch_add(1, Ordering::SeqCst)));
    cx.session.role = role;
    cx.session.policy = Policy::resolve(role, &Default::default());
    // the person's vault is open, as after signing in: the tokens of the connections live there
    cx.services.on_login(&cx.session.user, "rootpass12", role == Role::Admin);
    let root = cx.services.dir().join("Repositories");
    std::fs::create_dir_all(&root).unwrap();
    let net = Arc::new(Net { cur: Mutex::new(Arc::new(FakeGitHttp::default())), all: Mutex::new(vec![]) });
    let run = Arc::new(FakeRunner::new(&["git"]));
    let timer = FakeTimer::new();
    let rig = Rig {
        http: net.clone(),
        run: run.clone(),
        timer: timer.clone(),
        scratch: cx.services.dir().join("scratch"),
        repos_root: Some(root.clone()),
        tools: Some(Tools { git: PathBuf::from("/usr/bin/git"), ssh: Err("no ssh in tests".into()), allow_local: false }),
        probe: Some(Arc::new(|_, _| Err("no network in tests".into()))),
        run_jobs: false,
    };
    let app = GitApp::with_rig(&cx, rig);
    let mut g = Gfx::new(w, h);
    g.clear(rgb(8, 10, 22));
    Fx { app, cx, net, run, timer, root, g, area: R::new(0.0, 0.0, w as f32, h as f32), s, tick: 0 }
}

fn fixture() -> Fx {
    fixture_with("git-tile", Role::Admin, 1280, 720, 1.0)
}

fn ts(wall: u64, ago: u64) -> String {
    let t = (wall - ago) as i64;
    let (days, rem) = (t.div_euclid(86_400), t.rem_euclid(86_400));
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!("{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z", rem / 3600, rem % 3600 / 60, rem % 60)
}

impl Fx {
    fn draw(&mut self) {
        let (w, h) = (self.area.w as u32 + self.area.x as u32, self.area.h as u32 + self.area.y as u32);
        if (self.g.width(), self.g.height()) != (w, h) {
            self.g = Gfx::new(w, h);
        }
        self.g.clear_hits();
        self.g.clear_probes();
        self.g.clear(rgb(8, 10, 22));
        self.tick += 1;
        self.app.render(&mut self.g, self.area, self.s, &self.cx, self.tick);
    }
    fn send(&mut self, ev: Event) -> Flow {
        let f = self.app.handle(&ev, &self.cx);
        self.draw();
        f
    }
    fn key(&mut self, c: char) -> Flow {
        self.send(Event::Key(Key::Char(c)))
    }
    fn press(&mut self, k: Key) -> Flow {
        self.send(Event::Key(k))
    }
    fn click(&mut self, label: &str) {
        let ev = self.g.hits.iter().rev().find(|h| h.label == label).unwrap_or_else(|| panic!("nothing clickable is labelled `{label}`: {:?}", self.labels())).ev.clone();
        self.send(ev);
    }
    fn labels(&self) -> Vec<String> {
        self.g.hits.iter().map(|h| h.label.clone()).collect()
    }
    fn has_hit(&self, label: &str) -> bool {
        self.g.hits.iter().any(|h| h.label == label)
    }
    fn code(&mut self, n: usize) {
        self.send(Event::Tab(n));
    }
    fn type_text(&mut self, t: &str) {
        for c in t.chars() {
            self.key(c);
        }
    }
    /// Draw and tick until `ok` holds (threads are answering).
    fn wait(&mut self, what: &str, ok: impl Fn(&mut Fx) -> bool) {
        let t0 = Instant::now();
        loop {
            self.draw();
            self.app.handle(&Event::Tick, &self.cx);
            self.draw();
            if ok(self) {
                return;
            }
            assert!(t0.elapsed() < Duration::from_secs(8), "timed out waiting for: {what}; status {:?}; requests {:?}", self.app.status, self.net.lines());
            std::thread::sleep(Duration::from_millis(5));
        }
    }
    /// A few heartbeats, for things that must NOT happen.
    fn settle(&mut self) {
        for _ in 0..6 {
            self.draw();
            self.app.handle(&Event::Tick, &self.cx);
            std::thread::sleep(Duration::from_millis(4));
        }
        self.draw();
    }
    fn conn(&mut self, kind: Kind, name: &str, url: &str) -> Connection {
        let c = self.app.conns(&self.cx).add(Connection { name: name.into(), kind, url: url.into(), account: "octo".into(), ..Default::default() }, Some(TOK)).unwrap();
        self.app.refresh_entries(&self.cx);
        c
    }
    fn wall(&self) -> u64 {
        self.app.wall()
    }
    fn audit_text(&self) -> String {
        std::fs::read_to_string(self.cx.audit.path()).unwrap_or_default().replace("\\\"", "\"")
    }
    fn clipboard(&self) -> String {
        crate::ui::clipboard::latest().unwrap_or_default()
    }
    fn names(&mut self) -> Vec<String> {
        let n = self.app.rows_now().len();
        let order: Vec<usize> = self.app.order.clone();
        (0..n).map(|i| self.app.rows[order[i]].name.clone()).collect()
    }
    fn select_conn(&mut self, id: &str) {
        let i = self.app.srcs.iter().position(|s| s.key() == id).unwrap();
        self.app.select_source(i);
        self.draw();
    }
    fn open_row(&mut self, name: &str) {
        let names = self.names();
        let i = names.iter().position(|n| n == name).unwrap_or_else(|| panic!("no row {name}: {names:?}"));
        self.code(T_ROW + i);
    }
    fn view(&self) -> &crate::apps::git_tile_repo::RepoView {
        self.app.view.as_ref().expect("a repository is open")
    }
    /// A clone folder on this computer, with the answers git gives about it.
    fn mk_clone(&mut self, name: &str, remote: &str, branch: &str) -> PathBuf {
        let d = self.root.join(name);
        std::fs::create_dir_all(d.join(".git")).unwrap();
        self.run.answer(&format!("-C {} remote get-url origin", d.display()), &format!("{remote}\n"));
        self.run.answer(&format!("-C {} symbolic-ref --short -q HEAD", d.display()), &format!("{branch}\n"));
        d
    }
}

// ------------------------------------------------------------------------------------------------ canned answers

fn gitea_repo(wall: u64, name: &str, desc: &str, ago: u64, private: bool, stars: u64) -> Value {
    json!({"full_name": name, "description": desc, "default_branch": "main", "private": private, "updated_at": ts(wall, ago), "stars_count": stars, "size": 2048,
        "clone_url": format!("https://git.example.org/{name}.git"), "ssh_url": format!("git@git.example.org:{name}.git"), "html_url": format!("https://git.example.org/{name}")})
}

fn repo_list(wall: u64) -> String {
    Value::Array(vec![gitea_repo(wall, "ops/infra", "Cluster files and runbooks", 3600, true, 3), gitea_repo(wall, "ops/web", "The public site", 86_400 * 3, false, 12), gitea_repo(wall, "dev/tools", "", 600, false, 0)]).to_string()
}

fn entry(name: &str, kind: &str, size: u64, path: &str) -> Value {
    json!({"name": name, "path": path, "type": kind, "size": size})
}

fn file_json(bytes: &[u8]) -> String {
    json!({"type": "file", "encoding": "base64", "size": bytes.len(), "content": iyakki_agent::story::base64_encode(bytes)}).to_string()
}

const BASE: &str = "GET https://git.example.org/api/v1";

/// A Gitea host with the list above and one repository, ops/infra, with files, commits, branches, tags, issues and pull requests.
fn gitea_host(wall: u64) -> Arc<FakeGitHttp> {
    let h = Arc::new(FakeGitHttp::default());
    h.answer(&format!("{BASE}/user/repos?limit=50&page=1"), 200, &[], &repo_list(wall));
    h.answer(&format!("{BASE}/repos/ops/infra/contents?ref=main"), 200, &[], &Value::Array(vec![entry("README.md", "file", 40, "README.md"), entry("docs", "dir", 0, "docs"), entry("src", "dir", 0, "src"), entry("logo.png", "file", 70, "logo.png"), entry("big.bin", "file", 900_000, "big.bin"), entry("pack.zip", "file", 12, "pack.zip"), entry("main.rs", "file", 30, "main.rs")]).to_string());
    h.answer(&format!("{BASE}/repos/ops/infra/contents/docs?ref=main"), 200, &[], &Value::Array(vec![entry("guide.txt", "file", 20, "docs/guide.txt")]).to_string());
    h.answer(&format!("{BASE}/repos/ops/infra/contents/src?ref=main"), 200, &[], &Value::Array(vec![entry("lib.rs", "file", 10, "src/lib.rs")]).to_string());
    h.answer(&format!("{BASE}/repos/ops/infra/contents/README.md?ref=main"), 200, &[], &file_json(b"# Infra\n\nCluster **files**.\n\n- one\n- two\n"));
    h.answer(&format!("{BASE}/repos/ops/infra/contents/main.rs?ref=main"), 200, &[], &file_json(b"fn main() {\n\tprintln!(\"hi\");\n}\n"));
    h.answer(&format!("{BASE}/repos/ops/infra/contents/docs/guide.txt?ref=main"), 200, &[], &file_json(b"guide line\n"));
    h.answer(&format!("{BASE}/repos/ops/infra/contents/pack.zip?ref=main"), 200, &[], &file_json(b"PK\x03\x04\x14\x00\x00\x00\x08\x00"));
    h.answer(&format!("{BASE}/repos/ops/infra/contents/logo.png?ref=main"), 200, &[], &file_json(&tiny_png()));
    h
}

fn tiny_png() -> Vec<u8> {
    let mut pm = tiny_skia::Pixmap::new(4, 3).unwrap();
    pm.fill(tiny_skia::Color::from_rgba8(200, 40, 40, 255));
    pm.encode_png().unwrap()
}

// ------------------------------------------------------------------------------------------------ connections and the first run

#[test]
fn where_the_data_lives_is_told_in_the_tiles_own_words() {
    let c = |u: &str, cluster: &str| Connection { name: "x".into(), url: u.into(), cluster: cluster.into(), ..Default::default() };
    assert_eq!(place_text(&c("https://github.com", "")), "on the internet");
    assert_eq!(place_text(&c("http://10.0.0.5:3000", "")), "on the network");
    assert_eq!(place_text(&c("https://git.home.arpa", "")), "on the network");
    assert_eq!(place_text(&c("http://10.0.0.5:30300", "gitea-iyakki-gitea")), "on the cluster");
    assert_eq!(place_text(&c("http://localhost:3000", "")), "this computer");
    assert_eq!(place_text(&c("http://127.0.0.1:3000", "")), "this computer");
}

#[test]
fn the_selector_lists_every_connection_with_its_kind_address_and_place_plus_local_clones_and_add() {
    let mut f = fixture();
    f.conn(Kind::GitHub, "Work GitHub", "https://github.com");
    f.conn(Kind::Gitea, "Home Gitea", "http://192.168.1.20:3000");
    f.conn(Kind::GitLab, "Lab GitLab", "https://gitlab.example.org");
    f.draw();
    assert_eq!(f.app.srcs.iter().map(|s| s.name()).collect::<Vec<_>>(), vec!["Work GitHub", "Home Gitea", "Lab GitLab", "Local clones"]);
    assert!(f.has_hit("connection") && f.has_hit("Refresh") && f.has_hit("Show log") && f.has_hit("Add connection..."));
    f.click("connection");
    assert_eq!(f.app.overlay, Some(Overlay::Sources));
    for l in ["Work GitHub", "Home Gitea", "Lab GitLab", "Local clones", "Add connection..."] {
        assert!(f.has_hit(l), "{l}: {:?}", f.labels());
    }
    // choosing one closes the list and is remembered for the next time
    f.click("Home Gitea");
    assert!(f.app.overlay.is_none());
    assert_eq!(f.app.src().name(), "Home Gitea");
    let cx = f.cx.clone();
    let again = GitApp::with_rig(&cx, f.app.rig.clone());
    assert_eq!(again.src().name(), "Home Gitea", "the choice is kept");
    // Add connection... in the list opens Settings, Git with the wizard
    f.click("connection");
    f.click("Add connection...");
    assert_eq!(f.app.take_request(), Some(("settings".into(), "git:add".into())));
    // the tokens are in the vault only: nothing in the tile's state or on disk holds one
    assert!(!format!("{:?}", f.app.srcs).contains(TOK));
}

#[test]
fn with_no_connection_a_calm_first_run_view_still_lists_the_local_clones() {
    let mut f = fixture();
    f.mk_clone("notes", "https://git.example.org/me/notes.git", "main");
    f.draw();
    assert_eq!(f.app.srcs.len(), 1, "only the local clones");
    assert!(f.app.src().key() == "local");
    assert!(FIRST_RUN_TITLE.contains("Connect a Git host to browse its repositories") && FIRST_RUN_HINT.contains("Gitea") && FIRST_RUN_HINT.contains("cluster"));
    assert!(f.has_hit("Add connection..."));
    f.wait("the clones", |f| f.app.locals.loaded);
    assert_eq!(f.names(), vec!["notes"]);
    // nothing was asked of any host
    assert!(f.net.requests().is_empty());
    f.click("Add connection...");
    assert_eq!(f.app.take_request(), Some(("settings".into(), "git:add".into())));
    f.key('a');
    assert_eq!(f.app.take_request(), Some(("settings".into(), "git:add".into())));
}

// ------------------------------------------------------------------------------------------------ the repository list

#[test]
fn the_list_of_a_gitea_host_has_name_description_branch_visibility_age_stars_and_size() {
    let mut f = fixture();
    let c = f.conn(Kind::Gitea, "Home", "https://git.example.org");
    let w = f.wall();
    f.net.set(gitea_host(w));
    f.select_conn(&c.id);
    f.wait("the list", |f| f.app.board().loaded);
    // the newest first, as the host's own sort has it
    assert_eq!(f.names(), vec!["dev/tools", "ops/infra", "ops/web"]);
    let rows = f.app.rows.clone();
    let infra = rows.iter().find(|r| r.name == "ops/infra").unwrap();
    assert_eq!((infra.desc.as_str(), infra.branch.as_str(), infra.private, infra.stars, infra.size_kb, infra.updated), ("Cluster files and runbooks", "main", Some(true), Some(3), Some(2048), w - 3600));
    assert_eq!(cell(infra, 3, w), "private");
    assert_eq!(cell(infra, 4, w), "1h");
    assert_eq!(cell(infra, 6, w), "2.0 MB");
    assert_eq!(cell(rows.iter().find(|r| r.name == "ops/web").unwrap(), 3, w), "public");
    // the headers and the rows are clickable and named
    assert!(f.has_hit("Name") && f.has_hit("Updated") && f.has_hit("ops/infra"), "{:?}", f.labels());
    // a Gitea host is asked with its token style, GET only, with the time limit
    let q = &f.net.requests()[0];
    assert_eq!((q.method.as_str(), q.timeout_s, q.style), ("GET", 15, gitconn::Style::Token));
    assert!(f.app.status_line().is_none(), "nothing to say when all is well");
}

#[test]
fn github_and_gitlab_lists_are_read_from_their_own_json() {
    let mut f = fixture();
    let gh = f.conn(Kind::GitHub, "Work", "https://github.com");
    let gl = f.conn(Kind::GitLab, "Lab", "https://gitlab.example.org");
    let w = f.wall();
    let h = Arc::new(FakeGitHttp::default());
    h.answer("GET https://api.github.com/user/repos?per_page=50&sort=updated&page=1", 200, &[], &json!([
        {"full_name":"octo/hello","description":"Hello","default_branch":"trunk","private":false,"pushed_at":ts(w, 7200),"stargazers_count":120,"size":10,"clone_url":"https://github.com/octo/hello.git","ssh_url":"git@github.com:octo/hello.git","html_url":"https://github.com/octo/hello"}
    ]).to_string());
    h.answer("GET https://gitlab.example.org/api/v4/projects?membership=true&per_page=50&order_by=last_activity_at&page=1", 200, &[], &json!([
        {"path_with_namespace":"team/app","description":"An app","default_branch":"develop","visibility":"internal","last_activity_at":ts(w, 60),"star_count":4,"http_url_to_repo":"https://gitlab.example.org/team/app.git","ssh_url_to_repo":"git@gitlab.example.org:team/app.git","web_url":"https://gitlab.example.org/team/app"}
    ]).to_string());
    f.net.set(h);
    f.select_conn(&gh.id);
    f.wait("github", |f| f.app.board().loaded);
    let r = f.app.rows[0].clone();
    assert_eq!((r.name.as_str(), r.branch.as_str(), r.private, r.stars), ("octo/hello", "trunk", Some(false), Some(120)));
    assert_eq!(f.net.requests()[0].style, gitconn::Style::Bearer);
    f.select_conn(&gl.id);
    f.wait("gitlab", |f| f.app.board().loaded);
    let r = f.app.rows[0].clone();
    assert_eq!((r.name.as_str(), r.branch.as_str(), r.private, r.stars), ("team/app", "develop", Some(true), Some(4)), "internal is not public");
}

#[test]
fn a_host_with_many_repositories_loads_a_page_of_fifty_at_a_time() {
    let mut f = fixture();
    let c = f.conn(Kind::Gitea, "Home", "https://git.example.org");
    let w = f.wall();
    let page = |from: usize, n: usize| Value::Array((from..from + n).map(|i| gitea_repo(w, &format!("team/r{i:03}"), "", 100 + i as u64, false, 0)).collect()).to_string();
    let h = Arc::new(FakeGitHttp::default());
    h.answer(&format!("{BASE}/user/repos?limit=50&page=1"), 200, &[("Link", "<x>; rel=\"next\"")], &page(0, 50));
    h.answer(&format!("{BASE}/user/repos?limit=50&page=2"), 200, &[], &page(45, 20));
    f.net.set(h);
    f.select_conn(&c.id);
    f.wait("page 1", |f| f.app.board().loaded);
    assert_eq!(f.names().len(), 50);
    assert!(f.app.board().more && f.has_hit("Load more"), "{:?}", f.labels());
    assert_eq!(f.net.count("page=2"), 0, "the second page is only asked for on request");
    f.click("Load more");
    f.wait("page 2", |f| f.app.board().page == 2 && f.app.board().job.is_none());
    // 5 of the 20 repeat what page 1 had: they are not listed twice
    assert_eq!(f.names().len(), 65);
    assert!(!f.app.board().more && !f.has_hit("Load more"));
    // a refresh starts again from the first page
    f.key('r');
    f.wait("page 1 again", |f| f.app.board().loaded && f.app.board().page == 1);
    assert_eq!(f.names().len(), 50);
}

#[test]
fn the_table_sorts_by_a_clicked_header_or_the_keys_and_filters_as_you_type() {
    let mut f = fixture();
    let c = f.conn(Kind::Gitea, "Home", "https://git.example.org");
    let w = f.wall();
    f.net.set(gitea_host(w));
    f.select_conn(&c.id);
    f.wait("the list", |f| f.app.board().loaded);
    f.click("Name");
    assert_eq!(f.names(), vec!["dev/tools", "ops/infra", "ops/web"], "ascending by name");
    f.click("Name");
    assert_eq!(f.names(), vec!["ops/web", "ops/infra", "dev/tools"], "descending the second time");
    f.click("Stars");
    assert_eq!(f.names()[0], "ops/web", "stars: most first");
    // the key o goes to the next column, O reverses
    f.key('o');
    f.key('O');
    let (col, desc) = f.app.sort.unwrap();
    assert!(col < COLS.len() && desc != (col == 4 || col == 5 || col == 6));
    // the filter: / then letters; case does not matter; the description counts too
    f.key('/');
    assert_eq!(f.app.focus, Focus::Filter);
    f.type_text("RUNBOOK");
    assert_eq!(f.names(), vec!["ops/infra"]);
    f.press(Key::Esc);
    assert_eq!(f.names().len(), 3, "Esc clears the filter");
    assert_eq!(f.app.focus, Focus::List);
    f.key('/');
    f.type_text("zzz");
    assert!(f.names().is_empty());
    assert!(f.has_hit("Clear"));
    f.click("Clear");
    assert_eq!(f.names().len(), 3);
}

#[test]
fn a_dot_marks_a_repository_that_is_cloned_here_matched_by_its_remote_whatever_the_spelling() {
    let mut f = fixture();
    let c = f.conn(Kind::Gitea, "Home", "https://git.example.org");
    f.mk_clone("infra", "git@git.example.org:ops/infra.git", "main");
    f.mk_clone("web-copy", "https://me:SECRETPW99@git.example.org:3000/Ops/Web", "dev");
    f.mk_clone("elsewhere", "https://github.com/ops/tools.git", "main");
    let w = f.wall();
    f.net.set(gitea_host(w));
    f.select_conn(&c.id);
    f.wait("list and clones", |f| f.app.board().loaded && f.app.locals.loaded);
    f.draw();
    let cloned: Vec<(String, bool)> = f.app.rows.iter().map(|r| (r.name.clone(), r.clone.is_some())).collect();
    assert!(cloned.contains(&("ops/infra".into(), true)) && cloned.contains(&("ops/web".into(), true)) && cloned.contains(&("dev/tools".into(), false)), "{cloned:?}");
    // the remote of a clone never keeps a credential
    assert!(f.app.locals.list.iter().all(|l| !l.remote.clone().unwrap_or_default().contains("SECRETPW99")));
    // the local source lists the three
    f.select_conn("local");
    f.wait("clones", |f| f.app.locals.loaded);
    assert_eq!(f.names(), vec!["elsewhere", "infra", "web-copy"]);
    assert_eq!(f.app.rows.iter().find(|r| r.name == "infra").unwrap().branch, "main");
}

// ------------------------------------------------------------------------------------------------ failures, stale data, slowing down

#[test]
fn when_the_host_goes_quiet_the_last_list_stays_on_screen_and_the_tile_tries_again_with_backoff() {
    let mut f = fixture();
    let c = f.conn(Kind::Gitea, "Home", "https://git.example.org");
    let w = f.wall();
    f.net.set(gitea_host(w));
    f.select_conn(&c.id);
    f.wait("the list", |f| f.app.board().loaded);
    assert_eq!(f.names().len(), 3);
    // the host stops answering
    let down = Arc::new(FakeGitHttp::default());
    down.fail("user/repos", NetErr::Unreachable("could not connect".into()));
    f.net.set(down);
    f.click("Refresh");
    f.wait("the failure", |f| f.app.board().err.is_some());
    assert_eq!(f.names().len(), 3, "the last good list stays");
    let line = f.app.status_line().expect("a line that says what is going on");
    assert!(line.0.contains("Updating") && line.0.contains("retrying"), "{line:?}");
    assert!(f.app.board().retry_at.is_some(), "a transient failure is tried again");
    // nothing more is asked until the wait is over, then once; the waits grow
    let calls = f.net.count("user/repos");
    f.settle();
    assert_eq!(f.net.count("user/repos"), calls);
    f.timer.advance(2_100);
    f.wait("the second try", |f| f.net.count("user/repos") == calls + 1 && f.app.board().job.is_none());
    let first_gap = f.app.board().retry_at.unwrap() - f.timer.now_ms();
    f.timer.advance(first_gap + 100);
    f.wait("the third try", |f| f.net.count("user/repos") == calls + 2 && f.app.board().job.is_none());
    let second_gap = f.app.board().retry_at.unwrap() - f.timer.now_ms();
    assert!(second_gap > first_gap, "{first_gap} then {second_gap}");
    // past the window the line says the list is old, but the rows are still the last good ones
    f.timer.advance(40_000);
    f.wait("a late try", |f| f.net.count("user/repos") >= calls + 3 && f.app.board().job.is_none());
    assert_eq!(f.names().len(), 3);
    assert!(f.app.status_line().unwrap().0.contains("Showing the last list"), "{:?}", f.app.status_line());
    // the host is back: the next try takes it, the line goes
    f.net.set(gitea_host(w));
    f.click("Refresh");
    f.wait("recovery", |f| f.app.board().err.is_none() && f.app.board().loaded && f.app.board().job.is_none());
    assert!(f.app.status_line().is_none() || f.app.status_line().unwrap().0.starts_with("Refreshing"));
    f.settle();
    assert!(f.app.status_line().is_none() || f.app.status.is_some());
}

#[test]
fn a_refusal_is_not_retried_and_says_what_to_fix() {
    let mut f = fixture();
    let c = f.conn(Kind::Gitea, "Home", "https://git.example.org");
    let h = Arc::new(FakeGitHttp::default());
    h.answer("user/repos", 401, &[], &format!("{{\"message\":\"bad token {TOK}\"}}"));
    f.net.set(h);
    f.select_conn(&c.id);
    f.wait("the refusal", |f| f.app.board().err.is_some());
    assert!(matches!(f.app.board().err, Some(BrowseErr::Persistent(_))));
    // a calm panel, not a half-empty table; its buttons help
    for b in ["Refresh", "Show log", "Edit the connection"] {
        assert!(f.has_hit(b), "{b}: {:?}", f.labels());
    }
    let n = f.net.count("user/repos");
    f.timer.advance(120_000);
    f.settle();
    assert_eq!(f.net.count("user/repos"), n, "nothing is retried until the person presses Refresh");
    f.click("Edit the connection");
    assert_eq!(f.app.take_request(), Some(("settings".into(), "tab:git".into())));
    f.click("Show log");
    assert!(f.app.show_log && f.app.log_text().contains("persistent") && f.app.log_text().contains("401") || f.app.log_text().contains("refused"), "{}", f.app.log_text());
    assert!(!f.app.log_text().contains(TOK), "the log holds no token");
    f.click("Copy");
    assert!(!f.clipboard().is_empty() && !f.clipboard().contains(TOK));
    f.click("Hide log");
    assert!(!f.app.show_log);
}

#[test]
fn a_rate_limit_shows_how_long_to_wait_and_waits_then_refresh_ends_the_wait() {
    let mut f = fixture();
    let c = f.conn(Kind::GitHub, "Work", "https://github.com");
    let w = f.wall();
    let limited = Arc::new(FakeGitHttp::default());
    limited.answer("user/repos", 429, &[("Retry-After", "17")], "");
    f.net.set(limited);
    f.select_conn(&c.id);
    f.wait("the limit", |f| f.app.slow_down_s().is_some());
    assert_eq!(f.app.status_line().unwrap().0, "The host asked us to slow down: retry in 17 s.");
    let n = f.net.count("user/repos");
    f.timer.advance(10_000);
    f.draw();
    assert_eq!(f.app.status_line().unwrap().0, "The host asked us to slow down: retry in 7 s.");
    f.settle();
    assert_eq!(f.net.count("user/repos"), n, "no request while the host asked us to wait");
    // the wait is over: one new try
    f.timer.advance(7_100);
    let ok = Arc::new(FakeGitHttp::default());
    ok.answer("user/repos", 200, &[], "[]");
    f.net.set(ok);
    f.wait("the retry", |f| f.app.board().loaded);
    assert!(f.app.slow_down_s().is_none());
    // Refresh ends a wait at once
    let limited = Arc::new(FakeGitHttp::default());
    limited.answer("user/repos", 403, &[("X-RateLimit-Remaining", "0"), ("X-RateLimit-Reset", &format!("{}", w + 600))], "");
    f.net.set(limited);
    f.click("Refresh");
    f.wait("limit again", |f| f.app.slow_down_s().is_some());
    let secs = f.app.slow_down_s().unwrap();
    assert!((secs as i64 - (w as i64 + 600 - f.wall() as i64)).abs() <= 2, "{secs}");
    let ok = Arc::new(FakeGitHttp::default());
    ok.answer("user/repos", 200, &[], "[]");
    f.net.set(ok);
    f.click("Refresh");
    f.wait("refreshed through the wait", |f| f.app.board().loaded && f.app.board().err.is_none());
}

#[test]
fn nothing_is_asked_while_the_tile_is_not_on_screen() {
    let mut f = fixture();
    let c = f.conn(Kind::Gitea, "Home", "https://git.example.org");
    let w = f.wall();
    f.net.set(gitea_host(w));
    f.app.select_source(f.app.srcs.iter().position(|s| s.key() == c.id).unwrap());
    // never drawn: heartbeats alone ask nothing
    for _ in 0..20 {
        f.app.handle(&Event::Tick, &f.cx);
        std::thread::sleep(Duration::from_millis(2));
    }
    assert!(f.net.requests().is_empty() && f.run.calls().is_empty(), "{:?}", f.net.lines());
    f.draw();
    f.wait("the first look", |f| f.app.board().loaded);
    assert!(!f.net.requests().is_empty());
    // the tile is hidden (another pane or app is in front): Refresh is wanted but nothing is sent until it is drawn again
    std::thread::sleep(Duration::from_millis(600));
    let n = f.net.requests().len();
    f.app.handle(&Event::Key(Key::Char('r')), &f.cx);
    for _ in 0..20 {
        f.app.handle(&Event::Tick, &f.cx);
        std::thread::sleep(Duration::from_millis(2));
    }
    assert_eq!(f.net.requests().len(), n, "hidden");
    f.draw();
    f.wait("shown again", |f| f.net.requests().len() > n);
}

#[test]
fn an_account_that_may_not_reach_the_host_is_refused_before_any_request() {
    let mut f = fixture();
    f.cx.session.policy.net_hosts = vec!["other.example".into()];
    let c = f.conn(Kind::Gitea, "Home", "https://git.example.org");
    f.select_conn(&c.id);
    f.wait("the refusal", |f| f.app.board().err.is_some());
    assert!(f.app.board().err.as_ref().unwrap().text().contains("may not reach git.example.org"));
    assert!(f.net.requests().is_empty(), "the policy is asked before the network is");
}

// ------------------------------------------------------------------------------------------------ one repository

const DIFF: &str = "diff --git a/src/a.rs b/src/a.rs\nindex 111..222 100644\n--- a/src/a.rs\n+++ b/src/a.rs\n@@ -1,3 +1,3 @@\n fn a() {\n-    old();\n+    new();\n }\ndiff --git a/new.txt b/new.txt\nnew file mode 100644\n--- /dev/null\n+++ b/new.txt\n@@ -0,0 +1,2 @@\n+one\n+two\ndiff --git a/img.png b/img.png\nBinary files a/img.png and b/img.png differ\n";

fn commit_json(wall: u64, n: usize) -> String {
    Value::Array((0..n).map(|i| json!({"sha": format!("{:040x}", 0xabc000 + i), "commit": {"message": format!("Change number {i}\n\nMore words about change {i}."), "author": {"name": "Ada", "date": ts(wall, 600 * (i as u64 + 1))}}})).collect()).to_string()
}

/// The host of `gitea_host` plus the rest of ops/infra.
fn full_host(wall: u64) -> Arc<FakeGitHttp> {
    let h = gitea_host(wall);
    let r = format!("{BASE}/repos/ops/infra");
    h.answer(&format!("{r}/branches?limit=50"), 200, &[], &json!([{"name":"dev","commit":{"id":"d1","message":"Dev work","timestamp":ts(wall, 500)}},{"name":"main","commit":{"id":"b1","message":"Main work\nmore","timestamp":ts(wall, 100)}}]).to_string());
    h.answer(&format!("{r}/tags?limit=50"), 200, &[], &json!([{"name":"v1","commit":{"sha":"c1c1c1c"}}]).to_string());
    h.answer(&format!("{r}/commits?limit=50&stat=false&verification=false&files=false&sha=main"), 200, &[], &commit_json(wall, 60));
    h.answer(&format!("{r}/commits?limit=50&stat=false&verification=false&files=false&sha=v1"), 200, &[], &commit_json(wall, 2));
    h.answer(&format!("{r}/contents?ref=v1"), 200, &[], &Value::Array(vec![entry("OLD.txt", "file", 5, "OLD.txt")]).to_string());
    h.answer(&format!("{r}/git/commits/{:040x}.diff", 0xabc000), 200, &[], DIFF);
    h.fail(&format!("{r}/git/commits/{:040x}.diff", 0xabc002), NetErr::Other(gitconn::TOO_BIG.into()));
    h.answer(&format!("{r}/git/commits/{:040x}.diff", 0xabc001), 200, &[], "");
    h.answer(&format!("{r}/issues?type=issues&state=all&limit=50"), 200, &[], &json!([
        {"number":4,"title":"Cluster does not start","state":"open","user":{"login":"ada"},"updated_at":ts(wall, 900),"labels":[{"name":"bug"}],"body":"Steps:\n1. start\n2. look","comments":2},
        {"number":3,"title":"Old thing","state":"closed","user":{"login":"bo"},"updated_at":ts(wall, 9000),"labels":[],"body":""}]).to_string());
    h.answer(&format!("{r}/issues?type=pulls&state=all&limit=50"), 200, &[], &json!([{"number":7,"title":"Fix the thing","state":"open","user":{"login":"cy"},"updated_at":ts(wall, 300),"labels":[],"body":"Please merge."}]).to_string());
    h
}

fn open_infra(f: &mut Fx) -> Connection {
    let c = f.conn(Kind::Gitea, "Home", "https://git.example.org");
    let w = f.wall();
    f.net.set(full_host(w));
    f.select_conn(&c.id);
    f.wait("the list", |f| f.app.board().loaded);
    f.open_row("ops/infra");
    f.wait("the root folder", |f| f.view().files.rows.iter().any(|r| r.name == "docs"));
    c
}

fn frow(f: &Fx, name: &str) -> usize {
    f.view().files.rows.iter().position(|r| r.name == name).unwrap_or_else(|| panic!("no file row {name}: {:?}", f.view().files.rows.iter().map(|r| r.name.clone()).collect::<Vec<_>>()))
}

#[test]
fn opening_a_repository_is_audited_by_name_and_its_folders_load_only_when_opened() {
    let mut f = fixture();
    let c = open_infra(&mut f);
    assert!(f.audit_text().contains(&format!("git.browse.open")) && f.audit_text().contains("Home ops/infra"), "{}", f.audit_text());
    assert!(!f.audit_text().contains(TOK));
    // the first look asked for the top folder only (and the README it shows)
    assert_eq!(f.net.count("/contents?ref=main"), 1);
    assert_eq!(f.net.count("/contents/docs"), 0);
    assert_eq!(f.net.count("/contents/src"), 0);
    // folders first, then files by name
    let names: Vec<String> = f.view().files.rows.iter().map(|r| r.name.clone()).collect();
    assert_eq!(names, vec!["docs", "src", "big.bin", "logo.png", "main.rs", "pack.zip", "README.md"]);
    // opening a folder asks for that folder; closing and opening again does not ask twice
    let i = frow(&f, "docs");
    f.code(T_FROW + i);
    f.wait("docs", |f| f.view().files.rows.iter().any(|r| r.name == "guide.txt"));
    assert_eq!(f.net.count("/contents/docs?ref=main"), 1);
    assert_eq!(f.view().files.rows[frow(&f, "guide.txt")].depth, 1);
    f.code(T_FROW + i);
    assert!(!f.view().files.rows.iter().any(|r| r.name == "guide.txt"));
    f.code(T_FROW + i);
    f.settle();
    assert_eq!(f.net.count("/contents/docs?ref=main"), 1);
    assert!(f.view().files.rows.iter().any(|r| r.name == "guide.txt"));
    // the whole tree was never asked for
    assert!(f.net.lines().iter().all(|l| !l.contains("git/trees") && !l.contains("recursive")));
    let _ = c;
}

#[test]
fn the_readme_is_shown_first_rendered_and_a_toggle_shows_its_source_with_line_numbers() {
    let mut f = fixture();
    open_infra(&mut f);
    f.wait("the README", |f| f.view().files.file.as_ref().is_some_and(|x| matches!(x.shown, Shown::Md)));
    let file = f.view().files.file.as_ref().unwrap();
    assert_eq!(file.name, "README.md");
    assert!(file.md.as_ref().unwrap().src.contains("# Infra"));
    // it draws (headings, bold, a list) without a problem, in the viewer's rectangle
    f.draw();
    assert!(f.view().rects.right.w > 0.0);
    assert!(f.has_hit("Copy") && f.has_hit("Source"));
    f.click("Source");
    f.draw();
    let first = f.view().files.file.as_ref().unwrap().text.lines[0].text.clone();
    assert_eq!(first, "    1  # Infra");
    assert!(f.has_hit("Rendered"));
    f.click("Copy");
    assert!(f.clipboard().starts_with("# Infra"), "{}", f.clipboard());
}

#[test]
fn the_viewer_shows_text_with_line_numbers_pictures_scaled_and_names_only_for_binary_or_big_files() {
    let mut f = fixture();
    open_infra(&mut f);
    let open = |f: &mut Fx, name: &str| {
        let i = frow(f, name);
        f.code(T_FROW + i);
    };
    // text: line numbers, tabs widened
    open(&mut f, "main.rs");
    f.wait("the text", |f| f.view().files.file.as_ref().is_some_and(|x| x.name == "main.rs" && matches!(x.shown, Shown::Text)));
    let lines: Vec<String> = f.view().files.file.as_ref().unwrap().text.lines.iter().map(|l| l.text.clone()).collect();
    assert_eq!(lines, vec!["    1  fn main() {", "    2      println!(\"hi\");", "    3  }"]);
    // a picture
    open(&mut f, "logo.png");
    f.wait("the picture", |f| f.view().files.file.as_ref().is_some_and(|x| x.name == "logo.png" && matches!(x.shown, Shown::Image(_))));
    f.draw();
    assert!(f.g.rgba().iter().any(|b| *b != 0));
    // a binary file: name, size and kind only
    open(&mut f, "pack.zip");
    f.wait("the archive", |f| f.view().files.file.as_ref().is_some_and(|x| x.name == "pack.zip" && matches!(x.shown, Shown::Binary(_))));
    assert!(matches!(&f.view().files.file.as_ref().unwrap().shown, Shown::Binary(l) if l == "ZIP archive"));
    // a file over 512 KB is not even asked for
    let before = f.net.count("big.bin");
    open(&mut f, "big.bin");
    f.wait("the big file", |f| f.view().files.file.as_ref().is_some_and(|x| x.name == "big.bin" && matches!(x.shown, Shown::Loading | Shown::TooBig)));
    f.settle();
    assert!(matches!(f.view().files.file.as_ref().unwrap().shown, Shown::TooBig));
    assert_eq!(f.net.count("big.bin"), before, "no request for it");
    // a failure is said in the viewer and goes to the log, without the token
    let h = full_host(f.wall());
    h.answer(&format!("{BASE}/repos/ops/infra/contents/docs/missing.txt"), 404, &[], "");
    f.net.set(h);
    f.app.view.as_mut().unwrap().open_file("docs/missing.txt", "missing.txt", 5);
    f.wait("the failure", |f| matches!(f.view().files.file.as_ref().unwrap().shown, Shown::Failed(_)));
    assert!(f.app.log_text().contains("404") || f.app.log_text().contains("does not have"), "{}", f.app.log_text());
}

#[test]
fn a_symbolic_link_or_submodule_is_explained_not_opened() {
    let mut f = fixture();
    let c = f.conn(Kind::Gitea, "Home", "https://git.example.org");
    let w = f.wall();
    let h = Arc::new(FakeGitHttp::default());
    h.answer(&format!("{BASE}/user/repos?limit=50&page=1"), 200, &[], &repo_list(w));
    h.answer(&format!("{BASE}/repos/ops/infra/contents?ref=main"), 200, &[], &Value::Array(vec![entry("link", "symlink", 4, "link"), entry("sub", "submodule", 0, "sub")]).to_string());
    f.net.set(h);
    f.select_conn(&c.id);
    f.wait("list", |f| f.app.board().loaded);
    f.open_row("ops/infra");
    f.wait("root", |f| f.view().files.rows.len() == 2);
    let i = frow(&f, "link");
    f.code(T_FROW + i);
    assert!(f.app.status.as_ref().unwrap().0.contains("is a link"));
    let i = frow(&f, "sub");
    f.code(T_FROW + i);
    assert!(f.app.status.as_ref().unwrap().0.contains("submodule"));
    assert!(f.view().files.file.is_none());
}

#[test]
fn the_branch_and_tag_chooser_reloads_the_files_and_commits_for_that_ref() {
    let mut f = fixture();
    open_infra(&mut f);
    assert_eq!(f.view().rf, "main");
    f.click("branch or tag");
    assert_eq!(f.app.overlay, Some(Overlay::Refs));
    f.wait("the refs", |f| f.view().refs.branches.ready().is_some() && f.view().refs.tags.ready().is_some());
    let rows = f.view().ref_rows();
    assert_eq!(rows, vec![("main".to_string(), false, true), ("dev".to_string(), false, false), ("v1".to_string(), true, false)], "the default first");
    f.draw();
    assert!(f.has_hit("v1") && f.has_hit("dev"));
    f.click("v1");
    assert!(f.app.overlay.is_none());
    assert_eq!(f.view().rf, "v1");
    f.wait("the old tree", |f| f.view().files.rows.iter().any(|r| r.name == "OLD.txt"));
    assert_eq!(f.net.count("/contents?ref=v1"), 1);
    f.key('2');
    f.wait("commits of the tag", |f| f.view().commits.slot.ready().is_some());
    assert_eq!(f.view().commits.slot.ready().unwrap().len(), 2);
    assert!(f.net.count("sha=v1") == 1);
    // by key: g opens the chooser, Esc closes it
    f.key('g');
    assert_eq!(f.app.overlay, Some(Overlay::Refs));
    f.press(Key::Esc);
    assert!(f.app.overlay.is_none() && f.app.view.is_some());
}

#[test]
fn commits_show_at_most_fifty_and_a_chosen_one_shows_its_message_and_a_colored_diff() {
    let mut f = fixture();
    open_infra(&mut f);
    f.key('2');
    f.wait("commits", |f| f.view().commits.slot.ready().is_some());
    let n = f.view().commits.slot.ready().unwrap().len();
    assert_eq!(n, 50, "the host sent 60, the tile shows 50");
    f.draw();
    assert!(f.has_hit("Change number 0"));
    // choose the first: Enter
    f.press(Key::Enter);
    assert!(f.view().commits.open.is_some());
    assert_eq!(f.view().commits.msg[0], "Change number 0");
    f.wait("the diff", |f| f.view().commits.diff.ready().is_some());
    let d = f.view().commits.diff.ready().unwrap();
    assert_eq!(d.files.iter().map(|x| (x.name().to_string(), x.added, x.removed)).collect::<Vec<_>>(), vec![("src/a.rs".to_string(), 1, 1), ("new.txt".to_string(), 2, 0), ("img.png".to_string(), 0, 0)]);
    let colors: Vec<([u8; 4], String)> = f.view().commits.text.lines.iter().map(|l| (l.color, l.text.clone())).collect();
    use crate::ui::{DIM, ERR, OK, TEXT};
    assert!(colors.contains(&(OK, "+    new();".to_string())) && colors.contains(&(ERR, "-    old();".to_string())) && colors.contains(&(TEXT, " fn a() {".to_string())) && colors.iter().any(|(c, t)| *c == crate::fx::CYAN && t.starts_with("@@")) && colors.iter().any(|(c, t)| *c == DIM && t.starts_with("diff --git")));
    // the file list: pick the second file, the lines change
    f.draw();
    assert!(f.has_hit("new.txt"));
    f.click("new.txt");
    assert!(f.view().commits.text.lines.iter().any(|l| l.text == "+two"));
    // a binary file says so
    f.click("img.png");
    assert!(f.view().commits.text.lines.iter().any(|l| l.text.contains("binary file changed")));
    // a commit that changes nothing says so
    let i = 1;
    f.code(T_CROW + i);
    f.wait("the empty diff", |f| f.view().commits.diff.ready().is_some());
    assert!(f.view().commits.text.lines[0].text.contains("changes no files"));
    // a diff over the limit: the fallback text, not an error
    f.code(T_CROW + 2);
    f.wait("the big diff", |f| f.view().commits.diff.ready().is_some_and(|d| d.too_large));
    assert!(f.view().commits.text.lines[0].text.contains("larger than 256 KB"), "{}", f.view().commits.text.lines[0].text);
    assert!(f.app.status_line().is_none(), "no error line for it");
}

#[test]
fn the_branches_tab_marks_the_default_and_lists_tags_and_a_row_goes_to_its_files() {
    let mut f = fixture();
    open_infra(&mut f);
    f.key('3');
    f.wait("branches and tags", |f| f.view().refs.branches.ready().is_some() && f.view().refs.tags.ready().is_some());
    f.draw();
    assert!(f.has_hit("main") && f.has_hit("dev") && f.has_hit("v1"));
    let names: Vec<String> = f.view().ref_rows().iter().map(|r| r.0.clone()).collect();
    assert_eq!(names, vec!["main", "dev", "v1"]);
    assert!(f.view().ref_rows()[0].2, "main is the default");
    // Enter on a row switches to that ref's files
    f.press(Key::Down);
    f.press(Key::Down);
    f.press(Key::Enter);
    assert_eq!((f.view().rf.as_str(), f.view().tab), ("v1", Tab::Files));
}

#[test]
fn issues_and_pull_requests_are_lists_to_read_and_nothing_can_be_written() {
    let mut f = fixture();
    open_infra(&mut f);
    f.key('4');
    f.wait("issues", |f| f.view().issues.issues.ready().is_some());
    f.draw();
    assert!(f.has_hit("Issues") && f.has_hit("Pull requests") && f.has_hit("Cluster does not start"));
    f.press(Key::Enter);
    let body: Vec<String> = f.view().issues.body.lines.iter().map(|l| l.text.clone()).collect();
    assert_eq!(body[0], "#4 Cluster does not start");
    assert!(body[1].contains("open") && body[1].contains("ada") && body[1].contains("bug"));
    assert!(body.iter().any(|l| l == "1. start") && body.iter().any(|l| l.contains("Read-only")));
    // the closed one without a text
    f.press(Key::Down);
    f.press(Key::Enter);
    assert!(f.view().issues.body.lines.iter().any(|l| l.text == "(no description)"));
    f.click("Pull requests");
    f.wait("pull requests", |f| f.view().issues.pulls.ready().is_some());
    f.draw();
    assert!(f.has_hit("Fix the thing"));
    f.click("Fix the thing");
    assert!(f.view().issues.body.lines.iter().any(|l| l.text == "Please merge."));
    // read-only, in every way: only GET went out, and no button or key writes
    assert!(f.net.requests().iter().all(|r| r.method == "GET" && r.body.is_none()));
    for bad in ["New issue", "Comment", "Close", "Merge", "Delete", "Rename", "Edit", "Settings", "Create"] {
        assert!(!f.labels().iter().any(|l| l.contains(bad)), "{bad}: {:?}", f.labels());
    }
    for k in ['n', 'e', 'd', 'x'] {
        f.key(k);
    }
    assert!(f.net.requests().iter().all(|r| r.method == "GET" && r.body.is_none()));
}

#[test]
fn the_info_tab_has_the_clone_addresses_with_copy_and_the_details() {
    let mut f = fixture();
    open_infra(&mut f);
    f.key('5');
    f.draw();
    assert!(f.has_hit("Copy"));
    f.code(T_COPY_HTTPS);
    assert_eq!(f.clipboard(), "https://git.example.org/ops/infra.git");
    f.code(T_COPY_SSH);
    assert_eq!(f.clipboard(), "git@git.example.org:ops/infra.git");
    // the key y copies the one the connection signs in with
    f.key('y');
    assert_eq!(f.clipboard(), "https://git.example.org/ops/infra.git");
    assert!(f.app.status.as_ref().unwrap().0.contains("HTTPS"));
    // the page of the repository opens in the browser tile only after a yes
    f.click("Open in browser");
    assert!(f.app.card.is_some() && f.app.take_request().is_none());
    f.click("Cancel");
    assert!(f.app.card.is_none() && f.app.take_request().is_none());
    f.key('w');
    f.click("Open");
    assert_eq!(f.app.take_request(), Some(("browser".into(), "newtab:https://git.example.org/ops/infra".into())));
}

#[test]
fn a_connection_that_signs_in_with_ssh_copies_the_ssh_address() {
    let mut f = fixture();
    let c = f.app.conns(&f.cx).add(Connection { name: "Keyed".into(), kind: Kind::Gitea, url: "https://git.example.org".into(), method: gitconn::Method::Ssh, key: "k1".into(), ..Default::default() }, None).unwrap();
    f.app.refresh_entries(&f.cx);
    let w = f.wall();
    f.net.set(gitea_host(w));
    f.select_conn(&c.id);
    f.wait("list", |f| f.app.board().loaded);
    f.key('y');
    assert_eq!(f.clipboard(), "git@git.example.org:dev/tools.git", "the first row by update time");
    assert_eq!(GitApp::clone_url(&c, "dev/tools"), "git@git.example.org:dev/tools.git");
}

#[test]
fn leaving_a_repository_drops_what_was_still_on_its_way() {
    let mut f = fixture();
    let c = f.conn(Kind::Gitea, "Home", "https://git.example.org");
    let w = f.wall();
    f.net.set(full_host(w));
    f.select_conn(&c.id);
    f.wait("list", |f| f.app.board().loaded);
    f.open_row("ops/infra");
    let cancel = f.view().cancel.clone();
    assert!(!cancel.load(Ordering::SeqCst));
    // before any heartbeat nothing was asked; going back cancels the rest
    assert_eq!(f.net.count("/repos/ops/infra"), 0);
    f.press(Key::Esc);
    assert!(f.app.view.is_none() && cancel.load(Ordering::SeqCst), "the view is gone and its requests are cancelled");
    f.settle();
    assert_eq!(f.net.count("/repos/ops/infra"), 0);
    // a load that was already on its way finishes into nothing: the README that would follow is never asked for
    f.open_row("ops/infra");
    f.app.handle(&Event::Tick, &f.cx);
    f.press(Key::Esc);
    std::thread::sleep(Duration::from_millis(80));
    for _ in 0..5 {
        f.app.handle(&Event::Tick, &f.cx);
        std::thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(f.net.count("README.md"), 0);
    assert!(f.app.view.is_none());
}

#[test]
fn a_rate_limit_inside_a_repository_holds_back_every_request_to_that_host() {
    let mut f = fixture();
    open_infra(&mut f);
    let limited = Arc::new(FakeGitHttp::default());
    limited.answer("/commits", 429, &[("Retry-After", "20")], "");
    f.net.set(limited);
    f.key('2');
    f.wait("the limit", |f| f.app.slow_down_s().is_some());
    assert_eq!(f.app.status_line().unwrap().0, "The host asked us to slow down: retry in 20 s.");
    let n = f.net.requests().len();
    // another tab does not get around it
    f.key('4');
    f.settle();
    assert_eq!(f.net.requests().len(), n, "no request while the host asked us to wait");
    f.timer.advance(20_500);
    f.net.set(full_host(f.wall()));
    f.wait("after the wait", |f| f.view().issues.issues.ready().is_some());
    assert!(f.app.slow_down_s().is_none());
}

#[test]
fn a_flaky_host_inside_a_repository_keeps_the_data_and_tries_again() {
    let mut f = fixture();
    open_infra(&mut f);
    f.key('2');
    f.wait("commits", |f| f.view().commits.slot.ready().is_some());
    let down = Arc::new(FakeGitHttp::default());
    down.fail("/commits", NetErr::Unreachable("down".into()));
    f.net.set(down);
    f.click("Refresh");
    f.wait("the failure", |f| f.view().commits.slot.stale.is_some());
    assert_eq!(f.view().commits.slot.ready().unwrap().len(), 50, "the old commits stay");
    assert!(f.view().commits.slot.retry_at.is_some());
    f.net.set(full_host(f.wall()));
    f.timer.advance(2_100);
    f.wait("the retry", |f| f.view().commits.slot.stale.is_none() && f.view().commits.slot.ready().is_some());
}

// ------------------------------------------------------------------------------------------------ a clone on this computer, read with git

const LS_ROOT: &str = "100644 blob aaa      12\tREADME.md\0040000 tree bbb       -\tsrc\0100644 blob ccc      30\tmain.rs\0";

fn git_answers(f: &Fx) {
    let r = &f.run;
    r.answer("ls-tree -l -z main -- src/", "100644 blob ddd      10\tsrc/lib.rs\0");
    r.answer("ls-tree -l -z main", LS_ROOT);
    r.answer("show main:README.md", "# App\n\nA local clone.\n");
    r.answer("show main:main.rs", "fn main() {}\n");
    r.answer("log -n50", "aaaaaaa1111\u{1f}Ada\u{1f}1799999000\u{1f}First line\n\nmore\n\u{1e}\nbbbbbbb2222\u{1f}Bo\u{1f}1799990000\u{1f}Second\n\u{1e}\n");
    r.answer("show --format= --patch --no-color --no-ext-diff --no-textconv -M aaaaaaa1111 --", DIFF);
    r.answer("symbolic-ref --short -q refs/remotes/origin/HEAD", "origin/main\n");
    r.answer("refs/heads", "dev\u{1f}1111\u{1f}Dev work\u{1f}1799990000\nmain\u{1f}2222\u{1f}Main work\u{1f}1799999000\n");
    r.answer("refs/tags", "v2\u{1f}3333\u{1f}Release two\u{1f}1799990000\n");
}

#[test]
fn a_local_clone_is_browsed_with_git_only_and_never_asks_a_host() {
    let mut f = fixture();
    f.mk_clone("app", "https://git.example.org/me/app.git", "main");
    git_answers(&f);
    f.draw();
    f.wait("the clones", |f| f.app.locals.loaded);
    assert_eq!(f.app.src().key(), "local");
    f.open_row("app");
    assert!(f.audit_text().contains("git.browse.open") && f.audit_text().contains("local app"));
    f.wait("the files", |f| f.view().files.rows.iter().any(|r| r.name == "main.rs"));
    assert_eq!(f.view().files.rows.iter().map(|r| (r.name.clone(), r.kind)).collect::<Vec<_>>(), vec![("src".to_string(), Some(EntryKind::Dir)), ("main.rs".to_string(), Some(EntryKind::File)), ("README.md".to_string(), Some(EntryKind::File))]);
    f.wait("the README", |f| f.view().files.file.as_ref().is_some_and(|x| matches!(x.shown, Shown::Md)));
    let i = frow(&f, "src");
    f.code(T_FROW + i);
    f.wait("src", |f| f.view().files.rows.iter().any(|r| r.name == "lib.rs"));
    f.key('2');
    f.wait("commits", |f| f.view().commits.slot.ready().is_some());
    assert_eq!(f.view().commits.slot.ready().unwrap()[0].subject(), "First line");
    f.press(Key::Enter);
    f.wait("diff", |f| f.view().commits.diff.ready().is_some());
    assert_eq!(f.view().commits.diff.ready().unwrap().files.len(), 3);
    f.key('3');
    f.wait("refs", |f| f.view().refs.branches.ready().is_some() && f.view().refs.tags.ready().is_some());
    assert_eq!(f.view().ref_rows().iter().map(|r| r.0.clone()).collect::<Vec<_>>(), vec!["main", "dev", "v2"]);
    // issues live on the host: said in words, not as an error of git
    f.key('4');
    f.wait("issues", |f| f.view().issues.issues.problem().is_some());
    assert!(f.view().issues.issues.problem().unwrap().contains("live on the host"));
    f.key('5');
    f.draw();
    // nothing went to a host; everything git was asked is a read
    assert!(f.net.requests().is_empty());
    for c in f.run.calls() {
        let sub = c.argv.iter().skip_while(|a| *a != "-C").nth(2).cloned().unwrap_or_default();
        assert!(["ls-tree", "show", "log", "for-each-ref", "cat-file", "symbolic-ref", "remote", "status"].contains(&sub.as_str()), "{}", c.line());
        assert!(c.argv.iter().all(|a| !a.contains(TOK)));
    }
}

// ------------------------------------------------------------------------------------------------ the actions and their reviews

fn list_with_clone(f: &mut Fx, cloned: bool) -> Connection {
    let c = f.conn(Kind::Gitea, "Home", "https://git.example.org");
    if cloned {
        f.mk_clone("infra", "https://git.example.org/ops/infra.git", "main");
    }
    let w = f.wall();
    f.net.set(full_host(w));
    f.select_conn(&c.id);
    f.wait("list and clones", |f| f.app.board().loaded && f.app.locals.loaded);
    // the selected row is ops/infra
    let names = f.names();
    f.app.list.sel = names.iter().position(|n| n == "ops/infra").unwrap();
    f.draw();
    c
}

fn card_title(f: &Fx) -> String {
    f.app.card.as_ref().map(|c| c.title.clone()).unwrap_or_default()
}

fn card_text(f: &Fx) -> String {
    f.app.card.as_ref().map(|c| c.lines.iter().map(|l| l.0.clone()).collect::<Vec<_>>().join("\n")).unwrap_or_default()
}

#[test]
fn clone_shows_a_review_first_and_then_plans_the_clone_into_the_repositories_folder() {
    let mut f = fixture();
    let c = list_with_clone(&mut f, false);
    f.key('C');
    assert_eq!(card_title(&f), "Clone this repository?");
    let t = card_text(&f);
    assert!(t.contains("ops/infra from Home (Gitea / Forgejo, on the internet)") && t.contains("https://git.example.org/ops/infra.git") && t.contains("personal access token") && t.contains(&format!("{}/infra", f.root.display())), "{t}");
    assert!(f.app.run.is_none(), "nothing starts before the yes");
    assert!(f.has_hit("Clone") && f.has_hit("Cancel"));
    f.press(Key::Esc);
    assert!(f.app.card.is_none() && f.app.run.is_none());
    // and now yes
    f.key('C');
    f.click("Clone");
    let run = f.app.run.as_ref().expect("the clone is planned");
    let it = &run.batch.items[0];
    assert_eq!((it.op, it.url.url.as_str(), it.folder.as_str(), it.state.clone()), (Op::Clone, "https://git.example.org/ops/infra.git", "infra", ItemState::Queued));
    assert_eq!(it.url.url, GitApp::clone_url(&c, "ops/infra"));
    assert!(f.root.join(".iyakki/project.json").is_file(), "the folder is set up as the jobs expect");
    assert!(f.audit_text().contains("git.clone.start"), "{}", f.audit_text());
    assert!(!f.audit_text().contains(TOK));
    // the status line follows it; a second action waits for it
    assert!(f.app.status_line().unwrap().0.starts_with("Cloning ops/infra from Home"), "{:?}", f.app.status_line());
    f.key('C');
    assert!(f.app.card.is_none() && f.app.status.as_ref().unwrap().0.contains("still running"));
}

#[test]
fn the_clone_review_has_a_submodules_box_that_is_off_until_ticked_and_the_choice_reaches_the_job() {
    let mut f = fixture();
    list_with_clone(&mut f, false);
    f.key('C');
    assert!(f.has_hit("Also clone its submodules"));
    let t = f.app.card.as_ref().unwrap().toggle.as_ref().unwrap();
    assert!(!t.on && t.note_off.starts_with("Off:"), "off by default");
    // not ticked: the job asks for no submodules
    f.click("Clone");
    assert!(!f.app.run.as_ref().unwrap().batch.items[0].spec.submodules);
    assert!(f.audit_text().contains("git.clone.start") && !f.audit_text().contains("with submodules"), "{}", f.audit_text());
    // the key `s` and a click on the box both flip it; the note under it follows
    let mut f = fixture();
    list_with_clone(&mut f, false);
    f.key('C');
    f.key('s');
    let t = f.app.card.as_ref().unwrap().toggle.as_ref().unwrap();
    assert!(t.on && t.note_on.contains(".gitmodules") && t.note_on.contains("https only"));
    f.click("Also clone its submodules");
    assert!(!f.app.card.as_ref().unwrap().toggle.as_ref().unwrap().on);
    f.click("Also clone its submodules");
    assert!(f.app.run.is_none(), "ticking starts nothing");
    f.click("Clone");
    let it = &f.app.run.as_ref().unwrap().batch.items[0];
    assert!(it.spec.submodules && it.url.url == "https://git.example.org/ops/infra.git");
    assert!(f.audit_text().contains("1 repositories into Repositories with submodules"), "{}", f.audit_text());
    // the other cards have no box
    let mut f = fixture();
    list_with_clone(&mut f, true);
    f.key('P');
    f.key('s');
    assert!(f.app.card.as_ref().is_none_or(|c| c.toggle.is_none()));
}

#[test]
fn the_clone_review_with_its_box_fits_at_the_four_sizes() {
    for (w, h, s) in [(1280u32, 720u32, 1.0f32), (1920, 1080, 1.5), (1024, 600, 0.8), (800, 480, 0.62)] {
        let mut f = fixture_with("git-tile-box", Role::Admin, w, h, s);
        open_infra(&mut f);
        f.app.view = None;
        f.draw();
        f.key('C');
        for on in [false, true] {
            assert_hits_inside(&f, &format!("{w}x{h} s{s} clone card, box {on}"));
            let rect = |label: &str| f.g.hits.iter().rev().find(|h| h.label == label).unwrap_or_else(|| panic!("{w}x{h}: no `{label}`")).rect;
            let (bx, yes, no) = (rect("Also clone its submodules"), rect("Clone"), rect("Cancel"));
            let apart = |a: crate::ui::R, b: crate::ui::R| a.x + a.w <= b.x + 0.5 || b.x + b.w <= a.x + 0.5 || a.y + a.h <= b.y + 0.5 || b.y + b.h <= a.y + 0.5;
            assert!(apart(bx, yes) && apart(bx, no) && apart(yes, no), "{w}x{h}: the box and the buttons overlap: {bx:?} {yes:?} {no:?}");
            assert!(bx.h >= 13.5 * s * 2.0, "{w}x{h}: the box row is big enough to hit");
            if !on {
                f.key('s');
            }
        }
        f.press(Key::Esc);
        assert!(f.app.card.is_none());
    }
}

#[test]
fn a_clone_that_finished_is_listed_with_its_dot_and_a_failed_one_says_why() {
    let mut f = fixture();
    list_with_clone(&mut f, false);
    f.key('C');
    f.click("Clone");
    // the job ends well: the folder is there
    f.mk_clone("infra", "https://git.example.org/ops/infra.git", "main");
    f.app.run.as_mut().unwrap().batch.items[0].state = ItemState::Done;
    f.wait("the clone is told", |f| f.app.status.as_ref().is_some_and(|s| s.0.starts_with("Cloned ops/infra")));
    f.wait("listed", |f| f.app.rows.iter().any(|r| r.name == "ops/infra" && r.clone.is_some()));
    // a failed one: git's own last line, no credential
    f.mk_clone("web", "https://git.example.org/ops/web.git", "main");
    f.app.run = None;
    f.app.list.sel = f.names().iter().position(|n| n == "ops/web").unwrap();
    f.key('C');
    f.click("Clone");
    f.app.run.as_mut().unwrap().batch.items[0].state = ItemState::Failed(format!("fatal: repository not found {TOK}").replace(TOK, "***"));
    f.wait("the failure", |f| f.app.status.as_ref().is_some_and(|s| s.0.contains("failed")));
    assert!(f.app.status.as_ref().unwrap().0.contains("fatal: repository not found") && !f.app.status.as_ref().unwrap().0.contains(TOK));
}

#[test]
fn open_in_the_ai_ide_needs_a_clone_so_it_offers_the_clone_first_then_opens_the_workspace() {
    let mut f = fixture();
    list_with_clone(&mut f, false);
    f.key('I');
    assert_eq!(card_title(&f), "Clone this repository?");
    assert!(card_text(&f).contains("the AI IDE opens it"), "{}", card_text(&f));
    assert!(f.app.take_request().is_none());
    f.click("Clone");
    assert!(f.app.run.as_ref().unwrap().then_ide.is_some());
    // the clone arrives
    f.mk_clone("infra", "https://git.example.org/ops/infra.git", "main");
    f.app.run.as_mut().unwrap().batch.items[0].state = ItemState::Done;
    f.wait("the IDE is asked for", |f| f.app.request.is_some());
    let (app, item) = f.app.take_request().unwrap();
    assert_eq!(app, "ide");
    let id = item.strip_prefix("ws:").expect("a workspace").to_string();
    let reg = iyakki_agent::workspace::Registry::load(f.cx.services.dir(), "root");
    let w = reg.get(&id).expect("registered");
    assert_eq!((w.name.as_str(), std::path::Path::new(&w.root)), ("infra", std::fs::canonicalize(f.root.join("infra")).unwrap().as_path()));
    // the AI IDE takes the item and opens that workspace
    let mut ide = crate::apps::ide::IdeApp::new(&f.cx);
    crate::apps::App::open_item(&mut ide, &item, &f.cx);
    assert_eq!(ide.active_ws().map(|w| w.meta.id.clone()), Some(id.clone()));
    // with a clone present there is nothing to ask: it opens at once, and the same workspace is used
    f.app.run = None;
    f.key('I');
    assert!(f.app.card.is_none());
    assert_eq!(f.app.take_request(), Some(("ide".into(), format!("ws:{id}"))));
    assert_eq!(iyakki_agent::workspace::Registry::load(f.cx.services.dir(), "root").items.len(), 1);
}

#[test]
fn fetch_pull_and_push_need_a_clone_and_say_so() {
    let mut f = fixture();
    list_with_clone(&mut f, false);
    for k in ['F', 'P', 'U'] {
        f.key(k);
        assert!(f.app.status.as_ref().unwrap().0.contains("needs a clone"), "{k}: {:?}", f.app.status);
        assert!(f.app.run.is_none() && f.app.card.is_none());
    }
}

#[test]
fn fetch_starts_the_fetch_job_for_the_clone_at_once() {
    let mut f = fixture();
    list_with_clone(&mut f, true);
    f.key('F');
    let run = f.app.run.as_ref().expect("a fetch");
    let it = &run.batch.items[0];
    assert_eq!((it.op, it.url.url.as_str(), it.folder.as_str()), (Op::Fetch, "https://git.example.org/ops/infra.git", "."));
    assert!(f.audit_text().contains("git.fetch.start"));
    assert!(f.app.card.is_none(), "a fetch changes nothing of yours: no review");
}

const CLEAN: &str = "# branch.oid abc\n# branch.head main\n# branch.upstream origin/main\n# branch.ab +0 -0\n";
const DIRTY: &str = "# branch.oid abc\n# branch.head main\n# branch.upstream origin/main\n# branch.ab +0 -1\n1 .M N... 100644 100644 100644 a b src/x.rs\n";
const AHEAD: &str = "# branch.oid abc\n# branch.head main\n# branch.upstream origin/main\n# branch.ab +2 -0\n";

#[test]
fn pull_goes_ahead_when_the_clone_is_clean_and_asks_when_it_has_uncommitted_changes() {
    let mut f = fixture();
    f.run.answer("status --porcelain=v2 --branch", CLEAN);
    list_with_clone(&mut f, true);
    f.key('P');
    f.wait("the pull", |f| f.app.run.is_some());
    assert_eq!(f.app.run.as_ref().unwrap().batch.items[0].op, Op::Pull);
    assert!(f.app.card.is_none() && f.audit_text().contains("git.pull.start"));
    // a clone with changes of its own: a card, then the pull only moves forward
    let mut d = fixture();
    d.run.answer("status --porcelain=v2 --branch", DIRTY);
    list_with_clone(&mut d, true);
    d.key('P');
    d.wait("the card", |d| d.app.card.is_some());
    assert_eq!(card_title(&d), "Pull over local changes?");
    assert!(card_text(&d).contains("not committed") && card_text(&d).contains("fast-forward"), "{}", card_text(&d));
    d.press(Key::Esc);
    assert!(d.app.card.is_none() && d.app.run.is_none());
    d.key('P');
    d.wait("the card again", |d| d.app.card.is_some());
    d.click("Pull, fast-forward only");
    assert_eq!(d.app.run.as_ref().unwrap().batch.items[0].op, Op::Pull);
}

#[test]
fn push_has_its_own_review_and_sends_only_the_branch_to_its_origin() {
    let mut f = fixture();
    f.run.answer("status --porcelain=v2 --branch", AHEAD);
    list_with_clone(&mut f, true);
    f.key('U');
    f.wait("the review", |f| f.app.card.is_some());
    assert_eq!(card_title(&f), "Push to the host?");
    let t = card_text(&f);
    assert!(t.contains("Branch main of infra goes to https://git.example.org/ops/infra.git") && t.contains("On Home (Gitea / Forgejo, on the internet)") && t.contains("2 commit(s) ahead") && t.contains("Nothing is forced"), "{t}");
    assert!(f.app.run.is_none());
    f.press(Key::Esc);
    assert!(f.app.card.is_none() && f.app.run.is_none());
    f.key('U');
    f.wait("the review again", |f| f.app.card.is_some());
    f.click("Push");
    let it = &f.app.run.as_ref().unwrap().batch.items[0];
    assert_eq!((it.op, it.folder.as_str(), it.spec.branch.as_str(), it.url.url.as_str()), (Op::Push, ".", "main", "https://git.example.org/ops/infra.git"));
    assert!(f.audit_text().contains("\"event\":\"git.push\",\"detail\":\"git.example.org infra\""), "{}", f.audit_text());
    assert!(!f.audit_text().contains(TOK));
}

#[test]
fn push_with_nothing_to_send_or_no_branch_does_not_ask_anything() {
    let mut f = fixture();
    f.run.answer("status --porcelain=v2 --branch", CLEAN);
    list_with_clone(&mut f, true);
    f.key('U');
    f.wait("the answer", |f| f.app.status.as_ref().is_some_and(|s| s.0.contains("Nothing to push")));
    assert!(f.app.card.is_none() && f.app.run.is_none());
    let mut d = fixture();
    d.run.answer("status --porcelain=v2 --branch", "# branch.oid abc\n# branch.head (detached)\n");
    list_with_clone(&mut d, true);
    d.key('U');
    d.wait("detached", |d| d.app.status.as_ref().is_some_and(|s| s.0.contains("detached")));
    assert!(d.app.card.is_none());
}

#[test]
fn the_actions_are_buttons_too_and_a_repository_has_no_delete_rename_or_settings() {
    let mut f = fixture();
    f.run.answer("status --porcelain=v2 --branch", CLEAN);
    list_with_clone(&mut f, true);
    f.open_row("ops/infra");
    f.draw();
    for b in ["Back", "Open in AI IDE", "Fetch", "Pull", "Push", "Copy clone URL", "Open in browser", "Refresh", "branch or tag"] {
        assert!(f.has_hit(b), "{b}: {:?}", f.labels());
    }
    assert!(!f.has_hit("Clone"), "already cloned: no Clone button");
    for tab in Tab::ALL {
        f.key((b'1' + Tab::ALL.iter().position(|t| *t == tab).unwrap() as u8) as char);
        for bad in ["Delete", "Remove", "Rename", "Archive", "Transfer", "Settings", "Webhook", "Visibility", "Create", "New "] {
            assert!(!f.labels().iter().any(|l| l.contains(bad)), "{tab:?} {bad}: {:?}", f.labels());
        }
    }
    f.click("Fetch");
    assert_eq!(f.app.run.as_ref().unwrap().batch.items[0].op, Op::Fetch);
    // a repository that is not cloned has the Clone button instead
    let mut g = fixture();
    list_with_clone(&mut g, false);
    g.open_row("ops/infra");
    g.draw();
    assert!(g.has_hit("Clone") && !g.has_hit("Fetch") && !g.has_hit("Pull") && !g.has_hit("Push"));
    g.click("Clone");
    assert_eq!(card_title(&g), "Clone this repository?");
}

fn hostkey(seed: u8) -> iyakki_agent::ssh::HostKey {
    let mut blob = Vec::new();
    for part in [&b"ssh-ed25519"[..], &[seed; 32][..]] {
        blob.extend_from_slice(&(part.len() as u32).to_be_bytes());
        blob.extend_from_slice(part);
    }
    iyakki_agent::ssh::HostKey { pattern: "git.example.org".into(), kind: "ssh-ed25519".into(), b64: iyakki_agent::story::base64_encode(&blob) }
}

fn ssh_conn(f: &mut Fx) -> Connection {
    let c = f.app.conns(&f.cx).add(Connection { name: "Keyed".into(), kind: Kind::Gitea, url: "https://git.example.org".into(), method: gitconn::Method::Ssh, key: "k1".into(), ..Default::default() }, None).unwrap();
    f.app.refresh_entries(&f.cx);
    let w = f.wall();
    f.net.set(gitea_host(w));
    f.select_conn(&c.id);
    f.wait("list", |f| f.app.board().loaded);
    c
}

#[test]
fn a_first_time_ssh_host_waits_for_the_persons_decision_showing_its_fingerprint() {
    let mut f = fixture();
    let key = hostkey(7);
    let k2 = key.clone();
    f.app.rig.probe = Some(Arc::new(move |_, _| Ok(vec![k2.clone()])));
    // the probe is part of the batch's own heartbeat; the jobs themselves cannot start here (the fake tools have no ssh)
    f.app.rig.run_jobs = true;
    ssh_conn(&mut f);
    f.key('C');
    assert!(card_text(&f).contains("git@git.example.org:dev/tools.git") && card_text(&f).contains("SSH key k1"), "{}", card_text(&f));
    f.click("Clone");
    assert!(f.app.run.as_ref().unwrap().batch.items[0].url.is_ssh());
    f.wait("the host card", |f| matches!(f.app.card.as_ref().map(|c| &c.act), Some(Act::Host { .. })));
    assert_eq!(card_title(&f), "A new host: git.example.org");
    assert!(card_text(&f).contains(&key.fingerprint()) && card_text(&f).contains("Trust it only if it is the same"), "{}", card_text(&f));
    assert!(f.app.status_line().unwrap().0.contains("waiting for your decision"));
    // refusing sends nothing to the host: the item fails with the reason
    f.click("Do not trust");
    f.wait("the end", |f| f.app.status.as_ref().is_some_and(|s| s.0.contains("failed")));
    assert!(f.app.status.as_ref().unwrap().0.contains("did not trust"), "{:?}", f.app.status);
    // and trusting records the key and lets the job go on
    f.app.run = None;
    f.app.host_busy = false;
    f.app.status = None;
    f.key('C');
    f.click("Clone");
    f.wait("the host card again", |f| matches!(f.app.card.as_ref().map(|c| &c.act), Some(Act::Host { .. })));
    f.click("Trust this host");
    assert!(f.app.card.is_none());
    assert!(f.app.keys(&f.cx).known_hosts().for_host("git.example.org", 22).iter().any(|k| k.b64 == key.b64), "the key is recorded");
    f.wait("past the decision", |f| f.app.status.as_ref().is_some_and(|s| s.0.contains("failed")));
    assert!(f.app.status.as_ref().unwrap().0.contains("no ssh in tests"), "the job went on to start: {:?}", f.app.status);
}

#[test]
fn a_clone_from_an_unreachable_ssh_host_key_probe_fails_with_the_reason_and_no_card() {
    let mut f = fixture();
    f.app.rig.run_jobs = true;
    ssh_conn(&mut f);
    f.key('C');
    f.click("Clone");
    f.wait("the failure", |f| f.app.status.as_ref().is_some_and(|s| s.0.contains("failed")));
    assert!(f.app.status.as_ref().unwrap().0.contains("could not check the host key"), "{:?}", f.app.status);
    assert!(f.app.card.is_none());
}

// ------------------------------------------------------------------------------------------------ safety: the token

fn walk(d: &std::path::Path, out: &mut Vec<PathBuf>) {
    for e in std::fs::read_dir(d).into_iter().flatten().flatten() {
        let p = e.path();
        if p.is_dir() { walk(&p, out) } else { out.push(p) }
    }
}

#[test]
fn a_token_is_in_the_vault_and_the_request_header_only_never_in_an_address_a_log_the_audit_a_card_or_an_error() {
    let mut f = fixture();
    let c = f.conn(Kind::Gitea, "Home", "https://git.example.org");
    // a host that repeats the token in its error words, and a network error that holds it
    let h = Arc::new(FakeGitHttp::default());
    h.answer("user/repos", 200, &[], &repo_list(f.wall()));
    h.answer("/contents", 500, &[], &format!("{{\"message\":\"internal error, token {TOK}\"}}"));
    h.fail("/commits", NetErr::Unreachable(format!("connection to https://me:{TOK}@git.example.org reset")));
    f.net.set(h);
    f.select_conn(&c.id);
    f.wait("list", |f| f.app.board().loaded);
    f.open_row("ops/infra");
    f.wait("the failures", |f| f.view().files.dirs.get("").is_some_and(|d| d.problem().is_some()));
    f.key('2');
    f.wait("the network failure", |f| f.view().commits.slot.problem().is_some());
    f.key('C');
    f.key('L');
    f.draw();
    // everything the tile can say or keep
    let mut seen = vec![f.app.log_text(), f.audit_text(), format!("{:?}", f.app.status), card_text(&f), format!("{:?}", f.net.lines())];
    seen.push(f.view().files.rows.iter().map(|r| r.name.clone()).collect::<Vec<_>>().join("\n"));
    seen.push(f.view().commits.slot.problem().unwrap_or_default());
    for (i, t) in seen.iter().enumerate() {
        assert!(!t.contains(TOK), "#{i} holds the token: {t}");
    }
    assert!(f.app.log_text().contains("transient"), "{}", f.app.log_text());
    // the requests carry it only as the sign-in field the transport puts in a private file
    assert!(f.net.requests().iter().all(|r| r.token.as_deref() == Some(TOK) && !r.url.contains(TOK)));
    // no file on disk holds it in the clear (the vault is encrypted, the connection list holds a name)
    let mut files = Vec::new();
    walk(f.cx.services.dir(), &mut files);
    walk(&f.root, &mut files);
    for p in files {
        let b = std::fs::read(&p).unwrap_or_default();
        assert!(!b.windows(TOK.len()).any(|w| w == TOK.as_bytes()), "{} holds the token", p.display());
    }
}

// ------------------------------------------------------------------------------------------------ keyboard and mouse

#[test]
fn the_keyboard_moves_through_the_list_opens_a_repository_and_goes_back() {
    let mut f = fixture();
    let c = f.conn(Kind::Gitea, "Home", "https://git.example.org");
    f.conn(Kind::GitHub, "Work", "https://github.com");
    f.net.set(full_host(f.wall()));
    f.select_conn(&c.id);
    f.wait("list", |f| f.app.board().loaded);
    assert_eq!(f.app.list.sel, 0);
    f.press(Key::Down);
    f.press(Key::Down);
    f.press(Key::Down);
    assert_eq!(f.app.list.sel, 2, "stops at the last row");
    f.press(Key::Home);
    assert_eq!(f.app.list.sel, 0);
    f.press(Key::End);
    assert_eq!(f.app.list.sel, 2);
    f.press(Key::Up);
    f.press(Key::PageUp);
    assert_eq!(f.app.list.sel, 0);
    f.press(Key::PageDown);
    assert_eq!(f.app.list.sel, 2);
    // c: the connections; Down and Enter choose the next one; Esc closes without choosing
    f.key('c');
    assert_eq!(f.app.overlay, Some(Overlay::Sources));
    f.press(Key::Esc);
    assert!(f.app.overlay.is_none() && f.app.src().name() == "Home");
    f.key('c');
    f.press(Key::Down);
    f.press(Key::Enter);
    assert_eq!(f.app.src().name(), "Work");
    f.select_conn(&c.id);
    f.wait("list again", |f| f.app.board().loaded);
    // Enter opens, digits switch the tabs, Tab moves to the viewer, Esc steps back out, Esc leaves
    f.app.list.sel = f.names().iter().position(|n| n == "ops/infra").unwrap();
    f.press(Key::Enter);
    assert!(f.app.view.is_some());
    f.wait("files", |f| f.view().files.rows.iter().any(|r| r.name == "docs"));
    for (k, t) in [('2', Tab::Commits), ('3', Tab::Branches), ('4', Tab::Issues), ('5', Tab::Info), ('1', Tab::Files)] {
        f.key(k);
        assert_eq!(f.view().tab, t);
    }
    f.press(Key::Down);
    assert_eq!(f.view().files.list.sel, 1);
    f.press(Key::Right);
    f.wait("src", |f| f.view().files.rows.iter().any(|r| r.name == "lib.rs"));
    f.press(Key::Left);
    assert!(!f.view().files.rows.iter().any(|r| r.name == "lib.rs"), "Left closes an open folder");
    f.press(Key::Tab);
    assert_eq!(f.app.focus, Focus::Right);
    f.press(Key::Tab);
    assert_eq!(f.app.focus, Focus::Left);
    f.press(Key::Backspace);
    assert!(f.app.view.is_none());
    assert!(matches!(f.press(Key::Esc), Flow::Back), "Esc in the list leaves the tile");
    // r refreshes, L shows the log, / filters
    f.key('r');
    f.key('L');
    assert!(f.app.show_log);
    f.press(Key::Esc);
    assert!(!f.app.show_log);
}

#[test]
fn the_mouse_opens_rows_sorts_headers_clicks_tabs_and_scrolls_with_the_wheel() {
    let mut f = fixture_with("git-tile-wheel", Role::Admin, 800, 480, 1.0);
    let c = f.conn(Kind::Gitea, "Home", "https://git.example.org");
    let w = f.wall();
    let many: Vec<Value> = (0..200).map(|i| gitea_repo(w, &format!("team/p{i:03}"), "", 60 * (i as u64 + 1), false, 0)).collect();
    let h2 = Arc::new(FakeGitHttp::default());
    h2.answer(&format!("{BASE}/user/repos?limit=50&page=1"), 200, &[], &Value::Array(many).to_string());
    h2.answer(&format!("{BASE}/repos/team/p000/contents?ref=main"), 200, &[], &Value::Array((0..100).map(|i| entry(&format!("f{i:03}.txt"), "file", 5, &format!("f{i:03}.txt"))).collect()).to_string());
    f.net.set(h2);
    f.select_conn(&c.id);
    f.wait("list", |f| f.app.board().loaded);
    f.draw();
    assert!(f.app.list.fit > 0 && f.app.list.fit < 200);
    let (x, y) = (f.app.list.rect.x + 40.0, f.app.list.rect.y + 40.0);
    f.send(Event::MouseMove(x as i32, y as i32));
    assert_eq!(f.app.list.top, 0);
    f.send(Event::Scroll(-1));
    assert_eq!(f.app.list.top, 3, "a wheel notch moves three rows");
    f.send(Event::Scroll(1));
    assert_eq!(f.app.list.top, 0);
    for _ in 0..1000 {
        f.app.handle(&Event::Scroll(-1), &f.cx);
    }
    f.draw();
    assert_eq!(f.app.list.top, 200 - f.app.list.fit, "stops at the end");
    // the click regions follow the rows
    assert!(f.has_hit("team/p199") || f.has_hit("team/p000"), "{:?}", f.labels().len());
    f.click("Name");
    assert_eq!(f.app.sort, Some((0, false)));
    f.send(Event::Scroll(1000));
    f.draw();
    f.click("team/p000");
    assert!(f.app.view.is_some());
    f.wait("files", |f| f.view().files.rows.len() == 100);
    f.draw();
    f.send(Event::MouseMove(f.view().rects.left.x as i32 + 30, f.view().rects.left.y as i32 + 30));
    f.send(Event::Scroll(-1));
    assert_eq!(f.view().files.list.top, 3);
    // the tabs and Back are buttons
    f.click("Commits");
    assert_eq!(f.view().tab, Tab::Commits);
    f.click("Info");
    assert_eq!(f.view().tab, Tab::Info);
    f.click("Back");
    assert!(f.app.view.is_none(), "Back from the repository returns to the list");
}

#[test]
fn a_cut_description_has_a_tooltip_with_all_of_it() {
    let mut f = fixture_with("git-tile-tip", Role::Admin, 800, 480, 1.0);
    let c = f.conn(Kind::Gitea, "Home", "https://git.example.org");
    let w = f.wall();
    let long = "A very long description of the cluster files, the runbooks, the policies and everything that nobody could read in one short line of a table cell in a small window";
    let h = Arc::new(FakeGitHttp::default());
    h.answer(&format!("{BASE}/user/repos?limit=50&page=1"), 200, &[], &Value::Array(vec![gitea_repo(w, "ops/infra", long, 60, true, 3)]).to_string());
    f.net.set(h);
    f.select_conn(&c.id);
    f.wait("list", |f| f.app.board().loaded);
    f.draw();
    let r = f.g.hits.iter().find(|h| h.label == "ops/infra").unwrap().rect;
    f.g.mouse = ((r.x + 60.0) as i32, (r.y + r.h / 2.0) as i32);
    f.send(Event::MouseMove(f.g.mouse.0, f.g.mouse.1));
    for _ in 0..(crate::tooltip::POINTER_DELAY + 3) {
        f.app.handle(&Event::Tick, &f.cx);
    }
    f.draw();
    let shown = f.app.tip.shown.clone().expect("the tooltip appears after the pointer rests");
    assert!(shown.contains("ops/infra") && shown.contains("nobody could read"), "{shown}");
    // it goes at once on a click or a key
    f.press(Key::Down);
    f.draw();
    assert!(f.app.tip.shown.is_none(), "a key puts the tooltip away");
}

// ------------------------------------------------------------------------------------------------ layout

fn assert_hits_inside(f: &Fx, what: &str) {
    let a = f.area;
    assert!(!f.g.hits.is_empty(), "{what}: nothing is clickable");
    for h in &f.g.hits {
        let r = h.rect;
        assert!(r.x >= a.x - 0.5 && r.y >= a.y - 0.5 && r.x + r.w <= a.x + a.w + 0.5 && r.y + r.h <= a.y + a.h + 0.5, "{what}: `{}` {:?} is outside the app {:?}", h.label, r, a);
    }
}

#[test]
fn it_draws_at_every_size_with_every_click_region_inside_the_app() {
    for (w, h, s) in [(800u32, 480u32, 1.0f32), (1280, 720, 1.0), (1920, 1080, 1.5), (360, 740, 1.0)] {
        let mut f = fixture_with("git-tile-sizes", Role::Admin, w, h, s);
        // first run
        f.draw();
        assert_hits_inside(&f, &format!("{w}x{h} first run"));
        open_infra(&mut f);
        f.mk_clone("infra", "https://git.example.org/ops/infra.git", "main");
        f.app.view = None;
        f.draw();
        assert_hits_inside(&f, &format!("{w}x{h} list"));
        let expect = if w >= 1000 { Mode::Wide } else if w >= 620 { Mode::Medium } else { Mode::Phone };
        assert_eq!(f.app.mode, expect, "{w}x{h}");
        f.click("connection");
        assert_hits_inside(&f, &format!("{w}x{h} connections"));
        f.press(Key::Esc);
        f.key('C');
        assert_hits_inside(&f, &format!("{w}x{h} clone card"));
        f.press(Key::Esc);
        f.key('L');
        assert_hits_inside(&f, &format!("{w}x{h} log"));
        f.press(Key::Esc);
        f.open_row("ops/infra");
        f.wait("root", |f| f.view().files.rows.iter().any(|r| r.name == "docs"));
        f.wait("README", |f| f.view().files.file.as_ref().is_some_and(|x| matches!(x.shown, Shown::Md)));
        assert_hits_inside(&f, &format!("{w}x{h} files"));
        // open a file; on a phone the viewer is a page of its own
        let i = frow(&f, "main.rs");
        f.code(T_FROW + i);
        f.wait("the file", |f| f.view().files.file.as_ref().is_some_and(|x| x.name == "main.rs" && matches!(x.shown, Shown::Text)));
        assert_hits_inside(&f, &format!("{w}x{h} viewer"));
        if expect == Mode::Phone {
            assert!(f.view().page_detail && f.has_hit("Back"));
        }
        for (k, what) in [('2', "commits"), ('3', "branches"), ('4', "issues"), ('5', "info"), ('1', "files")] {
            f.key(k);
            f.wait(what, |f| match f.view().tab {
                Tab::Commits => f.view().commits.slot.ready().is_some(),
                Tab::Branches => f.view().refs.branches.ready().is_some() && f.view().refs.tags.ready().is_some(),
                Tab::Issues => f.view().issues.issues.ready().is_some(),
                _ => true,
            });
            assert_hits_inside(&f, &format!("{w}x{h} {what}"));
        }
        f.key('2');
        f.press(Key::Enter);
        f.wait("the diff", |f| f.view().commits.diff.ready().is_some());
        assert_hits_inside(&f, &format!("{w}x{h} diff"));
        f.key('g');
        assert_hits_inside(&f, &format!("{w}x{h} branch chooser"));
        f.press(Key::Esc);
        f.key('w');
        assert_hits_inside(&f, &format!("{w}x{h} browser card"));
        f.press(Key::Esc);
        assert!(f.g.rgba().iter().any(|b| *b != 0));
    }
}

#[test]
fn it_works_inside_a_pane_of_half_the_screen() {
    let mut f = fixture_with("git-tile-pane", Role::Admin, 1280, 720, 1.0);
    f.area = R::new(640.0, 40.0, 640.0, 680.0);
    open_infra(&mut f);
    f.draw();
    assert_eq!(f.app.mode, Mode::Medium);
    assert_hits_inside(&f, "right pane");
    f.area = R::new(0.0, 40.0, 400.0, 400.0);
    f.draw();
    assert_eq!(f.app.mode, Mode::Phone);
    assert_hits_inside(&f, "narrow pane");
    f.area = R::new(0.0, 0.0, 800.0, 240.0);
    f.draw();
    assert_hits_inside(&f, "short pane");
}

// ------------------------------------------------------------------------------------------------ real git, local files only

fn git_in(dir: &std::path::Path, args: &[&str]) {
    let o = std::process::Command::new("git").arg("-C").arg(dir).args(["-c", "user.name=Test", "-c", "user.email=test@example.org", "-c", "init.defaultBranch=main", "-c", "commit.gpgsign=false"]).args(args).output().expect("git runs");
    assert!(o.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&o.stderr));
}

/// The jobs and the reads are real here, on a repository made in a temp folder: no network, no host.
#[test]
fn a_real_local_repository_is_cloned_by_the_jobs_listed_and_browsed_with_real_git() {
    let git = match gitjobs::check_git(&std::env::var("PATH").unwrap_or_default()) {
        gitjobs::GitCheck::Ready { path, .. } => path,
        _ => {
            eprintln!("git is not installed here: this test is skipped");
            return;
        }
    };
    let mut f = fixture_with("git-tile-real", Role::Admin, 1280, 720, 1.0);
    let base = f.cx.services.dir().join("real");
    let (work, bare) = (base.join("work"), base.join("bare/app.git"));
    std::fs::create_dir_all(&work).unwrap();
    git_in(&work, &["init", "-q"]);
    std::fs::write(work.join("README.md"), "# Real\n\nA real repository.\n").unwrap();
    std::fs::create_dir_all(work.join("src")).unwrap();
    std::fs::write(work.join("src/lib.rs"), "pub fn real() {}\n").unwrap();
    std::fs::write(work.join("logo.png"), tiny_png()).unwrap();
    git_in(&work, &["add", "-A"]);
    git_in(&work, &["commit", "-q", "-m", "First real commit\n\nWith a body."]);
    git_in(&work, &["tag", "v1"]);
    std::fs::create_dir_all(bare.parent().unwrap()).unwrap();
    let o = std::process::Command::new("git").args(["clone", "-q", "--bare"]).arg(&work).arg(&bare).output().unwrap();
    assert!(o.status.success());
    f.app.rig.run = Arc::new(iyakki_agent::wsexec::System);
    f.app.rig.run_jobs = true;
    f.app.rig.tools = Some(Tools { git, ssh: Err("no ssh in tests".into()), allow_local: true });
    // clone it through the tile's own job path
    let conn = Connection { name: "Local test".into(), kind: Kind::Gitea, url: "https://git.example.org".into(), ..Default::default() };
    f.app.start_clone(&f.cx.clone(), &conn, "me/app", &bare.display().to_string(), false, false);
    assert!(f.app.run.is_some(), "{:?}", f.app.status);
    f.wait("the clone", |f| f.app.status.as_ref().is_some_and(|s| s.0.starts_with("Cloned") || s.0.contains("failed")));
    assert!(f.app.status.as_ref().unwrap().0.starts_with("Cloned me/app"), "{:?}", f.app.status);
    assert!(f.root.join("app/.git").exists());
    // it is listed among the clones, with its branch and its remote
    f.select_conn("local");
    f.wait("listed", |f| f.app.locals.list.iter().any(|l| l.name == "app"));
    f.app.locals.want = true;
    f.wait("read again", |f| f.app.locals.loaded && f.app.locals.job.is_none() && !f.app.locals.want);
    let cl = f.app.locals.list.iter().find(|l| l.name == "app").unwrap().clone();
    assert_eq!(cl.branch, "main");
    assert_eq!(cl.remote.as_deref(), Some(bare.display().to_string().as_str()));
    f.open_row("app");
    f.wait("the files", |f| f.view().files.rows.iter().any(|r| r.name == "README.md"));
    assert_eq!(f.view().files.rows.iter().map(|r| r.name.clone()).collect::<Vec<_>>(), vec!["src", "logo.png", "README.md"]);
    f.wait("the README", |f| f.view().files.file.as_ref().is_some_and(|x| matches!(x.shown, Shown::Md)));
    // a text file in a folder, a picture read out of git without touching the working tree
    let i = frow(&f, "src");
    f.code(T_FROW + i);
    f.wait("src", |f| f.view().files.rows.iter().any(|r| r.name == "lib.rs"));
    let i = frow(&f, "lib.rs");
    f.code(T_FROW + i);
    f.wait("lib.rs", |f| f.view().files.file.as_ref().is_some_and(|x| x.name == "lib.rs" && matches!(x.shown, Shown::Text)));
    assert_eq!(f.view().files.file.as_ref().unwrap().text.lines[0].text, "    1  pub fn real() {}");
    let i = frow(&f, "logo.png");
    f.code(T_FROW + i);
    f.wait("the picture", |f| f.view().files.file.as_ref().is_some_and(|x| x.name == "logo.png" && matches!(x.shown, Shown::Image(_))));
    // commits and the diff of the root commit, branches and tags
    f.key('2');
    f.wait("commits", |f| f.view().commits.slot.ready().is_some());
    assert_eq!(f.view().commits.slot.ready().unwrap()[0].subject(), "First real commit");
    f.press(Key::Enter);
    f.wait("the diff", |f| f.view().commits.diff.ready().is_some());
    let names: Vec<String> = f.view().commits.diff.ready().unwrap().files.iter().map(|d| d.name().to_string()).collect();
    assert!(names.contains(&"README.md".to_string()) && names.contains(&"src/lib.rs".to_string()), "{names:?}");
    f.key('3');
    f.wait("refs", |f| f.view().refs.branches.ready().is_some() && f.view().refs.tags.ready().is_some());
    assert!(f.view().ref_rows().iter().any(|r| r.0 == "main" && r.2) && f.view().ref_rows().iter().any(|r| r.0 == "v1" && r.1));
    // nothing in the clone changed by looking: still clean, no stray index or files
    let st = std::process::Command::new("git").arg("-C").arg(f.root.join("app")).args(["status", "--porcelain"]).output().unwrap();
    assert!(st.stdout.is_empty(), "{}", String::from_utf8_lossy(&st.stdout));
    // a fetch is a real job too
    f.press(Key::Esc);
    f.press(Key::Esc);
    f.app.list.sel = 0;
    f.key('F');
    f.wait("the fetch", |f| f.app.status.as_ref().is_some_and(|s| s.0.starts_with("Fetched") || s.0.contains("failed")));
    assert!(f.app.status.as_ref().unwrap().0.starts_with("Fetched app"), "{:?}", f.app.status);
    // a pull of a clone that is up to date also goes through
    f.key('P');
    f.wait("the pull", |f| f.app.status.as_ref().is_some_and(|s| s.0.starts_with("Pulled") || s.0.contains("failed")));
    assert!(f.app.status.as_ref().unwrap().0.starts_with("Pulled app"), "{:?}", f.app.status);
}
