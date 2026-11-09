//! Per-camera motion settings, kept in `<data>/cameras/motion/settings.json` (mode 0600, written atomically). No secrets are in it:
//! zone names and shapes, the mask shapes, switches and alert limits. The vault key for the events is NOT here (it is the camera's
//! recording key in the owner's vault).
//!
//! This file is also how the recorder and the snapshot code learn which masks to apply before storing a picture
//! ([`storage_masks`]): it is read at most every two seconds.

use std::collections::HashMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use super::alerts::{AlertCfg, Quiet, DEFAULT_GROUP_S, DEFAULT_PER_HOUR, MAX_PER_HOUR};
use super::geom::{self, Mask, Pt, Zone, MAX_SHAPES};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum WatchMode {
    /// Nothing looks at the camera for motion.
    #[default]
    Off,
    /// Motion is looked for only while the camera's picture is on a screen or the camera is being recorded (both already show a sign).
    Watched,
    /// The camera is kept running and watched all the time; a red sign says so on every screen.
    Always,
}

impl WatchMode {
    pub fn word(self) -> &'static str {
        match self {
            WatchMode::Off => "off",
            WatchMode::Watched => "watched",
            WatchMode::Always => "always",
        }
    }
    pub fn parse(s: &str) -> Option<WatchMode> {
        match s.trim().to_lowercase().as_str() {
            "off" => Some(WatchMode::Off),
            "watched" | "when_watched" | "while_watched" => Some(WatchMode::Watched),
            "always" | "on" => Some(WatchMode::Always),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ShapeRec {
    pub name: String,
    /// Zones only; masks ignore it.
    #[serde(default)]
    pub include: bool,
    #[serde(default = "five")]
    pub sensitivity: u8,
    pub points: Vec<[f32; 2]>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AlertRec {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default = "per_hour")]
    pub max_per_hour: u32,
    /// `22:00-06:30` or empty.
    #[serde(default)]
    pub quiet: String,
    #[serde(default)]
    pub thumbnail: bool,
    #[serde(default = "group_s")]
    pub group_s: u64,
}

impl Default for AlertRec {
    fn default() -> AlertRec {
        AlertRec { enabled: false, max_per_hour: DEFAULT_PER_HOUR, quiet: String::new(), thumbnail: false, group_s: DEFAULT_GROUP_S }
    }
}

impl AlertRec {
    pub fn cfg(&self) -> AlertCfg {
        AlertCfg { enabled: self.enabled, max_per_hour: self.max_per_hour.clamp(1, MAX_PER_HOUR), quiet: Quiet::parse(&self.quiet).ok(), thumbnail: self.thumbnail, group_s: self.group_s }
    }
}

fn five() -> u8 {
    5
}
fn per_hour() -> u32 {
    DEFAULT_PER_HOUR
}
fn group_s() -> u64 {
    DEFAULT_GROUP_S
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CamMotion {
    pub camera: String,
    #[serde(default)]
    pub mode: WatchMode,
    #[serde(default = "five")]
    pub sensitivity: u8,
    #[serde(default)]
    pub zones: Vec<ShapeRec>,
    #[serde(default)]
    pub masks: Vec<ShapeRec>,
    /// Black out the masks in recordings and snapshots too (not only before detection).
    #[serde(default)]
    pub mask_storage: bool,
    #[serde(default)]
    pub alerts: AlertRec,
    /// Ask the person detector about events (only when one is available).
    #[serde(default)]
    pub person: bool,
    /// Changes with every edit, so a running detector knows to start again.
    #[serde(default)]
    pub version: u64,
}

impl CamMotion {
    pub fn off(camera: &str) -> CamMotion {
        CamMotion { camera: camera.to_string(), mode: WatchMode::Off, sensitivity: 5, zones: vec![], masks: vec![], mask_storage: false, alerts: AlertRec::default(), person: false, version: 0 }
    }

    pub fn zones_rt(&self) -> Vec<Zone> {
        self.zones.iter().map(|z| Zone { name: z.name.clone(), include: z.include, sensitivity: z.sensitivity.clamp(1, 10), points: z.points.iter().map(|p| Pt::new(p[0], p[1])).collect() }).collect()
    }

    pub fn masks_rt(&self) -> Vec<Mask> {
        self.masks.iter().map(|m| Mask { name: m.name.clone(), points: m.points.iter().map(|p| Pt::new(p[0], p[1])).collect() }).collect()
    }

    /// Check every shape again (a file edited by hand).
    pub fn valid(&self) -> Result<(), String> {
        for s in self.zones.iter().chain(self.masks.iter()) {
            geom::validate(&s.points.iter().map(|p| Pt::new(p[0], p[1])).collect::<Vec<_>>()).map_err(|e| format!("“{}”: {e}", s.name))?;
        }
        if self.zones.len() > MAX_SHAPES || self.masks.len() > MAX_SHAPES {
            return Err(format!("at most {MAX_SHAPES} zones and {MAX_SHAPES} masks per camera"));
        }
        Ok(())
    }
}

#[derive(Default, Serialize, Deserialize)]
pub struct File {
    #[serde(default)]
    pub cameras: Vec<CamMotion>,
}

pub fn file_of(data: &Path) -> PathBuf {
    data.join("cameras").join("motion").join("settings.json")
}

/// The settings; a missing file is empty settings, an unreadable one is an error.
pub fn load_strict(data: &Path) -> Result<File, String> {
    match std::fs::read(file_of(data)) {
        Ok(b) => serde_json::from_slice::<File>(&b).map_err(|e| format!("the motion settings file is damaged: {e}")),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(File::default()),
        Err(e) => Err(format!("the motion settings cannot be read: {e}")),
    }
}

pub fn load(data: &Path) -> File {
    load_strict(data).map(|mut f| {
        // a shape that no longer passes the checks is dropped rather than trusted
        f.cameras.retain(|c| c.valid().is_ok());
        f
    }).unwrap_or_default()
}

pub fn save(data: &Path, f: &File) -> Result<(), String> {
    use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
    let file = file_of(data);
    if let Some(d) = file.parent() {
        std::fs::DirBuilder::new().recursive(true).mode(0o700).create(d).map_err(|e| format!("could not save the motion settings: {e}"))?;
    }
    let tmp = file.with_extension("json.tmp");
    {
        let mut t = std::fs::OpenOptions::new().write(true).create(true).truncate(true).mode(0o600).open(&tmp).map_err(|e| format!("could not save the motion settings: {e}"))?;
        t.write_all(&serde_json::to_vec_pretty(f).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
        let _ = t.sync_all();
    }
    std::fs::rename(&tmp, &file).map_err(|e| format!("could not save the motion settings: {e}"))
}

/// The cached settings, or `None` when the file exists and cannot be read (then masks cannot be known: storage fails closed).
type Cache = Mutex<HashMap<PathBuf, (Instant, Option<File>)>>;

fn cache() -> &'static Cache {
    static C: OnceLock<Cache> = OnceLock::new();
    C.get_or_init(Default::default)
}

/// The masks to apply before a picture of `camera` is stored, when the person asked for that ("also in recordings and snapshots").
/// `None`: nothing to apply. If the settings file exists but cannot be read, the whole picture is covered (fail closed). Read from the settings file, cached for two seconds ([`forget_cache`] after a change).
pub fn storage_masks(data: &Path, camera: &str) -> Option<Vec<Vec<Pt>>> {
    let mut g = cache().lock().unwrap_or_else(|e| e.into_inner());
    let stale = g.get(data).is_none_or(|(t, _)| t.elapsed() > Duration::from_secs(2));
    if stale {
        g.insert(data.to_path_buf(), (Instant::now(), load_strict(data).ok()));
    }
    let (_, f) = g.get(data)?;
    let Some(f) = f else {
        // unreadable settings: nothing may be stored unmasked, so the whole picture is covered
        return Some(vec![vec![Pt::new(0.0, 0.0), Pt::new(1.0, 0.0), Pt::new(1.0, 1.0), Pt::new(0.0, 1.0)]]);
    };
    let cam = f.cameras.iter().find(|c| c.camera == camera)?;
    if !cam.mask_storage || cam.masks.is_empty() {
        return None;
    }
    Some(cam.masks_rt().into_iter().map(|m| m.points).collect())
}

/// Forget the cached copy of the settings file (after a change made by this process).
pub fn forget_cache(data: &Path) {
    cache().lock().unwrap_or_else(|e| e.into_inner()).remove(data);
}
