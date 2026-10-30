//! "Connect Git or Gitea": the wizard used by Settings → Git (Connections) and by the AI IDE ("Add connection…", in place).
//! Five steps: the kind of host, where it is (the Gitea of the person's cluster or an address), how to sign in (a personal access
//! token or an SSH key of the key service), a test with the host's own API, and the name and email for commits.
//!
//! Nothing is stored until the last step's Save: Cancel at any step leaves no connection, no token, no certificate and no key the wizard
//! made. The rules and the work are in `iyakki_agent::{gitconn, gitkeys}`; this file is the screens and their keys (the whole wizard works
//! from the keyboard: Up/Down and the digits choose, Tab and Enter walk the fields, PgUp goes back, Esc cancels).

use std::path::PathBuf;
use std::sync::mpsc::{channel, Receiver};
use std::sync::Arc;

use iyakki_agent::gitdiscover::{self, Found};
use iyakki_agent::pvcguard::KOut;
use iyakki_agent::gitconn::{self, CertInfo, ClusterGitea, Conns, Connection, GitHttp, Kind, Method, Placement, TestReport, Tls};
use iyakki_agent::gitkeys::{GitKeys, KeyInfo};

use crate::fx::{self, AMBER};
use crate::gfx::{Face, Gfx};
use crate::ui::{Event, Key, TextField, DIM, ERR, MUTED, OK, R, TEXT};

/// Click ids of the wizard.
pub const WIZ_BASE: usize = 8_900_000;

/// What the wizard needs from its host (a fake in tests).
#[derive(Clone)]
pub struct WizEnv {
    pub keys: Arc<GitKeys>,
    pub http: Arc<dyn GitHttp>,
    /// The services directory and the person, for the Gitea instances of the platform services.
    pub dir: PathBuf,
    pub user: String,
    /// May this account reach this host at all (the network policy)? `Err` says why not.
    pub egress: Arc<dyn Fn(&str) -> Result<(), String> + Send + Sync>,
    /// Read-only `kubectl` against the dev cluster (not ok when the cluster is not running); called off the UI thread.
    pub kube: Arc<dyn Fn(&[&str]) -> KOut + Send + Sync>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Step {
    Kind,
    Where,
    SignIn,
    Test,
    Commits,
}

impl Step {
    const ALL: [Step; 5] = [Step::Kind, Step::Where, Step::SignIn, Step::Test, Step::Commits];
    fn n(self) -> usize {
        Step::ALL.iter().position(|s| *s == self).unwrap_or(0)
    }
    fn title(self) -> &'static str {
        match self {
            Step::Kind => "Kind",
            Step::Where => "Where",
            Step::SignIn => "Sign in",
            Step::Test => "Test",
            Step::Commits => "Commits",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Slot {
    Mode,
    Name,
    Url,
    Cluster,
    Method,
    Token,
    ApiToken,
    Keys,
    KeyName,
    IdName,
    Email,
    Default,
}

const F_NAME: usize = 0;
const F_URL: usize = 1;
const F_TOKEN: usize = 2;
const F_KEYNAME: usize = 3;
const F_IDNAME: usize = 4;
const F_EMAIL: usize = 5;

#[derive(Clone, Debug, PartialEq)]
pub enum WizOut {
    None,
    Cancel,
    /// Saved: the connection as stored.
    Done(Box<Connection>),
    /// "Open Platform services" (no Gitea is installed on the cluster): the wizard is closed and nothing is kept.
    OpenPlatform,
    /// "Set it up (Quick install: Gitea)" for a Gitea found on the cluster: the wizard is closed, nothing is kept, and the host starts the
    /// existing Quick install item.
    QuickInstallGitea,
}

enum Msg {
    Report(Box<TestReport>),
    Cert(Result<CertInfo, String>),
}

/// The cluster's own answer about Gitea (see `gitdiscover`).
type Seen = Vec<Found>;

pub struct GitWizard {
    env: WizEnv,
    pub step: Step,
    editing: Option<Connection>,
    pub kind: usize,
    kind_default_url: String,
    /// 0: the Gitea of my cluster, 1: an existing server.
    pub mode: usize,
    pub cluster: Vec<ClusterGitea>,
    records: Vec<ClusterGitea>,
    disc_rx: Option<Receiver<Seen>>,
    pub cluster_sel: usize,
    pub method: Method,
    keys: Vec<KeyInfo>,
    /// The key list's row; one past the keys is "make a new key".
    pub key_sel: usize,
    made_key: Option<String>,
    tf: Vec<TextField>,
    pub focus: usize,
    pub make_default: bool,
    pub error: Option<String>,
    conns: Conns,
    // the test
    rx: Option<Receiver<Msg>>,
    pub report: Option<TestReport>,
    pub cert: Option<CertInfo>,
    trusted: Option<CertInfo>,
    staged: Option<PathBuf>,
    forced: bool,
    pub done: bool,
}

fn tf(placeholder: &str, secret: bool) -> TextField {
    let mut t = TextField::new(placeholder, secret);
    t.max = 2048;
    t
}

impl GitWizard {
    pub fn new(env: WizEnv) -> GitWizard {
        let conns = env.keys.conns();
        let cluster = gitconn::cluster_gitea(&env.dir, &env.user);
        let first = conns.list().is_empty();
        let mut w = GitWizard {
            keys: env.keys.keys(),
            env,
            step: Step::Kind,
            editing: None,
            kind: 2,
            kind_default_url: String::new(),
            mode: 1,
            records: cluster.clone(),
            cluster,
            disc_rx: None,
            cluster_sel: 0,
            method: Method::Token,
            key_sel: 0,
            made_key: None,
            tf: vec![tf("A name you will recognise, like Home Gitea", false), tf("https://git.example.org", false), tf("Paste the personal access token", true), tf("Name for the new key, like gitea", false), tf("Your name for commits", false), tf("you@example.org", false)],
            focus: 0,
            make_default: first,
            error: None,
            conns,
            rx: None,
            report: None,
            cert: None,
            trusted: None,
            staged: None,
            forced: false,
            done: false,
        };
        w.discover();
        w
    }

    /// Asks the cluster for a Gitea of its own (in the background); the list is merged when the answer comes.
    pub fn discover(&mut self) {
        if self.disc_rx.is_some() {
            return;
        }
        let kube = self.env.kube.clone();
        let (tx, rx) = channel();
        std::thread::spawn(move || {
            let _ = tx.send(gitdiscover::discover(&*kube));
        });
        self.disc_rx = Some(rx);
    }

    fn poll_discovery(&mut self) {
        let Some(rx) = &self.disc_rx else { return };
        let Ok(found) = rx.try_recv() else { return };
        self.disc_rx = None;
        let keep = self.cluster.get(self.cluster_sel).map(|g| g.instance.clone());
        self.cluster = gitdiscover::merge(self.records.clone(), &found);
        if let Some(i) = keep.and_then(|k| self.cluster.iter().position(|g| g.instance == k)) {
            self.cluster_sel = i;
        }
        self.cluster_sel = self.cluster_sel.min(self.cluster.len().saturating_sub(1));
        for g in self.cluster.iter().filter(|g| g.found) {
            self.env.keys.conns().audit("discover.cluster", &format!("found {}", g.namespace));
        }
    }

    /// Edit one connection: the wizard starts at "Where" with its values. A token field left empty keeps the stored token.
    pub fn edit(env: WizEnv, c: Connection) -> GitWizard {
        let mut w = GitWizard::new(env);
        w.kind = Kind::ALL.iter().position(|k| *k == c.kind).unwrap_or(2);
        w.mode = usize::from(c.cluster.is_empty());
        if let Some(i) = w.cluster.iter().position(|g| g.instance == c.cluster) {
            w.cluster_sel = i;
        }
        w.tf[F_NAME].set(&c.name);
        w.tf[F_URL].set(&c.url);
        w.method = c.method;
        if let Some(i) = w.keys.iter().position(|k| k.id == c.key) {
            w.key_sel = i;
        }
        w.tf[F_IDNAME].set(&c.identity_name);
        w.tf[F_EMAIL].set(&c.identity_email);
        w.make_default = c.default;
        w.kind_default_url = c.kind.default_url().to_string();
        w.step = Step::Where;
        w.editing = Some(c);
        w.focus = 0;
        w
    }

    pub fn is_open(&self) -> bool {
        !self.done
    }

    pub fn kind_value(&self) -> Kind {
        Kind::ALL[self.kind.min(3)]
    }

    pub fn set_text(&mut self, slot_name: &str, v: &str) {
        let i = match slot_name {
            "name" => F_NAME,
            "url" => F_URL,
            "token" => F_TOKEN,
            "keyname" => F_KEYNAME,
            "idname" => F_IDNAME,
            _ => F_EMAIL,
        };
        self.tf[i].set(v);
    }

    pub fn text(&self, slot_name: &str) -> String {
        let i = match slot_name {
            "name" => F_NAME,
            "url" => F_URL,
            "token" => F_TOKEN,
            "keyname" => F_KEYNAME,
            "idname" => F_IDNAME,
            _ => F_EMAIL,
        };
        self.tf[i].value.clone()
    }

    pub fn busy(&self) -> bool {
        self.rx.is_some()
    }

    /// What the Where step says when no Gitea is installed on the cluster (shown with an "Open Platform services" button).
    pub fn cluster_hint(&self) -> Option<String> {
        (self.cluster_mode() && self.cluster.is_empty()).then(|| "No Gitea is installed on your cluster yet. Install one in Platform services (Settings > Dev cluster > Platform services, or the IDE's K8S panel > Services), or choose \"An existing Gitea / Forgejo server\".".to_string())
    }

    fn cluster_mode(&self) -> bool {
        self.kind_value() == Kind::Gitea && self.mode == 0
    }

    fn slots(&self) -> Vec<Slot> {
        match self.step {
            Step::Kind | Step::Test => vec![],
            Step::Where => {
                let mut v = vec![];
                if self.kind_value() == Kind::Gitea {
                    v.push(Slot::Mode);
                }
                v.push(Slot::Name);
                v.push(if self.cluster_mode() { Slot::Cluster } else { Slot::Url });
                v
            }
            Step::SignIn => {
                if self.cluster_mode() {
                    return vec![];
                }
                let mut v = vec![Slot::Method];
                if self.method == Method::Token {
                    v.push(Slot::Token);
                } else {
                    v.push(Slot::Keys);
                    if self.key_sel >= self.keys.len() {
                        v.push(Slot::KeyName);
                    }
                    if self.kind_value().has_api() {
                        v.push(Slot::ApiToken);
                    }
                }
                v
            }
            Step::Commits => vec![Slot::IdName, Slot::Email, Slot::Default],
        }
    }

    fn slot(&self) -> Option<Slot> {
        let s = self.slots();
        s.get(self.focus.min(s.len().saturating_sub(1))).copied()
    }

    fn field_of(slot: Slot) -> Option<usize> {
        match slot {
            Slot::Name => Some(F_NAME),
            Slot::Url => Some(F_URL),
            Slot::Token | Slot::ApiToken => Some(F_TOKEN),
            Slot::KeyName => Some(F_KEYNAME),
            Slot::IdName => Some(F_IDNAME),
            Slot::Email => Some(F_EMAIL),
            _ => None,
        }
    }

    // ------------------------------------------------------------------ moving between steps

    fn go(&mut self, s: Step) {
        self.step = s;
        self.focus = 0;
        self.error = None;
    }

    pub fn back(&mut self) {
        self.rx = None;
        match self.step {
            Step::Kind => {}
            Step::Where => {
                if self.editing.is_none() {
                    self.go(Step::Kind)
                }
            }
            Step::SignIn => self.go(Step::Where),
            Step::Test => self.go(Step::SignIn),
            Step::Commits => self.go(Step::Test),
        }
    }

    pub fn can_back(&self) -> bool {
        !(self.step == Step::Kind || (self.step == Step::Where && self.editing.is_some()))
    }

    fn token_text(&self) -> String {
        self.tf[F_TOKEN].value.trim().to_string()
    }

    fn typed_url(&self) -> Result<String, String> {
        if self.cluster_mode() {
            let g = self.cluster.get(self.cluster_sel).ok_or("No Gitea is installed on your cluster yet.")?;
            if !g.problem.is_empty() {
                return Err(format!("{} cannot be used yet: {}.", g.title, g.problem));
            }
            return Ok(g.url.clone());
        }
        gitconn::clean_url(&self.tf[F_URL].value)
    }

    pub fn next(&mut self) -> WizOut {
        self.error = None;
        match self.step {
            Step::Kind => {
                let k = self.kind_value();
                if self.tf[F_URL].value.is_empty() || self.tf[F_URL].value == self.kind_default_url {
                    self.tf[F_URL].set(k.default_url());
                }
                self.kind_default_url = k.default_url().to_string();
                if k == Kind::Gitea && !self.cluster.is_empty() && self.editing.is_none() {
                    self.mode = 0;
                }
                if self.tf[F_NAME].value.trim().is_empty() {
                    self.tf[F_NAME].set(if k == Kind::Gitea && !self.cluster.is_empty() && self.mode == 0 { "My cluster Gitea" } else { k.label() });
                }
                self.go(Step::Where);
            }
            Step::Where => {
                let name = self.tf[F_NAME].value.trim().to_string();
                if !gitconn::valid_name(&name) {
                    self.error = Some("Give the connection a name (up to 40 characters).".into());
                    return WizOut::None;
                }
                let me = self.editing.as_ref().map(|c| c.id.clone());
                if self.conns.list().iter().any(|c| Some(&c.id) != me.as_ref() && c.name.eq_ignore_ascii_case(&name)) {
                    self.error = Some(format!("There is already a connection called {name}."));
                    return WizOut::None;
                }
                if let Err(e) = self.typed_url() {
                    self.error = Some(e);
                    return WizOut::None;
                }
                if self.cluster_mode() {
                    // the platform services already hold IYAKKI's token for it
                    self.method = Method::Token;
                    self.start_test();
                    self.go(Step::Test);
                } else {
                    self.go(Step::SignIn);
                }
            }
            Step::SignIn => {
                if self.method == Method::Token {
                    if self.token_text().is_empty() && !self.editing.as_ref().is_some_and(|c| c.has_token()) {
                        self.error = Some("Paste the personal access token (the host's settings page makes one).".into());
                        return WizOut::None;
                    }
                    if self.token_text().chars().any(|c| c.is_whitespace() || c.is_control()) {
                        self.error = Some("The token has spaces in it: paste it again, only the token.".into());
                        return WizOut::None;
                    }
                } else if self.key_sel >= self.keys.len() {
                    let id = self.tf[F_KEYNAME].value.trim().to_ascii_lowercase();
                    match self.env.keys.generate(&id, &format!("{}@iyakki", self.env.user)) {
                        Ok(info) => {
                            self.made_key = Some(info.id.clone());
                            self.keys = self.env.keys.keys();
                            self.key_sel = self.keys.iter().position(|k| k.id == info.id).unwrap_or(0);
                        }
                        Err(e) => {
                            self.error = Some(e);
                            return WizOut::None;
                        }
                    }
                }
                self.start_test();
                self.go(Step::Test);
            }
            Step::Test => {
                let ok = self.report.as_ref().is_some_and(|r| r.ok) || self.forced;
                if self.rx.is_some() {
                    self.error = Some("Wait for the test to finish.".into());
                } else if !ok {
                    self.error = Some("The test did not pass. Fix the problem and test again (r), or press s to save the connection anyway.".into());
                } else {
                    // the commit name and email are the person's to give: nothing is copied from the server's account (its profile can
                    // hold a test or placeholder address); the account's own values are only shown as a hint
                    self.go(Step::Commits);
                }
            }
            Step::Commits => return self.save(),
        }
        WizOut::None
    }

    /// The connection as the person described it so far (not stored).
    fn draft(&self) -> Connection {
        let base = self.editing.clone().unwrap_or_default();
        let g = self.cluster.get(self.cluster_sel).filter(|_| self.cluster_mode());
        Connection {
            name: self.tf[F_NAME].value.trim().to_string(),
            kind: self.kind_value(),
            url: self.typed_url().unwrap_or_default(),
            method: self.method,
            key: if self.method == Method::Ssh { self.keys.get(self.key_sel).map(|k| k.id.clone()).unwrap_or_default() } else { String::new() },
            cluster: g.map(|g| g.instance.clone()).unwrap_or_default(),
            token_vault: g.map(|g| g.token_vault.clone()).unwrap_or(base.token_vault.clone()),
            id: if base.id.is_empty() { "wizard-test".into() } else { base.id.clone() },
            ..base
        }
    }

    fn start_test(&mut self) {
        self.report = None;
        self.cert = None;
        self.forced = false;
        let c = self.draft();
        let host = c.host();
        let (tx, rx) = channel();
        // the network policy first
        if !c.cluster.is_empty() {
            // inside the cluster's own address: the policy of the host list does not apply to it
        } else if let Err(e) = (self.env.egress)(&host) {
            let mut r = blank_report(&c);
            r.error = Some(e);
            self.report = Some(r);
            return;
        }
        let token: Option<String> = if !self.token_text().is_empty() {
            Some(self.token_text())
        } else if c.token_vault.is_empty() {
            None
        } else {
            match self.conns.token(&c) {
                Ok(t) => Some(t.to_string()),
                Err(e) => {
                    let mut r = blank_report(&c);
                    r.error = Some(gitconn::scrub(&e, ""));
                    self.report = Some(r);
                    return;
                }
            }
        };
        let Some(token) = token else {
            // SSH without a token: there is no API to ask; the first fetch shows the host key and whether the key works
            let mut r = blank_report(&c);
            r.ok = true;
            r.notes.push("No token was given, so the host's API was not asked. The first fetch or push shows the host's fingerprint and whether the key works. Making a repository from the IDE needs a token (Edit).".into());
            self.report = Some(r);
            return;
        };
        let cert = self.staged.clone().or_else(|| self.editing.as_ref().and_then(|e| self.conns.pinned_cert(e)).filter(|_| self.editing.as_ref().is_some_and(|e| e.url == c.url)));
        let http = self.env.http.clone();
        std::thread::spawn(move || {
            let r = gitconn::test_connection(&*http, &c, &token, cert.as_ref());
            let _ = tx.send(Msg::Report(Box::new(r)));
        });
        self.rx = Some(rx);
    }

    fn fetch_cert(&mut self) {
        let url = self.draft().url;
        let http = self.env.http.clone();
        let (tx, rx) = channel();
        std::thread::spawn(move || {
            let _ = tx.send(Msg::Cert(http.peer_cert(&url)));
        });
        self.rx = Some(rx);
    }

    /// Collects the answer of the test (call on every tick).
    pub fn poll(&mut self) {
        self.poll_discovery();
        let Some(rx) = &self.rx else { return };
        let Ok(m) = rx.try_recv() else { return };
        self.rx = None;
        match m {
            Msg::Report(r) => {
                let untrusted = r.tls == Tls::Untrusted;
                self.report = Some(*r);
                if untrusted {
                    self.fetch_cert();
                }
            }
            Msg::Cert(Ok(c)) => self.cert = Some(c),
            Msg::Cert(Err(e)) => self.error = Some(format!("The certificate could not be read: {e}")),
        }
    }

    /// Runs until the test (and the certificate read) are done: for tests and scripted use.
    pub fn wait(&mut self) {
        for _ in 0..400 {
            self.poll();
            if self.rx.is_none() && self.disc_rx.is_none() {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
    }

    fn trust_shown_cert(&mut self) {
        let Some(c) = self.cert.clone() else {
            self.error = Some("There is no certificate to trust: test the connection first.".into());
            return;
        };
        let dir = self.conns.certs_dir();
        let path = dir.join("pending-wizard.pem");
        if let Err(e) = std::fs::create_dir_all(&dir).and_then(|_| std::fs::write(&path, &c.pem)) {
            self.error = Some(format!("The certificate could not be kept: {e}"));
            return;
        }
        self.staged = Some(path);
        self.trusted = Some(c);
        self.start_test();
    }

    fn cleanup(&mut self) {
        if let Some(p) = self.staged.take() {
            let _ = std::fs::remove_file(p);
        }
        // a key the wizard made for this connection and nobody uses
        if let Some(k) = self.made_key.take() {
            let _ = self.env.keys.delete_key(&k);
        }
    }

    pub fn cancel(&mut self) -> WizOut {
        self.cleanup();
        self.tf[F_TOKEN].set(""); // the pasted token leaves the screen's memory
        self.done = true;
        WizOut::Cancel
    }

    fn save(&mut self) -> WizOut {
        let (name, email) = (self.tf[F_IDNAME].value.trim().to_string(), self.tf[F_EMAIL].value.trim().to_string());
        let mut c = self.draft();
        if c.cluster.is_empty() && self.method == Method::Ssh && c.key.is_empty() {
            self.error = Some("Choose or make an SSH key.".into());
            return WizOut::None;
        }
        if let Some(r) = &self.report {
            if !r.account.is_empty() {
                c.account = r.account.clone();
            }
        }
        let typed = Some(self.token_text()).filter(|t| !t.is_empty());
        let saved = if let Some(old) = self.editing.clone() {
            c.id = old.id.clone();
            c.default = old.default;
            c.pinned = old.pinned.clone();
            c.last_test = old.last_test.clone();
            c.account = if c.url == old.url { if c.account.is_empty() { old.account.clone() } else { c.account } } else { c.account };
            self.conns.update(&c, typed.as_deref()).map(|_| old.id)
        } else {
            c.id.clear();
            self.conns.add(c, typed.as_deref()).map(|x| x.id)
        };
        let id = match saved {
            Ok(id) => id,
            Err(e) => {
                self.error = Some(gitconn::scrub(&e, &self.token_text()));
                return WizOut::None;
            }
        };
        if let Some(t) = &self.trusted {
            let _ = self.conns.trust_cert(&id, t);
        }
        if let Err(e) = self.conns.set_identity(&id, &name, &email) {
            self.error = Some(format!("Saved, but the name for commits was not: {e}"));
        }
        if self.make_default {
            let _ = self.conns.set_default(&id);
        }
        if let Some(r) = &self.report {
            let _ = self.conns.record_test(&id, r);
        }
        if let Some(p) = self.staged.take() {
            let _ = std::fs::remove_file(p);
        }
        self.made_key = None; // the key now belongs to the connection
        self.tf[F_TOKEN].set(""); // the token leaves the screen's memory
        self.done = true;
        match self.conns.get(&id) {
            Some(c) => WizOut::Done(Box::new(c)),
            None => WizOut::Cancel,
        }
    }

    // ------------------------------------------------------------------ events

    pub fn event(&mut self, ev: &Event) -> WizOut {
        if self.done {
            return WizOut::None;
        }
        match ev {
            Event::Tick => {
                self.poll();
                WizOut::None
            }
            Event::Paste(t) => {
                if let Some(i) = self.slot().and_then(Self::field_of) {
                    self.tf[i].paste(t);
                }
                WizOut::None
            }
            Event::FocusAt(i, x) => {
                let slots = self.slots();
                if let Some(p) = slots.iter().position(|s| Self::field_of(*s) == Some(*i)) {
                    self.focus = p;
                    self.tf[*i].click(*x);
                }
                WizOut::None
            }
            Event::Row(i) => {
                self.pick(*i);
                WizOut::None
            }
            Event::Tab(t) if *t >= WIZ_BASE && *t < WIZ_BASE + 100 => match t - WIZ_BASE {
                0 => self.cancel(),
                1 => {
                    self.back();
                    WizOut::None
                }
                2 => self.next(),
                3 => {
                    self.retest();
                    WizOut::None
                }
                4 => {
                    self.trust_shown_cert();
                    WizOut::None
                }
                5 => {
                    self.save_anyway();
                    WizOut::None
                }
                6 => {
                    self.copy_public();
                    WizOut::None
                }
                7 => {
                    self.cleanup();
                    self.done = true;
                    WizOut::OpenPlatform
                }
                8 => self.quick_install_gitea(),
                _ => WizOut::None,
            },
            Event::Tab(t) if *t >= WIZ_BASE + 100 && *t < WIZ_BASE + 200 => {
                self.pick(*t - WIZ_BASE - 100);
                WizOut::None
            }
            Event::Key(k) => self.key(k),
            _ => WizOut::None,
        }
    }

    /// The one action for a Gitea found on the cluster that this computer cannot use yet.
    fn quick_install_gitea(&mut self) -> WizOut {
        if !self.cluster.get(self.cluster_sel).is_some_and(|g| g.found) {
            return WizOut::None;
        }
        self.cleanup();
        self.done = true;
        WizOut::QuickInstallGitea
    }

    /// A click on row `i` of the list of the current step.
    fn pick(&mut self, i: usize) {
        match (self.step, self.slot()) {
            (Step::Kind, _) => self.kind = i.min(3),
            (Step::Where, Some(Slot::Cluster)) => self.cluster_sel = i.min(self.cluster.len().saturating_sub(1)),
            (Step::SignIn, _) => {
                self.key_sel = i.min(self.keys.len());
                if let Some(p) = self.slots().iter().position(|s| *s == Slot::Keys) {
                    self.focus = p;
                }
            }
            _ => {}
        }
    }

    fn retest(&mut self) {
        if self.step == Step::Test && self.rx.is_none() {
            self.start_test();
        }
    }

    fn save_anyway(&mut self) {
        if self.step == Step::Test && self.report.as_ref().is_some_and(|r| !r.ok) {
            self.forced = true;
            self.next();
        }
    }

    fn copy_public(&mut self) {
        if let Some(k) = self.keys.get(self.key_sel) {
            crate::ui::clipboard::copy(&k.public);
            self.error = None;
        }
    }

    fn key(&mut self, k: &Key) -> WizOut {
        let slot = self.slot();
        let text_field = slot.and_then(Self::field_of);
        match k {
            Key::Esc => return self.cancel(),
            Key::PageUp | Key::Ctrl('b') => {
                self.back();
                return WizOut::None;
            }
            Key::Ctrl('s') | Key::PageDown => return self.next(),
            Key::Enter => {
                let n = self.slots().len();
                if n > 0 && self.focus + 1 < n && !matches!(slot, Some(Slot::Keys | Slot::Cluster | Slot::Default)) {
                    self.focus += 1;
                    return WizOut::None;
                }
                return self.next();
            }
            Key::Tab => {
                let n = self.slots().len().max(1);
                self.focus = (self.focus + 1) % n;
                return WizOut::None;
            }
            Key::BackTab => {
                let n = self.slots().len().max(1);
                self.focus = (self.focus + n - 1) % n;
                return WizOut::None;
            }
            _ => {}
        }
        // the single-letter commands of the test step
        if self.step == Step::Test {
            if let Key::Char(c) = k {
                match c.to_ascii_lowercase() {
                    'r' => self.retest(),
                    't' => self.trust_shown_cert(),
                    's' => self.save_anyway(),
                    _ => {}
                }
            }
            return WizOut::None;
        }
        if let Some(i) = text_field {
            match k {
                Key::Up => self.focus = self.focus.saturating_sub(1),
                Key::Down => self.focus = (self.focus + 1).min(self.slots().len().saturating_sub(1)),
                other => {
                    self.tf[i].handle(other);
                }
            }
            return WizOut::None;
        }
        match (self.step, slot, k) {
            (Step::Kind, _, Key::Up) => self.kind = self.kind.saturating_sub(1),
            (Step::Kind, _, Key::Down) => self.kind = (self.kind + 1).min(3),
            (Step::Kind, _, Key::Char(c @ '1'..='4')) => self.kind = *c as usize - '1' as usize,
            (Step::Where, Some(Slot::Mode), Key::Left | Key::Right | Key::Up | Key::Down | Key::Char(' ')) => {
                if matches!(k, Key::Up) {
                    self.focus = 0;
                } else {
                    self.mode = 1 - self.mode;
                }
            }
            (Step::Where, Some(Slot::Cluster), Key::Up) => {
                if self.cluster_sel == 0 {
                    self.focus = self.focus.saturating_sub(1);
                } else {
                    self.cluster_sel -= 1;
                }
            }
            (Step::Where, Some(Slot::Cluster), Key::Char('r')) => self.discover(),
            (Step::Where, Some(Slot::Cluster), Key::Char('s')) => return self.quick_install_gitea(),
            (Step::Where, Some(Slot::Cluster), Key::Down) => self.cluster_sel = (self.cluster_sel + 1).min(self.cluster.len().saturating_sub(1)),
            (Step::SignIn, Some(Slot::Method), Key::Left | Key::Right | Key::Char(' ')) => self.method = if self.method == Method::Token { Method::Ssh } else { Method::Token },
            (Step::SignIn, Some(Slot::Method), Key::Down) => self.focus = 1,
            (Step::SignIn, Some(Slot::Keys), Key::Up) => {
                if self.key_sel == 0 {
                    self.focus = 0;
                } else {
                    self.key_sel -= 1;
                }
            }
            (Step::SignIn, Some(Slot::Keys), Key::Down) => self.key_sel = (self.key_sel + 1).min(self.keys.len()),
            (Step::SignIn, Some(Slot::Keys), Key::Char('c')) => self.copy_public(),
            (Step::Commits, Some(Slot::Default), Key::Left | Key::Right | Key::Char(' ') | Key::Char('y') | Key::Char('n')) => {
                self.make_default = match k {
                    Key::Char('y') => true,
                    Key::Char('n') => false,
                    _ => !self.make_default,
                }
            }
            (Step::Commits, Some(Slot::Default), Key::Up) => self.focus -= 1,
            _ => {}
        }
        WizOut::None
    }

    // ------------------------------------------------------------------ drawing

    pub fn render(&mut self, g: &mut Gfx, area: R, s: f32, accent: [u8; 4], blink: bool) {
        self.poll();
        let p = fx::modal(g, area, s, 820.0 * s, 620.0 * s, accent, false);
        let (x, w) = (p.x + 26.0 * s, p.w - 52.0 * s);
        fx::modal_title(g, p, s, if self.editing.is_some() { "Edit the connection" } else { "Connect Git or Gitea" }, accent);
        // the steps
        let mut sx = x;
        let sy = p.y + 60.0 * s;
        for st in Step::ALL {
            let cur = st == self.step;
            let label = format!("{} {}", st.n() + 1, st.title());
            let tw = g.text_width(12.0 * s, &label, Face::Bold);
            g.text(sx, sy, 12.0 * s, if cur { accent } else if st.n() < self.step.n() { OK } else { DIM }, &label, Face::Bold);
            sx += tw + 22.0 * s;
        }
        g.text(x + w - 150.0 * s, sy, 12.0 * s, DIM, &format!("step {} of 5", self.step.n() + 1), Face::Sans);
        let mut y = sy + 28.0 * s;
        let bottom = p.y + p.h - 70.0 * s;
        match self.step {
            Step::Kind => self.render_kind(g, x, &mut y, w, s, accent),
            Step::Where => self.render_where(g, x, &mut y, w, s, accent, blink),
            Step::SignIn => self.render_signin(g, x, &mut y, w, s, accent, blink),
            Step::Test => self.render_test(g, x, &mut y, w, bottom, s, accent),
            Step::Commits => self.render_commits(g, x, &mut y, w, s, accent, blink),
        }
        if let Some(e) = &self.error {
            for (i, l) in g.wrap(13.0 * s, e, Face::Sans, w).iter().take(2).enumerate() {
                g.text(x, bottom - 4.0 * s + i as f32 * 16.0 * s, 13.0 * s, ERR, l, Face::Sans);
            }
        }
        // buttons
        let by = p.y + p.h - 50.0 * s;
        let last = self.step == Step::Commits;
        let nw = g.button(p.x + p.w - 26.0 * s - 100.0 * s, by, 14.0 * s, if last { "Save" } else { "Next" }, accent, Event::Tab(WIZ_BASE + 2));
        let _ = nw;
        let mut bx = p.x + p.w - 26.0 * s - 100.0 * s - 90.0 * s;
        if self.can_back() {
            g.button(bx, by, 14.0 * s, "Back", MUTED, Event::Tab(WIZ_BASE + 1));
            bx -= 100.0 * s;
        }
        g.button(bx, by, 14.0 * s, "Cancel", MUTED, Event::Tab(WIZ_BASE));
        let help = match self.step {
            Step::Kind => "Up/Down or 1-4 choose · Enter next · Esc cancels",
            Step::Test => "r test again · t trust the shown certificate · s save anyway · PgUp back · Esc cancels",
            _ => "Tab/Enter move · Ctrl+V paste · Ctrl+S next · PgUp back · Esc cancels (nothing is kept)",
        };
        let help = g.fit(12.0 * s, help, Face::Sans, w - 330.0 * s);
        g.text(x, by + 8.0 * s, 12.0 * s, DIM, &help, Face::Sans);
    }

    fn row(&self, g: &mut Gfx, x: f32, y: f32, w: f32, s: f32, accent: [u8; 4], on: bool, hit: Event, title: &str, sub: &str) {
        let r = R::new(x, y, w, 46.0 * s);
        g.rrect(r.x, r.y, r.w, r.h, 6.0 * s, if on { fx::with_alpha(accent, 40) } else if g.hovered(r) { crate::gfx::rgb(26, 33, 60) } else { crate::gfx::rgb(20, 26, 48) });
        if on {
            g.rrect_outline(r.x, r.y, r.w, r.h, 6.0 * s, 1.5 * s, accent);
        }
        g.hit(r, hit, title);
        let t = g.fit(15.0 * s, title, Face::Bold, w - 24.0 * s);
        g.text(r.x + 12.0 * s, r.y + 6.0 * s, 15.0 * s, TEXT, &t, Face::Bold);
        let sub = g.fit(12.0 * s, sub, Face::Sans, w - 24.0 * s);
        g.text(r.x + 12.0 * s, r.y + 26.0 * s, 12.0 * s, MUTED, &sub, Face::Sans);
    }

    fn render_kind(&mut self, g: &mut Gfx, x: f32, y: &mut f32, w: f32, s: f32, accent: [u8; 4]) {
        g.text(x, *y, 15.0 * s, TEXT, "Which kind of git host is it?", Face::Bold);
        *y += 30.0 * s;
        let subs = ["github.com, or a GitHub Enterprise address", "gitlab.com, or your own GitLab", "A Gitea or Forgejo server: on your cluster, or one that exists already", "Any git over HTTPS or SSH (no account API: the first fetch tells if it works)"];
        for (i, k) in Kind::ALL.iter().enumerate() {
            self.row(g, x, *y, w, s, accent, i == self.kind, Event::Tab(WIZ_BASE + 100 + i), &format!("{}  {}", i + 1, k.label()), subs[i]);
            *y += 52.0 * s;
        }
    }

    fn field(&self, g: &mut Gfx, x: f32, y: &mut f32, w: f32, s: f32, accent: [u8; 4], blink: bool, label: &str, f: usize, focused: bool) {
        g.text(x, *y, 13.0 * s, if focused { TEXT } else { MUTED }, label, Face::Bold);
        let r = R::new(x, *y + 20.0 * s, w, 36.0 * s);
        self.tf[f].render(g, r, focused, s, blink, Face::Sans, f);
        let _ = accent;
        *y += 64.0 * s;
    }

    fn choice(&self, g: &mut Gfx, x: f32, y: &mut f32, w: f32, s: f32, accent: [u8; 4], label: &str, shown: &str, focused: bool, ev: Event) {
        g.text(x, *y, 13.0 * s, if focused { TEXT } else { MUTED }, label, Face::Bold);
        let r = R::new(x, *y + 20.0 * s, w, 36.0 * s);
        fx::field_frame(g, r, s, focused, accent);
        g.text(r.x + 14.0 * s, r.y + 8.0 * s, 16.0 * s, TEXT, &format!("◀  {shown}  ▶"), Face::Sans);
        g.hit(r, ev, label);
        *y += 64.0 * s;
    }

    fn render_where(&mut self, g: &mut Gfx, x: f32, y: &mut f32, w: f32, s: f32, accent: [u8; 4], blink: bool) {
        let slots = self.slots();
        let cur = self.slot();
        let mut i = 0;
        if slots.first() == Some(&Slot::Mode) {
            let shown = if self.mode == 0 { "The Gitea running on my cluster" } else { "An existing Gitea / Forgejo server" };
            self.choice(g, x, y, w, s, accent, "Which Gitea?", shown, cur == Some(Slot::Mode), Event::Key(Key::Char(' ')));
            i += 1;
        }
        let _ = i;
        self.field(g, x, y, w, s, accent, blink, "Name of this connection", F_NAME, cur == Some(Slot::Name));
        if self.cluster_mode() {
            g.text(x, *y, 13.0 * s, if cur == Some(Slot::Cluster) { TEXT } else { MUTED }, "Gitea instances installed through Platform services", Face::Bold);
            *y += 22.0 * s;
            if let Some(h) = self.cluster_hint() {
                for l in g.wrap(13.0 * s, &h, Face::Sans, w) {
                    g.text(x, *y, 13.0 * s, AMBER, &l, Face::Sans);
                    *y += 18.0 * s;
                }
                g.button(x, *y + 6.0 * s, 13.0 * s, "Open Platform services", accent, Event::Tab(WIZ_BASE + 7));
            }
            for (k, c) in self.cluster.iter().enumerate().take(3) {
                let sub = if c.problem.is_empty() { format!("{} · {}", c.url, gitconn::placement_of(&c.url, true).label()) } else if c.url.is_empty() { c.problem.clone() } else { format!("{} · {}", c.url, c.problem) };
                let title = if c.found { format!("{} · namespace {}", c.title, c.namespace) } else { format!("{} ({})", c.title, c.instance) };
                self.row(g, x, *y, w, s, accent, k == self.cluster_sel, Event::Tab(WIZ_BASE + 100 + k), &title, &sub);
                *y += 52.0 * s;
            }
            if self.cluster.get(self.cluster_sel).is_some_and(|c| c.found) {
                g.button(x, *y + 4.0 * s, 13.0 * s, gitdiscover::ACTION, accent, Event::Tab(WIZ_BASE + 8));
                *y += 36.0 * s;
            }
            g.text(x, *y + 4.0 * s, 12.0 * s, DIM, if self.disc_rx.is_some() { "Looking at your cluster..." } else { "R looks at your cluster again." }, Face::Sans);
        } else {
            self.field(g, x, y, w, s, accent, blink, if self.kind_value() == Kind::Gitea { "Address of the server" } else { "Address of the host (change it for an Enterprise or self-hosted server)" }, F_URL, cur == Some(Slot::Url));
            let u = self.tf[F_URL].value.trim();
            if u.starts_with("http://") {
                g.text(x, *y - 10.0 * s, 13.0 * s, AMBER, "http:// is not encrypted: the token and your code travel in the clear. Use it only on a network you trust.", Face::Sans);
            } else if !u.is_empty() {
                let pl = gitconn::placement_of(u, false);
                g.text(x, *y - 10.0 * s, 13.0 * s, if pl == Placement::Internet { MUTED } else { OK }, &format!("Your code will go to {} ({}).", gitconn::host_of(u), pl.label()), Face::Sans);
            }
        }
    }

    fn render_signin(&mut self, g: &mut Gfx, x: f32, y: &mut f32, w: f32, s: f32, accent: [u8; 4], blink: bool) {
        if self.cluster_mode() {
            g.text(x, *y, 15.0 * s, TEXT, "Signed in by IYAKKI", Face::Bold);
            *y += 26.0 * s;
            for l in g.wrap(13.0 * s, "This Gitea was installed through Platform services, which already keep a token for it in your vault. Nothing more to enter: press Next to test it.", Face::Sans, w) {
                g.text(x, *y, 13.0 * s, MUTED, &l, Face::Sans);
                *y += 18.0 * s;
            }
            return;
        }
        let cur = self.slot();
        let c = self.draft();
        let shown = if self.method == Method::Token { "Personal access token" } else { "SSH key" };
        self.choice(g, x, y, w, s, accent, "How do you sign in?", shown, cur == Some(Slot::Method), Event::Key(Key::Char(' ')));
        if self.method == Method::Token {
            let label = if self.editing.as_ref().is_some_and(|e| e.has_token()) { "Replace the token (leave empty to keep the stored one)" } else { "Personal access token (paste it; it is kept in your vault and never shown again)" };
            self.field(g, x, y, w, s, accent, blink, label, F_TOKEN, cur == Some(Slot::Token));
            let url = if c.url.is_empty() { self.tf[F_URL].value.trim().to_string() } else { c.url.clone() };
            if !url.is_empty() {
                let need = match self.kind_value() {
                    Kind::GitHub => "the repo scope",
                    Kind::GitLab => "the api scope",
                    _ => "read:user and write:repository",
                };
                let t = g.fit(12.0 * s, &format!("Make one at {} with {need}.", self.kind_value().token_url(&url)), Face::Sans, w);
                g.text(x, *y - 8.0 * s, 12.0 * s, DIM, &t, Face::Sans);
            }
        } else {
            g.text(x, *y, 13.0 * s, if cur == Some(Slot::Keys) { TEXT } else { MUTED }, "SSH key (Up/Down to choose)", Face::Bold);
            *y += 22.0 * s;
            let max_rows = 3usize;
            let first = self.key_sel.saturating_sub(max_rows - 1).min(self.keys.len().saturating_sub(max_rows - 1));
            for i in first..(self.keys.len() + 1).min(first + max_rows) {
                let (t, sub) = match self.keys.get(i) {
                    Some(k) => (format!("{}", k.id), k.fingerprint.clone()),
                    None => ("Make a new key".to_string(), "an ed25519 key kept in your vault; you add its public half to the host".to_string()),
                };
                self.row(g, x, *y, w, s, accent, i == self.key_sel, Event::Tab(WIZ_BASE + 100 + i), &t, &sub);
                *y += 50.0 * s;
            }
            if self.key_sel >= self.keys.len() {
                self.field(g, x, y, w, s, accent, blink, "Name for the new key (lower case: gitea, work)", F_KEYNAME, cur == Some(Slot::KeyName));
            } else if let Some(k) = self.keys.get(self.key_sel) {
                let t = g.fit(12.0 * s, &k.public, Face::Mono, w - 90.0 * s);
                g.text(x, *y, 12.0 * s, OK, &t, Face::Mono);
                g.button(x + w - 80.0 * s, *y - 4.0 * s, 12.0 * s, "Copy  c", accent, Event::Tab(WIZ_BASE + 6));
                *y += 20.0 * s;
                let base = if c.url.is_empty() { self.tf[F_URL].value.trim().to_string() } else { c.url.clone() };
                if !base.is_empty() {
                    let t = g.fit(12.0 * s, &format!("Add this public key at {}", self.kind_value().key_url(&base)), Face::Sans, w);
                    g.text(x, *y, 12.0 * s, DIM, &t, Face::Sans);
                }
                *y += 22.0 * s;
            }
            if self.kind_value().has_api() && cur != Some(Slot::KeyName) {
                self.field(g, x, y, w, s, accent, blink, "Token for the host's API (optional: to list and make repositories)", F_TOKEN, cur == Some(Slot::ApiToken));
            }
        }
    }

    fn render_test(&mut self, g: &mut Gfx, x: f32, y: &mut f32, w: f32, bottom: f32, s: f32, accent: [u8; 4]) {
        let c = self.draft();
        g.text(x, *y, 15.0 * s, TEXT, &format!("Testing {} ({})", c.name, c.url), Face::Bold);
        *y += 28.0 * s;
        let line = |g: &mut Gfx, y: &mut f32, col: [u8; 4], t: &str| {
            for l in g.wrap(13.0 * s, t, Face::Sans, w) {
                if *y < bottom {
                    g.text(x, *y, 13.0 * s, col, &l, Face::Sans);
                }
                *y += 18.0 * s;
            }
        };
        if self.rx.is_some() && self.report.is_none() {
            line(g, y, MUTED, "Asking the host who you are…");
            return;
        }
        let Some(r) = self.report.clone() else { return };
        match &r.error {
            Some(e) => line(g, y, ERR, e),
            None if r.ok => line(g, y, OK, &r.summary()),
            None => {}
        }
        if r.ok || r.error.is_some() && !r.account.is_empty() {
            line(g, y, TEXT, &format!("Account: {}{}", r.account, if r.name.is_empty() { String::new() } else { format!(" ({})", r.name) }));
        }
        let yn = |v: Option<bool>| match v {
            Some(true) => "yes",
            Some(false) => "no",
            None => "could not be checked",
        };
        if r.can_read.is_some() || r.can_write.is_some() {
            line(g, y, TEXT, &format!("May read repositories: {}.   May write repositories: {}.", yn(r.can_read), yn(r.can_write)));
        }
        let tls = match r.tls {
            Tls::Plain => ("Not encrypted (http).".to_string(), AMBER),
            Tls::Verified => ("Encrypted; the certificate is trusted.".to_string(), OK),
            Tls::Pinned => (format!("Encrypted; the certificate is the one you chose to trust ({}).", self.trusted.as_ref().map(|c| c.fingerprint.as_str()).unwrap_or("kept for this connection")), OK),
            Tls::Untrusted => ("The server's certificate is not trusted.".to_string(), AMBER),
            Tls::Unknown => (String::new(), MUTED),
        };
        if !tls.0.is_empty() {
            line(g, y, tls.1, &tls.0);
        }
        line(g, y, MUTED, &format!("Where your code goes: {} ({}).", r.placement.label(), c.host()));
        for n in &r.notes {
            line(g, y, MUTED, n);
        }
        if r.tls == Tls::Untrusted {
            *y += 6.0 * s;
            match &self.cert {
                Some(cert) => {
                    line(g, y, TEXT, "The server presents a certificate with this fingerprint. Compare it with the one the server's owner gave you; trust it only if it is the same:");
                    for l in g.wrap(13.0 * s, &cert.fingerprint, Face::Mono, w) {
                        g.text(x, *y, 13.0 * s, accent, &l, Face::Mono);
                        *y += 18.0 * s;
                    }
                    g.button(x, *y + 4.0 * s, 13.0 * s, "Trust this certificate  t", accent, Event::Tab(WIZ_BASE + 4));
                    *y += 40.0 * s;
                }
                None if self.rx.is_some() => line(g, y, MUTED, "Reading the server's certificate…"),
                None => {}
            }
        }
        if self.rx.is_none() {
            let mut bx = x;
            bx += g.button(bx, bottom - 30.0 * s, 13.0 * s, "Test again  r", MUTED, Event::Tab(WIZ_BASE + 3)) + 8.0 * s;
            if !r.ok {
                g.button(bx, bottom - 30.0 * s, 13.0 * s, "Save anyway  s", AMBER, Event::Tab(WIZ_BASE + 5));
            }
        }
    }

    fn render_commits(&mut self, g: &mut Gfx, x: f32, y: &mut f32, w: f32, s: f32, accent: [u8; 4], blink: bool) {
        let cur = self.slot();
        for l in g.wrap(13.0 * s, "Commits made through this connection are signed with this name and email (git's user.name and user.email, kept for this connection).", Face::Sans, w) {
            g.text(x, *y, 13.0 * s, MUTED, &l, Face::Sans);
            *y += 18.0 * s;
        }
        if let Some(r) = self.report.as_ref().filter(|r| !r.name.is_empty() || !r.email.is_empty()) {
            let hint = format!("Your account on the server shows {} {}. Type them below if you want to commit as that; nothing is filled in for you.", if r.name.is_empty() { r.account.clone() } else { r.name.clone() }, if r.email.is_empty() { String::new() } else { format!("<{}>", r.email) });
            for l in g.wrap(12.0 * s, hint.trim(), Face::Sans, w) {
                g.text(x, *y + 4.0 * s, 12.0 * s, MUTED, &l, Face::Sans);
                *y += 16.0 * s;
            }
        }
        *y += 6.0 * s;
        self.field(g, x, y, w, s, accent, blink, "Name", F_IDNAME, cur == Some(Slot::IdName));
        self.field(g, x, y, w, s, accent, blink, "Email", F_EMAIL, cur == Some(Slot::Email));
        self.choice(g, x, y, w, s, accent, "Use this connection for new workspaces?", if self.make_default { "Yes: make it my default" } else { "No" }, cur == Some(Slot::Default), Event::Key(Key::Char(' ')));
    }
}

fn blank_report(c: &Connection) -> TestReport {
    TestReport::blank(c)
}

/// The dot of a connection in a list: green after a good test, red after a failed one, grey when not tested.
pub fn dot_color(c: &Connection) -> [u8; 4] {
    match &c.last_test {
        Some(t) if t.ok => OK,
        Some(_) => ERR,
        None => DIM,
    }
}

/// Starts the existing Quick install item "Gitea" (it keeps existing volumes and hands the token to the vault). Returns the plain line.
pub fn start_quick_install_gitea(cx: &super::AppCx) -> Result<String, String> {
    use iyakki_agent::quickinstall as qi;
    if !cx.session.policy.manage_settings {
        return Err(format!("Only an administrator can install software for the whole machine (your role is {}).", cx.session.role.name()));
    }
    let launch = qi::Launch::this_program(cx.services.dir())?;
    let svc = &cx.services;
    let present = |i: qi::Item| qi::presence(svc, i).present();
    let started = qi::start(&svc.jobs, &cx.session.user, qi::Item::Gitea, &present, &launch)?;
    cx.audit.record(&cx.session.user, "quickinstall.start", &format!("{} (from the Git wizard)", qi::Item::Gitea.id()));
    Ok(format!("Started the Quick install for Gitea ({} step{}). Your existing Gitea data is kept. Progress is beside the clock; when it finishes, the token waits for your vault (unlock it) and Add connection will offer it.", started.len(), if started.len() == 1 { "" } else { "s" }))
}

impl WizEnv {
    /// The real thing for the signed-in person: curl through the system runner, the person's policy for the network.
    /// `http` replaces the network in tests.
    pub fn real(cx: &super::AppCx, keys: Arc<GitKeys>, http: Option<Arc<dyn GitHttp>>) -> WizEnv {
        let policy = cx.session.policy.clone();
        let http: Arc<dyn GitHttp> = http.unwrap_or_else(|| Arc::new(iyakki_agent::gitnet::NetGit::default()));
        WizEnv {
            keys,
            http,
            dir: cx.services.dir().to_path_buf(),
            user: cx.session.user.clone(),
            egress: Arc::new(move |h| if policy.allows_host(h) { Ok(()) } else { Err(format!("Your account may not reach {h} (the network policy of this account).")) }),
            kube: {
                let k3s = cx.services.k3sdev.clone();
                Arc::new(move |a: &[&str]| {
                    if k3s.state() != iyakki_agent::k3sdev::Svc::Running {
                        return KOut::default();
                    }
                    let o = k3s.kubectl(a);
                    KOut { ok: o.code == Some(0), stdout: o.stdout.clone(), text: o.combined() }
                })
            },
        }
    }
}

#[cfg(test)]
#[path = "git_wizard_tests.rs"]
mod tests;
