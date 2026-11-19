//! What travels between fleet members once work has been sent (F-FLEET-03 to 06; docs/development/FLEET-AND-MODEL-ROUTER-PLAN.md
//! sections 3.3 and 3.4): the status of a task, the **hand-back** of a finished task as a Git bundle (in small sealed chunks, checked by a
//! SHA-256 and by `git bundle verify` before anything is applied), and the shared project Context. Every message is the same signed and
//! sealed member message as a heartbeat; what arrives is checked here against what this system itself sent (a hand-back for a task this
//! system did not send to that member is refused).
//!
//! Nothing here runs git or a task: the hub only keeps the bytes (a bundle file in the hub's own folder, mode 0600) and the state.

use std::io::Write;
use std::sync::MutexGuard;

use super::*;
use crate::cloudkeys::unhex;

/// Raw bytes in one chunk of a bundle (the hex form is twice this, well inside one frame).
pub const CHUNK: usize = 32 * 1024;
/// The largest bundle this system keeps.
pub const MAX_BUNDLE: u64 = 512 * 1024 * 1024;

const TERMINAL: [&str; 5] = ["returned", "question", "requeued", "declined", "lost"];

/// Nothing more is expected for a task in this state (a failure counts: a task has one answer).
fn is_final(state: &str) -> bool {
    TERMINAL.contains(&state) || state.starts_with("failed")
}

/// A bundle being received.
pub(super) struct Assembly {
    next: u32,
    got: u64,
}

/// What a worker handed back (the meta file kept beside the bundle).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Returned {
    pub id: String,
    pub fleet: String,
    pub from: String,
    pub from_name: String,
    /// `done`, `failed` or `question`.
    pub status: String,
    pub head: String,
    pub base: String,
    pub branch: String,
    pub tests: String,
    pub summary: String,
    pub sha256: String,
    pub size: u64,
    /// A bundle file is kept (a failure or a question has none).
    pub bundle: bool,
    pub at: u64,
    /// The tasks this one must come after (from the assignment this system sent).
    #[serde(default)]
    pub after: Vec<String>,
    #[serde(default)]
    pub title: String,
    /// The files the task changed (names only, at most 40), so the planner sees the size of a step's result without the bundle (F-ROUTER-15).
    #[serde(default)]
    pub files: Vec<String>,
}

/// Which board row is with which member, from the assignments this system sent and the status messages that came back.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct RowOwner {
    pub row: String,
    pub member: String,
    pub task: String,
    /// `waiting` (their person has not answered), `accepted`, `running`, `needs_you` (waiting on their person), `paused`, `lost`.
    pub state: String,
}

/// Takes the shared Context a member sent: (fleet, sender's member id, sender's name, the message). Returns a plain line for the log.
/// What this computer's own router knows about its models (profiles carry no key and no address); answers a member's `models_query`.
pub type ModelsHandler = Arc<dyn Fn() -> Vec<crate::router_registry::Profile> + Send + Sync>;
pub type ContextHandler = Arc<dyn Fn(&str, &str, &str, &Value) -> Result<String, String> + Send + Sync>;

/// The SHA-256 of a file, read in pieces (a bundle may be hundreds of MB).
fn sha_file(path: &Path) -> Result<String, String> {
    use sha2::{Digest, Sha256};
    use std::io::Read;
    let mut f = std::fs::File::open(path).map_err(|e| format!("the bundle could not be read: {e}"))?;
    let mut h = Sha256::new();
    let mut buf = vec![0u8; 256 * 1024];
    loop {
        let n = f.read(&mut buf).map_err(|e| format!("the bundle could not be read: {e}"))?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
    }
    Ok(hex(&h.finalize()))
}

fn hexish(s: &str, len: &[usize]) -> bool {
    len.contains(&s.len()) && s.chars().all(|c| c.is_ascii_hexdigit())
}

fn safe_text(s: &str, max: usize) -> String {
    let c = clean(s, max);
    if looks_secret(&c) {
        "(withheld: it looked like a secret)".into()
    } else {
        c
    }
}

impl Hub {
    /// The worker side runs when this is called (an assignment arrived or was decided): set by whoever owns the task manager.
    pub fn set_work_hook(&self, f: Arc<dyn Fn() + Send + Sync>) {
        *lock(&self.work_hook) = Some(f);
    }

    pub fn set_models_handler(&self, f: ModelsHandler) {
        *lock(&self.models_handler) = Some(f);
    }

    /// Ask one member for the models its own router knows (only that router can say how it rates them), as profiles. Read only; sealed to
    /// the member. A model's zone is made no closer than the network: from here the member's computer is another computer.
    pub fn query_models(&self, fleet: &str, member: &str) -> Result<Vec<crate::router_registry::Profile>, String> {
        use crate::router_registry::Zone;
        let body = self.send_member(fleet, member, "models_query", &json!({}))?;
        let list: Vec<crate::router_registry::Profile> = serde_json::from_value(body["models"].clone()).map_err(|_| "the member's model list is not in the agreed form".to_string())?;
        Ok(list
            .into_iter()
            .take(200)
            .map(|mut p| {
                p.zone = p.zone.max(Zone::Network);
                p.local = p.zone != Zone::Internet;
                p.provider = clean(&p.provider, 60);
                p.model = clean(&p.model, 120);
                p.id = clean(&p.id, 200);
                p
            })
            .collect())
    }

    pub fn set_context_handler(&self, f: ContextHandler) {
        *lock(&self.ctx_handler) = Some(f);
    }

    /// The workspace of the IDE that runs work from other members and shares its Context (the last one opened).
    pub fn set_attached(&self, a: Arc<crate::fleet_work::Attached>) {
        *lock(&self.attached) = Some(a);
    }

    pub fn attached(&self) -> Option<Arc<crate::fleet_work::Attached>> {
        lock(&self.attached).clone()
    }

    /// One access-log line written by the caller (ids and numbers only).
    pub fn audit_line(&self, what: &str, detail: &str) {
        self.audit(what, detail);
    }

    /// Called by the hub after each heartbeat round: where the automatic requeue of a silent member's tasks runs (when the person allowed it).
    pub fn set_lost_hook(&self, f: Arc<dyn Fn() + Send + Sync>) {
        *lock(&self.lost_hook) = Some(f);
    }

    pub fn kick_lost(&self) {
        if !self.auto_requeue() {
            return;
        }
        if let Some(f) = lock(&self.lost_hook).clone() {
            std::thread::spawn(move || f());
        }
    }

    pub(super) fn kick(&self) {
        if let Some(f) = lock(&self.work_hook).clone() {
            std::thread::spawn(move || f());
        }
    }

    fn bundle_dir(&self) -> PathBuf {
        self.fd().join("bundles")
    }

    /// Where the bundle of a returned task is kept.
    pub fn bundle_path(&self, task: &str) -> Result<PathBuf, String> {
        if !hexish(task, &[32]) {
            return Err("not a task id".into());
        }
        Ok(self.bundle_dir().join(format!("{task}.bundle")))
    }

    /// Change what this system shows about a task it received (`running`, `done`, `failed ...`) and which local task runs it.
    pub fn inbox_set(&self, id: &str, state: &str, task: &str, detail: &str) {
        let mut i = lock(&self.inner);
        if let Some(x) = i.inbox.iter_mut().find(|x| x.id == id) {
            x.state = state.into();
            if !task.is_empty() {
                x.task = task.into();
            }
            x.detail = clean(detail, 200);
        }
        self.save_misc(&i);
    }

    /// Send one sealed member message to one member and read its answer. The caller has asked the core.
    pub fn send_member(&self, fleet: &str, to: &str, kind: &str, plain: &Value) -> Result<Value, String> {
        let (addr, env) = {
            let i = lock(&self.inner);
            let f = Self::pick(&i, fleet)?;
            let m = f.rec.find(to)?.clone();
            let addr = f.peers.get(&m.id).and_then(|p| p.addr.clone()).ok_or("this computer does not know where that member is yet")?;
            (addr, self.sealed(&i, &f.rec.fleet, kind, plain)?)
        };
        let r = self.send_to(&addr, &env)?;
        if r.ok {
            Ok(r.body)
        } else {
            Err(r.error)
        }
    }

    /// The same message to every other member; one line per member (its name and what came of it).
    pub fn broadcast(&self, fleet: &str, kind: &str, plain: &Value) -> Result<Vec<(String, Result<(), String>)>, String> {
        let (me, members) = {
            let i = lock(&self.inner);
            let f = Self::pick(&i, fleet)?;
            (i.keys.public().sign, f.rec.members.iter().map(|m| (m.id.clone(), m.name.clone())).collect::<Vec<_>>())
        };
        Ok(members.into_iter().filter(|(id, _)| *id != me).map(|(id, name)| (name, self.send_member(fleet, &id, kind, plain).map(|_| ()))).collect())
    }

    /// The person turns sharing the Context of one fleet on or off for this computer (sending needs a separate ask).
    pub fn set_context_sync(&self, fleet: &str, on: bool) -> Result<String, String> {
        let mut i = lock(&self.inner);
        let fid = Self::pick(&i, fleet)?.rec.fleet.clone();
        if on {
            i.config.context_sync.insert(fid.clone());
        } else {
            i.config.context_sync.remove(&fid);
        }
        self.save_misc(&i);
        Ok(fid)
    }

    pub fn context_sync_on(&self, fleet: &str) -> bool {
        lock(&self.inner).config.context_sync.contains(fleet)
    }

    // ------------------------------------------------------------------------------------------ the worker sends

    /// The planner's answer to a question of a task it sent, handed over once (the worker gives it to the running task).
    pub fn take_answer(&self, id: &str) -> Option<String> {
        let mut i = lock(&self.inner);
        let x = i.inbox.iter_mut().find(|x| x.id == id)?;
        let a = std::mem::take(&mut x.answer);
        if a.is_empty() {
            return None;
        }
        self.save_misc(&i);
        Some(a)
    }

    /// Answer the question of a task this system sent to a member (the caller has asked the core): sealed to that member only.
    pub fn send_answer(&self, fleet: &str, to: &str, task: &str, text: &str) -> Result<(), String> {
        {
            let i = lock(&self.inner);
            let mine = i.outbox.iter().any(|o| o.id == task && !is_final(&o.state));
            if !mine {
                return Err("no such task is out with a member (it may be over already)".into());
            }
        }
        self.send_member(fleet, to, "task_answer", &json!({"id": task, "text": clean(text, 600)})).map(|_| ())
    }

    /// Tell the assigner how a task is going (`running`, `needs_you`, `paused`). Numbers and words only.
    pub fn send_status(&self, item: &InboxItem, state: &str, detail: &str) -> Result<(), String> {
        self.send_member(&item.fleet, &item.from, "task_status", &json!({"id": item.id, "state": state, "detail": clean(detail, 120)})).map(|_| ())
    }

    /// Hand a finished task back: the bundle (when there is one) in chunks, with the head commit and the test result this system saw. Without
    /// a bundle file the message is a failure or a question.
    #[allow(clippy::too_many_arguments)]
    pub fn send_handback(&self, item: &InboxItem, status: &str, head: &str, base: &str, branch: &str, tests: &str, summary: &str, bundle: Option<&Path>) -> Result<(), String> {
        self.send_handback_with(item, status, head, base, branch, tests, summary, bundle, &[])
    }

    /// [`Hub::send_handback`] with the names of the files the task changed.
    #[allow(clippy::too_many_arguments)]
    pub fn send_handback_with(&self, item: &InboxItem, status: &str, head: &str, base: &str, branch: &str, tests: &str, summary: &str, bundle: Option<&Path>, files: &[String]) -> Result<(), String> {
        let files: Vec<String> = files.iter().take(40).map(|f| clean(f, 120)).collect();
        let meta = |index: u32, total: u32, data: &str, sha: &str, size: u64| {
            json!({"id": item.id, "status": status, "head": head, "base": base, "branch": branch, "tests": clean(tests, 600), "summary": clean(summary, 600), "files": files, "sha256": sha, "size": size, "index": index, "total": total, "data": data})
        };
        let Some(path) = bundle else {
            self.send_member(&item.fleet, &item.from, "handback", &meta(0, 0, "", "", 0))?;
            self.audit("fleet.handback.out", &format!("task={} status={status} bundle=no", &item.id[..8]));
            return Ok(());
        };
        let size = std::fs::metadata(path).map_err(|e| format!("the bundle could not be read: {e}"))?.len();
        if size > MAX_BUNDLE {
            return Err(format!("the bundle is bigger than the {} MB this system sends", MAX_BUNDLE / 1024 / 1024));
        }
        if size == 0 {
            return Err("the bundle is empty".into());
        }
        let sha = sha_file(path)?;
        let total = size.div_ceil(CHUNK as u64) as u32;
        // after a restart of either side the receiver may already hold the first chunks of this very bundle: ask, and send the rest
        let have = self.send_member(&item.fleet, &item.from, "handback_have", &json!({"id": item.id, "sha256": sha, "size": size})).ok().and_then(|v| v["chunks"].as_u64()).unwrap_or(0).min(total.saturating_sub(1) as u64) as u32;
        let mut f = std::fs::File::open(path).map_err(|e| format!("the bundle could not be read: {e}"))?;
        {
            use std::io::{Seek, SeekFrom};
            f.seek(SeekFrom::Start(have as u64 * CHUNK as u64)).map_err(|e| e.to_string())?;
        }
        for n in have..total {
            let want = (size - n as u64 * CHUNK as u64).min(CHUNK as u64) as usize;
            let mut c = vec![0u8; want];
            std::io::Read::read_exact(&mut f, &mut c).map_err(|e| format!("the bundle could not be read: {e}"))?;
            self.send_member(&item.fleet, &item.from, "handback", &meta(n, total, &hex(&c), &sha, size))?;
        }
        self.audit("fleet.handback.out", &format!("task={} status={status} bundle=yes bytes={size} chunks={total} resumed_at={have}", &item.id[..8]));
        Ok(())
    }

    // ------------------------------------------------------------------------------------------ the planner reads

    /// What came back for a fleet, oldest first.
    pub fn returned(&self, fleet: &str) -> Vec<Returned> {
        let mut v: Vec<Returned> = std::fs::read_dir(self.bundle_dir())
            .map(|rd| rd.flatten().filter(|e| e.path().extension().is_some_and(|x| x == "json")).filter_map(|e| std::fs::read_to_string(e.path()).ok()).filter_map(|t| serde_json::from_str::<Returned>(&t).ok()).filter(|r| fleet.is_empty() || r.fleet == fleet).collect())
            .unwrap_or_default();
        v.sort_by(|a, b| (a.at, &a.id).cmp(&(b.at, &b.id)));
        v
    }

    /// The board rows that are with other members, with the member's name and how the task stands (F-FLEET-04).
    pub fn rows_with_members(&self) -> Vec<RowOwner> {
        let now = self.now();
        let interval = lock(&self.interval).as_secs().max(1);
        let i = lock(&self.inner);
        i.outbox
            .iter()
            .filter(|o| !is_final(&o.state))
            .flat_map(|o| {
                let lost = Self::member_lost(&i, &o.fleet, &o.to, now, interval);
                o.rows.iter().map(move |r| RowOwner { row: r.clone(), member: o.to_name.clone(), task: o.id.clone(), state: if lost { "lost".into() } else { o.state.clone() } })
            })
            .collect()
    }

    fn member_lost(i: &Inner, fleet: &str, member: &str, now: u64, interval: u64) -> bool {
        let lost_after = (interval * 3).max(3);
        i.fleets.get(fleet).and_then(|f| f.peers.get(member)).is_some_and(|p| p.last_seen > 0 && now.saturating_sub(p.last_seen) > lost_after)
    }

    /// Tasks sent to a member that has gone silent (lost) and have not come back: they are marked `requeued` here and returned so the caller can
    /// send them to another member. A hand-back that arrives later for a requeued task is dropped (a task has one answer).
    pub fn requeue_lost(&self) -> Vec<OutItem> {
        let now = self.now();
        let interval = lock(&self.interval).as_secs().max(1);
        let mut i = lock(&self.inner);
        let lost: Vec<usize> = (0..i.outbox.len()).filter(|n| matches!(i.outbox[*n].state.as_str(), "accepted" | "running" | "needs_you" | "paused" | "waiting") && Self::member_lost(&i, &i.outbox[*n].fleet, &i.outbox[*n].to, now, interval)).collect();
        let mut out = vec![];
        for n in lost {
            i.outbox[n].state = "requeued".into();
            out.push(i.outbox[n].clone());
        }
        if !out.is_empty() {
            self.save_misc(&i);
        }
        drop(i);
        for o in &out {
            self.audit("fleet.task.requeue", &format!("fleet={} was_with={} task={}", short_id(&o.fleet), fingerprint(&o.to), &o.id[..8]));
        }
        out
    }

    // ------------------------------------------------------------------------------------------ what arrives

    pub(super) fn handle_work(&self, mut i: MutexGuard<'_, Inner>, env: &Env, fid: &str, sender: &str, plain: Value) -> Reply {
        match env.kind.as_str() {
            "task_status" => {
                let id = plain["id"].as_str().unwrap_or("").to_string();
                let st = plain["state"].as_str().unwrap_or("").to_string();
                if !matches!(st.as_str(), "running" | "needs_you" | "paused") {
                    return Reply::err("bad status");
                }
                let detail = clean(plain["detail"].as_str().unwrap_or(""), 120);
                match i.outbox.iter_mut().find(|o| o.id == id && o.to == env.from && o.fleet == fid) {
                    Some(o) if is_final(&o.state) => return Reply::ok("dropped", Value::Null),
                    Some(o) => {
                        o.state = st.clone();
                        o.detail = detail;
                    }
                    None => return Reply::err("no such task was sent to you"),
                }
                self.save_misc(&i);
                drop(i);
                self.audit("fleet.task.status", &format!("from={} task={} state={st}", fingerprint(&env.from), &id[..8.min(id.len())]));
                Reply::ok("ok", Value::Null)
            }
            "base_chunk" => self.handle_base_chunk(i, env, fid, &plain),
            "task_answer" => {
                // the planner answers a question of the task it sent: kept for the task, and only from the member that sent the task
                let id = plain["id"].as_str().unwrap_or("").to_string();
                let text = safe_text(plain["text"].as_str().unwrap_or(""), 600);
                match i.inbox.iter_mut().find(|x| x.id == id && x.from == env.from && x.fleet == fid) {
                    Some(x) if x.state == "running" || x.state == "accepted" => x.answer = text,
                    Some(_) => return Reply::err("that task is not running here"),
                    None => return Reply::err("no such task was sent here"),
                }
                self.save_misc(&i);
                drop(i);
                self.audit("fleet.task.answer", &format!("from={} task={}", fingerprint(&env.from), &id[..8.min(id.len())]));
                Reply::ok("ok", Value::Null)
            }
            "handback" => self.handle_handback(i, env, fid, sender, &plain),
            "handback_have" => {
                let id = plain["id"].as_str().unwrap_or("");
                let (sha, size) = (plain["sha256"].as_str().unwrap_or(""), plain["size"].as_u64().unwrap_or(0));
                if !i.outbox.iter().any(|o| o.id == id && o.to == env.from && o.fleet == fid && !is_final(&o.state)) || !hexish(sha, &[64]) || size == 0 || size > MAX_BUNDLE {
                    return Reply::ok("have", json!({"chunks": 0}));
                }
                let dir = self.bundle_dir();
                let same = std::fs::read_to_string(dir.join(format!("{id}.partmeta"))).is_ok_and(|m| m.trim() == format!("{sha} {size}"));
                let len = std::fs::metadata(dir.join(format!("{id}.part"))).map(|m| m.len()).unwrap_or(0);
                // only whole chunks count; the last chunk of the file is never "had" (it completes the bundle)
                Reply::ok("have", json!({"chunks": if same { len / CHUNK as u64 } else { 0 }}))
            }
            "context" => {
                if !i.config.context_sync.contains(fid) {
                    drop(i);
                    self.audit_once(&format!("cx/{}", env.from), "fleet.context.refused", &format!("reason=not_shared from={}", fingerprint(&env.from)));
                    return Reply::err("this computer does not share the project Context with that fleet");
                }
                drop(i);
                let Some(h) = lock(&self.ctx_handler).clone() else { return Reply::err("no project is open on this computer") };
                match h(fid, &env.from, sender, &plain) {
                    Ok(line) => {
                        self.audit("fleet.context.in", &format!("fleet={} from={} {}", short_id(fid), fingerprint(&env.from), clean(&line, 80)));
                        Reply::ok("ok", json!({"line": line}))
                    }
                    Err(e) => Reply::err(&e),
                }
            }
            _ => Reply::err("a message kind this system does not know"),
        }
    }

    /// The folder a bundle of a base commit is kept in on a worker.
    pub fn base_path(&self, commit: &str) -> Option<PathBuf> {
        if !hexish(commit, &[40, 64]) {
            return None;
        }
        let p = self.fd().join("bases").join(format!("{commit}.bundle"));
        p.is_file().then_some(p)
    }

    /// Send the planner's commits to a worker (the base of a follow-up task: the integration branch the worker does not have), as chunks like a
    /// hand-back, in the other direction. `commit` names the bundle. The caller has asked the core.
    pub fn send_base(&self, fleet: &str, to: &str, commit: &str, bundle: &Path) -> Result<(), String> {
        if !hexish(commit, &[40, 64]) {
            return Err("the base commit is not a commit id".into());
        }
        let size = std::fs::metadata(bundle).map_err(|e| format!("the bundle could not be read: {e}"))?.len();
        if size == 0 || size > MAX_BUNDLE {
            return Err(format!("the bundle is empty or bigger than the {} MB this system sends", MAX_BUNDLE / 1024 / 1024));
        }
        let sha = sha_file(bundle)?;
        let total = size.div_ceil(CHUNK as u64) as u32;
        let mut f = std::fs::File::open(bundle).map_err(|e| e.to_string())?;
        for n in 0..total {
            let want = (size - n as u64 * CHUNK as u64).min(CHUNK as u64) as usize;
            let mut c = vec![0u8; want];
            std::io::Read::read_exact(&mut f, &mut c).map_err(|e| e.to_string())?;
            self.send_member(fleet, to, "base_chunk", &json!({"commit": commit, "sha256": sha, "size": size, "index": n, "total": total, "data": hex(&c)}))?;
        }
        self.audit("fleet.base.out", &format!("commit={} bytes={size} chunks={total}", &commit[..12]));
        Ok(())
    }

    fn handle_base_chunk(&self, mut i: MutexGuard<'_, Inner>, env: &Env, _fid: &str, p: &Value) -> Reply {
        let s = |k: &str| p.get(k).and_then(Value::as_str).unwrap_or("").to_string();
        let n = |k: &str| p.get(k).and_then(Value::as_u64).unwrap_or(0);
        let (commit, sha) = (s("commit"), s("sha256"));
        let (size, index, total) = (n("size"), n("index") as u32, n("total") as u32);
        // only from the member that is allowed to send this computer work, and only while this computer takes work at all
        if i.config.accept == Accept::Never || (i.config.accept == Accept::Members && !i.config.trusted.contains(&env.from)) {
            return Reply::err("this computer does not take a base from that member");
        }
        if !hexish(&commit, &[40, 64]) || !hexish(&sha, &[64]) || size == 0 || size > MAX_BUNDLE || total as u64 != size.div_ceil(CHUNK as u64) || index >= total {
            return Reply::err("the base's size, chunk count or checksum is not valid");
        }
        let data = match unhex(&s("data")) {
            Some(d) if !d.is_empty() && d.len() <= CHUNK => d,
            _ => return Reply::err("a chunk of the base is not valid"),
        };
        let dir = self.fd().join("bases");
        let part = dir.join(format!("{commit}.part"));
        let key = format!("base:{commit}");
        let a = i.assembly.entry(key.clone()).or_insert(Assembly { next: 0, got: 0 });
        if index == 0 {
            *a = Assembly { next: 0, got: 0 };
        }
        if index != a.next || a.got + data.len() as u64 > size {
            i.assembly.remove(&key);
            let _ = std::fs::remove_file(&part);
            return Reply::err("the chunks of the base came out of order");
        }
        let wrote = (|| -> Result<(), String> {
            use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
            std::fs::DirBuilder::new().recursive(true).mode(0o700).create(&dir).map_err(|e| e.to_string())?;
            let mut f = std::fs::OpenOptions::new().create(true).write(true).truncate(index == 0).append(index != 0).mode(0o600).open(&part).map_err(|e| e.to_string())?;
            f.write_all(&data).map_err(|e| e.to_string())
        })();
        if let Err(e) = wrote {
            i.assembly.remove(&key);
            return Reply::err(&format!("could not keep the base: {e}"));
        }
        let a = i.assembly.get_mut(&key).unwrap_or_else(|| unreachable!());
        a.next += 1;
        a.got += data.len() as u64;
        if index + 1 < total {
            return Reply::ok("chunk", Value::Null);
        }
        let complete = a.got == size;
        i.assembly.remove(&key);
        if !complete || sha_file(&part).ok().as_deref() != Some(sha.as_str()) {
            let _ = std::fs::remove_file(&part);
            return Reply::err("the base does not match its checksum");
        }
        if std::fs::rename(&part, dir.join(format!("{commit}.bundle"))).is_err() {
            return Reply::err("could not keep the base");
        }
        drop(i);
        self.audit("fleet.base.in", &format!("from={} commit={}", fingerprint(&env.from), &commit[..12]));
        Reply::ok("ok", Value::Null)
    }

    fn handle_handback(&self, mut i: MutexGuard<'_, Inner>, env: &Env, fid: &str, sender: &str, p: &Value) -> Reply {
        let s = |k: &str| p.get(k).and_then(Value::as_str).unwrap_or("").to_string();
        let n = |k: &str| p.get(k).and_then(Value::as_u64).unwrap_or(0);
        let id = s("id");
        let status = s("status");
        if !matches!(status.as_str(), "done" | "failed" | "question") {
            return Reply::err("bad hand-back status");
        }
        let Some(out) = i.outbox.iter().find(|o| o.id == id && o.to == env.from && o.fleet == fid).cloned() else {
            drop(i);
            self.audit("fleet.handback.refused", &format!("from={} reason=not_sent_to_you", fingerprint(&env.from)));
            return Reply::err("no such task was sent to you");
        };
        if is_final(&out.state) {
            // a task has one answer: a late one (after a re-queue, or twice) is dropped
            drop(i);
            self.audit("fleet.handback.dropped", &format!("from={} task={} state={}", fingerprint(&env.from), &id[..8], out.state));
            return Reply::ok("dropped", Value::Null);
        }
        let (head, base) = (s("head"), s("base"));
        let branch = s("branch");
        let word = |t: &str, max: usize| t.len() <= max && t.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '/' | '-' | ':'));
        if !(head.is_empty() || hexish(&head, &[40, 64])) || !(base.is_empty() || hexish(&base, &[40, 64])) || !word(&branch, 160) {
            return Reply::err("the hand-back has a head, base or branch that is not valid");
        }
        let (total, index, size) = (n("total") as u32, n("index") as u32, n("size"));
        let meta = Returned {
            id: id.clone(),
            fleet: fid.into(),
            from: env.from.clone(),
            from_name: sender.into(),
            status: status.clone(),
            head,
            base,
            branch,
            tests: safe_text(&s("tests"), 600),
            summary: safe_text(&s("summary"), 600),
            sha256: s("sha256"),
            size,
            bundle: total > 0,
            at: self.now(),
            after: out.body.as_ref().map(|b| b.after.clone()).unwrap_or_default(),
            title: out.title.clone(),
            files: p.get("files").and_then(Value::as_array).map(|a| a.iter().filter_map(Value::as_str).take(40).map(|f| safe_text(f, 120)).collect()).unwrap_or_default(),
        };
        let dir = self.bundle_dir();
        if total > 0 {
            if !hexish(&meta.sha256, &[64]) || size == 0 || size > MAX_BUNDLE || total as u64 != size.div_ceil(CHUNK as u64) || index >= total {
                return Reply::err("the bundle's size, chunk count or checksum is not valid");
            }
            let data = match unhex(&s("data")) {
                Some(d) if !d.is_empty() && d.len() <= CHUNK => d,
                _ => return Reply::err("a chunk of the bundle is not valid"),
            };
            let part = dir.join(format!("{id}.part"));
            // a restart lost the count in memory: the part file and its note say how far the same bundle got
            if index > 0 && !i.assembly.contains_key(&id) {
                let same = std::fs::read_to_string(dir.join(format!("{id}.partmeta"))).is_ok_and(|m| m.trim() == format!("{} {size}", meta.sha256));
                let len = std::fs::metadata(&part).map(|m| m.len()).unwrap_or(0);
                if same && len == index as u64 * CHUNK as u64 {
                    i.assembly.insert(id.clone(), Assembly { next: index, got: len });
                }
            }
            let a = i.assembly.entry(id.clone()).or_insert(Assembly { next: 0, got: 0 });
            if index == 0 {
                *a = Assembly { next: 0, got: 0 };
            }
            if index != a.next || a.got + data.len() as u64 > size {
                i.assembly.remove(&id);
                let _ = std::fs::remove_file(&part);
                return Reply::err("the chunks of the bundle came out of order");
            }
            let wrote = (|| -> Result<(), String> {
                use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
                std::fs::DirBuilder::new().recursive(true).mode(0o700).create(&dir).map_err(|e| e.to_string())?;
                if index == 0 {
                    write_private(&dir.join(format!("{id}.partmeta")), &format!("{} {size}", meta.sha256)).map_err(|e| e.to_string())?;
                }
                let mut f = std::fs::OpenOptions::new().create(true).write(true).truncate(index == 0).append(index != 0).mode(0o600).open(&part).map_err(|e| e.to_string())?;
                f.write_all(&data).map_err(|e| e.to_string())
            })();
            if let Err(e) = wrote {
                i.assembly.remove(&id);
                return Reply::err(&format!("could not keep the bundle: {e}"));
            }
            let a = i.assembly.get_mut(&id).unwrap_or_else(|| unreachable!());
            a.next += 1;
            a.got += data.len() as u64;
            if index + 1 < total {
                return Reply::ok("chunk", Value::Null);
            }
            // the last chunk: the whole thing must match what the sender said
            let complete = a.got == size;
            i.assembly.remove(&id);
            let _ = std::fs::remove_file(dir.join(format!("{id}.partmeta")));
            if !complete || sha_file(&part).ok().as_deref() != Some(meta.sha256.as_str()) {
                let _ = std::fs::remove_file(&part);
                drop(i);
                self.audit("fleet.handback.refused", &format!("from={} task={} reason=checksum", fingerprint(&env.from), &id[..8]));
                return Reply::err("the bundle does not match its checksum");
            }
            if std::fs::rename(&part, dir.join(format!("{id}.bundle"))).is_err() {
                return Reply::err("could not keep the bundle");
            }
        }
        if write_private(&dir.join(format!("{id}.json")), &serde_json::to_string(&meta).unwrap_or_default()).is_err() {
            return Reply::err("could not keep the hand-back");
        }
        if let Some(o) = i.outbox.iter_mut().find(|o| o.id == id) {
            o.state = match status.as_str() {
                "done" => "returned".into(),
                "question" => "question".into(),
                _ => format!("failed: {}", clean(&meta.summary, 80)),
            };
            o.head = meta.head.clone();
            o.tests = clean(&meta.tests, 200);
        }
        self.save_misc(&i);
        drop(i);
        self.audit("fleet.handback.in", &format!("fleet={} from={} task={} status={status} bundle={} bytes={size}", short_id(fid), fingerprint(&env.from), &id[..8], meta.bundle));
        Reply::ok("returned", Value::Null)
    }
}
