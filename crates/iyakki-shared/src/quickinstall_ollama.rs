//! Moved out of `iyakki_agent::quickinstall_ollama` (F-SPLIT-03, bucket b0); the agent re-exports it.
use std::path::{Path, PathBuf};

use crate::quickinstall::state_dir;
pub use crate::quickinstall::LEMONADE_NAMESPACE as NS;


pub const STORAGE_GB: u64 = 30;

/// The marker text of an install in the cluster starts with this.
pub const CLUSTER_WORD: &str = "in the cluster";

/// What the Quick install page says about an install on the host.
pub const HOST_WORD: &str = "Ollama runs on this machine's host";

/// The titles of the two review cards (the shell tells them apart by the title, like the browser add-on's).
pub const MOVE_TITLE: &str = "Move Ollama into the cluster?";

pub const FRESH_TITLE: &str = "Start Ollama fresh in the cluster?";

/// The most the host models folder may be to copy in one step (no resume): bigger ones are offered "start fresh".
pub const MAX_COPY_BYTES: u64 = 20 << 30;

/// Room to keep free on the data partition after the copy (the copy exists twice for a while).
pub const KEEP_FREE_BYTES: u64 = 2 << 30;

/// Does this presence detail or marker detail say the cluster?
pub fn is_cluster_detail(d: &str) -> bool {
    d.contains(CLUSTER_WORD)
}

/// What the person chose for an Ollama on the host; the Store leaves it for the job as a request file.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    /// Copy the host models into the claim, then (after the cluster answers) stop the host server.
    Move,
    /// A new, empty install in the cluster; the host models stay where they are; the host server stops after the cluster answers.
    Fresh,
}

pub fn mode_file(data: &Path) -> PathBuf {
    state_dir(data).join("ollama-mode")
}

pub fn write_mode(data: &Path, m: Mode) -> Result<(), String> {
    std::fs::create_dir_all(state_dir(data)).map_err(|e| e.to_string())?;
    std::fs::write(mode_file(data), if m == Mode::Move { "move" } else { "fresh" }).map_err(|e| e.to_string())
}

/// The waiting choice, taken (removed) at once.
pub fn take_mode(data: &Path) -> Option<Mode> {
    let p = mode_file(data);
    let t = std::fs::read_to_string(&p).ok()?;
    let _ = std::fs::remove_file(&p);
    match t.trim() {
        "move" => Some(Mode::Move),
        "fresh" => Some(Mode::Fresh),
        _ => None,
    }
}

/// What the machine says about the host install (a fake in tests).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct HostInfo {
    pub models_dir: PathBuf,
    pub models_bytes: u64,
    /// Free bytes on the data partition; `None` when unknown.
    pub free_bytes: Option<u64>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Decision {
    /// The copy is safe in one step.
    Copy,
    /// Too large to copy in one step (no resume): offer "start fresh".
    TooLarge,
    /// The data partition has no room for a second copy.
    NoRoom,
}

/// May the host models be copied into the claim now? The folder must fit the claim and the one-step limit, and the data partition must
/// keep [`KEEP_FREE_BYTES`] free after the copy.
pub fn decide(h: &HostInfo) -> Decision {
    let claim = STORAGE_GB << 30;
    if h.models_bytes > MAX_COPY_BYTES.min(claim / 10 * 8) {
        return Decision::TooLarge;
    }
    match h.free_bytes {
        Some(f) if f < h.models_bytes.saturating_add(KEEP_FREE_BYTES) => Decision::NoRoom,
        _ => Decision::Copy,
    }
}

pub const ABOUT: &str = "Run language models on this machine: Ollama as one pod in the cluster (namespace iyakki-ai, ClusterIP, published on this machine only at 127.0.0.1:11436, a 30 GB volume for the models, up to 16 GB of memory), added to Models as ollama-local. Models are pulled later from Settings \u{2192} Models. An Ollama installed earlier on this machine's host keeps working; this page offers to move it into the cluster";
pub const LICENCE: &str = "MIT (the Ollama image, pulled by the cluster from Docker Hub at your click, not shipped); each model you pull has its own licence, outside IYAKKI";

pub fn gb(b: u64) -> String {
    if b >= 1 << 30 { format!("{:.1} GB", b as f64 / (1u64 << 30) as f64) } else { format!("{} MB", b >> 20) }
}

/// The review card's lines for a host install.
pub fn review_lines(mode: Mode, h: &HostInfo, d: Decision) -> Vec<String> {
    let r = crate::quickinstall::resources::OLLAMA;
    let mut v = vec![
        format!("{HOST_WORD}. This puts a new Ollama into the cluster: one pod in the namespace {NS}, published on this machine only at http://127.0.0.1:{LOCAL_PORT}, with a {STORAGE_GB} GB volume for the models."),
        format!("It may use up to {} of memory and {}; it asks for {} and {}.", crate::quickinstall::resources::mem_words(r.mem_lim), crate::quickinstall::resources::cpu_words(r.cpu_lim), crate::quickinstall::resources::mem_words(r.mem_req), crate::quickinstall::resources::cpu_words(r.cpu_req)),
        format!("The cluster downloads the Ollama image from Docker Hub when you confirm (about 1.5 GB). Ollama is MIT licensed; IYAKKI does not ship it. {}", LICENCE),
    ];
    match mode {
        Mode::Move => {
            v.push(format!("Your models: the folder {} holds {}. After you confirm, they are copied into the volume of the new pod (a copy; nothing is deleted from the host).", h.models_dir.display(), gb(h.models_bytes)));
            match d {
                Decision::Copy => v.push(format!("There is room: {} free on the data partition.", h.free_bytes.map_or("enough".to_string(), gb))),
                Decision::TooLarge => v.push(format!("This is too large to copy safely in one step (the limit is {}). Close this card and choose \"Start fresh in the cluster, keep host models where they are\" instead.", gb(MAX_COPY_BYTES))),
                Decision::NoRoom => v.push(format!("There is not enough room for a second copy ({} free, {} needed plus {} to spare). Free some space, or choose \"Start fresh in the cluster, keep host models where they are\".", h.free_bytes.map_or("unknown".to_string(), gb), gb(h.models_bytes), gb(KEEP_FREE_BYTES))),
            }
        }
        Mode::Fresh => {
            v.push(format!("Your models stay where they are: the folder {} ({}) is not touched and not copied. The new Ollama starts empty; pull models later from Settings \u{2192} Models.", h.models_dir.display(), gb(h.models_bytes)));
        }
    }
    v.push("The Ollama on the host keeps running until the new one answers; only then is it stopped. The program stays installed and nothing is deleted. Models then points at the cluster one (ollama-local).".into());
    v
}

/// The port on this machine's loopback.
pub const LOCAL_PORT: u16 = 11436;
