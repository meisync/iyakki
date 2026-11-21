//! What the CHECKUP tile asks of the rest of the system (the app standard, docs/development/APP-DEVELOPMENT-PROCEDURE.md section 12).
//!
//! The tile (`checkup.rs`) names no `Services`, no session and no audit log: it asks a [`CheckupPort`] for the machine's inventory, the test
//! tone, a camera picture, the voice settings, the person's data folder, the dev cluster and the audit line. The real port ([`CxPort`]) is the
//! only place that reads the session and calls the core. [`Parts`] stays the way tests replace each reader.

use std::any::Any;
use std::sync::Arc;

use iyakki_shared::checkup::{self, Inventory, ScreenOut};
use iyakki_shared::voicedesk::Recorders;

use super::browser_media;
use super::AppCx;
use crate::camera::probe::{self, Shot};

/// How long a camera picture may take.
const CAMERA_WAIT: std::time::Duration = std::time::Duration::from_secs(3);

pub trait CheckupPort {
    /// What this machine has (screens, sound, network, GPUs ...), read again every time.
    fn inventory(&self) -> Inventory;
    /// Play the test tone through the audio entry point; the returned value keeps the sound alive (dropping it stops it).
    fn play_tone(&self) -> Result<Box<dyn Any + Send>, String>;
    /// A job that takes one camera picture (it runs on a thread of its own).
    fn camera_job(&self) -> Box<dyn FnOnce() -> Shot + Send>;
    fn voice_settings(&self) -> iyakki_shared::voice::VoiceSettings;
    /// Where the signed result file goes.
    fn data_dir(&self) -> std::path::PathBuf;
    /// The services that know the dev cluster's state.
    fn k3s(&self) -> Arc<dyn iyakki_agent_api::DevCluster>;
    /// A line in the audit log for the signed-in person.
    fn audit(&self, action: &str, subject: &str);
}

/// What the tile reads and plays; tests replace every part. The closures get the port (the real ones ask it, the test ones ignore it).
#[derive(Clone)]
pub struct Parts {
    pub inventory: Arc<dyn Fn(&dyn CheckupPort) -> Inventory + Send + Sync>,
    pub recorders: Arc<dyn Recorders>,
    /// Play the test tone through the audio entry point; the returned value keeps the sound alive (dropping it stops it).
    pub play: Arc<dyn Fn(&dyn CheckupPort) -> Result<Box<dyn Any + Send>, String> + Send + Sync>,
    /// Take one camera picture (runs on a thread of its own).
    pub camera: Arc<dyn Fn(&dyn CheckupPort) -> Box<dyn FnOnce() -> Shot + Send> + Send + Sync>,
    /// Tell the shell's sign what is capturing ( bits).
    pub sign: Arc<dyn Fn(u8) + Send + Sync>,
    /// The person clicked the sign: stop everything that captures.
    pub stop_asked: Arc<dyn Fn() -> bool + Send + Sync>,
}

impl Parts {
    pub fn real() -> Parts {
        Parts {
            inventory: Arc::new(|p| p.inventory()),
            recorders: crate::recorders(),
            play: Arc::new(|p| p.play_tone()),
            camera: Arc::new(|p| p.camera_job()),
            sign: Arc::new(browser_media::publish_capture),
            stop_asked: Arc::new(browser_media::take_stop),
        }
    }
}

/// The real port, over the shell's context.
pub struct CxPort<'a>(pub &'a AppCx);

impl CheckupPort for CxPort<'_> {
    fn inventory(&self) -> Inventory {
        real_inventory(self.0)
    }
    fn play_tone(&self) -> Result<Box<dyn Any + Send>, String> {
        use iyakki_core_api::SoundKind;
        self.0.core.mixer().play_wav(SoundKind::Notification, "Check-up tone", &crate::agent().b0().checkup().tone_wav())
    }
    fn camera_job(&self) -> Box<dyn FnOnce() -> Shot + Send> {
        let home = std::path::PathBuf::from(format!("/home/{}", self.0.session.user));
        Box::new(move || probe::system_grab(&home, CAMERA_WAIT))
    }
    fn voice_settings(&self) -> iyakki_shared::voice::VoiceSettings {
        self.0.voice_settings()
    }
    fn data_dir(&self) -> std::path::PathBuf {
        self.0.data_dir().to_path_buf()
    }
    fn k3s(&self) -> Arc<dyn iyakki_agent_api::DevCluster> {
        self.0.k3sdev()
    }
    fn audit(&self, action: &str, subject: &str) {
        self.0.audit.record(&self.0.session.user, action, subject);
    }
}

pub(super) fn real_inventory(cx: &AppCx) -> Inventory {
    let screens = crate::display::outputs::hub().status().outputs.iter().map(|o| ScreenOut { name: o.name.clone(), connected: o.connected, current: o.current }).collect();
    let mut inv = cx.agent.b0().checkup().inventory(screens);
    inv.sound_chosen = cx.sound_settings().output;
    inv.k3s_gpu = k3s_sees(&*cx.k3sdev());
    inv.sound_chosen_present = inv.sound_chosen.is_empty() || iyakki_shared::audioout::choose(&crate::agent().helpers_b4().sound_outputs(), &inv.sound_chosen).chosen;
    inv
}

/// What the managed dev cluster says about GPUs on its nodes (read-only `kubectl get nodes -o json`). A cluster that is not running is not asked.
fn k3s_sees(k: &dyn iyakki_agent_api::DevCluster) -> checkup::K3sSees {
    let state = k.health().state;
    if state != "running" {
        return checkup::K3sSees::NoCluster(state);
    }
    let o = k.kubectl_cap(&["get", "nodes", "-o", "json"], 512 * 1024);
    if !o.ok() {
        return checkup::K3sSees::Unreadable(o.combined().lines().last().unwrap_or("no answer").trim().chars().take(120).collect());
    }
    let n: u32 = [iyakki_shared::gpu_workloads::GPU_RESOURCE, iyakki_shared::gpu_workloads::NVIDIA_RESOURCE, iyakki_shared::gpu_rocm::RESOURCE].iter().map(|r| crate::agent().gpu_workloads_node_resource_count(&o.stdout, r)).sum();
    checkup::K3sSees::Count(n)
}

