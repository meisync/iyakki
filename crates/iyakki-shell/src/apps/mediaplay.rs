//! MEDIA's own video and audio view: the picture IYAKKI's in-house player ([`iyakki_agent::player`], `ffmpeg` as the decoder)
//! decodes, drawn straight into the page area, with a progress bar, the time, an info line and the controls.
//!
//! Keys: Space or `p` pause and play, Left and Right seek 5 s, Down and Up 30 s, Home starts again, `f` fullscreen (the page
//! area without the bars; the controls come back when the mouse moves or a key is pressed), Esc leaves fullscreen and then
//! stops. Mouse: click the bar to seek, click the picture to pause. Everything else about playing is in `player.rs`; this file
//! only draws and turns keys into player calls.

use std::sync::Arc;

use iyakki_agent::player::{clock_text, Config, Frame, Player};

use super::neon::{level_bars, CYAN, MAGENTA};
use crate::gfx::{rgb, Face, Gfx};
use crate::ui::{Event, Key, DIM, ERR, MUTED, OK, R, TEXT};

/// The ids of the controls (clicks come back as `Event::Tab`, the bar as `Event::Focus`).
pub const T_STOP: usize = 210;
pub const T_PAUSE: usize = 211;
pub const T_BACK: usize = 212;
pub const T_FWD: usize = 213;
pub const T_FULL: usize = 214;
pub const T_PICTURE: usize = 215;
pub const F_BAR: usize = 216;

/// How many heartbeats (~30 a second) the controls stay up in fullscreen after the last movement or key.
const SHOW_TICKS: u32 = 90;
/// A new size must hold this many frames before the picture is decoded again at it.
const RESIZE_SETTLE: u32 = 10;

#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum Out {
    Stay,
    /// The person stopped it.
    Close,
}

pub struct NativeView {
    player: Player,
    pub title: String,
    info_line: String,
    audio_only: bool,
    fullscreen: bool,
    seen: u64,
    last: Option<Arc<Frame>>,
    idle: u32,
    bar: R,
    want: (u32, u32),
    settled: u32,
    tick: u64,
}

impl NativeView {
    /// Start `path` (the installed ffmpeg and IYAKKI's sound output); `area` is the page area in pixels.
    pub fn start(path: &str, title: &str, area: R) -> Result<NativeView, String> {
        let cfg = Config::system().ok_or_else(|| {
            if iyakki_agent::player::find_program("ffmpeg").is_some() {
                iyakki_agent::player::plain_reason("Error loading shared library")
            } else {
                "ffmpeg is not installed".to_string()
            }
        })?;
        NativeView::start_with(cfg, path, title, area)
    }

    pub fn start_with(cfg: Config, path: &str, title: &str, area: R) -> Result<NativeView, String> {
        let px = (area.w.max(64.0) as u32, area.h.max(64.0) as u32);
        let player = Player::start(cfg, path, 0.0, px)?;
        let info = player.info();
        Ok(NativeView { info_line: info.line(), audio_only: info.video.is_none(), player, title: title.to_string(), fullscreen: false, seen: 0, last: None, idle: 0, bar: R::new(0.0, 0.0, 1.0, 1.0), want: px, settled: 0, tick: 0 })
    }

    pub fn is_fullscreen(&self) -> bool {
        self.fullscreen
    }

    pub fn player(&self) -> &Player {
        &self.player
    }

    /// What it says when something went wrong (shown over the picture); `None` while all is well.
    pub fn problem(&self) -> Option<String> {
        self.player.status().error
    }

    pub fn event(&mut self, ev: &Event) -> Out {
        match ev {
            Event::Tick => {
                self.tick += 1;
                self.idle = self.idle.saturating_add(1);
                if let Some(f) = self.player.frame(self.seen) {
                    self.seen = f.seq;
                    self.last = Some(f);
                }
            }
            Event::MouseMove(..) | Event::MouseDown(..) => self.idle = 0,
            Event::Key(k) => {
                self.idle = 0;
                return self.key(k);
            }
            Event::Tab(T_STOP) => return Out::Close,
            Event::Tab(T_PAUSE) | Event::Tab(T_PICTURE) => self.toggle(),
            Event::Tab(T_BACK) => self.player.seek_by(-10.0),
            Event::Tab(T_FWD) => self.player.seek_by(10.0),
            Event::Tab(T_FULL) => self.fullscreen = !self.fullscreen,
            Event::FocusAt(F_BAR, x) => {
                if let Some(d) = self.player.info().duration {
                    self.player.seek_to(d * (*x / self.bar.w.max(1.0)).clamp(0.0, 1.0) as f64);
                }
            }
            _ => {}
        }
        Out::Stay
    }

    fn toggle(&mut self) {
        if self.player.status().ended {
            self.player.seek_to(0.0);
        } else {
            self.player.toggle_pause();
        }
    }

    fn key(&mut self, k: &Key) -> Out {
        match k {
            Key::Esc if self.fullscreen => self.fullscreen = false,
            Key::Esc | Key::Char('s') => return Out::Close,
            Key::Char(' ') | Key::Char('p') | Key::Enter => self.toggle(),
            Key::Left => self.player.seek_by(-5.0),
            Key::Right => self.player.seek_by(5.0),
            Key::Down => self.player.seek_by(-30.0),
            Key::Up => self.player.seek_by(30.0),
            Key::Home | Key::Char('0') => self.player.seek_to(0.0),
            Key::Char('f') | Key::Char('F') => self.fullscreen = !self.fullscreen,
            _ => {}
        }
        Out::Stay
    }

    /// The hint line of the app while this plays.
    pub fn hint(&self) -> &'static str {
        "Space pause or play \u{b7} \u{2190} \u{2192} seek 5 s \u{b7} \u{2191} \u{2193} 30 s \u{b7} f fullscreen \u{b7} Esc stop"
    }

    pub fn render(&mut self, g: &mut Gfx, area: R, s: f32) {
        let st = self.player.status();
        let ctl_h = 62.0 * s;
        let top_h = 40.0 * s;
        let show_ctl = !self.fullscreen || self.idle < SHOW_TICKS || st.paused || st.ended || st.error.is_some();
        let (pic, top) = if self.fullscreen { (area, None) } else { (R::new(area.x, area.y + top_h, area.w, (area.h - top_h - ctl_h).max(1.0)), Some(R::new(area.x, area.y, area.w, top_h))) };
        g.rect(area.x, area.y, area.w, area.h, rgb(0, 0, 0));

        // a size that has been the same for a moment is decoded again (fullscreen, a resized window)
        let px = (pic.w.max(64.0) as u32, pic.h.max(64.0) as u32);
        if px != self.want {
            self.want = px;
            self.settled = 0;
        } else if self.settled < RESIZE_SETTLE {
            self.settled += 1;
            if self.settled == RESIZE_SETTLE {
                self.player.set_area(px);
            }
        }

        if let Some(f) = &self.last {
            blit(g, f, pic);
        }
        if self.audio_only {
            let levels = super::media::visualizer(self.tick, !st.paused && !st.ended);
            level_bars(g, pic.x + pic.w * 0.15, pic.y + pic.h * 0.6, pic.w * 0.7, pic.h * 0.4, s, &levels, CYAN, MAGENTA);
        }
        g.hit(pic, Event::Tab(T_PICTURE), "Picture");

        if let Some(t) = top {
            g.rect(t.x, t.y, t.w, t.h, rgb(12, 16, 32));
            let title = g.fit(15.0 * s, &self.title, Face::Bold, (t.w * 0.4).max(80.0 * s));
            let tw = g.text(t.x + 16.0 * s, t.y + 11.0 * s, 15.0 * s, TEXT, &title, Face::Bold);
            let room = (t.w - tw - 48.0 * s).max(40.0 * s);
            let info = g.fit(12.0 * s, &self.info_line, Face::Sans, room);
            g.text(t.x + 32.0 * s + tw, t.y + 14.0 * s, 12.0 * s, MUTED, &info, Face::Sans);
        }

        if let Some(e) = &st.error {
            let msg = g.fit(14.0 * s, &format!("Could not play it: {e}"), Face::Sans, pic.w - 40.0 * s);
            g.text(pic.x + 20.0 * s, pic.y + pic.h / 2.0, 14.0 * s, ERR, &msg, Face::Sans);
        } else if self.last.is_none() && !self.audio_only {
            g.text(pic.x + 20.0 * s, pic.y + 20.0 * s, 14.0 * s, MUTED, "Starting the player\u{2026}", Face::Sans);
        }

        if show_ctl {
            let c = R::new(area.x, area.y + area.h - ctl_h, area.w, ctl_h);
            g.rect(c.x, c.y, c.w, c.h, if self.fullscreen { rgb(8, 10, 20) } else { rgb(12, 16, 32) });
            // the bar
            let (bx, by, bw) = (c.x + 16.0 * s, c.y + 12.0 * s, (c.w - 32.0 * s - 150.0 * s).max(40.0 * s));
            self.bar = R::new(bx, by - 6.0 * s, bw, 18.0 * s);
            g.rrect(bx, by, bw, 6.0 * s, 3.0 * s, rgb(40, 46, 70));
            if let Some(d) = st.duration.filter(|d| *d > 0.0) {
                let f = (st.position / d).clamp(0.0, 1.0) as f32;
                g.rrect(bx, by, (bw * f).max(6.0 * s), 6.0 * s, 3.0 * s, CYAN);
                g.hit(self.bar, Event::Focus(F_BAR), "Progress");
            }
            let t = match st.duration {
                Some(d) => format!("{} / {}", clock_text(st.position), clock_text(d)),
                None => clock_text(st.position),
            };
            g.text(bx + bw + 14.0 * s, by - 6.0 * s, 13.0 * s, if st.ended { OK } else { TEXT }, &t, Face::Sans);
            // the buttons
            let mut x = c.x + 16.0 * s;
            let y = c.y + 28.0 * s;
            let pause = if st.ended { "Play again" } else if st.paused { "Play" } else { "Pause" };
            for (label, tab, col) in [(pause, T_PAUSE, OK), ("\u{2212}10 s", T_BACK, MUTED), ("+10 s", T_FWD, MUTED), (if self.fullscreen { "Window" } else { "Fullscreen" }, T_FULL, CYAN), ("Stop", T_STOP, MAGENTA)] {
                x += g.button(x, y, 12.0 * s, label, col, Event::Tab(tab)) + 8.0 * s;
            }
            if !self.fullscreen || st.paused {
                let state = if st.ended { "finished" } else if st.paused { "paused" } else { "" };
                g.text(x + 8.0 * s, y + 6.0 * s, 12.0 * s, DIM, state, Face::Sans);
            }
        }
    }
}


/// Draw `f` centred in `into` (clipped to the screen). Both are opaque RGBA.
fn blit(g: &mut Gfx, f: &Frame, into: R) {
    let (gw, gh) = (g.width() as i32, g.height() as i32);
    let x0 = into.x as i32 + ((into.w as i32 - f.w as i32) / 2).max(0);
    let y0 = into.y as i32 + ((into.h as i32 - f.h as i32) / 2).max(0);
    let w = (f.w as i32).min(into.w as i32).min(gw - x0).max(0);
    if w == 0 {
        return;
    }
    let dst = g.pm.data_mut();
    for row in 0..(f.h as i32).min(into.h as i32) {
        let dy = y0 + row;
        if dy < 0 || dy >= gh || x0 < 0 {
            continue;
        }
        let s0 = (row as usize) * f.w as usize * 4;
        let d0 = (dy as usize * gw as usize + x0 as usize) * 4;
        dst[d0..d0 + w as usize * 4].copy_from_slice(&f.rgba[s0..s0 + w as usize * 4]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    /// A fake ffmpeg and a fake speaker in a temporary folder: no media and no device is touched.
    fn fakes(tag: &str) -> (std::path::PathBuf, Config) {
        let d = std::env::temp_dir().join(format!("iyakki-mediaplay-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        let w = |name: &str, body: &str| {
            let p = d.join(name);
            std::fs::write(&p, format!("#!/bin/sh\n{body}")).unwrap();
            std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
            p.display().to_string()
        };
        let ff = w(
            "ffmpeg",
            r#"echo "$@" >> "$0.log"
case "$*" in
  *rawvideo*)
    for a in "$@"; do case "$a" in *scale=*) wh=${a#*scale=};; esac; done
    W=${wh%%:*}; H=${wh#*:}; S=$((W*H*4))
    head -c $S /dev/zero | tr '\0' '\377'; head -c $S /dev/zero | tr '\0' '\200' ;;
  *s16le*) head -c 192000 /dev/zero ;;
  *) cat >&2 <<'EOF2'
  Duration: 00:00:02.50, start: 0.000000, bitrate: 5000 kb/s
  Stream #0:0: Video: mjpeg (Baseline), yuvj422p, 64x36, 10 fps, 10 tbr, 10 tbn
  Stream #0:1: Audio: pcm_s16le, 44100 Hz, stereo, s16, 1411 kb/s
EOF2
     exit 1 ;;
esac
"#,
        );
        let sp = w("aplay", "cat > /dev/null\n");
        (d.clone(), Config { ffmpeg: ff, audio_cmd: vec![sp], max_fps: 30.0, audio_lag: 0.9 })
    }

    fn pump(v: &mut NativeView, what: &str, f: impl Fn(&NativeView) -> bool) {
        for _ in 0..2000 {
            v.event(&Event::Tick);
            if f(v) {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        panic!("timed out waiting for {what}");
    }

    fn area() -> R {
        R::new(0.0, 0.0, 400.0, 300.0)
    }

    #[test]
    fn the_picture_is_drawn_with_the_bar_the_time_and_the_buttons_and_the_info_line_names_the_codecs() {
        let (d, cfg) = fakes("draw");
        let mut v = NativeView::start_with(cfg, "/media/cam.avi", "Cam", area()).unwrap();
        assert_eq!(v.info_line, "mjpeg 64x36 10 fps \u{b7} pcm_s16le 44100 Hz stereo \u{b7} 0:02");
        pump(&mut v, "the last picture", |v| v.player().status().ended);
        v.event(&Event::Tick);
        let mut g = Gfx::new(400, 300);
        v.render(&mut g, area(), 1.0);
        // the 0x80 picture is fitted into the picture area (400 x 220 minus bars): the centre is its grey
        let px = |g: &Gfx, x: usize, y: usize| g.pm.data()[(y * 400 + x) * 4..(y * 400 + x) * 4 + 4].to_vec();
        assert_eq!(px(&g, 200, 150), vec![0x80, 0x80, 0x80, 0x80]);
        let labels: Vec<String> = g.hits.iter().map(|h| h.label.clone()).collect();
        for want in ["Play again", "Fullscreen", "Stop", "Progress", "Picture"] {
            assert!(labels.iter().any(|l| l == want), "{want} in {labels:?}");
        }
        assert!(v.hint().contains("f fullscreen"));
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn keys_and_clicks_pause_seek_go_fullscreen_and_stop() {
        let (d, cfg) = fakes("keys");
        let mut v = NativeView::start_with(cfg, "/media/cam.avi", "Cam", area()).unwrap();
        pump(&mut v, "a picture", |v| v.last.is_some());
        assert_eq!(v.event(&Event::Key(Key::Char(' '))), Out::Stay);
        assert!(v.player().status().paused);
        v.event(&Event::Tab(T_PAUSE));
        assert!(!v.player().status().paused);
        // a click on the bar seeks to that share of the length
        let mut g = Gfx::new(400, 300);
        v.render(&mut g, area(), 1.0);
        v.event(&Event::FocusAt(F_BAR, v.bar.w / 2.0));
        let log = || std::fs::read_to_string(d.join("ffmpeg.log")).unwrap_or_default();
        pump(&mut v, "the seek", |_| log().contains("-ss 1.250"));
        // both decoders must have restarted before the next key: a seek that comes while one is pending is merged into it
        pump(&mut v, "the seek to restart both decoders", |_| log().matches("-ss").count() >= 4);
        v.event(&Event::Key(Key::Right));
        pump(&mut v, "the key seek", |_| log().matches("-ss").count() >= 6);
        // fullscreen: no title bar, the picture area is the whole area; Esc leaves it, the next Esc stops
        v.event(&Event::Key(Key::Char('f')));
        assert!(v.is_fullscreen());
        let mut g = Gfx::new(400, 300);
        v.render(&mut g, area(), 1.0);
        assert!(!g.hits.iter().any(|h| h.label == "Fullscreen") && g.hits.iter().any(|h| h.label == "Window"));
        assert_eq!(v.event(&Event::Key(Key::Esc)), Out::Stay);
        assert!(!v.is_fullscreen());
        assert_eq!(v.event(&Event::Key(Key::Esc)), Out::Close);
        assert_eq!(v.event(&Event::Tab(T_STOP)), Out::Close);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn a_file_ffmpeg_cannot_read_is_refused_with_its_reason_and_without_ffmpeg_nothing_starts() {
        let (d, mut cfg) = fakes("bad");
        std::fs::write(d.join("bad"), "#!/bin/sh\necho 'moov atom not found' >&2\nexit 1\n").unwrap();
        std::fs::set_permissions(d.join("bad"), std::fs::Permissions::from_mode(0o755)).unwrap();
        cfg.ffmpeg = d.join("bad").display().to_string();
        let e = NativeView::start_with(cfg, "/media/x.mp4", "x", area()).err().unwrap();
        assert!(e.contains("moov atom"), "{e}");
        let _ = std::fs::remove_dir_all(&d);
    }
}
