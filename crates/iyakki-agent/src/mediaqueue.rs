//! The download queue: addresses a person adds (in the MEDIA app, or through the chat tool `media_download`) wait their turn
//! (first in, first out, at most [`MAX_ACTIVE`] at a time), run through [`crate::mediadl`], and end as *Succeeded*,
//! *Failed* (with the reason in plain English) or *Cancelled*. The list is kept per person in
//! `media-downloads-<user>.json` under the services folder and survives a restart: whatever was queued or running when the
//! program stopped comes back as *Failed: interrupted*, to be retried with one key.
//!
//! The folder is chosen automatically by the kind of file unless the person picked one: songs go to Music, videos to
//! Videos, pictures to Pictures (see [`crate::mediadl::Target::Auto`]). The words typed as a category are free text; see
//! [`crate::mediadl::Choice::from_words`].

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use iyakki_sec::policy::Policy;
use serde::{Deserialize, Serialize};

use crate::mediadl::{self, Choice, Fetched, Progress, Target};
use crate::mediaindex::ItemKind;
use crate::services::Services;

/// How many downloads run at the same time.
pub const MAX_ACTIVE: usize = 2;
/// How many addresses may wait or run at once.
pub const MAX_UNFINISHED: usize = 60;
/// How many entries the list keeps (the oldest finished ones go first).
pub const MAX_ENTRIES: usize = 200;
/// What a restart makes of a download that was not finished.
pub const INTERRUPTED: &str = "Interrupted: IYAKKI was restarted before it finished. Retry it.";

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Status {
    Queued,
    Downloading,
    Succeeded,
    Failed,
    Cancelled,
}

impl Status {
    pub fn label(self) -> &'static str {
        match self {
            Status::Queued => "Queued",
            Status::Downloading => "Downloading",
            Status::Succeeded => "Succeeded",
            Status::Failed => "Failed",
            Status::Cancelled => "Cancelled",
        }
    }
    pub fn finished(self) -> bool {
        matches!(self, Status::Succeeded | Status::Failed | Status::Cancelled)
    }
}

/// What a person asks for: an address and the free-text words that go with it.
#[derive(Clone, Debug, Default)]
pub struct Request {
    pub url: String,
    /// Free-text categories (any words); known ones become types, the others tags.
    pub categories: Vec<String>,
    pub tags: Vec<String>,
    pub rating: u8,
    /// A folder chosen by hand; `None` = automatic, by the kind of file.
    pub folder: Option<String>,
}

/// One entry of the list, as it is saved.
#[derive(Clone, Debug, Serialize, Deserialize)]
struct Saved {
    id: u64,
    url: String,
    status: Status,
    categories: Vec<String>,
    tags: Vec<String>,
    rating: u8,
    folder: Option<String>,
    kind: Option<ItemKind>,
    /// The folder the file goes (or went) to.
    dest: String,
    file: String,
    size: u64,
    error: String,
    note: String,
    added: i64,
    finished: i64,
}

#[derive(Serialize, Deserialize, Default)]
struct Disk {
    next_id: u64,
    items: Vec<Saved>,
}

struct Entry {
    s: Saved,
    live: Option<Arc<Progress>>,
    started: Option<Instant>,
    cancelling: bool,
}

#[derive(Default)]
struct State {
    next_id: u64,
    items: Vec<Entry>,
    /// Finished since the app looked: how many, and the last one (for a flash in the header).
    finished_seq: u64,
    last: Option<(u64, Status)>,
}

/// Everything a worker needs from the person's session.
#[derive(Clone)]
pub struct Context {
    pub svc: Arc<Services>,
    pub policy: Policy,
    pub user: String,
    /// The folders an automatic download may go to (the person's library folders and Pictures; only writable ones are used).
    pub folders: Vec<String>,
}

struct Inner {
    path: PathBuf,
    state: Mutex<State>,
    ctx: Mutex<Option<Context>>,
    max_active: AtomicUsize,
}

/// What the list shows for one entry.
#[derive(Clone, Debug, PartialEq)]
pub struct View {
    pub id: u64,
    pub url: String,
    pub status: Status,
    pub kind: Option<ItemKind>,
    /// The folder as far as it is known: the chosen one, the automatic choice, or the one the file went to.
    pub folder: String,
    /// The folder was chosen by the program (by the kind of file), not by hand.
    pub auto: bool,
    pub file: String,
    pub name: String,
    pub size: u64,
    pub bytes: u64,
    pub total: u64,
    /// Bytes per second since it started (0 when not known).
    pub speed: u64,
    pub eta_secs: Option<u64>,
    pub error: String,
    pub note: String,
    pub categories: Vec<String>,
    pub tags: Vec<String>,
}

pub struct DownloadQueue {
    inner: Arc<Inner>,
}

fn safe_user(user: &str) -> String {
    user.chars().filter(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.')).collect()
}

impl DownloadQueue {
    /// Open (and load) a person's list. What was waiting or running comes back as failed: interrupted.
    pub fn open(dir: &Path, user: &str) -> DownloadQueue {
        let path = dir.join(format!("media-downloads-{}.json", safe_user(user)));
        let disk: Disk = std::fs::read_to_string(&path).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default();
        let mut st = State { next_id: disk.next_id.max(1), ..Default::default() };
        let mut changed = false;
        for mut s in disk.items {
            if !s.status.finished() {
                s.status = Status::Failed;
                s.error = INTERRUPTED.into();
                changed = true;
            }
            st.next_id = st.next_id.max(s.id + 1);
            st.items.push(Entry { s, live: None, started: None, cancelling: false });
        }
        let q = DownloadQueue { inner: Arc::new(Inner { path, state: Mutex::new(st), ctx: Mutex::new(None), max_active: AtomicUsize::new(MAX_ACTIVE) }) };
        if changed {
            q.save();
        }
        q
    }

    /// A different limit of simultaneous downloads (tests; 1 to 8).
    pub fn set_max_active(&self, n: usize) {
        self.inner.max_active.store(n.clamp(1, 8), Ordering::SeqCst);
    }

    fn st(&self) -> std::sync::MutexGuard<'_, State> {
        self.inner.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn save(&self) {
        save(&self.inner);
    }

    /// Remember who is asking (retries and later steps of the queue use it).
    pub fn set_context(&self, ctx: Context) {
        *self.inner.ctx.lock().unwrap_or_else(|e| e.into_inner()) = Some(ctx);
    }

    /// Add one address at the end of the list; the answer is its number. Only an address that cannot be downloaded at all
    /// (not a web address, a host the account may not reach) is refused here; everything else is tried and reported in the list.
    pub fn enqueue(&self, ctx: Context, req: Request) -> Result<u64, String> {
        mediadl::check_url(&ctx.policy, &req.url)?;
        if let Some(f) = &req.folder {
            if !ctx.policy.allows_write(&ctx.user, &format!("{}/x", f.trim_end_matches('/'))) {
                return Err(format!("your account may not write in {f} (outside your write scope)"));
            }
        }
        let kind = mediadl::kind_of_url(&req.url);
        let id = {
            let mut st = self.st();
            if st.items.iter().filter(|e| !e.s.status.finished()).count() >= MAX_UNFINISHED {
                return Err(format!("{MAX_UNFINISHED} downloads are waiting already; wait for some to finish"));
            }
            let id = st.next_id;
            st.next_id += 1;
            let dest = match &req.folder {
                Some(f) => f.clone(),
                None => mediadl::planned_folder(&ctx.folders, &ctx.policy, &ctx.user, kind).map(|p| p.to_string_lossy().into_owned()).unwrap_or_default(),
            };
            st.items.push(Entry {
                s: Saved {
                    id,
                    url: req.url.trim().to_string(),
                    status: Status::Queued,
                    categories: req.categories,
                    tags: req.tags,
                    rating: req.rating.min(5),
                    folder: req.folder,
                    kind,
                    dest,
                    file: String::new(),
                    size: 0,
                    error: String::new(),
                    note: String::new(),
                    added: crate::clock::now_unix(),
                    finished: 0,
                },
                live: None,
                started: None,
                cancelling: false,
            });
            trim(&mut st);
            id
        };
        self.set_context(ctx);
        self.save();
        pump(&self.inner);
        Ok(id)
    }

    /// Cancel a queued or running download (a running one stops at its next chunk).
    pub fn cancel(&self, id: u64) -> Result<(), String> {
        {
            let mut st = self.st();
            let e = st.items.iter_mut().find(|e| e.s.id == id).ok_or("no such download")?;
            match e.s.status {
                Status::Queued => {
                    e.s.status = Status::Cancelled;
                    e.s.finished = crate::clock::now_unix();
                    e.s.error = "Cancelled before it started.".into();
                }
                Status::Downloading => {
                    e.cancelling = true;
                    if let Some(p) = &e.live {
                        p.cancel.store(true, Ordering::SeqCst);
                    }
                }
                _ => return Err("it is not waiting or running".into()),
            }
        }
        self.save();
        Ok(())
    }

    /// Put a failed or cancelled download back at the end of the line.
    pub fn retry(&self, id: u64) -> Result<(), String> {
        {
            let mut st = self.st();
            let e = st.items.iter_mut().find(|e| e.s.id == id).ok_or("no such download")?;
            if !matches!(e.s.status, Status::Failed | Status::Cancelled) {
                return Err("only a failed or cancelled download can be retried".into());
            }
            e.s.status = Status::Queued;
            e.s.error.clear();
            e.s.note.clear();
            e.s.finished = 0;
            e.cancelling = false;
            // it goes to the end of the line
            let pos = st.items.iter().position(|e| e.s.id == id).unwrap_or(0);
            let entry = st.items.remove(pos);
            st.items.push(entry);
        }
        if self.inner.ctx.lock().unwrap_or_else(|e| e.into_inner()).is_none() {
            // nobody to download for: leave it failed with the reason
            let mut st = self.st();
            if let Some(e) = st.items.iter_mut().find(|e| e.s.id == id) {
                e.s.status = Status::Failed;
                e.s.error = "Not signed in; open MEDIA and try again.".into();
            }
        }
        self.save();
        pump(&self.inner);
        Ok(())
    }

    /// Take a finished (or still waiting) entry off the list. A running one has to be cancelled first.
    pub fn remove(&self, id: u64) -> Result<(), String> {
        {
            let mut st = self.st();
            let i = st.items.iter().position(|e| e.s.id == id).ok_or("no such download")?;
            if st.items[i].s.status == Status::Downloading {
                return Err("it is running; cancel it first".into());
            }
            st.items.remove(i);
        }
        self.save();
        Ok(())
    }

    /// Take all finished entries off the list; returns how many.
    pub fn clear_finished(&self) -> usize {
        let n = {
            let mut st = self.st();
            let before = st.items.len();
            st.items.retain(|e| !e.s.status.finished());
            before - st.items.len()
        };
        self.save();
        n
    }

    /// (running, waiting).
    pub fn counts(&self) -> (usize, usize) {
        let st = self.st();
        (st.items.iter().filter(|e| e.s.status == Status::Downloading).count(), st.items.iter().filter(|e| e.s.status == Status::Queued).count())
    }

    /// How many downloads have finished since this program started, and the last one's number and outcome.
    pub fn last_finished(&self) -> (u64, Option<(u64, Status)>) {
        let st = self.st();
        (st.finished_seq, st.last)
    }

    pub fn views(&self) -> Vec<View> {
        self.st().items.iter().map(view_of).collect()
    }

    pub fn view(&self, id: u64) -> Option<View> {
        self.st().items.iter().find(|e| e.s.id == id).map(view_of)
    }

    /// The list as lines of text, for the assistant: number, state, what and where.
    pub fn describe(&self) -> String {
        let v = self.views();
        if v.is_empty() {
            return "The download queue is empty.".into();
        }
        let (run, wait) = self.counts();
        let mut out = format!("{} download{} ({run} running, {wait} waiting):\n", v.len(), if v.len() == 1 { "" } else { "s" });
        for x in v.iter().rev().take(30).rev() {
            out.push_str(&format!("#{} {}: {}", x.id, x.status.label(), x.url.chars().take(100).collect::<String>()));
            match x.status {
                Status::Downloading => out.push_str(&format!(" ({} KB{})", x.bytes / 1024, if x.total > 0 { format!(" of {} KB", x.total / 1024) } else { String::new() })),
                Status::Succeeded if !x.name.is_empty() => out.push_str(&format!(" -> {} in {} ({} KB)", x.name, x.folder.rsplit('/').next().unwrap_or(""), x.size / 1024)),
                Status::Succeeded => out.push_str(&format!(" ({})", x.note)),
                Status::Failed | Status::Cancelled => out.push_str(&format!(" — {}", x.error)),
                Status::Queued => {}
            }
            out.push('\n');
        }
        out.trim_end().to_string()
    }
}

fn view_of(e: &Entry) -> View {
    let s = &e.s;
    let (bytes, total, folder_now) = match &e.live {
        Some(p) => (p.bytes.load(Ordering::Relaxed), p.total.load(Ordering::Relaxed), p.folder.lock().unwrap_or_else(|x| x.into_inner()).clone()),
        None => (if s.status == Status::Succeeded { s.size } else { 0 }, if s.status == Status::Succeeded { s.size } else { 0 }, String::new()),
    };
    let secs = e.started.map_or(0.0, |t| t.elapsed().as_secs_f64());
    let speed = if s.status == Status::Downloading && secs > 0.2 { (bytes as f64 / secs) as u64 } else { 0 };
    let eta = (speed > 0 && total > bytes).then(|| (total - bytes) / speed);
    let name = Path::new(&s.file).file_name().and_then(|n| n.to_str()).unwrap_or("").to_string();
    View {
        id: s.id,
        url: s.url.clone(),
        status: s.status,
        kind: s.kind,
        folder: if !folder_now.is_empty() { folder_now } else { s.dest.clone() },
        auto: s.folder.is_none(),
        file: s.file.clone(),
        name,
        size: s.size,
        bytes,
        total,
        speed,
        eta_secs: eta,
        error: s.error.clone(),
        note: s.note.clone(),
        categories: s.categories.clone(),
        tags: s.tags.clone(),
    }
}

fn trim(st: &mut State) {
    while st.items.len() > MAX_ENTRIES {
        match st.items.iter().position(|e| e.s.status.finished()) {
            Some(i) => {
                st.items.remove(i);
            }
            None => break,
        }
    }
}

fn save(inner: &Inner) {
    let disk = {
        let st = inner.state.lock().unwrap_or_else(|e| e.into_inner());
        Disk { next_id: st.next_id, items: st.items.iter().map(|e| e.s.clone()).collect() }
    };
    if let Ok(text) = serde_json::to_string_pretty(&disk) {
        let tmp = inner.path.with_extension("tmp");
        if std::fs::write(&tmp, text).is_ok() {
            let _ = std::fs::rename(&tmp, &inner.path);
        }
    }
}

/// Start waiting downloads while fewer than the limit run.
fn pump(inner: &Arc<Inner>) {
    let Some(ctx) = inner.ctx.lock().unwrap_or_else(|e| e.into_inner()).clone() else { return };
    loop {
        let (id, progress) = {
            let mut st = inner.state.lock().unwrap_or_else(|e| e.into_inner());
            let active = st.items.iter().filter(|e| e.s.status == Status::Downloading).count();
            if active >= inner.max_active.load(Ordering::SeqCst) {
                return;
            }
            let Some(e) = st.items.iter_mut().find(|e| e.s.status == Status::Queued) else { return };
            let p = Arc::new(Progress::default());
            e.s.status = Status::Downloading;
            e.live = Some(p.clone());
            e.started = Some(Instant::now());
            e.cancelling = false;
            (e.s.id, p)
        };
        save(inner);
        let (inner2, ctx2) = (inner.clone(), ctx.clone());
        std::thread::Builder::new().name("iyakki-download".into()).spawn(move || work(&inner2, &ctx2, id, &progress)).ok();
    }
}

fn work(inner: &Arc<Inner>, ctx: &Context, id: u64, progress: &Arc<Progress>) {
    let (url, choice, folder) = {
        let st = inner.state.lock().unwrap_or_else(|e| e.into_inner());
        let Some(e) = st.items.iter().find(|e| e.s.id == id) else { return };
        (e.s.url.clone(), Choice::from_words(&e.s.categories, &e.s.tags, e.s.rating), e.s.folder.clone())
    };
    let result = match ctx.svc.streamer() {
        None => Err("downloads are not available (no network transport)".to_string()),
        Some(streamer) => {
            let limit = ctx.svc.media_download_limit();
            match &folder {
                Some(f) => mediadl::run_blocking_to(&ctx.svc, streamer.as_ref(), &ctx.policy, &ctx.user, &url, Target::Dir(Path::new(f)), &choice, limit, progress),
                None => mediadl::run_blocking_to(&ctx.svc, streamer.as_ref(), &ctx.policy, &ctx.user, &url, Target::Auto(&ctx.folders), &choice, limit, progress),
            }
        }
    };
    let mut more: Vec<String> = vec![];
    {
        let mut st = inner.state.lock().unwrap_or_else(|e| e.into_inner());
        let now = crate::clock::now_unix();
        let Some(e) = st.items.iter_mut().find(|e| e.s.id == id) else { return };
        let bytes_now = progress.bytes.load(Ordering::Relaxed);
        e.live = None;
        e.started = None;
        e.s.finished = now;
        match result {
            Ok(Fetched::File { path, bytes }) => {
                e.s.status = Status::Succeeded;
                e.s.file = path.to_string_lossy().into_owned();
                e.s.size = bytes;
                e.s.dest = path.parent().map(|p| p.to_string_lossy().into_owned()).unwrap_or_default();
                e.s.kind = path.to_str().and_then(|p| mediadl::kind_of_url(p)).or(e.s.kind);
            }
            Ok(Fetched::Links(links)) => {
                e.s.status = Status::Succeeded;
                e.s.note = format!("a list of {} address{}: each is in the queue", links.len(), if links.len() == 1 { "" } else { "es" });
                more = links;
            }
            Err(msg) if msg == "cancelled" || e.cancelling => {
                e.s.status = Status::Cancelled;
                e.s.error = format!("Cancelled{}.", if bytes_now > 0 { format!(" after {} KB; the partial file was removed", bytes_now / 1024) } else { String::new() });
            }
            Err(msg) => {
                e.s.status = Status::Failed;
                e.s.error = plain_failure(&msg);
            }
        }
        let status = e.s.status;
        st.finished_seq += 1;
        st.last = Some((id, status));
    }
    // the addresses of a playlist or feed are queued like any others
    for l in more {
        let (cats, tags, rating, folder) = {
            let st = inner.state.lock().unwrap_or_else(|e| e.into_inner());
            let Some(e) = st.items.iter().find(|e| e.s.id == id) else { break };
            (e.s.categories.clone(), e.s.tags.clone(), e.s.rating, e.s.folder.clone())
        };
        let q = DownloadQueue { inner: inner.clone() };
        let _ = q.enqueue(ctx.clone(), Request { url: l, categories: cats, tags, rating, folder });
    }
    save(inner);
    pump(inner);
}

/// A reason in plain English for a failure message of the download code: what kind of trouble it was, then the detail.
pub fn plain_failure(msg: &str) -> String {
    let m = msg.trim();
    let l = m.to_lowercase();
    let cap = |s: &str| {
        let mut c = s.chars();
        c.next().map_or(String::new(), |f| f.to_uppercase().collect::<String>() + c.as_str())
    };
    if l.contains("web page") || l.contains("not a song, video or picture") || l.contains("no media addresses") || l.contains("sent an empty file") || l.contains("serves web pages") {
        format!("Not a media file: {m}.")
    } else if l.contains("limit") {
        format!("Too large: {m}.")
    } else if l.contains("may not reach") || l.contains("redirected to an address that cannot be used") || l.contains("not a web address") {
        format!("Host refused: {m}.")
    } else if l.contains("the server answered 4") || l.contains("the server answered 5") {
        format!("The server refused or failed: {m}.")
    } else if l.contains("no space left") || l.contains("disk full") || l.contains("quota exceeded") {
        format!("Disk full: {m}. Free some space and retry.")
    } else if l.contains("permission denied") || l.contains("write scope") || l.contains("may not be written") || l.contains("may not write") || l.contains("read-only file system") {
        format!("Outside the write scope: {m}.")
    } else if l.contains("does not exist") {
        format!("Folder missing: {m}.")
    } else if l.contains("no network transport") {
        format!("Network error: {m}.")
    } else {
        format!("Network error: {}.", cap(m).trim_end_matches('.'))
    }
}

/// The folder name to show for a path (`/home/me/Videos` → `Videos`).
pub fn folder_label(path: &str) -> String {
    path.trim_end_matches('/').rsplit('/').next().unwrap_or(path).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use iyakki_sec::policy::Role;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::time::Duration;

    struct Off;
    impl iyakki_proxy::Upstream for Off {
        fn request(&self, _: &str, _: &str, _: &[(String, String)], _: &[u8]) -> Result<iyakki_proxy::UpstreamResponse, String> {
            Err("offline".into())
        }
    }

    struct W {
        svc: Arc<Services>,
        ctx: Context,
        home: PathBuf,
        q: DownloadQueue,
    }

    fn world(tag: &str) -> W {
        let d = std::fs::canonicalize({
            let d = std::env::temp_dir().join(format!("iyakki-mq-{}-{}", std::process::id(), tag));
            let _ = std::fs::remove_dir_all(&d);
            for sub in ["home/Music", "home/Videos", "home/Pictures", "svc"] {
                std::fs::create_dir_all(d.join(sub)).unwrap();
            }
            d
        })
        .unwrap();
        let svc = Services::open(&d.join("svc"), Arc::new(Off), Arc::new(Off)).unwrap();
        svc.set_streamer(Arc::new(iyakki_proxy::https::UreqStreamer::default()));
        let home = d.join("home");
        let mut policy = Role::User.template();
        policy.fs_read = vec![home.display().to_string()];
        policy.fs_write = vec![home.display().to_string()];
        policy.net_hosts = vec!["127.0.0.1".into()];
        let folders = ["Music", "Videos", "Pictures"].iter().map(|f| home.join(f).display().to_string()).collect();
        let ctx = Context { svc: svc.clone(), policy, user: "alice".into(), folders };
        let q = DownloadQueue::open(svc.dir(), "alice");
        W { svc, ctx, home, q }
    }

    /// A server whose answers take `delay`; it counts how many requests are open at once.
    fn slow_server(delay: Duration, routes: Vec<(&'static str, &'static str, Vec<u8>)>) -> (String, Arc<(AtomicUsize, AtomicUsize)>, Arc<Mutex<Vec<String>>>) {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = l.local_addr().unwrap();
        let counts = Arc::new((AtomicUsize::new(0), AtomicUsize::new(0))); // (now, max)
        let order = Arc::new(Mutex::new(Vec::<String>::new()));
        let (c2, o2) = (counts.clone(), order.clone());
        std::thread::spawn(move || {
            for s in l.incoming().flatten() {
                let (routes, counts, order) = (routes.clone(), c2.clone(), o2.clone());
                std::thread::spawn(move || {
                    let mut s = s;
                    let mut buf = [0u8; 4096];
                    let n = s.read(&mut buf).unwrap_or(0);
                    let path = String::from_utf8_lossy(&buf[..n]).split_whitespace().nth(1).unwrap_or("/").to_string();
                    order.lock().unwrap().push(path.clone());
                    let now = counts.0.fetch_add(1, Ordering::SeqCst) + 1;
                    counts.1.fetch_max(now, Ordering::SeqCst);
                    std::thread::sleep(delay);
                    let out = match routes.iter().find(|r| r.0 == path) {
                        Some((_, ct, body)) => [format!("HTTP/1.1 200 OK\r\nContent-Type: {ct}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len()).into_bytes(), body.clone()].concat(),
                        None => b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".to_vec(),
                    };
                    let _ = s.write_all(&out);
                    counts.0.fetch_sub(1, Ordering::SeqCst);
                });
            }
        });
        (format!("http://{addr}"), counts, order)
    }

    fn wait_until(q: &DownloadQueue, what: &str, f: impl Fn(&DownloadQueue) -> bool) {
        for _ in 0..1000 {
            if f(q) {
                return;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        panic!("timed out waiting for {what}: {:?}", q.views().iter().map(|v| (v.id, v.status, v.error.clone())).collect::<Vec<_>>());
    }

    fn req(url: String) -> Request {
        Request { url, ..Default::default() }
    }

    #[test]
    fn entries_run_first_in_first_out_with_at_most_two_at_a_time() {
        let w = world("fifo");
        let mp3 = mediadl::tests::tiny_mp3();
        let routes: Vec<(&'static str, &'static str, Vec<u8>)> = vec![("/a.mp3", "audio/mpeg", mp3.clone()), ("/b.mp3", "audio/mpeg", mp3.clone()), ("/c.mp3", "audio/mpeg", mp3.clone()), ("/d.mp3", "audio/mpeg", mp3.clone()), ("/e.mp3", "audio/mpeg", mp3)];
        let (base, counts, order) = slow_server(Duration::from_millis(250), routes);
        let ids: Vec<u64> = ["a", "b", "c", "d", "e"].iter().map(|n| w.q.enqueue(w.ctx.clone(), req(format!("{base}/{n}.mp3"))).unwrap()).collect();
        assert_eq!(ids, vec![1, 2, 3, 4, 5], "numbers count up from 1");
        // right after adding: two running, three waiting
        assert_eq!(w.q.counts(), (2, 3));
        let v = w.q.views();
        assert_eq!(v.iter().map(|x| x.status).collect::<Vec<_>>(), vec![Status::Downloading, Status::Downloading, Status::Queued, Status::Queued, Status::Queued]);
        wait_until(&w.q, "all done", |q| q.views().iter().all(|v| v.status == Status::Succeeded));
        assert_eq!(counts.1.load(Ordering::SeqCst), 2, "never more than two requests open at once");
        let seen = order.lock().unwrap().clone();
        assert_eq!(seen.len(), 5);
        assert!(seen[..2].contains(&"/a.mp3".to_string()) && seen[..2].contains(&"/b.mp3".to_string()), "the first two in line start first: {seen:?}");
        assert!(seen[4] == "/e.mp3", "the last in line starts last: {seen:?}");
        // every file is in Music (auto by extension) with its own name
        for n in ["a", "b", "c", "d", "e"] {
            assert!(w.home.join(format!("Music/{n}.mp3")).is_file(), "{n}");
        }
        assert_eq!(w.q.counts(), (0, 0));
        assert_eq!(w.q.last_finished().0, 5);
    }

    #[test]
    fn status_text_for_success_and_each_kind_of_failure_and_retry_works() {
        let w = world("status");
        let mp3 = mediadl::tests::tiny_mp3();
        let base = mediadl::tests::serve(vec![("/ok.mp3", "audio/mpeg", mp3), ("/page.mp3", "text/html", b"<html>hi</html>".to_vec())]);
        let ok = w.q.enqueue(w.ctx.clone(), req(format!("{base}/ok.mp3"))).unwrap();
        let page = w.q.enqueue(w.ctx.clone(), req(format!("{base}/page.mp3"))).unwrap();
        let missing = w.q.enqueue(w.ctx.clone(), req(format!("{base}/missing.mp3"))).unwrap();
        wait_until(&w.q, "finished", |q| q.views().iter().all(|v| v.status.finished()));
        let v = w.q.view(ok).unwrap();
        assert_eq!((v.status, v.name.as_str(), v.size > 0), (Status::Succeeded, "ok.mp3", true));
        assert!(v.folder.ends_with("/Music") && v.auto);
        let v = w.q.view(page).unwrap();
        assert_eq!(v.status, Status::Failed);
        assert!(v.error.starts_with("Not a media file") && v.error.contains("web page"), "{}", v.error);
        let v = w.q.view(missing).unwrap();
        assert!(v.error.starts_with("The server refused or failed") && v.error.contains("404"), "{}", v.error);
        // retry: the missing file appears on the server side? it does not, so it fails again with the same words; a retried
        // download goes through the queue again and the old reason is cleared while it waits
        w.q.retry(missing).unwrap();
        assert!(matches!(w.q.view(missing).unwrap().status, Status::Queued | Status::Downloading));
        assert!(w.q.retry(ok).is_err(), "a succeeded one cannot be retried");
        wait_until(&w.q, "retried", |q| q.view(missing).unwrap().status == Status::Failed);
        // plain wording for the other kinds
        assert!(plain_failure("the file is 900 MB, more than the 512 MB limit").starts_with("Too large"));
        assert!(plain_failure("your account may not reach evil.test").starts_with("Host refused"));
        assert!(plain_failure("cannot write the file: No space left on device (os error 28)").starts_with("Disk full"));
        assert!(plain_failure("permission denied: /etc is outside the folders you may write").starts_with("Outside the write scope"));
        assert!(plain_failure("the download failed: connection reset by peer").starts_with("Network error"));
        assert!(plain_failure("that file is not a song, video or picture IYAKKI recognises").starts_with("Not a media file"));
        // the inbox got a note for each finished item
        let inbox = w.svc.inbox.list("alice");
        assert!(inbox.iter().any(|n| n.kind == crate::inbox::Kind::Download && n.title.contains("ok.mp3")));
        assert!(inbox.iter().any(|n| n.kind == crate::inbox::Kind::Error && n.title.contains("Download failed")));
    }

    #[test]
    fn a_waiting_item_can_be_cancelled_and_a_running_one_stops() {
        let w = world("cancel");
        w.q.set_max_active(1);
        let big = [mediadl::tests::tiny_mp3(), vec![0u8; 100_000]].concat();
        let (base, _c, _o) = slow_server(Duration::from_millis(300), vec![("/one.mp3", "audio/mpeg", big.clone()), ("/two.mp3", "audio/mpeg", big)]);
        let one = w.q.enqueue(w.ctx.clone(), req(format!("{base}/one.mp3"))).unwrap();
        let two = w.q.enqueue(w.ctx.clone(), req(format!("{base}/two.mp3"))).unwrap();
        assert_eq!(w.q.view(two).unwrap().status, Status::Queued);
        w.q.cancel(two).unwrap();
        let v = w.q.view(two).unwrap();
        assert_eq!(v.status, Status::Cancelled);
        assert!(v.error.starts_with("Cancelled"));
        assert!(w.q.cancel(two).is_err());
        // the running one: cancelled while the server is still thinking
        w.q.cancel(one).unwrap();
        wait_until(&w.q, "cancelled", |q| q.view(one).unwrap().status == Status::Cancelled);
        assert!(std::fs::read_dir(w.home.join("Music")).unwrap().count() == 0, "nothing was kept, no partial file");
        // retry brings a cancelled one back
        w.q.set_max_active(2);
        w.q.retry(two).unwrap();
        wait_until(&w.q, "retried", |q| q.view(two).unwrap().status == Status::Succeeded);
        assert!(w.home.join("Music/two.mp3").is_file());
        // remove and clear
        assert!(w.q.remove(two).is_ok());
        assert_eq!(w.q.clear_finished(), 1);
        assert!(w.q.views().is_empty());
        assert!(w.q.remove(99).is_err());
    }

    #[test]
    fn the_list_survives_a_restart_and_unfinished_items_become_interrupted() {
        let w = world("persist");
        let mp3 = mediadl::tests::tiny_mp3();
        let (base, _c, _o) = slow_server(Duration::from_millis(400), vec![("/done.mp3", "audio/mpeg", mp3.clone()), ("/slow.mp3", "audio/mpeg", mp3.clone()), ("/wait.mp3", "audio/mpeg", mp3)]);
        w.q.set_max_active(1);
        let done = w.q.enqueue(w.ctx.clone(), Request { url: format!("{base}/done.mp3"), categories: vec!["melody".into(), "my road trip".into()], rating: 3, ..Default::default() }).unwrap();
        wait_until(&w.q, "first", |q| q.view(done).unwrap().status == Status::Succeeded);
        let slow = w.q.enqueue(w.ctx.clone(), req(format!("{base}/slow.mp3"))).unwrap();
        let wait = w.q.enqueue(w.ctx.clone(), req(format!("{base}/wait.mp3"))).unwrap();
        assert_eq!((w.q.view(slow).unwrap().status, w.q.view(wait).unwrap().status), (Status::Downloading, Status::Queued));
        // "restart": a new queue reads the file the first one saved
        let again = DownloadQueue::open(w.svc.dir(), "alice");
        let v = again.views();
        assert_eq!(v.len(), 3);
        assert_eq!(v[0].status, Status::Succeeded);
        assert_eq!(v[0].name, "done.mp3");
        assert_eq!(v[0].categories, vec!["melody".to_string(), "my road trip".to_string()]);
        for i in [1, 2] {
            assert_eq!(v[i].status, Status::Failed, "{:?}", v[i]);
            assert_eq!(v[i].error, INTERRUPTED);
        }
        // numbers keep counting; a retry works after a restart once somebody is signed in
        again.set_context(w.ctx.clone());
        again.retry(wait).unwrap();
        wait_until(&again, "retry after restart", |q| q.view(wait).unwrap().status == Status::Succeeded);
        let next = again.enqueue(w.ctx.clone(), req(format!("{base}/done.mp3"))).unwrap();
        assert!(next > wait);
        // another person has their own list
        assert!(DownloadQueue::open(w.svc.dir(), "bob").views().is_empty());
        // wait for the first queue's threads so the folder is quiet
        wait_until(&w.q, "first queue quiet", |q| q.counts() == (0, 0));
    }

    #[test]
    fn the_folder_follows_the_kind_of_file_by_extension_and_by_content_type_and_a_chosen_folder_wins() {
        let w = world("folders");
        let webm = b"\x1a\x45\xdf\xa3\x01\x00\x00\x00\x00\x00\x00\x1fB\x86\x81\x01B\xf7\x81\x01B\xf2\x81\x04B\xf3\x81\x08B\x82\x84webm".to_vec();
        let png = b"\x89PNG\r\n\x1a\n0000IHDR0000".to_vec();
        let base = mediadl::tests::serve(vec![("/clip.webm", "video/webm", webm.clone()), ("/photo.png", "image/png", png.clone()), ("/song.mp3", "audio/mpeg", mediadl::tests::tiny_mp3()), ("/stream", "video/webm", webm), ("/pic", "image/png", png)]);
        let a = w.q.enqueue(w.ctx.clone(), req(format!("{base}/clip.webm"))).unwrap();
        let b = w.q.enqueue(w.ctx.clone(), req(format!("{base}/photo.png"))).unwrap();
        let c = w.q.enqueue(w.ctx.clone(), req(format!("{base}/song.mp3"))).unwrap();
        // the address says what it is: the folder is known before anything is fetched
        assert!(w.q.view(a).unwrap().folder.ends_with("/Videos") && w.q.view(b).unwrap().folder.ends_with("/Pictures") && w.q.view(c).unwrap().folder.ends_with("/Music"));
        // an address without an extension: decided after the response (first bytes, then content type)
        let d = w.q.enqueue(w.ctx.clone(), req(format!("{base}/stream"))).unwrap();
        let e = w.q.enqueue(w.ctx.clone(), req(format!("{base}/pic"))).unwrap();
        // chosen by hand: the video goes to Music because the person said so
        let f = w.q.enqueue(w.ctx.clone(), Request { url: format!("{base}/clip.webm"), folder: Some(w.home.join("Music").display().to_string()), ..Default::default() }).unwrap();
        assert!(!w.q.view(f).unwrap().auto);
        wait_until(&w.q, "done", |q| q.views().iter().all(|v| v.status.finished()));
        let dest = |id: u64| folder_label(&w.q.view(id).unwrap().folder);
        assert_eq!(dest(a), "Videos");
        assert_eq!(dest(b), "Pictures");
        assert_eq!(dest(c), "Music");
        assert_eq!(dest(d), "Videos", "{:?}", w.q.view(d));
        assert_eq!(dest(e), "Pictures", "{:?}", w.q.view(e));
        assert_eq!(dest(f), "Music");
        assert!(w.home.join("Videos/clip.webm").is_file() && w.home.join("Music/clip.webm").is_file() && w.home.join("Pictures/photo.png").is_file());
        // outside the write scope: refused up front for a chosen folder, and reported in the list for an automatic one
        assert!(w.q.enqueue(w.ctx.clone(), Request { url: format!("{base}/song.mp3"), folder: Some("/etc".into()), ..Default::default() }).unwrap_err().contains("outside your write scope"));
        let mut ro = w.ctx.clone();
        ro.policy.fs_write.clear();
        let g = w.q.enqueue(ro, req(format!("{base}/song.mp3"))).unwrap();
        wait_until(&w.q, "refused", |q| q.view(g).unwrap().status == Status::Failed);
        assert!(w.q.view(g).unwrap().error.starts_with("Outside the write scope"), "{}", w.q.view(g).unwrap().error);
        // a host the account may not reach is refused at once
        assert!(w.q.enqueue(w.ctx.clone(), req("https://evil.test/a.mp3".into())).unwrap_err().contains("may not reach"));
    }

    #[test]
    fn free_text_categories_become_types_and_tags_and_nothing_is_rejected() {
        let w = world("cats");
        let base = mediadl::tests::serve(vec![("/m.mp3", "audio/mpeg", mediadl::tests::tiny_mp3())]);
        let id = w.q.enqueue(w.ctx.clone(), Request { url: format!("{base}/m.mp3"), categories: mediadl::split_words("Action, horror\nKutthu; my road trip,  ,anime"), tags: vec!["gym".into()], rating: 5, ..Default::default() }).unwrap();
        wait_until(&w.q, "done", |q| q.view(id).unwrap().status.finished());
        let v = w.q.view(id).unwrap();
        assert_eq!(v.status, Status::Succeeded, "{}", v.error);
        let ix = w.svc.media_index("alice");
        let g = ix.lock().unwrap();
        let it = g.find_path(&v.file).expect("in the index");
        let types: Vec<String> = it.effective_types().iter().cloned().collect();
        assert!(types.contains(&"kuthu".to_string()), "{types:?}");
        // known words (and their spellings: anime is a cartoon, gym is motivational) are types or canonical tags; a word the
        // vocabulary does not know ("my road trip") is kept as a tag as typed
        for t in ["action", "horror", "cartoon", "kuthu", "motivational", "my road trip"] {
            assert!(it.tags.contains(t) || it.effective_types().contains(t), "{t} in {:?} {types:?}", it.tags);
        }
        assert!(it.tags.contains("my road trip"));
        assert_eq!(it.rating, 5);
    }

    #[test]
    fn describe_lists_the_queue_for_the_assistant() {
        let w = world("describe");
        assert_eq!(w.q.describe(), "The download queue is empty.");
        let base = mediadl::tests::serve(vec![("/m.mp3", "audio/mpeg", mediadl::tests::tiny_mp3())]);
        let id = w.q.enqueue(w.ctx.clone(), req(format!("{base}/m.mp3"))).unwrap();
        wait_until(&w.q, "done", |q| q.view(id).unwrap().status.finished());
        let t = w.q.describe();
        assert!(t.contains("#1 Succeeded") && t.contains("m.mp3") && t.contains("Music"), "{t}");
    }
}
