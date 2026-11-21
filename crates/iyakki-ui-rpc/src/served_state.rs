//! `iyakki dev state` for the UI program over the core's socket (F-SPLIT-03, D-SPLIT-03-APPS): the screen the program draws is captured here,
//! but the paired computer asks the CORE for it. The core keeps the slots (`iyakki_shared::devmode_state`: "wanted", "alive" and the last frame);
//! the program tells the core it is drawing, asks whether a frame is wanted and hands the raw frame over. The safety rules (secret fields,
//! private apps, credentials, size limits) run in the core on the raw frame, as they do for a shell inside the core, and a frame is only wanted
//! while developer mode is on there. The program does not publish on every frame: a beat asks at most every [`BEAT`], a frame goes at most every
//! [`PUBLISH`], both on a clock the caller gives (a manual one in the tests).

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use iyakki_shared::devmode_state::{self as state, Raw, RawField, Rect, RemoteState};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::served::Served;
use crate::served_hosts::{bytes_in, send};
use crate::{de, ser, Obj, Registry, RemoteServicesHandle, Rpc};

/// The program asks the core "is a frame wanted?" at most this often (the core counts a screen alive for five seconds).
pub const BEAT: Duration = Duration::from_millis(500);
/// A frame goes to the core at most this often while one is wanted.
pub const PUBLISH: Duration = Duration::from_millis(150);
/// The most bytes of one raw frame the core takes.
const MAX_FRAME: usize = 4 * 1024 * 1024;

type R4 = [f32; 4];

#[derive(Serialize, Deserialize)]
struct WRaw {
    app: String,
    page: String,
    size: (u32, u32),
    texts: Vec<(f32, f32, String)>,
    hits: Vec<(String, R4)>,
    fields: Vec<(R4, bool, bool)>,
}

fn rect(r: R4) -> Rect {
    Rect { x: r[0], y: r[1], w: r[2], h: r[3] }
}
fn arr(r: &Rect) -> R4 {
    [r.x, r.y, r.w, r.h]
}

impl From<&Raw> for WRaw {
    fn from(r: &Raw) -> WRaw {
        WRaw { app: r.app.clone(), page: r.page.clone(), size: r.size, texts: r.texts.clone(), hits: r.hits.iter().map(|(n, x)| (n.clone(), arr(x))).collect(), fields: r.fields.iter().map(|f| (arr(&f.rect), f.focused, f.secret)).collect() }
    }
}
impl From<WRaw> for Raw {
    fn from(w: WRaw) -> Raw {
        Raw { app: w.app, page: w.page, size: w.size, texts: w.texts, hits: w.hits.into_iter().map(|(n, x)| (n, rect(x))).collect(), fields: w.fields.into_iter().map(|(r, focused, secret)| RawField { rect: rect(r), focused, secret }).collect() }
    }
}

// ---------------------------------------------------------------------------------------------------------------- the core's end

/// The core's slots of one data folder as the root `DevState`.
pub fn serve_dev_state(data_dir: PathBuf) -> Obj {
    Obj::Served(Arc::new(DevStateObj { dir: data_dir }))
}

struct DevStateObj {
    dir: PathBuf,
}

impl Served for DevStateObj {
    fn iface(&self) -> &'static str {
        "DevState"
    }
    fn serve(&self, reg: &Registry<'_>, method: &str, a: Value) -> Result<Value, String> {
        match method {
            // one call for both: "a screen is being drawn" and the answer to "is a frame wanted"
            "beat" => {
                state::local_heartbeat(&self.dir);
                ser(&state::local_wanted(&self.dir))
            }
            "publish" => {
                let (inline, upload): (String, u64) = de(a)?;
                let bytes = bytes_in(reg, &inline, upload)?;
                if bytes.len() > MAX_FRAME {
                    return Err(format!("a frame of {} bytes is more than the {MAX_FRAME} the core takes", bytes.len()));
                }
                let raw: WRaw = serde_json::from_slice(&bytes).map_err(|e| format!("the frame is not one: {e}"))?;
                state::local_publish(&self.dir, &raw.into());
                ser(&true)
            }
            other => Err(format!("DevState has no call {other}")),
        }
    }
}

pub(crate) fn serve_state(t: &dyn iyakki_agent_api::ServicesHandle, reg: &Registry<'_>, method: &str) -> Option<Result<Value, String>> {
    Some(match method {
        "dev_state" => ser(&reg.add(serve_dev_state(t.dir().to_path_buf()))),
        _ => return None,
    })
}

// ---------------------------------------------------------------------------------------------------------------- the UI's end

/// What the UI program does with the three shell-side calls of `iyakki dev state`: it asks the core.
pub struct RemoteDevState {
    rpc: Rpc,
    obj: u64,
    now: Arc<dyn Fn() -> Instant + Send + Sync>,
    beat: Mutex<Option<Instant>>,
    sent: Mutex<Option<Instant>>,
    wanted: AtomicBool,
}

impl Drop for RemoteDevState {
    fn drop(&mut self) {
        self.rpc.release(self.obj);
    }
}

/// The UI's end of a served `DevState` (`obj` 0 is a root) on the clock `now`.
pub fn remote_dev_state(rpc: &Rpc, obj: u64, now: Arc<dyn Fn() -> Instant + Send + Sync>) -> Arc<RemoteDevState> {
    Arc::new(RemoteDevState { rpc: rpc.clone(), obj, now, beat: Mutex::new(None), sent: Mutex::new(None), wanted: AtomicBool::new(false) })
}

fn due(last: &Mutex<Option<Instant>>, now: Instant, every: Duration) -> bool {
    let mut g = last.lock().unwrap_or_else(PoisonError::into_inner);
    if g.is_some_and(|t| now.saturating_duration_since(t) < every) {
        return false;
    }
    *g = Some(now);
    true
}

impl RemoteState for RemoteDevState {
    fn heartbeat(&self) {
        if due(&self.beat, (self.now)(), BEAT) {
            // a core that does not answer is a core that wants nothing
            let w = self.rpc.call::<bool>("DevState", self.obj, "beat", json!([])).unwrap_or(false);
            self.wanted.store(w, Ordering::SeqCst);
        }
    }
    fn wanted(&self) -> bool {
        self.wanted.load(Ordering::SeqCst)
    }
    fn publish(&self, raw: &Raw) {
        if !due(&self.sent, (self.now)(), PUBLISH) {
            return;
        }
        let Ok(bytes) = serde_json::to_vec(&WRaw::from(raw)) else { return };
        if let Ok(sent) = send(&self.rpc, &bytes) {
            let _ = self.rpc.call::<bool>("DevState", self.obj, "publish", json!([sent.inline, sent.id]));
        }
    }
}

impl RemoteServicesHandle {
    /// Send the shell-side calls of `iyakki dev state` of this process to the core (once per process; `false` when that was done already).
    pub fn install_dev_state(&self) -> bool {
        state::install_remote(self.dev_state())
    }

    /// The core's `iyakki dev state` slots as the program's shell sees them, on the real clock.
    pub fn dev_state(&self) -> Arc<RemoteDevState> {
        remote_dev_state(&self.rpc, self.rpc.call::<u64>("ServicesHandle", 0, "dev_state", json!([])).unwrap_or(u64::MAX), Arc::new(Instant::now))
    }
}
