//! The producers behind `bulk.get` on the UI socket (F-SPLIT-07): the blocks that do not travel through the line transport because they are big and
//! binary. Two are served, each asked for by name and handed over as a sealed memory file beside the answer (`iyakki_ui_client::ipc`):
//!
//! * `display.<name>`: the newest frame of an SDK app the core runs, as display list text (`DisplayList::to_text`, the format a browser or client draws).
//!   The app runs on a thread of its own ([`HostThread`]): an app and its host need not be `Send`, and the frame is drawn there, by
//!   `iyakki_ui_sdk::host::Host::record`, the one recorder every target uses. Scope `display:view`.
//! * `camera.<view>.<camera>`: the newest JPEG picture of one camera of a view the person already said yes to (`Cameras::frame`). A view exists only
//!   after the consent card was answered, so this block never reaches a camera nobody agreed to share; the user is the caller's, never the name's.
//!   Scope `cameras:view`.
//!
//! Every other name is `not_found`; a caller without the scope is `denied`; neither leaks whether the thing exists.

use std::collections::BTreeMap;
use std::sync::{mpsc, Arc, Mutex, PoisonError};

use iyakki_core_api::cameras::{Cameras, FrameFormat};
use iyakki_core_api::contract::{CallContext, CallError, CallResult, ErrorCode};
use iyakki_ui_client::server::BulkSource;
use iyakki_ui_sdk::app::UiApp;
use iyakki_ui_sdk::host::Host;

/// The scope that lets a caller read the display lists the core draws.
pub use iyakki_ui_client::facets::DISPLAY_VIEW;

enum Cmd {
    Frame(mpsc::Sender<String>),
    Resize(f32, f32, mpsc::Sender<()>),
}

/// An SDK app and its host on a thread of their own, asked for frames over a channel. The thread ends with the handle.
pub struct HostThread {
    tx: Mutex<mpsc::Sender<Cmd>>,
}

impl HostThread {
    /// Build the host on the new thread (`make` runs there) and keep answering frame requests until the handle is dropped.
    pub fn spawn<A: UiApp + 'static>(make: impl FnOnce() -> Host<A> + Send + 'static) -> HostThread {
        let (tx, rx) = mpsc::channel::<Cmd>();
        let _ = std::thread::Builder::new().name("iyakki-frames".into()).spawn(move || {
            let mut host = make();
            while let Ok(cmd) = rx.recv() {
                match cmd {
                    Cmd::Frame(reply) => {
                        let _ = reply.send(host.record().to_text());
                    }
                    Cmd::Resize(w, h, reply) => {
                        host.resize(w, h);
                        let _ = reply.send(());
                    }
                }
            }
        });
        HostThread { tx: Mutex::new(tx) }
    }

    /// The newest frame as display list text; an error when the thread has ended (the app panicked).
    pub fn frame(&self) -> Result<String, String> {
        let (reply, got) = mpsc::channel();
        self.tx.lock().unwrap_or_else(PoisonError::into_inner).send(Cmd::Frame(reply)).map_err(|_| "the app has stopped".to_string())?;
        got.recv().map_err(|_| "the app has stopped".to_string())
    }

    /// The pane changed size (logical units): the next frame is drawn at it.
    pub fn resize(&self, w: f32, h: f32) -> Result<(), String> {
        let (reply, got) = mpsc::channel();
        self.tx.lock().unwrap_or_else(PoisonError::into_inner).send(Cmd::Resize(w, h, reply)).map_err(|_| "the app has stopped".to_string())?;
        got.recv().map_err(|_| "the app has stopped".to_string())
    }
}

/// The names of the apps whose frames are served, and the camera views of the core.
#[derive(Default)]
pub struct FrameBulk {
    apps: Mutex<BTreeMap<String, Arc<HostThread>>>,
    cameras: Option<Arc<dyn Cameras>>,
}

impl FrameBulk {
    pub fn new() -> FrameBulk {
        FrameBulk::default()
    }

    /// Serve the camera views of the core as `camera.<view>.<camera>`.
    pub fn with_cameras(mut self, c: Arc<dyn Cameras>) -> FrameBulk {
        self.cameras = Some(c);
        self
    }

    /// Serve an app's frames as `display.<name>` (a name is letters, digits, `-` and `_`; the same name replaces the earlier app).
    pub fn add_app(&self, name: &str, app: HostThread) -> Result<(), String> {
        if name.is_empty() || name.len() > 32 || !name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_') {
            return Err("the name of an app is letters, digits, - and _ (1 to 32)".into());
        }
        self.apps.lock().unwrap_or_else(PoisonError::into_inner).insert(name.to_string(), Arc::new(app));
        Ok(())
    }

    /// The names served now, sorted.
    pub fn names(&self) -> Vec<String> {
        self.apps.lock().unwrap_or_else(PoisonError::into_inner).keys().map(|n| format!("display.{n}")).collect()
    }
}

fn denied(scope: &str) -> CallError {
    CallError::new(ErrorCode::Denied, "transport.no_scope", &format!("This connection was not given the scope {scope}."))
}

fn not_found() -> CallError {
    CallError::new(ErrorCode::NotFound, "uisocket.no_such_block", "This core has no block of that name.")
}

impl BulkSource for FrameBulk {
    fn get(&self, ctx: &CallContext, name: &str) -> CallResult<Vec<u8>> {
        if let Some(app) = name.strip_prefix("display.") {
            if !ctx.allows(DISPLAY_VIEW) {
                return Err(denied(DISPLAY_VIEW));
            }
            let host = self.apps.lock().unwrap_or_else(PoisonError::into_inner).get(app).cloned().ok_or_else(not_found)?;
            return host.frame().map(String::into_bytes).map_err(|e| CallError::new(ErrorCode::Internal, "uisocket.app_stopped", &e));
        }
        if let Some(rest) = name.strip_prefix("camera.") {
            if !ctx.allows(iyakki_ui_client::facets::CAMERAS_VIEW) {
                return Err(denied(iyakki_ui_client::facets::CAMERAS_VIEW));
            }
            let cameras = self.cameras.as_ref().ok_or_else(not_found)?;
            let (view, camera) = rest.split_once('.').ok_or_else(not_found)?;
            return match cameras.frame(&ctx.identity.user, view, camera, 0, FrameFormat::Jpeg) {
                Ok(Some(f)) => Ok(f.data.as_ref().clone()),
                // nothing new, the view waits for the person's yes, is hidden or closed: the same answer for all, so a block says nothing about why
                Ok(None) | Err(_) => Err(not_found()),
            };
        }
        Err(not_found())
    }
}
