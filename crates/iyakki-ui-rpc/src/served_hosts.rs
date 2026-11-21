//! The traits the UI used to be given as objects and that no generator reaches, served over the UI socket (F-SPLIT-03 part 3,
//! D-SPLIT-03-SERVED): the cue output, the two upstreams, the streamer, the reading synth, the lesson host and the dashboards' host.
//!
//! One rule for every byte that does not fit a line (a line is at most 64 KiB): what goes to the core is put in an `Upload` object in chunks
//! and named in the call; what comes back is a `Download` object the UI reads chunk by chunk. A callback the core made to the UI becomes a pull:
//! the UI asks for the next chunk. Nothing here hands out a secret (the dashboards' `resolve_secret` answers "not served": a secret is resolved
//! in the core), and nothing runs a program for the UI (`runner` answers "not served"; `kubectl` is read-only and checked by the core).

use std::io::{Cursor, Read};
use std::sync::{Arc, Mutex, PoisonError};

use iyakki_core_api::soundtypes::CueOut;
use iyakki_dash::tools::DashHost;
use iyakki_proxy::{StreamHead, Streamer, TlsMode, Upstream, UpstreamResponse};
use iyakki_shared::b64::{b64_decode, base64_encode};
use iyakki_shared::speechtime::Timed;
use iyakki_shared::story::Synth;
use iyakki_shared::studylesson::LessonHost;
use iyakki_shared::voice::SpeakOpts;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::served::{call, Served, NOT_SERVED};
use crate::{de, ser, Obj, Registry, RemoteServicesHandle, Rpc, ServicesRoot};

/// Raw bytes in one line (base64 makes it 4/3 of this, under the line limit with room for the rest of the call).
pub(crate) const CHUNK: usize = 36 * 1024;
/// The most one object may carry either way (a cue, a spoken page, an HTTP body).
pub(crate) const MAX_BYTES: usize = 32 * 1024 * 1024;

fn unb64(s: &str) -> Result<Vec<u8>, String> {
    b64_decode(s).ok_or_else(|| "the bytes are not valid base64".to_string())
}

// ---------------------------------------------------------------------------------------------------------------- bytes both ways

/// Bytes the UI is sending to the core, put together chunk by chunk.
#[derive(Default)]
struct UploadObj(Mutex<Vec<u8>>);
impl Served for UploadObj {
    fn iface(&self) -> &'static str {
        "Upload"
    }
    fn serve(&self, _reg: &Registry<'_>, method: &str, a: Value) -> Result<Value, String> {
        match method {
            "put" => {
                let (c,): (String,) = de(a)?;
                let bytes = unb64(&c)?;
                let mut b = self.0.lock().unwrap_or_else(PoisonError::into_inner);
                if b.len() + bytes.len() > MAX_BYTES {
                    return Err(format!("more than {MAX_BYTES} bytes in one upload"));
                }
                b.extend_from_slice(&bytes);
                ser(&b.len())
            }
            other => Err(format!("Upload has no call {other}")),
        }
    }
    fn take_bytes(&self) -> Option<Vec<u8>> {
        Some(std::mem::take(&mut *self.0.lock().unwrap_or_else(PoisonError::into_inner)))
    }
}

/// The root `ServicesHandle` of a core that serves nothing but the upload objects (`upload_new`): the reference server and the tests that carry
/// bytes bigger than a line use it; the real core serves `upload_new` as part of its whole `ServicesHandle` root.
pub fn serve_uploads() -> Obj {
    struct Only;
    impl Served for Only {
        fn iface(&self) -> &'static str {
            "ServicesHandle"
        }
        fn serve(&self, reg: &Registry<'_>, method: &str, _a: Value) -> Result<Value, String> {
            match method {
                "upload_new" => ser(&reg.add(Obj::Served(Arc::new(UploadObj::default())))),
                other => Err(format!("{NOT_SERVED}: {other}")),
            }
        }
    }
    Obj::Served(Arc::new(Only))
}

/// Bytes the core is sending to the UI: read a chunk at a time until an empty answer.
struct DownObj(Mutex<Box<dyn Read + Send>>);
impl Served for DownObj {
    fn iface(&self) -> &'static str {
        "Download"
    }
    fn serve(&self, _reg: &Registry<'_>, method: &str, a: Value) -> Result<Value, String> {
        match method {
            "read" => {
                let (max,): (usize,) = de(a)?;
                let mut buf = vec![0u8; max.clamp(1, CHUNK)];
                let n = self.0.lock().unwrap_or_else(PoisonError::into_inner).read(&mut buf).map_err(|e| e.to_string())?;
                ser(&base64_encode(&buf[..n]))
            }
            other => Err(format!("Download has no call {other}")),
        }
    }
}

fn down(reg: &Registry<'_>, r: Box<dyn Read + Send>) -> u64 {
    reg.add(Obj::Served(Arc::new(DownObj(Mutex::new(r)))))
}
pub(crate) fn down_bytes(reg: &Registry<'_>, b: Vec<u8>) -> u64 {
    down(reg, Box::new(Cursor::new(b)))
}

/// What the call carries of the bytes it was sent: inline when small, else the upload it names.
pub(crate) fn bytes_in(reg: &Registry<'_>, inline: &str, upload: u64) -> Result<Vec<u8>, String> {
    if upload == 0 {
        return unb64(inline);
    }
    reg.find(upload, "Upload").and_then(|o| match o {
        Obj::Served(s) => s.take_bytes(),
        _ => None,
    }).ok_or_else(|| "that upload is not there (it was released or never made)".to_string())
}

/// What the UI sends: small inline, big in an upload object that is released when this guard goes.
pub(crate) struct Sent {
    rpc: Rpc,
    pub(crate) inline: String,
    pub(crate) id: u64,
}
impl Drop for Sent {
    fn drop(&mut self) {
        self.rpc.release(self.id);
    }
}
pub(crate) fn send(rpc: &Rpc, data: &[u8]) -> Result<Sent, String> {
    if data.len() > MAX_BYTES {
        return Err(format!("more than {MAX_BYTES} bytes cannot be sent over the UI socket"));
    }
    if data.len() <= CHUNK {
        return Ok(Sent { rpc: rpc.clone(), inline: base64_encode(data), id: 0 });
    }
    let id = rpc.call::<u64>("ServicesHandle", 0, "upload_new", json!([]))?;
    let s = Sent { rpc: rpc.clone(), inline: String::new(), id };
    for c in data.chunks(CHUNK) {
        rpc.call::<usize>("Upload", id, "put", json!([base64_encode(c)]))?;
    }
    Ok(s)
}

/// A download object read as a stream; released when dropped.
struct RemoteBody {
    rpc: Rpc,
    obj: u64,
    done: bool,
}
impl Read for RemoteBody {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        if self.done || buf.is_empty() {
            return Ok(0);
        }
        let io = |e: String| std::io::Error::other(e);
        let c = self.rpc.call::<String>("Download", self.obj, "read", json!([buf.len().min(CHUNK)])).map_err(io)?;
        let bytes = unb64(&c).map_err(io)?;
        if bytes.is_empty() {
            self.done = true;
        }
        buf[..bytes.len()].copy_from_slice(&bytes);
        Ok(bytes.len())
    }
}
impl Drop for RemoteBody {
    fn drop(&mut self) {
        self.rpc.release(self.obj);
    }
}
pub(crate) fn read_all(rpc: &Rpc, obj: u64) -> Result<Vec<u8>, String> {
    let mut body = RemoteBody { rpc: rpc.clone(), obj, done: false };
    let mut out = Vec::new();
    std::io::Read::take(&mut body, MAX_BYTES as u64 + 1).read_to_end(&mut out).map_err(|e| e.to_string())?;
    if out.len() > MAX_BYTES {
        return Err(format!("more than {MAX_BYTES} bytes cannot come over the UI socket"));
    }
    Ok(out)
}

// ---------------------------------------------------------------------------------------------------------------- the core's end

/// The calls of the accessors of this file (the core's end; `served::serve_manual` falls through to here).
pub(crate) fn serve_hosts(root: &ServicesRoot, reg: &Registry<'_>, method: &str, a: Value) -> Option<Result<Value, String>> {
    let t: &dyn iyakki_agent_api::ServicesHandle = root.svc.as_ref();
    let add = |s: Arc<dyn Served>| reg.add(Obj::Served(s));
    Some(match method {
        "upload_new" => ser(&add(Arc::new(UploadObj::default()))),
        "cue_out" => ser(&add(Arc::new(CueOutObj(t.cue_out())))),
        "quick_upstream" => ser(&add(Arc::new(UpstreamObj(t.quick_upstream())))),
        "slow_upstream" => ser(&add(Arc::new(UpstreamObj(t.slow_upstream())))),
        "streamer" => ser(&t.streamer().map(|s| add(Arc::new(StreamerObj(s))))),
        "reading_synth" => de::<(SpeakOpts,)>(a).and_then(|(o,)| ser(&add(Arc::new(SynthObj(t.reading_synth(o)))))),
        "lesson_host" => ser(&add(Arc::new(LessonHostObj(t.lesson_host())))),
        "dash_host" => ser(&add(Arc::new(DashHostObj(t.dash_host())))),
        _ => return None,
    })
}

struct CueOutObj(Arc<dyn CueOut>);
impl Served for CueOutObj {
    fn iface(&self) -> &'static str {
        "CueOut"
    }
    fn serve(&self, reg: &Registry<'_>, method: &str, a: Value) -> Result<Value, String> {
        match method {
            "ready" => ser(&self.0.ready()),
            "play" => {
                let (inline, upload): (String, u64) = de(a)?;
                let wav = bytes_in(reg, &inline, upload)?;
                ser(&self.0.play(&wav))
            }
            other => Err(format!("CueOut has no call {other}")),
        }
    }
}

struct UpstreamObj(Arc<dyn Upstream>);
type Headers = Vec<(String, String)>;
impl Served for UpstreamObj {
    fn iface(&self) -> &'static str {
        "Upstream"
    }
    fn serve(&self, reg: &Registry<'_>, method: &str, a: Value) -> Result<Value, String> {
        let (m, url, headers, inline, upload): (String, String, Headers, String, u64) = de(a)?;
        let body = bytes_in(reg, &inline, upload)?;
        match method {
            "request" => ser(&self.0.request(&m, &url, &headers, &body).map(|r| (r.status, down_bytes(reg, r.body)))),
            "request_full" => ser(&self.0.request_full(&m, &url, &headers, &body).map(|(r, h)| (r.status, down_bytes(reg, r.body), h))),
            other => Err(format!("Upstream has no call {other}")),
        }
    }
}

/// A TLS check as it crosses (the certificates are base64 of DER).
#[derive(Serialize, Deserialize)]
enum WireTls {
    Pin(String),
    ExtraChain(Vec<String>),
}

struct StreamerObj(Arc<dyn Streamer>);
fn head(reg: &Registry<'_>, h: Result<StreamHead, String>) -> Result<Value, String> {
    ser(&h.map(|h| (h.status, h.headers, down(reg, h.body))))
}
impl Served for StreamerObj {
    fn iface(&self) -> &'static str {
        "Streamer"
    }
    fn serve(&self, reg: &Registry<'_>, method: &str, a: Value) -> Result<Value, String> {
        match method {
            "open" => {
                let (url, headers): (String, Headers) = de(a)?;
                head(reg, self.0.open(&url, &headers))
            }
            "post" => {
                let (url, headers, inline, upload): (String, Headers, String, u64) = de(a)?;
                let body = bytes_in(reg, &inline, upload)?;
                head(reg, self.0.post(&url, &headers, &body))
            }
            "open_tls" => {
                let (url, headers, mode): (String, Headers, WireTls) = de(a)?;
                let mode = match mode {
                    WireTls::Pin(f) => TlsMode::Pin(f),
                    WireTls::ExtraChain(c) => TlsMode::ExtraChain(c.iter().map(|d| unb64(d)).collect::<Result<_, _>>()?),
                };
                head(reg, self.0.open_tls(&url, &headers, &mode))
            }
            "peer_chain" => {
                let (host, port): (String, u16) = de(a)?;
                ser(&self.0.peer_chain(&host, port).map(|c| c.iter().map(|d| base64_encode(d)).collect::<Vec<String>>()))
            }
            other => Err(format!("Streamer has no call {other}")),
        }
    }
}

struct SynthObj(Arc<dyn Synth>);
impl Served for SynthObj {
    fn iface(&self) -> &'static str {
        "Synth"
    }
    fn serve(&self, reg: &Registry<'_>, method: &str, a: Value) -> Result<Value, String> {
        match method {
            "synth" => {
                let (text,): (String,) = de(a)?;
                ser(&self.0.synth(&text).map(|w| down_bytes(reg, w)))
            }
            "synth_timed" => {
                let (text,): (String,) = de(a)?;
                ser(&self.0.synth_timed(&text).map(|(w, t)| (down_bytes(reg, w), t)))
            }
            "align" => {
                let (text, inline, upload): (String, String, u64) = de(a)?;
                let wav = bytes_in(reg, &inline, upload)?;
                ser(&self.0.align(&text, &wav))
            }
            other => Err(format!("Synth has no call {other}")),
        }
    }
}

struct LessonHostObj(Arc<dyn LessonHost>);
impl Served for LessonHostObj {
    fn iface(&self) -> &'static str {
        "LessonHost"
    }
    fn serve(&self, reg: &Registry<'_>, method: &str, a: Value) -> Result<Value, String> {
        match method {
            "study_dir" => ser(&self.0.study().dir),
            "has_image_model" => ser(&self.0.has_image_model()),
            "generate_image" => {
                let (prompt, size): (String, String) = de(a)?;
                ser(&self.0.generate_image(&prompt, &size).map(|b| down_bytes(reg, b)))
            }
            other => Err(format!("LessonHost has no call {other}")),
        }
    }
}

/// The `kubectl` words a dashboard may ask for: reads only. The core runs them with its own cluster login and the answer is capped.
const KUBECTL_READS: [&str; 6] = ["get", "top", "describe", "version", "cluster-info", "api-resources"];

/// May the core run this `kubectl` for a dashboard? The first word must be a read, no flag may name another config or a proxy, and nothing may
/// pass a value to a shell.
pub(crate) fn kubectl_allowed(args: &[String]) -> Result<(), String> {
    let first = args.iter().find(|a| !a.starts_with('-')).map(String::as_str).unwrap_or("");
    if !KUBECTL_READS.contains(&first) {
        return Err(format!("{NOT_SERVED}: a dashboard may only read from the cluster (get, top, describe, version, cluster-info, api-resources), not `{first}`"));
    }
    for a in args {
        let low = a.to_ascii_lowercase();
        if ["--kubeconfig", "--server", "-s", "--token", "--as", "--as-group", "--raw", "--insecure-skip-tls-verify", "--certificate-authority", "--client-key", "--client-certificate", "--username", "--password"].iter().any(|f| low == *f || low.starts_with(&format!("{f}="))) {
            return Err(format!("{NOT_SERVED}: `{a}` would change which cluster or which login is used"));
        }
    }
    Ok(())
}

struct DashHostObj(Arc<dyn DashHost>);
impl Served for DashHostObj {
    fn iface(&self) -> &'static str {
        "DashHost"
    }
    fn serve(&self, _reg: &Registry<'_>, method: &str, a: Value) -> Result<Value, String> {
        let t = &self.0;
        match method {
            "data_dir" => ser(&t.data_dir()),
            "home_root" => {
                let (u,): (String,) = de(a)?;
                ser(&t.home_root(&u))
            }
            "vault_names" => {
                let (u,): (String,) = de(a)?;
                ser(&t.vault_names(&u))
            }
            "vault_set" => {
                let (u, n, v): (String, String, String) = de(a)?;
                ser(&t.vault_set(&u, &n, &v))
            }
            "read_ok" => {
                let (p, u, path): (iyakki_sec::policy::Policy, String, std::path::PathBuf) = de(a)?;
                ser(&t.read_ok(&p, &u, &path))
            }
            "now_unix" => ser(&t.now_unix()),
            "kubectl" | "kubectl_big" => {
                let (args,): (Vec<String>,) = de(a)?;
                kubectl_allowed(&args)?;
                let refs: Vec<&str> = args.iter().map(String::as_str).collect();
                let k = t.kubectl();
                ser(&if method == "kubectl" { k.kubectl(&refs) } else { k.kubectl_big(&refs) })
            }
            other => Err(format!("DashHost has no call {other}")),
        }
    }
}

// ---------------------------------------------------------------------------------------------------------------- the UI's end

impl RemoteServicesHandle {
    fn obj_of(&self, method: &str, args: Value) -> u64 {
        self.rpc.call::<u64>("ServicesHandle", 0, method, args).unwrap_or(u64::MAX)
    }

    /// The core's sound output for cues: the UI sends the sound, the core plays it.
    pub(crate) fn m_cue_out(&self) -> Arc<dyn CueOut> {
        Arc::new(RemoteCueOut { rpc: self.rpc.clone(), obj: self.obj_of("cue_out", json!([])) })
    }

    pub(crate) fn m_quick_upstream(&self) -> Arc<dyn Upstream> {
        Arc::new(RemoteUpstream { rpc: self.rpc.clone(), obj: self.obj_of("quick_upstream", json!([])) })
    }

    pub(crate) fn m_slow_upstream(&self) -> Arc<dyn Upstream> {
        Arc::new(RemoteUpstream { rpc: self.rpc.clone(), obj: self.obj_of("slow_upstream", json!([])) })
    }

    pub(crate) fn m_streamer(&self) -> Option<Arc<dyn Streamer>> {
        let id = self.rpc.call::<Option<u64>>("ServicesHandle", 0, "streamer", json!([])).ok().flatten()?;
        Some(Arc::new(RemoteStreamer { rpc: self.rpc.clone(), obj: id }))
    }

    pub(crate) fn m_reading_synth(&self, opts: SpeakOpts) -> Arc<dyn Synth> {
        Arc::new(RemoteSynth { rpc: self.rpc.clone(), obj: self.obj_of("reading_synth", json!([opts])) })
    }

    pub(crate) fn m_lesson_host(&self) -> Arc<dyn LessonHost> {
        let obj = self.obj_of("lesson_host", json!([]));
        let dir = call::<std::path::PathBuf>(&self.rpc, "LessonHost", obj, "study_dir", json!([]));
        Arc::new(RemoteLessonHost { rpc: self.rpc.clone(), obj, study: iyakki_shared::study::StudyStore::new(&dir) })
    }

    pub(crate) fn m_dash_host(&self) -> Arc<dyn DashHost> {
        Arc::new(RemoteDashHost { rpc: self.rpc.clone(), obj: self.obj_of("dash_host", json!([])) })
    }
}

macro_rules! drops {
    ($($t:ty),*) => { $(impl Drop for $t { fn drop(&mut self) { self.rpc.release(self.obj); } })* };
}

struct RemoteCueOut {
    rpc: Rpc,
    obj: u64,
}
struct RemoteUpstream {
    rpc: Rpc,
    obj: u64,
}
struct RemoteStreamer {
    rpc: Rpc,
    obj: u64,
}
struct RemoteSynth {
    rpc: Rpc,
    obj: u64,
}
struct RemoteLessonHost {
    rpc: Rpc,
    obj: u64,
    study: iyakki_shared::study::StudyStore,
}
struct RemoteDashHost {
    rpc: Rpc,
    obj: u64,
}
drops!(RemoteCueOut, RemoteUpstream, RemoteStreamer, RemoteSynth, RemoteLessonHost, RemoteDashHost);

impl CueOut for RemoteCueOut {
    fn ready(&self) -> Result<(), String> {
        call(&self.rpc, "CueOut", self.obj, "ready", json!([]))
    }
    fn play(&self, wav: &[u8]) -> Result<(), String> {
        let s = send(&self.rpc, wav)?;
        self.rpc.call::<Result<(), String>>("CueOut", self.obj, "play", json!([s.inline, s.id]))?
    }
}

impl RemoteUpstream {
    fn go<T: serde::de::DeserializeOwned>(&self, method: &str, m: &str, url: &str, headers: &[(String, String)], body: &[u8]) -> Result<T, String> {
        let s = send(&self.rpc, body)?;
        self.rpc.call::<Result<T, String>>("Upstream", self.obj, method, json!([m, url, headers, s.inline, s.id]))?
    }
}
impl Upstream for RemoteUpstream {
    fn request(&self, method: &str, url: &str, headers: &[(String, String)], body: &[u8]) -> Result<UpstreamResponse, String> {
        let (status, id): (u16, u64) = self.go("request", method, url, headers, body)?;
        Ok(UpstreamResponse { status, body: read_all(&self.rpc, id)? })
    }
    fn request_full(&self, method: &str, url: &str, headers: &[(String, String)], body: &[u8]) -> Result<(UpstreamResponse, Vec<(String, String)>), String> {
        let (status, id, h): (u16, u64, Vec<(String, String)>) = self.go("request_full", method, url, headers, body)?;
        Ok((UpstreamResponse { status, body: read_all(&self.rpc, id)? }, h))
    }
}

impl RemoteStreamer {
    fn head(&self, method: &str, args: Value) -> Result<StreamHead, String> {
        let (status, headers, id): (u16, Vec<(String, String)>, u64) = self.rpc.call::<Result<_, String>>("Streamer", self.obj, method, args)??;
        Ok(StreamHead { status, headers, body: Box::new(RemoteBody { rpc: self.rpc.clone(), obj: id, done: false }) })
    }
}
impl Streamer for RemoteStreamer {
    fn open(&self, url: &str, headers: &[(String, String)]) -> Result<StreamHead, String> {
        self.head("open", json!([url, headers]))
    }
    fn post(&self, url: &str, headers: &[(String, String)], body: &[u8]) -> Result<StreamHead, String> {
        let s = send(&self.rpc, body)?;
        self.head("post", json!([url, headers, s.inline, s.id]))
    }
    fn open_tls(&self, url: &str, headers: &[(String, String)], mode: &TlsMode) -> Result<StreamHead, String> {
        let mode = match mode {
            TlsMode::Pin(f) => WireTls::Pin(f.clone()),
            TlsMode::ExtraChain(c) => WireTls::ExtraChain(c.iter().map(|d| base64_encode(d)).collect()),
        };
        self.head("open_tls", json!([url, headers, mode]))
    }
    fn peer_chain(&self, host: &str, port: u16) -> Result<Vec<Vec<u8>>, String> {
        let chain: Vec<String> = self.rpc.call::<Result<Vec<String>, String>>("Streamer", self.obj, "peer_chain", json!([host, port]))??;
        chain.iter().map(|d| unb64(d)).collect()
    }
}

impl Synth for RemoteSynth {
    fn synth(&self, text: &str) -> Result<Vec<u8>, String> {
        let id = self.rpc.call::<Result<u64, String>>("Synth", self.obj, "synth", json!([text]))??;
        read_all(&self.rpc, id)
    }
    fn synth_timed(&self, text: &str) -> Result<(Vec<u8>, Option<Timed>), String> {
        let (id, t) = self.rpc.call::<Result<(u64, Option<Timed>), String>>("Synth", self.obj, "synth_timed", json!([text]))??;
        Ok((read_all(&self.rpc, id)?, t))
    }
    fn align(&self, text: &str, wav: &[u8]) -> Option<Timed> {
        let s = send(&self.rpc, wav).ok()?;
        self.rpc.call::<Option<Timed>>("Synth", self.obj, "align", json!([text, s.inline, s.id])).ok().flatten()
    }
}

impl LessonHost for RemoteLessonHost {
    fn study(&self) -> &iyakki_shared::study::StudyStore {
        &self.study
    }
    fn has_image_model(&self) -> bool {
        call(&self.rpc, "LessonHost", self.obj, "has_image_model", json!([]))
    }
    fn generate_image(&self, prompt: &str, size: &str) -> Result<Vec<u8>, String> {
        let id = self.rpc.call::<Result<u64, String>>("LessonHost", self.obj, "generate_image", json!([prompt, size]))??;
        read_all(&self.rpc, id)
    }
}

/// The dashboards' host as the UI sees it. A secret is never resolved here and no program is run for the UI.
impl DashHost for RemoteDashHost {
    fn data_dir(&self) -> std::path::PathBuf {
        call(&self.rpc, "DashHost", self.obj, "data_dir", json!([]))
    }
    fn home_root(&self, user: &str) -> std::path::PathBuf {
        call(&self.rpc, "DashHost", self.obj, "home_root", json!([user]))
    }
    fn resolve_secret(&self, _user: &str, _reference: &str) -> Result<zeroize::Zeroizing<String>, String> {
        Err(format!("{NOT_SERVED}: a secret is resolved in the core, never handed to the UI"))
    }
    fn vault_names(&self, user: &str) -> Vec<String> {
        call(&self.rpc, "DashHost", self.obj, "vault_names", json!([user]))
    }
    fn vault_set(&self, user: &str, name: &str, value: &str) -> Result<(), String> {
        call(&self.rpc, "DashHost", self.obj, "vault_set", json!([user, name, value]))
    }
    fn read_ok(&self, policy: &iyakki_sec::policy::Policy, user: &str, path: &std::path::Path) -> bool {
        call(&self.rpc, "DashHost", self.obj, "read_ok", json!([policy, user, path]))
    }
    fn kubectl(&self) -> Arc<dyn iyakki_shared::kubectl::Kubectl> {
        Arc::new(RemoteKubectl { rpc: self.rpc.clone(), obj: self.obj })
    }
    fn runner(&self) -> Arc<dyn iyakki_shared::wsexec::Runner> {
        Arc::new(NoRunner)
    }
    fn now_unix(&self) -> i64 {
        call(&self.rpc, "DashHost", self.obj, "now_unix", json!([]))
    }
}

/// `kubectl` through the core's own login, read words only (checked on both ends). It does not own the handle: the host does.
struct RemoteKubectl {
    rpc: Rpc,
    obj: u64,
}
impl iyakki_shared::kubectl::Kubectl for RemoteKubectl {
    fn kubectl(&self, args: &[&str]) -> Result<String, String> {
        self.go("kubectl", args)
    }
    fn kubectl_big(&self, args: &[&str]) -> Result<String, String> {
        self.go("kubectl_big", args)
    }
}
impl RemoteKubectl {
    fn go(&self, method: &str, args: &[&str]) -> Result<String, String> {
        let owned: Vec<String> = args.iter().map(|a| a.to_string()).collect();
        kubectl_allowed(&owned)?;
        self.rpc.call::<Result<String, String>>("DashHost", self.obj, method, json!([owned]))?
    }
}

/// A runner that runs nothing: the UI is not given a way to start a program through the core.
struct NoRunner;
impl iyakki_shared::wsexec::Runner for NoRunner {
    fn run(&self, _cwd: &std::path::Path, _argv: &[String], _env: &[(String, String)], _timeout: std::time::Duration, _cap: usize, _sink: Option<iyakki_shared::wsexec::Sink>) -> iyakki_shared::wsexec::Out {
        iyakki_shared::wsexec::Out { stderr: NOT_SERVED.to_string(), missing: true, ..Default::default() }
    }
    fn has(&self, _program: &str) -> bool {
        false
    }
}
