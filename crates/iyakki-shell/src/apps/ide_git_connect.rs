//! The AI IDE's "Connect Git / Gitea": connect the open project to a git host. Pick one of the connections made in Settings → Git (or add
//! one here, in place: the same wizard), then either **create a new repository** on that host (name from the project, private by default,
//! shown in a review first), set `origin`, make the first commit if there is none, and push after a second review, or **connect to an
//! existing repository** (pick it from the host's list or type its address) and set `origin` without pushing.
//!
//! The work is in `iyakki_agent::gitconn` (the host's API, `origin`, the first commit) and, for the Gitea of the person's cluster,
//! `iyakki_agent::platform::connect` (the same plan, review and token handling as Services → Connect workspace). The push itself is a
//! normal git job (`gitjobs`) started by `ide_git.rs`, with the host-key card, the key service and the token through askpass.
//! Nothing here puts a token in a command line, a log or a message.

use std::path::PathBuf;
use std::sync::mpsc::{channel, Receiver};
use std::sync::Arc;

use iyakki_agent::gitconn::{self, Connection, Kind, Method, RepoInfo, WsGit};
use iyakki_agent::platform::connect::{self as pconnect, ConnectOpts, Transport, WsInfo};
use iyakki_agent::platform::Platform;
use iyakki_agent::wsexec::Runner;

use super::git_wizard::{WizEnv, WIZ_BASE};
use crate::fx::{self, AMBER};
use crate::gfx::{Face, Gfx};
use crate::ui::{Event, Key, TextField, DIM, ERR, MUTED, OK, R, TEXT};

/// Click ids of this flow (next to the wizard's).
pub const FLOW_BASE: usize = WIZ_BASE + 1000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stage {
    /// The connections to choose from (the last row adds one).
    Pick,
    /// Create a new repository, or connect to an existing one.
    Choose,
    CreateReview,
    Existing,
    /// Something is running (the host's API, git).
    Working,
    /// A repository is connected; push it (review) or close.
    Connected,
    PushReview,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FlowOut {
    None,
    Close,
    /// Open the wizard in place.
    AddConnection,
    OpenPlatform,
    /// Push the branch of the project to `origin` (the review was confirmed).
    Push { remote: String, branch: String, host: String, repo: String },
    /// Something changed in the project's git setup (the panel's header reads it again).
    Changed,
}

enum Msg {
    Repos(Result<Vec<RepoInfo>, String>),
    Created(Result<Done, String>),
}

#[derive(Clone, Debug)]
struct Done {
    remote: String,
    full: String,
    committed: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Slot {
    Name,
    Private,
    Commit,
}

pub struct ConnectFlow {
    pub env: WizEnv,
    run: Arc<dyn Runner>,
    platform: Option<Arc<Platform>>,
    pub ws_name: String,
    root: PathBuf,
    pub state: WsGit,
    pub conns: Vec<Connection>,
    pub sel: usize,
    pub chosen: Option<Connection>,
    pub stage: Stage,
    name: TextField,
    pub private: bool,
    pub commit: bool,
    focus: usize,
    pub repos: Vec<RepoInfo>,
    filter: TextField,
    repo_sel: usize,
    pub repos_note: Option<String>,
    listing: bool,
    /// An existing repository was picked: the remote that will be set, shown for a second Enter.
    pub confirm: Option<(String, String)>,
    rx: Option<Receiver<Msg>>,
    pub error: Option<String>,
    pub message: Option<String>,
    /// What was made or connected: (remote, `owner/repo`, whether the first commit was made).
    done: Option<Done>,
}

/// The git side of a project and what it can do with a connection (shared by the IDE and its tests).
pub struct FlowEnv {
    pub wiz: WizEnv,
    pub run: Arc<dyn Runner>,
    pub platform: Option<Arc<Platform>>,
}

impl ConnectFlow {
    pub fn new(e: FlowEnv, ws_name: &str, root: PathBuf, pick: Option<&str>) -> ConnectFlow {
        let conns_store = e.wiz.keys.conns();
        let conns = conns_store.list();
        let state = gitconn::ws_git(&*e.run, &root);
        let name = {
            let mut t = TextField::new("lower case: letters, digits, - _ .", false);
            t.set(&gitconn::repo_name(ws_name));
            t
        };
        let mut f = ConnectFlow {
            env: e.wiz,
            run: e.run,
            platform: e.platform,
            ws_name: ws_name.to_string(),
            root,
            commit: state.commits == 0,
            state,
            sel: conns.iter().position(|c| c.default).unwrap_or(0),
            conns,
            chosen: None,
            stage: Stage::Pick,
            name,
            private: true,
            focus: 0,
            repos: vec![],
            filter: TextField::new("type to search, or paste an address or owner/repo", false),
            repo_sel: 0,
            repos_note: None,
            listing: false,
            confirm: None,
            rx: None,
            error: None,
            message: None,
            done: None,
        };
        if let Some(id) = pick {
            if let Some(i) = f.conns.iter().position(|c| c.id == id) {
                f.sel = i;
                f.chosen = Some(f.conns[i].clone());
                f.stage = Stage::Choose;
            }
        }
        f
    }

    pub fn busy(&self) -> bool {
        self.rx.is_some()
    }

    /// Straight to the push review of the project's own repository (the panel's Push button); an error says what is missing.
    pub fn start_at_push(&mut self) {
        self.chosen = None;
        self.begin_push();
    }

    pub fn set_name(&mut self, n: &str) {
        self.name.set(n);
    }
    pub fn set_filter(&mut self, n: &str) {
        self.filter.set(n);
    }
    pub fn repo_name(&self) -> String {
        self.name.value.trim().to_string()
    }

    fn conn(&self) -> Option<&Connection> {
        self.chosen.as_ref()
    }

    fn cluster(&self) -> bool {
        self.conn().is_some_and(|c| !c.cluster.is_empty())
    }

    fn reread(&mut self) {
        self.state = gitconn::ws_git(&*self.run, &self.root);
    }

    // ------------------------------------------------------------------ events

    pub fn poll(&mut self) -> bool {
        let Some(rx) = &self.rx else { return false };
        let Ok(m) = rx.try_recv() else { return false };
        self.rx = None;
        match m {
            Msg::Repos(r) => {
                self.listing = false;
                match r {
                    Ok(v) => {
                        self.repos_note = v.is_empty().then(|| "This account has no repositories on the host yet.".to_string());
                        self.repos = v;
                    }
                    Err(e) => self.repos_note = Some(format!("{e} You can still type the address of the repository.")),
                }
            }
            Msg::Created(r) => match r {
                Ok(d) => {
                    self.reread();
                    self.message = Some(format!("Created {} and set origin to {}.{}", d.full, gitsite_redact(&d.remote), if d.committed { " The first commit was made." } else { "" }));
                    self.done = Some(d);
                    self.stage = Stage::Connected;
                    self.error = None;
                }
                Err(e) => {
                    self.error = Some(e);
                    self.stage = Stage::CreateReview;
                }
            },
        }
        true
    }

    /// Runs until the background work is done (tests and scripted use).
    pub fn wait(&mut self) {
        for _ in 0..400 {
            self.poll();
            if self.rx.is_none() {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
    }

    fn slots(&self) -> Vec<Slot> {
        let mut v = vec![Slot::Name, Slot::Private];
        if self.state.commits == 0 {
            v.push(Slot::Commit);
        }
        v
    }

    fn choose_connection(&mut self) {
        let Some(c) = self.conns.get(self.sel).cloned() else { return };
        self.chosen = Some(c);
        self.stage = Stage::Choose;
        self.error = None;
        self.message = None;
    }

    pub fn event(&mut self, ev: &Event) -> FlowOut {
        match ev {
            Event::Tick => {
                self.poll();
                FlowOut::None
            }
            Event::Paste(t) => {
                match self.stage {
                    Stage::CreateReview if self.slots().get(self.focus) == Some(&Slot::Name) => {
                        self.name.paste(t);
                    }
                    Stage::Existing => {
                        self.filter.paste(t);
                        self.repo_sel = 0;
                        self.confirm = None;
                    }
                    _ => {}
                }
                FlowOut::None
            }
            Event::Row(i) => {
                match self.stage {
                    Stage::Pick => self.sel = (*i).min(self.conns.len()),
                    Stage::Existing => self.repo_sel = *i,
                    _ => {}
                }
                FlowOut::None
            }
            Event::Tab(t) if *t >= FLOW_BASE && *t < FLOW_BASE + 100 => self.button(t - FLOW_BASE),
            Event::Key(k) => self.key(k),
            _ => FlowOut::None,
        }
    }

    fn button(&mut self, n: usize) -> FlowOut {
        match n {
            0 => FlowOut::Close,
            1 => {
                self.back();
                FlowOut::None
            }
            2 => self.key(&Key::Ctrl('s')),
            3 => {
                self.private = !self.private;
                FlowOut::None
            }
            4 => {
                if self.stage == Stage::Choose {
                    self.begin_create()
                }
                FlowOut::None
            }
            5 => {
                if self.stage == Stage::Choose {
                    self.begin_existing()
                }
                FlowOut::None
            }
            6 => self.begin_push(),
            7 => FlowOut::OpenPlatform,
            _ => FlowOut::None,
        }
    }

    pub fn back(&mut self) {
        self.error = None;
        self.confirm = None;
        self.stage = match self.stage {
            Stage::Pick | Stage::Working => self.stage,
            Stage::Choose => Stage::Pick,
            Stage::CreateReview | Stage::Existing => Stage::Choose,
            Stage::Connected => Stage::Connected,
            Stage::PushReview => Stage::Connected,
        };
    }

    fn begin_create(&mut self) {
        let Some(c) = self.conn().cloned() else { return };
        if !c.kind.has_api() {
            self.error = Some("This kind of host has no API to make a repository: make it on the site, then connect to it.".into());
            return;
        }
        if !self.cluster() && !self.env.keys.conns().token_present(&c) {
            self.error = Some("Making a repository needs a token for this host: add one in Settings > Git (Edit).".into());
            return;
        }
        self.focus = 0;
        self.error = None;
        self.stage = Stage::CreateReview;
    }

    fn begin_existing(&mut self) {
        let Some(c) = self.conn().cloned() else { return };
        self.stage = Stage::Existing;
        self.error = None;
        self.confirm = None;
        self.repos.clear();
        self.repo_sel = 0;
        self.repos_note = None;
        if !c.kind.has_api() {
            self.repos_note = Some("This kind of host has no list of repositories: type the address of the repository.".into());
            return;
        }
        let conns = self.env.keys.conns();
        let Ok(token) = conns.token(&c) else {
            self.repos_note = Some("No token for this host, so its repositories cannot be listed. Type the address of the repository.".into());
            return;
        };
        if c.cluster.is_empty() {
            if let Err(e) = (self.env.egress)(&c.host()) {
                self.repos_note = Some(format!("{e} You can still type the address of the repository."));
                return;
            }
        }
        let (http, cert, token) = (self.env.http.clone(), conns.pinned_cert(&c), token.to_string());
        let (tx, rx) = channel();
        std::thread::spawn(move || {
            let _ = tx.send(Msg::Repos(gitconn::list_repos(&*http, &c, &token, cert.as_ref())));
        });
        self.listing = true;
        self.rx = Some(rx);
    }

    fn visible_repos(&self) -> Vec<&RepoInfo> {
        let q = self.filter.value.trim().to_lowercase();
        self.repos.iter().filter(|r| q.is_empty() || r.full_name.to_lowercase().contains(&q)).collect()
    }

    /// The remote the Existing step would set: a typed address or `owner/repo`, or the chosen row.
    fn existing_target(&self) -> Result<(String, String), String> {
        let c = self.conn().ok_or("Choose a connection first.")?;
        let q = self.filter.value.trim().to_string();
        let typed = q.contains("://") || q.contains('@') || (q.contains('/') && !q.contains(' ') && self.visible_repos().is_empty());
        if typed {
            let remote = if q.contains("://") || q.contains('@') { q.clone() } else { c.remote_for(q.trim_end_matches(".git")) };
            let u = iyakki_agent::gitsite::parse(&remote, Default::default())?;
            return Ok((u.url.clone(), u.short()));
        }
        let list = self.visible_repos();
        let r = list.get(self.repo_sel).ok_or("Choose a repository from the list, or type its address.")?;
        Ok((if c.method == Method::Ssh { r.ssh_url.clone() } else { r.https_url.clone() }, r.full_name.clone()))
    }

    fn begin_push(&mut self) -> FlowOut {
        if self.state.origin.is_none() {
            self.error = Some("This project has no origin yet.".into());
            return FlowOut::None;
        }
        if self.state.commits == 0 {
            self.error = Some("There is no commit to push yet: make the first commit (Create a new repository offers it).".into());
            return FlowOut::None;
        }
        if self.state.branch.is_empty() {
            self.error = Some("HEAD is detached: check out a branch first.".into());
            return FlowOut::None;
        }
        self.stage = Stage::PushReview;
        self.error = None;
        FlowOut::None
    }

    fn key(&mut self, k: &Key) -> FlowOut {
        if self.stage == Stage::Working {
            return if matches!(k, Key::Esc) { FlowOut::Close } else { FlowOut::None };
        }
        match (self.stage, k) {
            (_, Key::Esc) => return FlowOut::Close,
            (Stage::Pick, Key::Up) => self.sel = self.sel.saturating_sub(1),
            (Stage::Pick, Key::Down) => self.sel = (self.sel + 1).min(self.conns.len()),
            (Stage::Pick, Key::Char(c @ '1'..='9')) if (*c as usize - '1' as usize) <= self.conns.len() => self.sel = *c as usize - '1' as usize,
            (Stage::Pick, Key::Char('a')) => return FlowOut::AddConnection,
            (Stage::Pick, Key::Enter | Key::Ctrl('s')) => {
                if self.sel >= self.conns.len() {
                    return FlowOut::AddConnection;
                }
                self.choose_connection();
            }
            (Stage::Choose, Key::Char('1') | Key::Char('n')) => self.begin_create(),
            (Stage::Choose, Key::Char('2') | Key::Char('e')) => self.begin_existing(),
            (Stage::Choose, Key::PageUp) => self.back(),
            (Stage::Choose, Key::Char('p')) if self.state.origin.is_some() => return self.begin_push(),
            (Stage::CreateReview, Key::PageUp) => self.back(),
            (Stage::CreateReview, Key::Ctrl('s')) => return self.create(),
            (Stage::CreateReview, Key::Enter) => {
                let n = self.slots().len();
                if self.focus + 1 < n {
                    self.focus += 1;
                } else {
                    return self.create();
                }
            }
            (Stage::CreateReview, Key::Tab | Key::Down) => self.focus = (self.focus + 1) % self.slots().len(),
            (Stage::CreateReview, Key::BackTab | Key::Up) => self.focus = (self.focus + self.slots().len() - 1) % self.slots().len(),
            (Stage::CreateReview, Key::Left | Key::Right | Key::Char(' ')) if self.slots().get(self.focus) == Some(&Slot::Private) => self.private = !self.private,
            (Stage::CreateReview, Key::Left | Key::Right | Key::Char(' ')) if self.slots().get(self.focus) == Some(&Slot::Commit) => self.commit = !self.commit,
            (Stage::CreateReview, other) if self.slots().get(self.focus) == Some(&Slot::Name) => {
                self.name.handle(other);
            }
            (Stage::Existing, Key::PageUp) => self.back(),
            (Stage::Existing, Key::Up) => {
                self.repo_sel = self.repo_sel.saturating_sub(1);
                self.confirm = None;
            }
            (Stage::Existing, Key::Down) => {
                self.repo_sel = (self.repo_sel + 1).min(self.visible_repos().len().saturating_sub(1));
                self.confirm = None;
            }
            (Stage::Existing, Key::Enter | Key::Ctrl('s')) => return self.existing_enter(),
            (Stage::Existing, other) => {
                if self.filter.handle(other) {
                    self.repo_sel = 0;
                    self.confirm = None;
                    self.error = None;
                }
            }
            (Stage::Connected, Key::Char('p') | Key::Enter) => return self.begin_push(),
            (Stage::PushReview, Key::PageUp) => self.back(),
            (Stage::PushReview, Key::Enter | Key::Ctrl('s')) => return self.push_out(),
            _ => {}
        }
        FlowOut::None
    }

    fn existing_enter(&mut self) -> FlowOut {
        let Ok((remote, repo)) = self.existing_target().map_err(|e| self.error = Some(e)) else { return FlowOut::None };
        if self.confirm.as_ref() != Some(&(remote.clone(), repo.clone())) {
            // the first Enter shows what will happen, the second does it
            self.confirm = Some((remote, repo));
            self.error = None;
            return FlowOut::None;
        }
        let Some(c) = self.conn().cloned() else { return FlowOut::None };
        if !self.state.is_repo {
            if let Err(e) = gitconn::git_init(&*self.run, &self.root) {
                self.error = Some(e);
                return FlowOut::None;
            }
        }
        if let Err(e) = gitconn::set_origin(&*self.run, &self.root, &remote, true) {
            self.error = Some(e);
            return FlowOut::None;
        }
        gitconn::apply_identity(&*self.run, &self.root, &c);
        self.env.keys.conns().audit("origin.set", &format!("{} {repo}", c.host()));
        self.reread();
        self.done = Some(Done { remote: remote.clone(), full: repo.clone(), committed: false });
        self.message = Some(format!("Connected to {repo}: origin is {}. Nothing was pushed or fetched.", gitsite_redact(&remote)));
        self.confirm = None;
        self.stage = Stage::Connected;
        FlowOut::Changed
    }

    fn push_out(&mut self) -> FlowOut {
        let Some(remote) = self.state.origin.clone() else { return FlowOut::None };
        let u = iyakki_agent::gitsite::parse(&remote, Default::default());
        match u {
            Ok(u) => FlowOut::Push { remote: u.url.clone(), branch: self.state.branch.clone(), host: u.host.clone(), repo: u.short() },
            Err(e) => {
                self.error = Some(e);
                FlowOut::None
            }
        }
    }

    /// The review was confirmed: make the repository (in the background), then `origin`, then the first commit.
    fn create(&mut self) -> FlowOut {
        let Some(c) = self.conn().cloned() else { return FlowOut::None };
        let name = self.repo_name();
        if !gitconn::valid_repo_name(&name) {
            self.error = Some(format!("`{name}` is not a repository name (letters, digits, - _ . only)."));
            return FlowOut::None;
        }
        if c.cluster.is_empty() {
            if let Err(e) = (self.env.egress)(&c.host()) {
                self.error = Some(e);
                return FlowOut::None;
            }
        }
        if self.state.commits == 0 && !self.commit {
            // nothing to push: allowed, the repository and origin are still made
        }
        let conns = self.env.keys.conns();
        let (http, run, root, state, private, commit, platform) = (self.env.http.clone(), self.run.clone(), self.root.clone(), self.state.clone(), self.private, self.commit && self.state.commits == 0, self.platform.clone());
        let (dir, user, ws_name) = (self.env.dir.clone(), self.env.user.clone(), self.ws_name.clone());
        let (tx, rx) = channel();
        std::thread::spawn(move || {
            let r = make_repository(&conns, &*http, &run, platform.as_deref(), &dir, &user, &ws_name, &root, &state, &c, &name, private, commit);
            let _ = tx.send(Msg::Created(r));
        });
        self.rx = Some(rx);
        self.stage = Stage::Working;
        self.error = None;
        FlowOut::None
    }

    // ------------------------------------------------------------------ the review texts

    /// The lines of the create review: what will be done, in order, before anything is.
    pub fn create_review(&self) -> Vec<String> {
        let Some(c) = self.conn() else { return vec![] };
        let name = self.repo_name();
        let mut v = vec![format!("Host: {} ({}), account {}.", c.url, c.placement().label(), if c.account.is_empty() { "not tested yet".to_string() } else { c.account.clone() })];
        let (path, _) = gitconn::create_body(c.kind, &name, self.private, "");
        v.push(format!("1. Create the repository {name} ({}): POST {}{}", if self.private { "PRIVATE" } else { "PUBLIC: anyone who can reach the host may read it" }, gitconn::api_base(c.kind, &c.url), path));
        let mut n = 2;
        if !self.state.is_repo {
            v.push(format!("{n}. This folder is not a git repository yet: git init (branch main)."));
            n += 1;
        }
        if self.state.commits == 0 {
            v.push(if self.commit { self.first_commit_text(n) } else { format!("{n}. No first commit (a push needs one: do it later).") });
            n += 1;
            if self.commit {
                let risky = secret_looking(&self.root);
                if !risky.is_empty() {
                    v.push(format!("Warning: {} look like secrets and are committed unless .gitignore lists them. Add them to .gitignore first, or choose No for the first commit.", risky.join(", ")));
                }
            }
        }
        v.push(format!("{n}. Set origin to the new repository's {} address (no credential in it).", if c.method == Method::Ssh { "SSH" } else { "HTTPS" }));
        if let Some(o) = &self.state.origin {
            v.push(format!("   origin is now {}: it will be REPLACED.", gitsite_redact(o)));
        }
        v.push("Nothing is pushed by this step: the push has its own review next.".into());
        if c.url.starts_with("http://") {
            v.push("Warning: the host speaks plain http, so the token and your code travel unencrypted.".into());
        }
        v
    }

    fn first_commit_text(&self, n: usize) -> String {
        if self.state.is_repo {
            format!("{n}. Make the first commit of the {} file(s) git does not ignore.", self.state.changed)
        } else {
            format!("{n}. Make the first commit of the project's files (git's own ignore rules apply: .gitignore is honoured).")
        }
    }

    pub fn push_review(&self) -> Vec<String> {
        let Some(origin) = &self.state.origin else { return vec![] };
        let c = self.conns.iter().find(|c| iyakki_agent::gitsite::parse(origin, Default::default()).is_ok_and(|u| gitconn::host_of(&c.url) == u.host)).or(self.conn());
        let how = match c {
            Some(c) if c.method == Method::Ssh => format!("With your SSH key {} (through the key service; a new host's fingerprint is shown first).", c.key),
            Some(c) => format!("With the token of the connection {} (handed to git through the key service, never in the address or a command).", c.name),
            None => "With whatever credential the host table has for it.".into(),
        };
        let mut v = vec![format!("Remote: {}", gitsite_redact(origin)), format!("Branch: {} ({} commit(s))", self.state.branch, self.state.commits), format!("Command: git push --set-upstream origin {}", self.state.branch), how];
        if let Some(c) = c {
            v.push(format!("Your code goes to {} ({}).", c.host(), c.placement().label()));
        }
        if self.state.changed > 0 {
            v.push(format!("{} changed file(s) are not committed and are not part of the push.", self.state.changed));
        }
        v
    }

    // ------------------------------------------------------------------ drawing

    pub fn render(&mut self, g: &mut Gfx, area: R, s: f32, accent: [u8; 4], blink: bool) {
        self.poll();
        let p = fx::modal(g, area, s, 820.0 * s, 600.0 * s, accent, false);
        let (x, w) = (p.x + 26.0 * s, p.w - 52.0 * s);
        let title = match self.stage {
            Stage::Pick => "Connect Git / Gitea".to_string(),
            Stage::PushReview => "Push: review".to_string(),
            _ => format!("Connect Git / Gitea: {}", self.conn().map(|c| c.name.as_str()).unwrap_or("")),
        };
        fx::modal_title(g, p, s, &title, accent);
        let mut y = p.y + 62.0 * s;
        let bottom = p.y + p.h - 70.0 * s;
        let line = |g: &mut Gfx, y: &mut f32, col: [u8; 4], t: &str| {
            for l in g.wrap(13.0 * s, t, Face::Sans, w) {
                if *y < bottom {
                    g.text(x, *y, 13.0 * s, col, &l, Face::Sans);
                }
                *y += 18.0 * s;
            }
        };
        let mut buttons: Vec<(String, [u8; 4], Event)> = vec![];
        match self.stage {
            Stage::Pick => {
                let open = self.state.origin.as_ref().map(|o| gitsite_redact(o));
                line(g, &mut y, MUTED, &match open {
                    Some(o) => format!("This project already has an origin ({o}). Choosing a connection below can replace it, after a review."),
                    None => "This project is not connected to Git. Choose a connection, or add one.".to_string(),
                });
                y += 6.0 * s;
                let max_rows = ((bottom - y) / (52.0 * s)).max(1.0) as usize;
                let first = self.sel.saturating_sub(max_rows - 1);
                for i in first..(self.conns.len() + 1).min(first + max_rows) {
                    let (t, sub) = match self.conns.get(i) {
                        Some(c) => (format!("{}  {}{}", i + 1, c.name, if c.default { "  (default)" } else { "" }), format!("{} · {} · {} · {}", c.kind.label(), c.url, if c.account.is_empty() { "not tested".to_string() } else { c.account.clone() }, c.placement().label())),
                        None => ("Add connection…".to_string(), "GitHub, GitLab, Gitea or Forgejo, any other git host".to_string()),
                    };
                    self.row(g, x, y, w, s, accent, i == self.sel, i, &t, &sub);
                    y += 52.0 * s;
                }
                buttons.push(("Choose".into(), accent, Event::Tab(FLOW_BASE + 2)));
            }
            Stage::Choose => {
                let c = self.conn().cloned().unwrap_or_default();
                line(g, &mut y, MUTED, &format!("{} at {} ({}), account {}.", c.kind.label(), c.url, c.placement().label(), if c.account.is_empty() { "not tested yet" } else { &c.account }));
                y += 8.0 * s;
                self.row(g, x, y, w, s, accent, false, 100, "1  Create a new repository", &format!("named {} on this host, private by default; you review it first", gitconn::repo_name(&self.ws_name)));
                g.hit(R::new(x, y, w, 46.0 * s), Event::Tab(FLOW_BASE + 4), "Create a new repository");
                y += 52.0 * s;
                self.row(g, x, y, w, s, accent, false, 101, "2  Connect to an existing repository", "pick it from the host's list or type its address; nothing is pushed");
                g.hit(R::new(x, y, w, 46.0 * s), Event::Tab(FLOW_BASE + 5), "Connect to an existing repository");
                y += 58.0 * s;
                if let Some(o) = &self.state.origin {
                    line(g, &mut y, AMBER, &format!("origin is now {}; a new choice replaces it after a review.", gitsite_redact(o)));
                    if self.state.commits > 0 {
                        buttons.push(("Push…  p".into(), accent, Event::Tab(FLOW_BASE + 6)));
                    }
                }
            }
            Stage::CreateReview => {
                let cur = self.slots().get(self.focus).copied();
                g.text(x, y, 13.0 * s, if cur == Some(Slot::Name) { TEXT } else { MUTED }, "Repository name", Face::Bold);
                self.name.render(g, R::new(x, y + 20.0 * s, w, 34.0 * s), cur == Some(Slot::Name), s, blink, Face::Sans, FLOW_BASE + 50);
                y += 62.0 * s;
                g.text(x, y, 13.0 * s, if cur == Some(Slot::Private) { TEXT } else { MUTED }, "Who can see it", Face::Bold);
                let r = R::new(x, y + 20.0 * s, w, 34.0 * s);
                fx::field_frame(g, r, s, cur == Some(Slot::Private), accent);
                g.text(r.x + 14.0 * s, r.y + 7.0 * s, 16.0 * s, TEXT, &format!("◀  {}  ▶", if self.private { "Private (only you and who you add)" } else { "Public (anyone who can reach the host)" }), Face::Sans);
                g.hit(r, Event::Tab(FLOW_BASE + 3), "visibility");
                y += 62.0 * s;
                if self.state.commits == 0 {
                    g.text(x, y, 13.0 * s, if cur == Some(Slot::Commit) { TEXT } else { MUTED }, "First commit", Face::Bold);
                    let r = R::new(x, y + 20.0 * s, w, 34.0 * s);
                    fx::field_frame(g, r, s, cur == Some(Slot::Commit), accent);
                    g.text(r.x + 14.0 * s, r.y + 7.0 * s, 16.0 * s, TEXT, &format!("◀  {}  ▶", if self.commit { if self.state.is_repo { format!("Yes: commit the {} file(s) git does not ignore", self.state.changed) } else { "Yes: commit the project's files (.gitignore is honoured)".to_string() } } else { "No: I will commit later".to_string() }), Face::Sans);
                    y += 62.0 * s;
                }
                for l in self.create_review() {
                    line(g, &mut y, if l.starts_with("Warning") || l.contains("REPLACED") { AMBER } else { MUTED }, &l);
                }
                buttons.push(("Create repository".into(), accent, Event::Tab(FLOW_BASE + 2)));
            }
            Stage::Existing => {
                g.text(x, y, 13.0 * s, TEXT, "Repository (type to search, or paste an address or owner/repo)", Face::Bold);
                self.filter.render(g, R::new(x, y + 20.0 * s, w, 34.0 * s), true, s, blink, Face::Sans, FLOW_BASE + 51);
                y += 62.0 * s;
                if self.listing {
                    line(g, &mut y, MUTED, "Asking the host for your repositories…");
                }
                if let Some(n) = &self.repos_note {
                    line(g, &mut y, AMBER, n);
                }
                let list = self.visible_repos();
                let room = ((bottom - y - if self.confirm.is_some() { 70.0 * s } else { 0.0 }) / (40.0 * s)).max(1.0) as usize;
                let sel = self.repo_sel.min(list.len().saturating_sub(1));
                let first = sel.saturating_sub(room - 1);
                for (i, r) in list.iter().enumerate().skip(first).take(room) {
                    let rr = R::new(x, y, w, 36.0 * s);
                    g.rrect(rr.x, rr.y, rr.w, rr.h, 6.0 * s, if i == sel { fx::with_alpha(accent, 40) } else if g.hovered(rr) { crate::gfx::rgb(26, 33, 60) } else { crate::gfx::rgb(20, 26, 48) });
                    g.hit(rr, Event::Row(i), &r.full_name);
                    let nm = g.fit(14.0 * s, &r.full_name, Face::Bold, w * 0.6);
                    g.text(rr.x + 12.0 * s, rr.y + 9.0 * s, 14.0 * s, TEXT, &nm, Face::Bold);
                    g.text(rr.x + w - 90.0 * s, rr.y + 10.0 * s, 12.0 * s, if r.private { MUTED } else { AMBER }, if r.private { "private" } else { "public" }, Face::Sans);
                    y += 40.0 * s;
                }
                if let Some((remote, repo)) = self.confirm.clone() {
                    y = bottom - 64.0 * s;
                    line(g, &mut y, TEXT, &format!("Connect this project to {repo}? origin becomes {}.{} Nothing is pushed or fetched. Press Enter again to do it.", gitsite_redact(&remote), if self.state.origin.is_some() { " The current origin is REPLACED." } else { "" }));
                }
                buttons.push(("Connect".into(), accent, Event::Tab(FLOW_BASE + 2)));
            }
            Stage::Working => line(g, &mut y, MUTED, "Working… (the host is asked, then the project's git is set up)"),
            Stage::Connected => {
                if let Some(m) = &self.message {
                    line(g, &mut y, OK, m);
                }
                y += 6.0 * s;
                if self.state.commits > 0 {
                    line(g, &mut y, MUTED, &format!("The branch {} has {} commit(s). Push them to the host? (a review comes first)", self.state.branch, self.state.commits));
                    buttons.push(("Push…  p".into(), accent, Event::Tab(FLOW_BASE + 6)));
                } else {
                    line(g, &mut y, MUTED, "There is no commit yet, so there is nothing to push. Commit in the IDE, then use Push in the Repositories panel.");
                }
            }
            Stage::PushReview => {
                for l in self.push_review() {
                    line(g, &mut y, TEXT, &l);
                }
                buttons.push(("Push".into(), accent, Event::Tab(FLOW_BASE + 2)));
            }
        }
        if let Some(e) = &self.error {
            for (i, l) in g.wrap(13.0 * s, e, Face::Sans, w).iter().take(2).enumerate() {
                g.text(x, bottom - 2.0 * s + i as f32 * 16.0 * s, 13.0 * s, ERR, l, Face::Sans);
            }
        }
        let by = p.y + p.h - 50.0 * s;
        let mut bx = p.x + p.w - 26.0 * s;
        for (label, col, ev) in buttons.into_iter() {
            let tw = g.text_width(14.0 * s, &label, Face::Bold) + 34.0 * s;
            bx -= tw + 8.0 * s;
            g.button(bx, by, 14.0 * s, &label, col, ev);
        }
        if !matches!(self.stage, Stage::Pick | Stage::Connected | Stage::Working) {
            bx -= 90.0 * s;
            g.button(bx, by, 14.0 * s, "Back", MUTED, Event::Tab(FLOW_BASE + 1));
        }
        bx -= 100.0 * s;
        g.button(bx.max(x), by, 14.0 * s, if self.stage == Stage::Connected { "Close" } else { "Cancel" }, MUTED, Event::Tab(FLOW_BASE));
        let help = g.fit(12.0 * s, "↑ ↓ choose · Enter next · PgUp back · Esc closes (nothing is changed until the review is confirmed)", Face::Sans, (bx - x - 10.0 * s).max(60.0 * s));
        g.text(x, by + 8.0 * s, 12.0 * s, DIM, &help, Face::Sans);
    }

    #[allow(clippy::too_many_arguments)]
    fn row(&self, g: &mut Gfx, x: f32, y: f32, w: f32, s: f32, accent: [u8; 4], on: bool, i: usize, title: &str, sub: &str) {
        let r = R::new(x, y, w, 46.0 * s);
        g.rrect(r.x, r.y, r.w, r.h, 6.0 * s, if on { fx::with_alpha(accent, 40) } else if g.hovered(r) { crate::gfx::rgb(26, 33, 60) } else { crate::gfx::rgb(20, 26, 48) });
        if on {
            g.rrect_outline(r.x, r.y, r.w, r.h, 6.0 * s, 1.5 * s, accent);
        }
        if i < 100 {
            g.hit(r, Event::Row(i), title);
        }
        let t = g.fit(15.0 * s, title, Face::Bold, w - 24.0 * s);
        g.text(r.x + 12.0 * s, r.y + 6.0 * s, 15.0 * s, TEXT, &t, Face::Bold);
        let sub = g.fit(12.0 * s, sub, Face::Sans, w - 24.0 * s);
        g.text(r.x + 12.0 * s, r.y + 26.0 * s, 12.0 * s, MUTED, &sub, Face::Sans);
    }
}

/// Files in the project's top folder whose names say "secret" and that `.gitignore` does not name (a warning, not a rule).
pub fn secret_looking(root: &std::path::Path) -> Vec<String> {
    let ignore = std::fs::read_to_string(root.join(".gitignore")).unwrap_or_default();
    let mut out: Vec<String> = std::fs::read_dir(root)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.path().is_file())
        .map(|e| e.file_name().to_string_lossy().to_string())
        .filter(|n| {
            let l = n.to_ascii_lowercase();
            let secret = l == ".env" || l.starts_with(".env.") || l.ends_with(".pem") || l.ends_with(".key") || l.ends_with(".p12") || l.starts_with("id_rsa") || l.starts_with("id_ed25519") || l.starts_with("credentials") || l.contains("secret");
            secret && !ignore.lines().any(|i| i.trim().trim_start_matches('/') == n || (i.trim().starts_with("*.") && l.ends_with(&i.trim()[1..])))
        })
        .collect();
    out.sort();
    out.truncate(4);
    out
}

fn gitsite_redact(u: &str) -> String {
    iyakki_agent::gitsite::redact_url(u)
}

/// The create step, off the screen's thread: the repository on the host, then git in the project. For the Gitea of the cluster the
/// platform services' own connect step does the host side (`platform::connect`), so there is one implementation of it.
#[allow(clippy::too_many_arguments)]
fn make_repository(conns: &gitconn::Conns, http: &dyn gitconn::GitHttp, run: &Arc<dyn Runner>, platform: Option<&Platform>, dir: &std::path::Path, user: &str, ws_name: &str, root: &std::path::Path, state: &WsGit, c: &Connection, name: &str, private: bool, commit: bool) -> Result<Done, String> {
    let prep = |run: &dyn Runner| -> Result<(), String> {
        if !state.is_repo {
            gitconn::git_init(run, root)?;
        }
        if commit {
            gitconn::first_commit(run, root, c)?;
        }
        Ok(())
    };
    if !c.cluster.is_empty() {
        let inst = iyakki_agent::platform::installed::find(dir, user, &c.cluster).ok_or("That Gitea is no longer installed: connect again or choose another connection.")?;
        let pl = platform.ok_or("Platform services are not available here, so the cluster's Gitea cannot be used.")?;
        prep(&**run)?;
        let ws = WsInfo { name: ws_name.to_string(), root: root.to_path_buf() };
        let plan = pconnect::plan_connect(pl, &inst, &ws, &ConnectOpts { repo: name.to_string(), private, transport: if c.method == Method::Ssh { Transport::Ssh } else { Transport::Https }, replace_origin: true })?;
        if !plan.blockers.is_empty() {
            return Err(format!("Not connected: {}.", plan.blockers.join("; ")));
        }
        let done = pconnect::connect(pl, &plan, true)?;
        let full = format!("{}/{name}", done.owner);
        conns.audit_repo(c, &full);
        gitconn::apply_identity(&**run, root, c);
        return Ok(Done { remote: done.remote_url, full, committed: commit });
    }
    let info = gitconn::create_repo(conns, http, c, name, private, &format!("Project {ws_name} (IYAKKI)"))?;
    prep(&**run)?;
    let remote = if c.method == Method::Ssh && !info.ssh_url.is_empty() { info.ssh_url.clone() } else { info.https_url.clone() };
    gitconn::set_origin(&**run, root, &remote, true).map_err(|e| format!("The repository {} was made on the host, but origin could not be set: {e}", info.full_name))?;
    gitconn::apply_identity(&**run, root, c);
    Ok(Done { remote, full: info.full_name, committed: commit })
}

/// Which kind of host the flow talks about (the panel header shows it).
pub fn kind_label(k: Kind) -> &'static str {
    k.label()
}
