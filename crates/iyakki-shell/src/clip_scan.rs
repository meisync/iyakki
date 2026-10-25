//! "Scan QR" of the clipboard history: copy text by holding a QR code up to the camera, so a secret or any text reaches this
//! computer without a remote channel (the person makes a QR code of it on a phone or another computer).
//!
//! The camera is the shared one (scan_core.rs): it asks first, shows a "CAMERA IS ON" card with the live picture and a frame guide,
//! and stops after a read, on Esc, when this dialog goes away and after a minute. What was read is shown *before* it is copied
//! (long text cut; text that looks like a secret is hidden behind dots with a Show button), and the person chooses Copy, Scan again
//! or Cancel. Copy goes through the normal clipboard `copy()`, or `copy_secret()` when the box "Secret" is ticked (it is ticked
//! from the start for hidden text). A scanned address is only text: it is never opened. Nothing read is logged, stored on disk or
//! put in the audit (the shell records only the number of characters and secret yes/no).

use std::sync::Arc;
use std::time::Duration;

use crate::gfx::{rgb, Face, Gfx};
use crate::scan_core::{self, CamSource, Card, Session, Step};
use crate::ui::clipboard;
use crate::ui::{Event, Key, DIM, ERR, MUTED, OK, R, TEXT};
use crate::fx;

const ACCENT: [u8; 4] = rgb(255, 184, 77);

pub(crate) const T_ALLOW: usize = 1_410_001;
pub(crate) const T_DENY: usize = 1_410_002;
pub(crate) const T_STOP: usize = 1_410_003;
pub(crate) const T_COPY: usize = 1_410_004;
pub(crate) const T_AGAIN: usize = 1_410_005;
pub(crate) const T_CANCEL: usize = 1_410_006;
pub(crate) const T_SHOW: usize = 1_410_007;
pub(crate) const T_SECRET: usize = 1_410_008;
/// How many characters of long text are shown before it is copied.
const SHOW_CHARS: usize = 120;

/// What the dialog asks of the shell.
#[derive(Debug, PartialEq, Eq)]
pub enum Out {
    Stay,
    Close,
    /// The text was copied: how many characters, and whether as a secret (for the audit: never the text).
    Copied { chars: usize, secret: bool },
}

struct Review {
    text: String,
    /// Hidden behind dots until Show.
    masked: bool,
    secret: bool,
}

pub struct ClipScan {
    sess: Session,
    review: Option<Review>,
    max_bytes: usize,
}

impl ClipScan {
    /// Open the dialog on the machine's camera and ask whether it may turn on.
    pub fn open(src: Arc<CamSource>) -> ClipScan {
        let mut c = ClipScan { sess: Session::new(src, "There is no camera on this device. Nothing was scanned."), review: None, max_bytes: clipboard::MAX_BYTES };
        if cfg!(test) {
            // a busy test machine must not make the decoder give up on a frame
            c.sess = c.sess.with_budget(Duration::from_secs(10));
        }
        c.sess.ask();
        c
    }

    /// A shorter time limit (tests).
    pub fn with_limit(mut self, limit: Duration) -> ClipScan {
        self.sess = self.sess.with_limit(limit);
        self
    }

    /// A smaller most-text limit (tests: a QR code cannot hold 64 KB).
    pub fn with_max_bytes(mut self, n: usize) -> ClipScan {
        self.max_bytes = n;
        self
    }

    pub fn session(&self) -> &Session {
        &self.sess
    }

    pub fn reviewing(&self) -> bool {
        self.review.is_some()
    }

    /// The Secret box is ticked.
    pub fn secret(&self) -> bool {
        self.review.as_ref().is_some_and(|r| r.secret)
    }

    pub fn masked(&self) -> bool {
        self.review.as_ref().is_some_and(|r| r.masked)
    }

    /// What the person sees of the read text now: dots while hidden, else the text cut to [`SHOW_CHARS`] characters.
    pub fn shown(&self) -> Option<String> {
        let r = self.review.as_ref()?;
        let n = r.text.chars().count();
        if r.masked {
            return Some("•".repeat(n.clamp(8, 24)));
        }
        let one_line: String = r.text.chars().take(SHOW_CHARS).map(|c| if c.is_control() { ' ' } else { c }).collect();
        Some(if n > SHOW_CHARS { format!("{one_line}…") } else { one_line })
    }

    /// Look at the camera (every heartbeat). True when something on screen changed.
    pub fn tick(&mut self) -> bool {
        if self.sess.step() != Step::On {
            return false;
        }
        let max = self.max_bytes;
        let got = self.sess.poll(|bytes| {
            let Ok(text) = std::str::from_utf8(bytes) else {
                return Err("That QR code does not hold text. Nothing was copied.".to_string());
            };
            if text.is_empty() || bytes.len() > max {
                return Err(format!("That QR code holds more than {max} bytes, too much to copy. Nothing was copied."));
            }
            Ok((text.to_string(), format!("Read {} characters. The camera is off.", text.chars().count())))
        });
        if let Some(text) = got {
            let masked = clipboard::looks_secret(&text);
            self.review = Some(Review { text, masked, secret: masked });
        }
        true
    }

    fn again(&mut self) {
        self.review = None;
        self.sess.stop(None);
        self.sess.ask();
    }

    pub fn handle(&mut self, ev: &Event) -> Out {
        if self.review.is_some() {
            return match ev {
                Event::Tab(T_COPY) | Event::Key(Key::Enter) => self.copy(),
                Event::Tab(T_AGAIN) | Event::Key(Key::Ctrl('k' | 'K')) => {
                    self.again();
                    Out::Stay
                }
                Event::Tab(T_CANCEL) | Event::Key(Key::Esc) => Out::Close,
                Event::Tab(T_SHOW) => {
                    if let Some(r) = self.review.as_mut() {
                        r.masked = !r.masked;
                    }
                    Out::Stay
                }
                Event::Tab(T_SECRET) => {
                    if let Some(r) = self.review.as_mut() {
                        r.secret = !r.secret;
                    }
                    Out::Stay
                }
                _ => Out::Stay,
            };
        }
        match (self.sess.step(), ev) {
            (Step::Asking, Event::Tab(T_ALLOW)) | (Step::Asking, Event::Key(Key::Enter)) => self.sess.allow(),
            (Step::Asking, Event::Tab(T_DENY)) | (Step::Asking, Event::Key(Key::Esc)) | (Step::Asking, Event::Tab(T_CANCEL)) => return Out::Close,
            (Step::On, Event::Tab(T_STOP)) | (Step::On, Event::Key(Key::Esc)) | (Step::On, Event::Tab(T_CANCEL)) => return Out::Close,
            // the camera is off (no camera, refused, time up, pulled out): the message with Scan again and Close
            (Step::Off, Event::Tab(T_AGAIN)) | (Step::Off, Event::Key(Key::Ctrl('k' | 'K'))) => self.again(),
            (Step::Off, Event::Tab(T_CANCEL)) | (Step::Off, Event::Key(Key::Esc | Key::Enter)) => return Out::Close,
            _ => {}
        }
        Out::Stay
    }

    fn copy(&mut self) -> Out {
        let Some(r) = self.review.take() else { return Out::Stay };
        let chars = r.text.chars().count();
        if r.secret {
            clipboard::copy_secret(&r.text);
        } else {
            clipboard::copy(&r.text);
        }
        Out::Copied { chars, secret: r.secret }
    }

    pub fn render(&mut self, g: &mut Gfx, p: R, s: f32, tick: u64) {
        if self.review.is_some() {
            return self.render_review(g, p, s);
        }
        if self.sess.step() == Step::Off {
            return self.render_message(g, p, s);
        }
        let card = Card {
            ask_title: "Turn on the camera?",
            ask_body: "IYAKKI will use the camera only to read one QR code: make a QR code of the text on a phone or another computer and show it to the camera. You will see what was read before anything is copied. The pictures are looked at in memory, never saved and never sent. The camera turns off as soon as a code is read, when you press Esc, when this closes, and after one minute.",
            hold_note: "Hold the QR code inside the corners, about 20 cm from the camera. Nothing is saved.",
            allow: T_ALLOW,
            deny: T_DENY,
            stop: T_STOP,
        };
        scan_core::draw_card(g, p, s, tick, &self.sess, &card);
    }

    fn backdrop(g: &mut Gfx, p: R) {
        g.rect(p.x, p.y, p.w, p.h, [3, 5, 14, 200]);
        g.clear_hits();
        g.hit(p, Event::Tick, "backdrop");
    }

    /// The camera is off and nothing was read: what happened, in plain words.
    fn render_message(&mut self, g: &mut Gfx, p: R, s: f32) {
        Self::backdrop(g, p);
        let (cw, ch) = (520.0 * s, 210.0 * s);
        let c = R::new(p.x + (p.w - cw) / 2.0, p.y + ((p.h - ch) / 2.0).max(6.0 * s), cw, ch);
        fx::modal_panel(g, c, s, ACCENT, false);
        let (x, w) = (c.x + 26.0 * s, c.w - 52.0 * s);
        g.text(x, c.y + 22.0 * s, 21.0 * s, TEXT, "Scan QR", Face::Bold);
        let (m, ok) = self.sess.said().cloned().unwrap_or(("The camera is off.".to_string(), true));
        let mut y = c.y + 62.0 * s;
        for l in g.wrap(14.0 * s, &m, Face::Sans, w).into_iter().take(4) {
            g.text(x, y, 14.0 * s, if ok { OK } else { ERR }, &l, Face::Sans);
            y += 19.0 * s;
        }
        let by = c.y + c.h - 58.0 * s;
        let b1 = g.button(x, by, 15.0 * s, "Scan again", OK, Event::Tab(T_AGAIN));
        g.button(x + b1 + 12.0 * s, by, 15.0 * s, "Close", MUTED, Event::Tab(T_CANCEL));
    }

    /// What was read, before it is copied: Copy, Scan again, Cancel, the Secret box and Show for hidden text.
    fn render_review(&mut self, g: &mut Gfx, p: R, s: f32) {
        Self::backdrop(g, p);
        let (cw, ch) = (580.0 * s, 360.0 * s);
        let c = R::new(p.x + (p.w - cw) / 2.0, p.y + ((p.h - ch) / 2.0).max(6.0 * s), cw, ch);
        fx::modal_panel(g, c, s, ACCENT, false);
        let (x, w) = (c.x + 26.0 * s, c.w - 52.0 * s);
        let Some(r) = self.review.as_ref() else { return };
        let n = r.text.chars().count();
        g.text(x, c.y + 20.0 * s, 20.0 * s, TEXT, &format!("Read from the QR code: {n} characters"), Face::Bold);
        let shown = self.shown().unwrap_or_default();
        let box_r = R::new(x, c.y + 60.0 * s, w, 120.0 * s);
        g.rrect(box_r.x, box_r.y, box_r.w, box_r.h, 8.0 * s, rgb(8, 10, 20));
        let mut y = box_r.y + 10.0 * s;
        for l in g.wrap(14.0 * s, &shown, Face::Mono, w - 20.0 * s).into_iter().take(5) {
            g.text(box_r.x + 10.0 * s, y, 14.0 * s, TEXT, &l, Face::Mono);
            y += 19.0 * s;
        }
        let (masked, secret) = (r.masked, r.secret);
        let note = if masked {
            "Hidden because it looks like a secret."
        } else if n > SHOW_CHARS {
            "Long text: only the start is shown here; all of it is copied."
        } else {
            "Check it before copying. A web address is only copied, never opened."
        };
        g.text(x, box_r.y + box_r.h + 8.0 * s, 12.0 * s, DIM, note, Face::Sans);
        if masked {
            g.button(x + w - 90.0 * s, box_r.y + box_r.h + 4.0 * s, 13.0 * s, "Show", MUTED, Event::Tab(T_SHOW));
        } else if clipboard::looks_secret(&self.review.as_ref().map(|r| r.text.clone()).unwrap_or_default()) {
            g.button(x + w - 90.0 * s, box_r.y + box_r.h + 4.0 * s, 13.0 * s, "Hide", MUTED, Event::Tab(T_SHOW));
        }
        // the Secret box
        let cy = box_r.y + box_r.h + 44.0 * s;
        let bx = R::new(x, cy, 20.0 * s, 20.0 * s);
        g.rrect(bx.x, bx.y, bx.w, bx.h, 4.0 * s, rgb(8, 10, 20));
        g.rrect_outline(bx.x, bx.y, bx.w, bx.h, 4.0 * s, 1.4 * s, if secret { ACCENT } else { MUTED });
        if secret {
            g.text(bx.x + 3.0 * s, bx.y - 1.0 * s, 17.0 * s, ACCENT, "✓", Face::Bold);
        }
        let label = "Secret: keep out of the history and clear after paste or in 60 seconds";
        for (i, l) in g.wrap(13.0 * s, label, Face::Sans, w - 32.0 * s).into_iter().take(2).enumerate() {
            g.text(x + 30.0 * s, cy + 1.0 * s + i as f32 * 16.0 * s, 13.0 * s, TEXT, &l, Face::Sans);
        }
        g.hit(R::new(x, cy, w, 36.0 * s), Event::Tab(T_SECRET), "Secret");
        let by = c.y + c.h - 58.0 * s;
        let b1 = g.button(x, by, 15.0 * s, "Copy", OK, Event::Tab(T_COPY));
        let b2 = g.button(x + b1 + 12.0 * s, by, 15.0 * s, "Scan again", ACCENT, Event::Tab(T_AGAIN));
        g.button(x + b1 + b2 + 24.0 * s, by, 15.0 * s, "Cancel", MUTED, Event::Tab(T_CANCEL));
        g.text(x, by + 38.0 * s, 12.0 * s, DIM, "Enter copies · Ctrl+K scans again · Esc cancels", Face::Sans);
    }
}

#[cfg(test)]
#[path = "clip_scan_tests.rs"]
mod tests;
