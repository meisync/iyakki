//! Transcoding for the DLNA server (F-MEDIA-11): a file a player cannot play is converted on the fly. The converter is the person's own
//! ffmpeg (the audio and video add-on, downloaded at their click, never shipped); without it nothing is offered. The decision, the
//! command line and the stream are here; the file itself is opened and checked by `upnp_serve::media` exactly as for a plain download
//! (inside a shared folder, no link, no pipe), and only then handed to the converter.
//!
//! What the converter is given: a fixed argument list built here (no shell, no text from a player: the player only chooses one of the
//! target names below), the input path of a file already proven to be in the share, no network protocols, no metadata copied.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};

/// One thing a file can be turned into.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Target {
    pub name: &'static str,
    pub mime: &'static str,
    pub video: bool,
}

/// The targets, by the name a player asks for (`?t=name`). WAV always works (the converter needs no encoder library for it); the others
/// need an encoder the person's ffmpeg may not have, which `Transcoder::can` says.
pub const TARGETS: [Target; 3] = [Target { name: "wav", mime: "audio/wav", video: false }, Target { name: "mp3", mime: "audio/mpeg", video: false }, Target { name: "mpegts", mime: "video/mpeg", video: true }];

pub fn target(name: &str) -> Option<Target> {
    TARGETS.iter().copied().find(|t| t.name == name)
}

/// What to do with a file for a player.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Plan {
    /// The player plays it as it is.
    Direct,
    Transcode(Target),
}

/// The types almost every player plays as they are.
const PLAYS_AS_IS: [&str; 6] = ["audio/mpeg", "audio/wav", "audio/x-wav", "audio/mp4", "video/mp4", "image/jpeg"];

/// A file of this type (`video` for pictures in motion): handed over as it is when players play it, else the target that suits it, if the
/// converter can make it (`can`).
pub fn plan(mime: &str, video: bool, can: &dyn Fn(&Target) -> bool) -> Option<Plan> {
    if PLAYS_AS_IS.contains(&mime) {
        return Some(Plan::Direct);
    }
    let want: &[&str] = if video { &["mpegts"] } else { &["mp3", "wav"] };
    want.iter().filter_map(|n| target(n)).find(|t| can(t)).map(Plan::Transcode)
}

/// The converter's arguments for `input` and `t`: audio only for an audio target (a picture or a cover is dropped), a stream on stdout.
pub fn ffmpeg_args(t: &Target, input: &Path) -> Vec<String> {
    let mut a: Vec<String> = ["-nostdin", "-hide_banner", "-loglevel", "error", "-protocol_whitelist", "file", "-i"].iter().map(|s| s.to_string()).collect();
    a.push(input.display().to_string());
    a.extend(["-map_metadata", "-1"].iter().map(|s| s.to_string()));
    let rest: &[&str] = match t.name {
        "wav" => &["-vn", "-c:a", "pcm_s16le", "-ar", "44100", "-ac", "2", "-f", "wav"],
        "mp3" => &["-vn", "-c:a", "libmp3lame", "-b:a", "192k", "-f", "mp3"],
        _ => &["-c:v", "mpeg2video", "-q:v", "4", "-c:a", "mp2", "-b:a", "192k", "-f", "mpegts"],
    };
    a.extend(rest.iter().map(|s| s.to_string()));
    a.push("pipe:1".into());
    a
}

/// The converter. The real one runs ffmpeg; a test puts its own.
pub trait Transcoder: Send + Sync {
    /// Whether this target can be made (the add-on is there and has the encoder).
    fn can(&self, t: &Target) -> bool;
    /// Start converting `input`; the stream ends when the conversion does. Dropping the reader stops the conversion.
    fn start(&self, t: &Target, input: &Path) -> Result<Box<dyn Read + Send>, String>;
}

/// The person's ffmpeg.
pub struct Ffmpeg {
    pub exe: PathBuf,
    /// The encoders the build has (read once from `ffmpeg -encoders`), by name.
    pub encoders: Vec<String>,
}

impl Ffmpeg {
    /// Look at the program: it must run, and its encoder list is read. `None` when it does not.
    pub fn probe(exe: &Path) -> Option<Ffmpeg> {
        let out = Command::new(exe).args(["-hide_banner", "-encoders"]).env_clear().stdin(Stdio::null()).stderr(Stdio::null()).output().ok()?;
        if !out.status.success() {
            return None;
        }
        Some(Ffmpeg { exe: exe.to_path_buf(), encoders: encoder_names(&String::from_utf8_lossy(&out.stdout)) })
    }
}

/// The names in `ffmpeg -encoders` output: `V....D mpeg2video  MPEG-2 video`, the second column.
pub fn encoder_names(text: &str) -> Vec<String> {
    text.lines().filter_map(|l| {
        let mut it = l.split_whitespace();
        let flags = it.next()?;
        let name = it.next()?;
        (name != "=" && flags.len() == 6 && flags.chars().next().is_some_and(|c| matches!(c, 'V' | 'A' | 'S')) && flags.chars().skip(1).all(|c| c == '.' || c.is_ascii_uppercase())).then(|| name.to_string())
    }).collect()
}

/// The encoders a target needs.
fn needs(t: &Target) -> &'static [&'static str] {
    match t.name {
        "wav" => &["pcm_s16le"],
        "mp3" => &["libmp3lame"],
        _ => &["mpeg2video", "mp2"],
    }
}

struct Running {
    child: Child,
    out: std::process::ChildStdout,
}

impl Read for Running {
    fn read(&mut self, b: &mut [u8]) -> std::io::Result<usize> {
        self.out.read(b)
    }
}

impl Drop for Running {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl Transcoder for Ffmpeg {
    fn can(&self, t: &Target) -> bool {
        needs(t).iter().all(|n| self.encoders.iter().any(|e| e == n))
    }
    fn start(&self, t: &Target, input: &Path) -> Result<Box<dyn Read + Send>, String> {
        if !self.can(t) {
            return Err("this ffmpeg cannot make that".into());
        }
        let mut child = Command::new(&self.exe).args(ffmpeg_args(t, input)).env_clear().stdin(Stdio::null()).stderr(Stdio::null()).stdout(Stdio::piped()).spawn().map_err(|e| format!("ffmpeg did not start: {e}"))?;
        let out = child.stdout.take().ok_or("ffmpeg gave no output")?;
        Ok(Box::new(Running { child, out }))
    }
}

#[cfg(test)]
#[path = "upnp_transcode_tests.rs"]
mod tests;
