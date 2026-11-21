//! Types moved out of `iyakki_agent::macfw` (F-SPLIT-03, scripted by dist/split/move_types.py); the agent re-exports them.

use std::path::PathBuf;

/// The notice shown before anything starts and again at the top of the log.
/// Said plainly (the job's lines, Settings -> Bluetooth, the Mac drivers page) when macOS has no patch file for the BCM4364 UART controller.
pub const NO_BCM4364_FILE: &str = "This Mac's Bluetooth chip has no firmware file in macOS: the controller runs in its ROM state (limited); a USB Bluetooth adapter is the reliable option";

/// The ready-to-paste prompt for Claude Code, Copilot CLI or the IYAKKI assistant (the Mac drivers page copies it; the same text is in
/// docs/MAC-WIFI-BLUETOOTH-SETUP.md, and a test keeps the two equal).
pub const CHAT_PROMPT: &str = include_str!("macfw_prompt.txt");

pub const NOTICE: &str = "These files are Apple's. They stay on this machine, are never uploaded and are not part of IYAKKI.";

/// Where the files come from.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Source {
    Download,
    /// A disk image file.
    Image(PathBuf),
    /// A folder (a stick): the script's output or a copy of macOS's firmware folder.
    Folder(PathBuf),
}

impl Source {
    /// `--source download|image|folder [--path P]`.
    pub fn from_args(args: &[String]) -> Result<Source, String> {
        let flag = |n: &str| args.iter().position(|a| a == n).and_then(|i| args.get(i + 1)).cloned();
        let path = flag("--path");
        match flag("--source").as_deref().unwrap_or("download") {
            "download" => Ok(Source::Download),
            "image" => path.map(|p| Source::Image(p.into())).ok_or_else(|| "--source image needs --path <disk image file>".into()),
            "folder" => path.map(|p| Source::Folder(p.into())).ok_or_else(|| "--source folder needs --path <folder>".into()),
            other => Err(format!("unknown source `{other}` (download, image or folder)")),
        }
    }

    pub fn args(&self) -> Vec<String> {
        match self {
            Source::Download => vec!["--source".into(), "download".into()],
            Source::Image(p) => vec!["--source".into(), "image".into(), "--path".into(), p.display().to_string()],
            Source::Folder(p) => vec!["--source".into(), "folder".into(), "--path".into(), p.display().to_string()],
        }
    }

    pub fn words(&self) -> String {
        match self {
            Source::Download => "Apple's recovery image for this Mac".into(),
            Source::Image(p) => format!("the disk image {}", p.display()),
            Source::Folder(p) => format!("the folder {}", p.display()),
        }
    }
}

/// What the machine shows after the drivers were loaded again.
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct Seen {
    /// Wireless interfaces (`wlan0`).
    pub wifi: Vec<String>,
    /// A Bluetooth controller (`hci0`) exists.
    pub bt_controller: bool,
    /// Its address when it has one (the controller started).
    pub bt_address: Option<String>,
}

/// The plain result of a run, also written to `last.json` for the Mac drivers page.
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct Outcome {
    pub at: i64,
    pub source: String,
    /// Packs were installed (the job succeeded).
    pub installed: bool,
    pub wifi_files: usize,
    pub bt_files: usize,
    pub seen: Seen,
    /// One short line for the page.
    pub headline: String,
    /// The detail lines (what happened, what is known).
    pub lines: Vec<String>,
    pub kernel: Vec<String>,
}
