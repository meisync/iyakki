//! Types moved out of `iyakki_agent::player` (F-SPLIT-03, scripted by dist/split/move_types.py); the agent re-exports them.



/// One picture, ready to draw (RGBA, opaque).
pub struct Frame {
    pub w: u32,
    pub h: u32,
    pub rgba: Vec<u8>,
    /// Grows with every new picture, so the shell can tell whether it has drawn this one.
    pub seq: u64,
    pub time: f64,
}

// ---- moved from iyakki_agent::player (F-SPLIT-03) ----


#[derive(Clone, Debug, Default, PartialEq)]
pub struct VideoInfo {
    pub codec: String,
    pub w: u32,
    pub h: u32,
    pub fps: f64,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct AudioInfo {
    pub codec: String,
    pub hz: u32,
    pub layout: String,
}

/// What `ffmpeg -i` says about a file.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Info {
    pub duration: Option<f64>,
    pub video: Option<VideoInfo>,
    pub audio: Option<AudioInfo>,
}

impl Info {
    /// The media info line: `h264 1280x720 25 fps · aac 44100 Hz stereo · 1:23`.
    pub fn line(&self) -> String {
        let mut parts = Vec::new();
        if let Some(v) = &self.video {
            parts.push(format!("{} {}x{} {} fps", v.codec, v.w, v.h, if v.fps.fract() == 0.0 { format!("{}", v.fps as u32) } else { format!("{:.2}", v.fps) }));
        }
        if let Some(a) = &self.audio {
            parts.push(format!("{} {} Hz {}", a.codec, a.hz, a.layout).trim().to_string());
        }
        if let Some(d) = self.duration {
            parts.push(clock_text(d));
        }
        parts.join(" \u{b7} ")
    }
}

/// What the screen shows about the playback.
#[derive(Clone, Debug, PartialEq)]
pub struct Status {
    pub position: f64,
    pub duration: Option<f64>,
    pub paused: bool,
    pub ended: bool,
    pub error: Option<String>,
}

/// `1:05` or `1:02:03`.
pub fn clock_text(secs: f64) -> String {
    let s = secs.max(0.0) as u64;
    if s >= 3600 {
        format!("{}:{:02}:{:02}", s / 3600, s / 60 % 60, s % 60)
    } else {
        format!("{}:{:02}", s / 60, s % 60)
    }
}

// ---- moved from iyakki-agent (F-SPLIT-03) ----


/// The size a picture of `src` gets inside `area`: as large as fits, the proportions kept, both sides even (the decoder wants it).
pub fn fit(src: (u32, u32), area: (u32, u32)) -> (u32, u32) {
    let (sw, sh) = (src.0.max(1) as f64, src.1.max(1) as f64);
    let k = (area.0.max(2) as f64 / sw).min(area.1.max(2) as f64 / sh);
    let even = |v: f64| ((v.round() as u32).max(2)) & !1;
    (even(sw * k), even(sh * k))
}
