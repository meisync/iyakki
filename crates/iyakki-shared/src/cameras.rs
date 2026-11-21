use serde::Deserialize;
use serde::Serialize;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Spec {
    pub id: String,
    pub name: String,
    /// `rtsp://host:port/path` or `http://...`; never a user name or password.
    pub address: String,
    pub user: String,
    pub owner: String,
    #[serde(default)]
    pub allowed: Vec<String>,
    /// The owner's vault entry that holds the password (empty: none).
    #[serde(default)]
    pub secret: String,
}

/// The person using the hub.
#[derive(Clone)]
pub struct Who {
    pub user: String,
    /// May manage cameras of others (administrators).
    pub manage: bool,
    /// May use this computer's USB cameras (the CAMERA app is allowed for the account).
    pub usb_ok: bool,
    pub home: PathBuf,
    pub may_write: Arc<dyn Fn(&Path) -> bool + Send + Sync>,
}

impl Who {
    pub fn can_manage(&self, s: &Spec) -> bool {
        s.owner == self.user || self.manage
    }
}

pub enum Picture {
    /// A complete JPEG file.
    Jpeg(Vec<u8>),
    Rgba { width: u32, height: u32, data: Vec<u8> },
}

#[derive(Debug, PartialEq, Eq)]
pub enum FeedErr {
    /// Nothing arrived yet; ask again.
    Timeout,
    Lost(String),
    Addon(String),
    Auth(String),
    Busy(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UsbCam {
    pub id: String,
    pub name: String,
    pub builtin: bool,
}

pub struct NoUsb;

impl UsbProvider for NoUsb {
    fn list(&self) -> Vec<UsbCam> {
        vec![]
    }
    fn open(&self, _: &str) -> Result<Box<dyn Feed>, FeedErr> {
        Err(FeedErr::Lost("no USB camera".into()))
    }
}

/// One open source of pictures.
pub trait Feed: Send {
    fn next(&mut self) -> Result<Picture, FeedErr>;
    /// The sound that came with the pictures since the last call, decoded (a camera with a microphone). Default: none.
    fn take_sound(&mut self) -> Option<Sound> {
        None
    }
}

/// A piece of a camera's sound: 16-bit mono samples at `rate` Hz.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Sound {
    pub rate: u32,
    pub pcm: Vec<i16>,
}

/// The cameras plugged into this computer. The shell implements it on top of its shared V4L2 module.
pub trait UsbProvider: Send + Sync {
    fn list(&self) -> Vec<UsbCam>;
    fn open(&self, id: &str) -> Result<Box<dyn Feed>, FeedErr>;
}
