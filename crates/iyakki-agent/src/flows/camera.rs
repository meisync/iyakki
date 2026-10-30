//! The camera node's side of the real host: one picture per `cam_photo` node, saved in a private folder of the run
//! (`<data>/camera/<run>/`, readable by this account only, never in Pictures) and deleted when the run ends unless the node asked to
//! keep it. The camera-in-use sign shows while the picture is taken. Nothing is sent anywhere here: the picture reaches only the
//! model of a later agent step, and the host warns in the node's log when that model is not on this computer.
//!
//! The flow's own "camera allowed" setting is checked by the node and again here; the account must have the CAMERA app and the
//! shell's camera service must be running. See `docs/FLOWS.md` ("Camera node").

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use iyakki_flow::host::{Cancel, CamPhotoRequest, CamPhotoResult};
use iyakki_flow::retry::NodeError;
use iyakki_sec::policy::App;

use super::host::AgentHost;

/// The pictures of the runs going on: run id -> (file, keep).
#[derive(Default)]
pub struct CamRuns(Mutex<Vec<(String, PathBuf, bool)>>);

fn safe_name(name: &str) -> String {
    let s: String = name.chars().map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' { c } else { '_' }).take(60).collect();
    if s.is_empty() { "unnamed".into() } else { s }
}

/// Is this provider somewhere other than this computer or the home network? A cloud service gets the warning.
pub fn provider_is_cloud(kind: crate::providers::Kind, base_url: Option<&str>) -> bool {
    use crate::providers::Kind;
    if matches!(kind, Kind::OpenAi | Kind::OpenRouter | Kind::Anthropic | Kind::Gemini) {
        return true;
    }
    let url = base_url.map(str::trim).filter(|u| !u.is_empty()).or_else(|| kind.default_base());
    let Some(url) = url else { return true };
    let auth = url.split("://").nth(1).unwrap_or(url).split(['/', '?', '#']).next().unwrap_or("");
    let auth = auth.rsplit_once('@').map_or(auth, |x| x.1);
    let host = auth.rsplit_once(':').filter(|(_, p)| p.chars().all(|c| c.is_ascii_digit())).map_or(auth, |x| x.0).trim_matches(['[', ']']).to_ascii_lowercase();
    let private_v4 = host.parse::<std::net::Ipv4Addr>().is_ok_and(|a| a.is_loopback() || a.is_private() || a.is_link_local());
    let local_v6 = host.parse::<std::net::Ipv6Addr>().is_ok_and(|a| a.is_loopback());
    !(host == "localhost" || host.ends_with(".local") || private_v4 || local_v6)
}

impl AgentHost {
    pub(super) fn cam_folder(&self, run_id: &str) -> PathBuf {
        self.data.join("camera").join(safe_name(run_id))
    }

    /// Delete pictures that were not asked to be kept and are older than `max_age`: the picture of a run that was left waiting
    /// (blocked on a failed step) or of a run that a restart cut short must not stay forever. Kept pictures live in `kept/`.
    pub fn cam_sweep(&self, max_age: std::time::Duration) -> usize {
        let mut n = 0;
        let now = std::time::SystemTime::now();
        for run in std::fs::read_dir(self.data.join("camera")).into_iter().flatten().flatten() {
            for f in std::fs::read_dir(run.path()).into_iter().flatten().flatten() {
                let old = f.metadata().and_then(|m| m.modified()).ok().and_then(|t| now.duration_since(t).ok()).is_some_and(|a| a >= max_age);
                if f.path().is_file() && old && std::fs::remove_file(f.path()).is_ok() {
                    n += 1;
                }
            }
            let _ = std::fs::remove_dir(run.path());
        }
        let mut g = self.cam_runs.0.lock().unwrap_or_else(|e| e.into_inner());
        g.retain(|(_, p, keep)| *keep || p.exists());
        n
    }

    pub fn cam_photo_impl(&self, req: &CamPhotoRequest, cancel: &Cancel) -> Result<CamPhotoResult, NodeError> {
        let refuse = |why: &str, msg: String, consent: bool| {
            self.audit_to("flow.camera.refused", &format!("{}/{} {why}", req.run_id, req.node_id));
            Err(if consent { NodeError::consent(msg) } else { NodeError::config(msg) })
        };
        let deny = |why: &str, msg: String| refuse(why, msg, false);
        let consent = |why: &str, msg: String| refuse(why, msg, true);
        if !req.allowed {
            return consent("not allowed", "This flow is not allowed to use the camera. Switch on \"Camera allowed\" in the flow's settings (only you can), then run it again.".into());
        }
        if !req.attended && !req.unattended {
            return consent("unattended", "Nobody started this run, and the flow may use the camera only when you start it. Switch on \"Camera also when nobody is there\" in the flow's settings.".into());
        }
        if !self.policy.apps.contains(&App::Camera) {
            return consent("account", "Your account may not use the camera.".into());
        }
        let Some(backend) = self.svc.camera.backend() else {
            return deny("no service", "There is no camera service here (the IYAKKI shell is not running), so no picture can be taken.".into());
        };
        if cancel.is_cancelled() {
            return Err(NodeError::cancelled());
        }
        // the model that will look at the picture: the person's Vision model, the chat model if it can see, else one picked from the providers
        // (never a cloud one on its own). None: the person is asked to choose, before any picture is taken and without the fixing agent.
        let vision = if req.needs_vision {
            match self.svc.vision_model(None) {
                Ok(p) => Some(p),
                Err(e) => {
                    self.audit_to("flow.camera.refused", &format!("{}/{} no vision model", req.run_id, req.node_id));
                    return Err(NodeError::choice(format!("{} No picture was taken.", e.message())));
                }
            }
        } else {
            None
        };
        let cams = backend.cameras();
        if cams.is_empty() {
            return deny("no camera", "No camera is connected. Plug in a USB camera and run the flow again.".into());
        }
        if !req.device.is_empty() && !cams.iter().any(|c| c.to_lowercase().contains(&req.device.to_lowercase())) {
            return deny("unknown camera", format!("There is no connected camera called \"{}\". Connected: {}.", req.device.chars().take(60).collect::<String>(), cams.join("; ")));
        }
        if let Some(what) = self.svc.camera.in_use() {
            let mut e = NodeError::new(format!("The camera is in use right now ({what}). Try again when it is free."));
            e.transient = Some(true);
            return Err(e);
        }
        self.cam_sweep(std::time::Duration::from_secs(3600));
        let dir = if req.keep { self.cam_folder(&req.run_id).join("kept") } else { self.cam_folder(&req.run_id) };
        make_private(&dir).map_err(|e| NodeError::config(format!("cannot make the run's private picture folder: {e}")))?;
        let busy = self.svc.camera.begin(&self.user, &format!("the flow \"{}\" taking a photo", req.flow_id));
        let shot = backend.photo(&dir);
        drop(busy);
        let path = shot.map_err(|e| NodeError::new(format!("The camera gave no picture: {e}")))?;
        self.cam_runs.0.lock().unwrap_or_else(|e| e.into_inner()).push((req.run_id.clone(), path.clone(), req.keep));
        let now = crate::clock::now_unix();
        self.audit_to("flow.camera.photo", &format!("{}/{} {}", req.run_id, req.node_id, if req.keep { "kept" } else { "deleted at the end of the run" }));
        let mut log = vec![];
        if let Some(p) = &vision {
            log.push(p.line());
            if p.place == crate::visionmodel::Place::Cloud {
                log.push(format!("Warning: the vision model ({}, provider {}) is a cloud service ({}), so this picture will be sent to it over the internet. A model on your own computer or network keeps it there.", p.model, p.provider, p.host));
            } else {
                log.push(format!("The picture goes only to {} {}.", p.model, p.place.describe(&p.host)));
            }
        }
        Ok(CamPhotoResult { path: path.display().to_string(), taken_at: now, device: cams.first().cloned().unwrap_or_default(), log })
    }

    /// The run is over: delete its pictures (kept ones stay) and the folder when it is empty.
    pub fn cam_end_impl(&self, run_id: &str) {
        let mine: Vec<(String, PathBuf, bool)> = {
            let mut g = self.cam_runs.0.lock().unwrap_or_else(|e| e.into_inner());
            let (mine, rest) = std::mem::take(&mut *g).into_iter().partition(|(r, _, _)| r == run_id);
            *g = rest;
            mine
        };
        if mine.is_empty() {
            return;
        }
        let mut deleted = 0;
        for (_, p, keep) in &mine {
            if !*keep && std::fs::remove_file(p).is_ok() {
                deleted += 1;
            }
        }
        let _ = std::fs::remove_dir(self.cam_folder(run_id).join("kept"));
        let _ = std::fs::remove_dir(self.cam_folder(run_id));
        self.audit_to("flow.camera.end", &format!("{run_id} {deleted} picture(s) deleted, {} kept", mine.len() - deleted));
    }
}

fn make_private(dir: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}
