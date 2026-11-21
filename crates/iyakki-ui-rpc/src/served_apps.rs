//! What the apps of the UI program asked of the core as live objects, served over the UI socket as calls and polled jobs (F-SPLIT-03,
//! D-SPLIT-03-APPS in docs/development/ALPHA-7-DECISIONS.md): the plain chat model (`ChatBackend`), the picture and vision models of LAB, the host
//! of the machine (power, sleep, WebAssembly runs) and the story store of TALES.
//!
//! One rule for everything that takes longer than a call: the UI starts a JOB on the core (`begin`), then polls it (`poll` waits at most a quarter of
//! a second for news, so a quiet job never holds the one line of the connection) and may cancel it (`cancel`). The pieces a job says on the way
//! (the text a model writes) come back with the polls; the end of the job comes back once. Bytes that do not fit a line (a picture, the pictures
//! of a question, a WebAssembly module) travel as the `Upload` and `Download` objects of `served_hosts.rs`.
//!
//! What the UI names about itself is never believed: a WebAssembly run uses the policy the core reads from its own accounts for the user the
//! call names, and the machine's power is switched only when the core's host says it can, with a line in the core's audit log.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, OnceLock, PoisonError};
use std::time::Duration;

use iyakki_agent_api::{AccountsApi, ServicesFacetB5, ServicesHandle};
use iyakki_core_api::host::WasmIo;
use iyakki_core_api::HostApi;
use iyakki_shared::chat::{ChatBackend, Message, ModelTurn, ToolSpec};
use iyakki_shared::lab::{CallTrace, PictureBackend, PictureRequest, VisionAnswer, VisionBackend, VisionRequest};
use iyakki_shared::providers::ImageTrace;
use iyakki_shared::settings::ModelChoice;
use serde::de::DeserializeOwned;
use serde_json::{json, Value};

use crate::served::{Served, NOT_SERVED};
pub(crate) use crate::served_store::*;
pub(crate) use crate::served_policy::*;
pub(crate) use crate::served_tiles::*;
use crate::served_hosts::{bytes_in, down_bytes, read_all, send};
// the generated trait code calls every hand-written method by `served_apps::c_<Trait>_<fn>`: the job calls of `served_jobs.rs` are re-exported here
pub(crate) use crate::served_jobs::{c_PlatformApi_apply, c_PlatformApi_apply_with, c_PlatformApi_plan_waiting, c_PlatformApi_private_finish_local, c_PlatformApi_private_repair, c_PlatformApi_private_start_job, c_PlatformApi_private_store_state, c_PlatformApi_upgrade, s_PlatformApi_apply, s_PlatformApi_apply_with, s_PlatformApi_plan_waiting, s_PlatformApi_private_finish_local, s_PlatformApi_private_repair, s_PlatformApi_private_start_job, s_PlatformApi_private_store_state, s_PlatformApi_upgrade};
use crate::{de, ser, Lost, Obj, Registry, RemoteServicesFacetB5, RemoteServicesHandle, Rpc, ServicesRoot};

/// How long a poll waits for news before it answers "nothing yet" (the line of the connection is shared by every call of the program).
pub const POLL_WAIT: Duration = Duration::from_millis(250);
/// The most jobs one object runs at the same time.
const MAX_JOBS: usize = 32;

// ---------------------------------------------------------------------------------------------------------------- jobs

struct JobState<R> {
    pieces: Vec<String>,
    done: Option<R>,
}

/// One job of the core: a thread that says pieces on the way and ends with one result.
struct Job<R> {
    st: Mutex<JobState<R>>,
    cv: Condvar,
    cancel: Arc<AtomicBool>,
}

/// The jobs one served object runs, by number.
pub(crate) struct Jobs<R> {
    next: AtomicU64,
    map: Mutex<HashMap<u64, Arc<Job<R>>>>,
    wait: Duration,
}

impl<R: Send + 'static> Jobs<R> {
    pub(crate) fn new() -> Jobs<R> {
        Jobs::waiting(POLL_WAIT)
    }

    pub(crate) fn waiting(wait: Duration) -> Jobs<R> {
        Jobs { next: AtomicU64::new(1), map: Mutex::new(HashMap::new()), wait }
    }

    /// Run `f` on a thread. It gets the way to say a piece (answers false when the UI asked to stop) and the flag that says the same.
    pub(crate) fn start(&self, f: impl FnOnce(&dyn Fn(&str) -> bool, &Arc<AtomicBool>) -> R + Send + 'static) -> Result<u64, String> {
        let mut map = self.map.lock().unwrap_or_else(PoisonError::into_inner);
        if map.len() >= MAX_JOBS {
            return Err(format!("{} jobs are running already: wait for one to end", map.len()));
        }
        let job = Arc::new(Job { st: Mutex::new(JobState { pieces: Vec::new(), done: None }), cv: Condvar::new(), cancel: Arc::new(AtomicBool::new(false)) });
        let id = self.next.fetch_add(1, Ordering::Relaxed);
        map.insert(id, job.clone());
        drop(map);
        std::thread::spawn(move || {
            let say = |piece: &str| {
                {
                    let mut g = job.st.lock().unwrap_or_else(PoisonError::into_inner);
                    // a model that is busy and shows nothing says "" over and over: one is enough
                    if !(piece.is_empty() && g.pieces.last().is_some_and(String::is_empty)) {
                        g.pieces.push(piece.to_string());
                    }
                }
                job.cv.notify_all();
                !job.cancel.load(Ordering::SeqCst)
            };
            let r = f(&say, &job.cancel);
            job.st.lock().unwrap_or_else(PoisonError::into_inner).done = Some(r);
            job.cv.notify_all();
        });
        Ok(id)
    }

    /// What the job said since the last poll and, when it has ended and said everything, its result (once: the job is forgotten then).
    pub(crate) fn poll(&self, id: u64) -> Result<(Vec<String>, Option<R>), String> {
        let job = self.map.lock().unwrap_or_else(PoisonError::into_inner).get(&id).cloned().ok_or("there is no such job (it ended, or it was never started)")?;
        let g = job.st.lock().unwrap_or_else(PoisonError::into_inner);
        let (mut g, _) = job.cv.wait_timeout_while(g, self.wait, |s| s.pieces.is_empty() && s.done.is_none()).unwrap_or_else(PoisonError::into_inner);
        let pieces = std::mem::take(&mut g.pieces);
        let done = if pieces.is_empty() { g.done.take() } else { None };
        drop(g);
        if done.is_some() {
            self.map.lock().unwrap_or_else(PoisonError::into_inner).remove(&id);
        }
        Ok((pieces, done))
    }

    pub(crate) fn cancel(&self, id: u64) -> bool {
        match self.map.lock().unwrap_or_else(PoisonError::into_inner).get(&id) {
            Some(j) => {
                j.cancel.store(true, Ordering::SeqCst);
                true
            }
            None => false,
        }
    }

    #[cfg(test)]
    pub(crate) fn running(&self) -> usize {
        self.map.lock().unwrap_or_else(PoisonError::into_inner).len()
    }
}

/// The UI's end of a job: poll until it ends. `on_piece` gets each piece; when it answers false the job is cancelled (once) and the poll goes on
/// until the core says how it ended. A lost core ends the wait with its reason.
pub(crate) fn drive<R: DeserializeOwned>(rpc: &Rpc, iface: &str, obj: u64, job: u64, on_piece: &mut dyn FnMut(&str) -> bool) -> Result<R, String> {
    let mut cancelled = false;
    loop {
        let (pieces, done): (Vec<String>, Option<R>) = rpc.call(iface, obj, "poll", json!([job]))?;
        for p in pieces {
            if !on_piece(&p) && !cancelled {
                cancelled = true;
                rpc.call::<bool>(iface, obj, "cancel", json!([job]))?;
            }
        }
        if let Some(r) = done {
            return Ok(r);
        }
    }
}

/// A job table's three calls, for the objects that hold one.
fn job_calls<R: Send + 'static>(jobs: &Jobs<R>, method: &str, a: &Value, finish: impl FnOnce(R) -> Result<Value, String>) -> Option<Result<Value, String>> {
    match method {
        "poll" => Some((|| {
            let (id,): (u64,) = de(a.clone())?;
            let (pieces, done) = jobs.poll(id)?;
            let done = match done {
                Some(r) => Some(finish(r)?),
                None => None,
            };
            ser(&(pieces, done))
        })()),
        "cancel" => Some(de::<(u64,)>(a.clone()).and_then(|(id,)| ser(&jobs.cancel(id)))),
        _ => None,
    }
}

// ---------------------------------------------------------------------------------------------------------------- the plain chat model

/// The core's chat model as an object: a turn is a job, the text the model writes comes back as pieces.
struct ChatObj {
    backend: Arc<dyn ChatBackend>,
    jobs: Jobs<Result<ModelTurn, String>>,
}

/// The core's chat model as the root of the trait `ChatBackend` (the composition root and the tests use it).
pub fn serve_chat_backend(b: Arc<dyn ChatBackend>) -> Obj {
    Obj::Served(Arc::new(ChatObj { backend: b, jobs: Jobs::new() }))
}

impl Served for ChatObj {
    fn iface(&self) -> &'static str {
        "ChatBackend"
    }
    fn serve(&self, _reg: &Registry<'_>, method: &str, a: Value) -> Result<Value, String> {
        if let Some(r) = job_calls(&self.jobs, method, &a, |t| ser(&t)) {
            return r;
        }
        match method {
            "provider_name" => ser(&self.backend.provider_name()),
            // mode 0: `complete`, 1: `complete_tools`, 2: `complete_tools_stream`
            "begin" => {
                let (mode, history, tools): (u8, Vec<Message>, Vec<ToolSpec>) = de(a)?;
                let b = self.backend.clone();
                ser(&self.jobs.start(move |say, _| match mode {
                    0 => b.complete(&history).map(|text| ModelTurn { text, calls: Vec::new() }),
                    1 => b.complete_tools(&history, &tools),
                    _ => b.complete_tools_stream(&history, &tools, &mut |t| say(t)),
                })?)
            }
            other => Err(format!("ChatBackend has no call {other}")),
        }
    }
}

/// `ServicesFacetB5::lab_frames_for` (the UI's end): the frame source of the core, a remote object; a core without one answers "no frames".
pub(crate) fn c_ServicesFacetB5_lab_frames_for(s: &RemoteServicesFacetB5, user: &str) -> Arc<dyn iyakki_shared::lab::FrameSource> {
    match s.rpc.call::<u64>("ServicesHandle", 0, "lab_frames", json!([user])) {
        Ok(id) => remote_frame_source(&s.rpc, id),
        Err(e) => Arc::new(NoFrames(e)),
    }
}

/// The core cannot be asked (or refuses): every call says why.
struct NoFrames(String);
impl iyakki_shared::lab::FrameSource for NoFrames {
    fn available(&self) -> Result<(), String> {
        Err(format!("{NOT_SERVED}: {}", self.0))
    }
    fn info(&self, _: &str) -> Result<iyakki_shared::lab::VideoInfo, String> {
        self.available().map(|_| unreachable!())
    }
    fn frame_rgba(&self, _: &str, _: f64, _: (u32, u32), _: &Arc<AtomicBool>) -> Result<Vec<u8>, String> {
        self.available().map(|_| unreachable!())
    }
}

/// The frame source is reached through the accessor `lab_frames` of the handle (it needs the person's file rules): the facet's own call has no core end.
pub(crate) fn s_ServicesFacetB5_lab_frames_for(_t: &Arc<dyn ServicesFacetB5>, _reg: &Registry<'_>, _a: Value) -> Result<Value, String> {
    Err(format!("{NOT_SERVED}: ask the handle's lab_frames, which judges every file by the person's rules"))
}

/// The UI's end of the core's chat model.
pub struct RemoteChatBackend {
    rpc: Rpc,
    obj: u64,
    name: OnceLock<String>,
}

/// The UI's end of a served chat model (`obj` 0 is a root).
pub fn remote_chat_backend(rpc: &Rpc, obj: u64) -> Arc<dyn ChatBackend> {
    Arc::new(RemoteChatBackend { rpc: rpc.clone(), obj, name: OnceLock::new() })
}

impl Drop for RemoteChatBackend {
    fn drop(&mut self) {
        self.rpc.release(self.obj);
    }
}

impl RemoteChatBackend {
    fn turn(&self, mode: u8, history: &[Message], tools: &[ToolSpec], on_text: &mut dyn FnMut(&str) -> bool) -> Result<ModelTurn, String> {
        let job = self.rpc.call::<u64>("ChatBackend", self.obj, "begin", json!([mode, history, tools]))?;
        drive::<Result<ModelTurn, String>>(&self.rpc, "ChatBackend", self.obj, job, on_text)?
    }
}

impl ChatBackend for RemoteChatBackend {
    fn provider_name(&self) -> String {
        self.name.get_or_init(|| self.rpc.call::<String>("ChatBackend", self.obj, "provider_name", json!([])).unwrap_or_default()).clone()
    }
    fn complete(&self, history: &[Message]) -> Result<String, String> {
        self.turn(0, history, &[], &mut |_| true).map(|t| t.text)
    }
    fn complete_tools(&self, history: &[Message], tools: &[ToolSpec]) -> Result<ModelTurn, String> {
        self.turn(1, history, tools, &mut |_| true)
    }
    fn complete_tools_stream(&self, history: &[Message], tools: &[ToolSpec], on_text: &mut dyn FnMut(&str) -> bool) -> Result<ModelTurn, String> {
        self.turn(2, history, tools, on_text)
    }
}

// ---------------------------------------------------------------------------------------------------------------- LAB: pictures and vision

type Drawn = (Result<(Vec<u8>, String), String>, ImageTrace);

struct PictureObj {
    backend: Arc<dyn PictureBackend>,
    jobs: Jobs<Drawn>,
}

/// The core's picture model as the root of the trait `PictureBackend`.
pub fn serve_picture_backend(b: Arc<dyn PictureBackend>) -> Obj {
    Obj::Served(Arc::new(PictureObj { backend: b, jobs: Jobs::new() }))
}

impl Served for PictureObj {
    fn iface(&self) -> &'static str {
        "PictureBackend"
    }
    fn serve(&self, reg: &Registry<'_>, method: &str, a: Value) -> Result<Value, String> {
        // the picture itself is a download object, only its number and extension are in the answer
        if let Some(r) = job_calls(&self.jobs, method, &a, |(res, trace): Drawn| {
            let res = res.map(|(bytes, ext)| (down_bytes(reg, bytes), ext));
            ser(&(res, trace))
        }) {
            return r;
        }
        match method {
            "begin" => {
                let (req,): (PictureRequest,) = de(a)?;
                let b = self.backend.clone();
                ser(&self.jobs.start(move |_, _| {
                    let (r, trace) = b.draw(&req);
                    (r.map(|(bytes, ext)| (bytes, ext.to_string())), trace)
                })?)
            }
            other => Err(format!("PictureBackend has no call {other}")),
        }
    }
}

/// The UI's end of the core's picture model. `draw` waits for the whole picture (the server may take minutes: the polls keep the line free).
pub struct RemotePictureBackend {
    rpc: Rpc,
    obj: u64,
}
impl Drop for RemotePictureBackend {
    fn drop(&mut self) {
        self.rpc.release(self.obj);
    }
}

/// The UI's end of a served picture model.
pub fn remote_picture_backend(rpc: &Rpc, obj: u64) -> Arc<dyn PictureBackend> {
    Arc::new(RemotePictureBackend { rpc: rpc.clone(), obj })
}

impl PictureBackend for RemotePictureBackend {
    fn draw(&self, r: &PictureRequest) -> (Result<(Vec<u8>, &'static str), String>, ImageTrace) {
        let lost = |why: String| (Err(why.clone()), ImageTrace { error: why, ..Default::default() });
        let job = match self.rpc.call::<u64>("PictureBackend", self.obj, "begin", json!([r])) {
            Ok(j) => j,
            Err(e) => return lost(e),
        };
        type Done = (Result<(u64, String), String>, ImageTrace);
        match drive::<Done>(&self.rpc, "PictureBackend", self.obj, job, &mut |_| true) {
            Ok((Ok((id, ext)), trace)) => match read_all(&self.rpc, id) {
                Ok(bytes) => (Ok((bytes, iyakki_shared::serde_static::intern(&ext))), trace),
                Err(e) => lost(e),
            },
            Ok((Err(e), trace)) => (Err(e), trace),
            Err(e) => lost(e),
        }
    }
}

// ---------------------------------------------------------------------------------------------------------------- LAB: video frames

/// The rule that names the file a frame call may open: it answers the real path or the sentence why not (the core's file rules of the person).
pub type FramePath = Arc<dyn Fn(&str) -> Result<String, String> + Send + Sync>;

struct FramesObj {
    src: Arc<dyn iyakki_shared::lab::FrameSource>,
    path: FramePath,
    jobs: Jobs<Result<Vec<u8>, String>>,
}

/// The core's frame source (ffmpeg) as the root of the trait `FrameSource`. Every file is named to `path` first: a file the person's rules do not
/// allow is refused before any program runs. The frame is a download object (raw RGBA is bigger than a line).
pub fn serve_frame_source(src: Arc<dyn iyakki_shared::lab::FrameSource>, path: FramePath) -> Obj {
    Obj::Served(Arc::new(FramesObj { src, path, jobs: Jobs::new() }))
}

impl Served for FramesObj {
    fn iface(&self) -> &'static str {
        "FrameSource"
    }
    fn serve(&self, reg: &Registry<'_>, method: &str, a: Value) -> Result<Value, String> {
        if let Some(r) = job_calls(&self.jobs, method, &a, |res: Result<Vec<u8>, String>| ser(&res.map(|b| down_bytes(reg, b)))) {
            return r;
        }
        match method {
            "available" => ser(&self.src.available()),
            "info" => {
                let (file,): (String,) = de(a)?;
                let real = (self.path)(&file)?;
                ser(&self.src.info(&real).map(|i| (i.duration, i.size)))
            }
            "begin" => {
                let (file, t, size): (String, f64, (u32, u32)) = de(a)?;
                let real = (self.path)(&file)?;
                let src = self.src.clone();
                ser(&self.jobs.start(move |_, cancel| src.frame_rgba(&real, t, size, cancel))?)
            }
            other => Err(format!("FrameSource has no call {other}")),
        }
    }
}

/// The UI's end of the core's frame source: a frame is a job the UI waits for, a stop flag of the caller cancels it on the core.
pub struct RemoteFrameSource {
    rpc: Rpc,
    obj: u64,
}
impl Drop for RemoteFrameSource {
    fn drop(&mut self) {
        self.rpc.release(self.obj);
    }
}

/// The UI's end of a served frame source.
pub fn remote_frame_source(rpc: &Rpc, obj: u64) -> Arc<dyn iyakki_shared::lab::FrameSource> {
    Arc::new(RemoteFrameSource { rpc: rpc.clone(), obj })
}

impl iyakki_shared::lab::FrameSource for RemoteFrameSource {
    fn available(&self) -> Result<(), String> {
        self.rpc.call::<Result<(), String>>("FrameSource", self.obj, "available", json!([])).unwrap_or_else(|e| Err(format!("{NOT_SERVED}: {e}")))
    }
    fn info(&self, file: &str) -> Result<iyakki_shared::lab::VideoInfo, String> {
        let (duration, size) = self.rpc.call::<Result<(f64, (u32, u32)), String>>("FrameSource", self.obj, "info", json!([file]))??;
        Ok(iyakki_shared::lab::VideoInfo { duration, size })
    }
    fn frame_rgba(&self, file: &str, t: f64, size: (u32, u32), cancel: &Arc<AtomicBool>) -> Result<Vec<u8>, String> {
        let job = self.rpc.call::<u64>("FrameSource", self.obj, "begin", json!([file, t, size]))?;
        let mut stopped = false;
        let id = loop {
            if cancel.load(Ordering::SeqCst) && !stopped {
                stopped = true;
                self.rpc.call::<bool>("FrameSource", self.obj, "cancel", json!([job]))?;
            }
            let (_, done): (Vec<String>, Option<Result<u64, String>>) = self.rpc.call("FrameSource", self.obj, "poll", json!([job]))?;
            if let Some(d) = done {
                break d?;
            }
        };
        read_all(&self.rpc, id)
    }
}

type Asked = (Result<VisionAnswer, String>, CallTrace);

struct VisionObj {
    backend: Arc<dyn VisionBackend>,
    jobs: Jobs<Asked>,
}

/// The core's vision model as the root of the trait `VisionBackend`.
pub fn serve_vision_backend(b: Arc<dyn VisionBackend>) -> Obj {
    Obj::Served(Arc::new(VisionObj { backend: b, jobs: Jobs::new() }))
}

impl Served for VisionObj {
    fn iface(&self) -> &'static str {
        "VisionBackend"
    }
    fn serve(&self, reg: &Registry<'_>, method: &str, a: Value) -> Result<Value, String> {
        if let Some(r) = job_calls(&self.jobs, method, &a, |t: Asked| ser(&t)) {
            return r;
        }
        match method {
            // the pictures are the bytes of one upload, cut at `sizes`
            "begin" => {
                let (provider, model, question, sizes, inline, upload): (String, String, String, Vec<usize>, String, u64) = de(a)?;
                let all = bytes_in(reg, &inline, upload)?;
                if sizes.iter().sum::<usize>() != all.len() {
                    return Err("the sizes of the pictures do not add up to the bytes sent".into());
                }
                let mut at = 0;
                let images = sizes.iter().map(|n| {
                    let one = all[at..at + n].to_vec();
                    at += n;
                    one
                }).collect();
                let req = VisionRequest { provider, model, question, images };
                let b = self.backend.clone();
                ser(&self.jobs.start(move |say, cancel| b.ask(&req, cancel, &mut |t| {
                    say(t);
                }))?)
            }
            other => Err(format!("VisionBackend has no call {other}")),
        }
    }
}

/// The UI's end of the core's vision model: the text arrives as the model writes it.
pub struct RemoteVisionBackend {
    rpc: Rpc,
    obj: u64,
}
impl Drop for RemoteVisionBackend {
    fn drop(&mut self) {
        self.rpc.release(self.obj);
    }
}

/// The UI's end of a served vision model.
pub fn remote_vision_backend(rpc: &Rpc, obj: u64) -> Arc<dyn VisionBackend> {
    Arc::new(RemoteVisionBackend { rpc: rpc.clone(), obj })
}

impl VisionBackend for RemoteVisionBackend {
    fn ask(&self, r: &VisionRequest, cancel: &AtomicBool, on_text: &mut dyn FnMut(&str)) -> (Result<VisionAnswer, String>, CallTrace) {
        let lost = |why: String| (Err(why.clone()), CallTrace { error: why, ..Default::default() });
        let all: Vec<u8> = r.images.iter().flat_map(|i| i.iter().copied()).collect();
        let sizes: Vec<usize> = r.images.iter().map(Vec::len).collect();
        let sent = match send(&self.rpc, &all) {
            Ok(s) => s,
            Err(e) => return lost(e),
        };
        let job = match self.rpc.call::<u64>("VisionBackend", self.obj, "begin", json!([r.provider, r.model, r.question, sizes, sent.inline, sent.id])) {
            Ok(j) => j,
            Err(e) => return lost(e),
        };
        let mut stopped = false;
        let out = drive::<Asked>(&self.rpc, "VisionBackend", self.obj, job, &mut |t| {
            on_text(t);
            if cancel.load(Ordering::SeqCst) {
                stopped = true;
            }
            !stopped
        });
        out.unwrap_or_else(lost)
    }
}

// ---------------------------------------------------------------------------------------------------------------- the host of the machine

type Ran = Result<String, String>;

struct HostObj {
    host: Arc<dyn HostApi>,
    accounts: Option<Arc<dyn AccountsApi>>,
    audit: iyakki_shared::gitkeys::AuditFn,
    jobs: Jobs<Ran>,
}

/// The core's host as the root of the trait `HostApi`. `accounts` is where the policy of a WebAssembly run is read from; `audit` is the core's
/// audit log, which gets a line for every power change and every run.
pub fn serve_host(host: Arc<dyn HostApi>, accounts: Option<Arc<dyn AccountsApi>>, audit: iyakki_shared::gitkeys::AuditFn) -> Obj {
    Obj::Served(Arc::new(HostObj { host, accounts, audit, jobs: Jobs::new() }))
}

impl Served for HostObj {
    fn iface(&self) -> &'static str {
        "HostApi"
    }
    fn serve(&self, reg: &Registry<'_>, method: &str, a: Value) -> Result<Value, String> {
        if let Some(r) = job_calls(&self.jobs, method, &a, |t: Ran| ser(&t)) {
            return r;
        }
        let h = &self.host;
        match method {
            "detect_env" => ser(&h.detect_env()),
            "net_status" => ser(&h.net_status()),
            "can_power" => ser(&h.can_power()),
            "can_sleep" => ser(&h.can_sleep()),
            // the person's tap on a power card is the approval; the core still switches only what its own host can
            "power_off" | "reboot" | "sleep" => {
                let ok = match method {
                    "sleep" => h.can_sleep(),
                    _ => h.can_power(),
                };
                if !ok {
                    (self.audit)("ui", &format!("host.{method} refused: this machine cannot"));
                    return Err(format!("this machine cannot {method} from here"));
                }
                (self.audit)("ui", &format!("host.{method}"));
                match method {
                    "power_off" => h.power_off(),
                    "reboot" => h.reboot(),
                    _ => h.sleep(), // flaky-ok: the machine suspends, nothing waits
                }
                ser(&true)
            }
            "wasm_tasks" => ser(&h.wasm_tasks().into_iter().map(|t| (t.id, t.user, t.entry, t.bytes, t.started)).collect::<Vec<_>>()),
            // the module and the input are inline when small, else uploads
            "wasm_begin" => {
                let (user, entry, wasm_inline, wasm_upload, stdin_inline, stdin_upload): (String, String, String, u64, String, u64) = de(a)?;
                let accounts = self.accounts.clone().ok_or_else(|| format!("{NOT_SERVED}: this core has no accounts to read the policy from"))?;
                let policy = accounts.get_public(&user).ok_or_else(|| format!("{NOT_SERVED}: no account {user}"))?.policy();
                let wasm = bytes_in(reg, &wasm_inline, wasm_upload)?;
                let stdin = bytes_in(reg, &stdin_inline, stdin_upload)?;
                (self.audit)(&user, &format!("host.run_wasm {entry} ({} bytes)", wasm.len()));
                let h = h.clone();
                ser(&self.jobs.start(move |_, cancel| h.run_wasm_io(&user, &policy, &wasm, &entry, &WasmIo { stdin, cancel: cancel.clone() }))?)
            }
            other => Err(format!("HostApi has no call {other}")),
        }
    }
}

/// The host of the machine as a UI program sees it: it asks the core. A power call the core refuses is a no-op here (the card says so through
/// `can_power`, which is the core's own answer, read once).
pub struct RemoteHost {
    rpc: Rpc,
    obj: u64,
    power: OnceLock<bool>,
    sleep: OnceLock<bool>,
}

/// The UI's end of a served host (`obj` 0 is a root).
pub fn remote_host(rpc: &Rpc, obj: u64) -> Arc<RemoteHost> {
    Arc::new(RemoteHost { rpc: rpc.clone(), obj, power: OnceLock::new(), sleep: OnceLock::new() })
}

impl Drop for RemoteHost {
    fn drop(&mut self) {
        self.rpc.release(self.obj);
    }
}

impl RemoteHost {
    fn text(&self, method: &str) -> String {
        self.rpc.call::<String>("HostApi", self.obj, method, json!([])).unwrap_or_else(|e| <String as Lost>::lost(&e))
    }
}

impl HostApi for RemoteHost {
    fn detect_env(&self) -> String {
        self.text("detect_env")
    }
    fn net_status(&self) -> String {
        self.text("net_status")
    }
    fn run_wasm(&self, user: &str, policy: &iyakki_sec::policy::Policy, wasm: &[u8], entry: &str) -> Result<String, String> {
        self.run_wasm_io(user, policy, wasm, entry, &WasmIo::default())
    }
    /// The policy the caller names is not sent: the core reads the account's own.
    fn run_wasm_io(&self, user: &str, _policy: &iyakki_sec::policy::Policy, wasm: &[u8], entry: &str, io: &WasmIo) -> Result<String, String> {
        if io.cancel.load(Ordering::SeqCst) {
            return Err("stopped before it started".into());
        }
        let module = send(&self.rpc, wasm)?;
        let input = send(&self.rpc, &io.stdin)?;
        let job = self.rpc.call::<u64>("HostApi", self.obj, "wasm_begin", json!([user, entry, module.inline, module.id, input.inline, input.id]))?;
        let mut asked = false;
        // the stop flag is looked at between polls: a run is stopped within one poll
        loop {
            if io.cancel.load(Ordering::SeqCst) && !asked {
                asked = true;
                self.rpc.call::<bool>("HostApi", self.obj, "cancel", json!([job]))?;
            }
            let (_, done): (Vec<String>, Option<Ran>) = self.rpc.call("HostApi", self.obj, "poll", json!([job]))?;
            if let Some(r) = done {
                return r;
            }
        }
    }
    fn power_off(&self) {
        let _ = self.rpc.call::<bool>("HostApi", self.obj, "power_off", json!([]));
    }
    fn wasm_tasks(&self) -> Vec<iyakki_core_api::WasmTask> {
        self.rpc.call::<Vec<(u64, String, String, usize, i64)>>("HostApi", self.obj, "wasm_tasks", json!([])).unwrap_or_default().into_iter().map(|(id, user, entry, bytes, started)| iyakki_core_api::WasmTask { id, user, entry, bytes, started }).collect()
    }
    fn reboot(&self) {
        let _ = self.rpc.call::<bool>("HostApi", self.obj, "reboot", json!([]));
    }
    fn can_power(&self) -> bool {
        *self.power.get_or_init(|| self.rpc.call::<bool>("HostApi", self.obj, "can_power", json!([])).unwrap_or(false))
    }
    fn can_sleep(&self) -> bool {
        *self.sleep.get_or_init(|| self.rpc.call::<bool>("HostApi", self.obj, "can_sleep", json!([])).unwrap_or(false))
    }
    fn sleep(&self) {
        let _ = self.rpc.call::<bool>("HostApi", self.obj, "sleep", json!([]));
    }
}

// ---------------------------------------------------------------------------------------------------------------- the accessors of the handle

/// The calls of the accessors of this file (the core's end; `served::serve_manual` falls through to here).
pub(crate) fn serve_apps(root: &ServicesRoot, reg: &Registry<'_>, method: &str, a: Value) -> Option<Result<Value, String>> {
    let t: &dyn ServicesHandle = root.svc.as_ref();
    let add = |o: Obj| reg.add(o);
    Some(match method {
        "chat_backend" => ser(&add(serve_chat_backend(t.chat_backend()))),
        "chat_backend_for" => de::<(ModelChoice,)>(a).and_then(|(c,)| ser(&add(serve_chat_backend(t.chat_backend_for(c))))),
        // the person's own file rules decide every file of the frame source; the account is read by name from the core's accounts (as `remote_kit` does)
        "lab_frames" => (|| {
            let (user,): (String,) = de(a.clone())?;
            let scope = crate::served::ServedParts::need(&root.parts.file_scope, "file rules")?;
            let accounts = crate::served::ServedParts::need(&root.parts.accounts, "accounts")?;
            let policy = accounts.get_public(&user).ok_or_else(|| format!("{NOT_SERVED}: no account {user}"))?.policy();
            let rule = crate::served::local_path_rule(scope, policy, user.clone(), t.dir().to_path_buf());
            let path: FramePath = Arc::new(move |file: &str| rule(file, false).map(|p| p.to_string_lossy().into_owned()));
            ser(&add(serve_frame_source(t.b5().lab_frames_for(&user), path)))
        })(),
        "ui_host" => crate::served::ServedParts::need(&root.parts.host, "host").and_then(|h| ser(&add(serve_host(h, root.parts.accounts.clone(), root.audit.clone())))),
        _ => return crate::served_sound::serve_sound(root, reg, method, a.clone()).or_else(|| crate::served_state::serve_state(t, reg, method)),
    })
}

impl RemoteServicesHandle {
    /// The core's chat model: a remote object (a lost core answers every turn with the reason).
    pub(crate) fn m_chat_backend(&self) -> Arc<dyn ChatBackend> {
        remote_chat_backend(&self.rpc, self.rpc.call::<u64>("ServicesHandle", 0, "chat_backend", json!([])).unwrap_or(u64::MAX))
    }

    /// The core's chat model for the provider and model the person picked.
    pub(crate) fn m_chat_backend_for(&self, choice: ModelChoice) -> Arc<dyn ChatBackend> {
        remote_chat_backend(&self.rpc, self.rpc.call::<u64>("ServicesHandle", 0, "chat_backend_for", json!([choice])).unwrap_or(u64::MAX))
    }

    /// The host of the core's machine: power, sleep and WebAssembly runs are the core's; a core without a host answers like a machine that cannot.
    pub fn host(&self) -> Arc<RemoteHost> {
        remote_host(&self.rpc, self.rpc.call::<u64>("ServicesHandle", 0, "ui_host", json!([])).unwrap_or(u64::MAX))
    }
}

// ---------------------------------------------------------------------------------------------------------------- the generated traits' hand-written methods

/// `ServicesFacetB5::lab_backends` (the UI's end): the picture and the vision model of the core as two remote objects.
pub(crate) fn c_ServicesFacetB5_lab_backends(s: &RemoteServicesFacetB5) -> Option<(Arc<dyn PictureBackend>, Arc<dyn VisionBackend>)> {
    let (p, v) = s.rpc.call::<Option<(u64, u64)>>("ServicesFacetB5", s.obj, "lab_backends", json!([])).ok().flatten()?;
    Some((remote_picture_backend(&s.rpc, p), remote_vision_backend(&s.rpc, v)))
}

/// `ServicesFacetB5::lab_backends` (the core's end).
pub(crate) fn s_ServicesFacetB5_lab_backends(t: &Arc<dyn ServicesFacetB5>, reg: &Registry<'_>, _a: Value) -> Result<Value, String> {
    ser(&t.lab_backends().map(|(p, v)| (reg.add(serve_picture_backend(p)), reg.add(serve_vision_backend(v)))))
}

/// `ServicesFacetB5::story_store` (the UI's end): the library of stories is a folder under the person's home, and the file rules are the
/// core's (asked over the socket: the home, the roots and every later path check), so the store is built here over the core's answers.
pub(crate) fn c_ServicesFacetB5_story_store(s: &RemoteServicesFacetB5, policy: &iyakki_sec::policy::Policy, user: &str, data_dir: &std::path::Path) -> Option<iyakki_shared::story::StoryStore> {
    let home = s.rpc.call::<PathBuf>("ServicesHandle", 0, "scope_home_root", json!([user, data_dir])).ok()?;
    c_ServicesFacetB5_story_store_at(s, policy, user, data_dir, home)
}

/// `ServicesFacetB5::story_store_at` (the UI's end): the same with the home folder given.
pub(crate) fn c_ServicesFacetB5_story_store_at(s: &RemoteServicesFacetB5, _policy: &iyakki_sec::policy::Policy, user: &str, data_dir: &std::path::Path, home: PathBuf) -> Option<iyakki_shared::story::StoryStore> {
    // the policy the caller names is never used: the store holds the core's reading of the account (D-SEC1-VOUCH); the roots are the core's too
    let policy = s.rpc.call::<iyakki_sec::policy::Policy>("ServicesHandle", 0, "policy_of", json!([user])).ok()?;
    let roots = s.rpc.call::<iyakki_core_api::FileRoots>("ServicesHandle", 0, "scope_roots_with_home", json!([policy, user, data_dir, home])).ok()?;
    Some(iyakki_shared::story::StoryStore::open_with(&policy, user, home, roots))
}

/// The two story store calls have no core end: the store lives where the person's files are and every path is judged by the core's file rules.
pub(crate) fn s_ServicesFacetB5_story_store(_t: &Arc<dyn ServicesFacetB5>, _reg: &Registry<'_>, _a: Value) -> Result<Value, String> {
    Err(format!("{NOT_SERVED}: the story store is built in the UI over the core's file rules"))
}

/// See [`s_ServicesFacetB5_story_store`].
pub(crate) fn s_ServicesFacetB5_story_store_at(_t: &Arc<dyn ServicesFacetB5>, _reg: &Registry<'_>, _a: Value) -> Result<Value, String> {
    Err(format!("{NOT_SERVED}: the story store is built in the UI over the core's file rules"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;

    #[test]
    fn a_job_says_pieces_ends_once_and_is_forgotten_and_a_wrong_number_is_refused() {
        let jobs: Jobs<u32> = Jobs::waiting(Duration::from_secs(30));
        let (go_tx, go_rx) = mpsc::channel::<()>();
        let id = jobs
            .start(move |say, _| {
                say("one");
                go_rx.recv().unwrap();
                say("two");
                7
            })
            .unwrap();
        // the first poll returns as soon as there is news, the end waits until the job says it
        assert_eq!(jobs.poll(id).unwrap(), (vec!["one".to_string()], None));
        go_tx.send(()).unwrap();
        let mut said = Vec::new();
        let done = loop {
            let (p, d) = jobs.poll(id).unwrap();
            said.extend(p);
            if let Some(d) = d {
                break d;
            }
        };
        assert_eq!((said, done), (vec!["two".to_string()], 7));
        assert_eq!(jobs.running(), 0, "an ended job is forgotten once its result was taken");
        assert!(jobs.poll(id).is_err(), "the result comes once");
        assert!(jobs.poll(999).is_err());
        assert!(!jobs.cancel(999));
    }

    #[test]
    fn the_number_of_jobs_is_capped_cancel_reaches_the_job_and_empty_pieces_are_not_piled_up() {
        let jobs: Jobs<bool> = Jobs::waiting(Duration::from_secs(30));
        let mut holds = Vec::new();
        for _ in 0..MAX_JOBS {
            let (tx, rx) = mpsc::channel::<()>();
            holds.push(tx);
            jobs.start(move |_, cancel| {
                let _ = rx.recv();
                cancel.load(Ordering::SeqCst)
            })
            .unwrap();
        }
        assert!(jobs.start(|_, _| true).is_err(), "more jobs than the cap are refused by name");
        assert!(jobs.cancel(1));
        holds[0].send(()).unwrap();
        assert_eq!(jobs.poll(1).unwrap().1, Some(true), "the job saw the cancel flag");
        // a busy model says "" many times: the poll sees one
        let quiet: Jobs<()> = Jobs::waiting(Duration::from_secs(30));
        let (said_tx, said_rx) = mpsc::channel::<()>();
        let id = quiet
            .start(move |say, _| {
                for _ in 0..50 {
                    say("");
                }
                say("x");
                said_tx.send(()).unwrap();
            })
            .unwrap();
        said_rx.recv().unwrap();
        let (p, _) = quiet.poll(id).unwrap();
        assert_eq!(p, vec!["".to_string(), "x".to_string()], "the fifty empty pieces are one");
        drop(holds);
    }
}
