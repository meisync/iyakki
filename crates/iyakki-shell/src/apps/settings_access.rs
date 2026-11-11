//! Settings -> Accessibility (docs/ACCESSIBILITY.md): the screen reader, high contrast, text size, the large pointer, the big focus ring,
//! reduced motion, sticky keys and slow keys. Everything here is the person's own (no administrator needed); the choices are kept by
//! `iyakki_agent::accessibility` and the shell applies them within a second, exactly as when the assistant changes them
//! (`accessibility_set`). Every row is a switch with a letter key, so the page works from the keyboard and, through the keyboard walk
//! (Super+Alt+Right), by a person who hears it.

use super::*;
use iyakki_agent::accessibility::{self as acc, Access, SLOW_CHOICES};

/// Whether a reading voice exists (Settings -> Voice, or an engine found on this computer): without one the reader is silent.
pub(crate) fn voice_ready(svc: &iyakki_agent::services::Services) -> bool {
    svc.voice_settings().tts.is_some_and(|e| !matches!(e, iyakki_agent::voice::SpeechEngine::Browser)) || svc.host_tts.report().is_some_and(|r| r.engines.iter().any(|e| e.usable()))
}

impl SettingsApp {
    fn ac_load(cx: &AppCx) -> Access {
        acc::load(cx.services.dir(), &cx.session.user)
    }

    fn ac_save(&mut self, cx: &AppCx, what: &str, f: impl FnOnce(&mut Access)) -> Option<Access> {
        let mut a = Self::ac_load(cx);
        f(&mut a);
        match acc::save(cx.services.dir(), &cx.session.user, &a) {
            Ok(()) => {
                cx.audit.record(&cx.session.user, "settings.accessibility", what);
                Some(a)
            }
            Err(e) => {
                self.fail(e);
                None
            }
        }
    }

    pub(super) fn handle_access(&mut self, k: &Key, cx: &AppCx) {
        let c = match k {
            Key::Char(c) => *c,
            _ => return,
        };
        let on_off = |b: bool| if b { "on" } else { "off" };
        match c {
            'r' => {
                if let Some(a) = self.ac_save(cx, "screen reader", |a| a.screen_reader = !a.screen_reader) {
                    if a.screen_reader && !voice_ready(&cx.services) {
                        self.fail("Screen reader on, but no reading voice is installed, so it cannot speak yet. Set one up in Settings, Voice. Super+Alt+S turns the reader on or off from any screen.");
                    } else {
                        self.ok(format!("Screen reader {}. Super+Alt+S turns it on or off from any screen, also at sign-in.", on_off(a.screen_reader)));
                    }
                }
            }
            't' => {
                use iyakki_agent::voiceassist::Speaker;
                let mut sp = iyakki_agent::voiceassist_ports::SystemSpeaker::new(cx.services.clone());
                match sp.say("This is the screen reader voice. If you can hear this, the reader can speak.") {
                    Ok(()) => self.ok("Speaking a test sentence."),
                    Err(e) => self.fail(format!("The voice could not speak: {e}")),
                }
            }
            'n' => {
                if let Some(a) = self.ac_save(cx, "speak notices", |a| a.speak_notices = !a.speak_notices) {
                    self.ok(format!("Messages, errors and notices are {} read aloud.", if a.speak_notices { "now" } else { "no longer" }));
                }
            }
            'c' => {
                let on = !acc::high_contrast(cx.services.dir(), &cx.session.user);
                match acc::set_high_contrast(cx.services.dir(), &cx.session.user, on) {
                    Ok(()) => {
                        crate::theme::apply(Some(crate::theme::load(cx.services.dir(), &cx.session.user)));
                        cx.audit.record(&cx.session.user, "settings.accessibility", &format!("high contrast {}", on_off(on)));
                        self.ok(format!("High contrast {}.", on_off(on)));
                    }
                    Err(e) => self.fail(e),
                }
            }
            'u' | 'U' => self.change_ui_size(c == 'u', cx),
            'p' => {
                if let Some(a) = self.ac_save(cx, "large pointer", |a| a.large_pointer = !a.large_pointer) {
                    self.ok(format!("Large pointer {} (it grows when the pointer moves next).", on_off(a.large_pointer)));
                }
            }
            'f' => {
                if let Some(a) = self.ac_save(cx, "big focus ring", |a| a.big_ring = !a.big_ring) {
                    self.ok(format!("Big focus ring {}.", on_off(a.big_ring)));
                }
            }
            'm' => {
                if let Some(a) = self.ac_save(cx, "reduce motion", |a| a.reduce_motion = !a.reduce_motion) {
                    self.ok(format!("Reduced motion {}: no moving backgrounds, no blinking caret.", on_off(a.reduce_motion)));
                }
            }
            'k' => {
                if let Some(a) = self.ac_save(cx, "sticky keys", |a| a.sticky_keys = !a.sticky_keys) {
                    self.ok(format!("Sticky keys {}. {}", on_off(a.sticky_keys), if a.sticky_keys { "Press Shift, Ctrl, Alt or Super, let go, then the next key. Twice in a row keeps it on until a third press." } else { "Modifier keys are held again." }));
                }
            }
            'w' => {
                let next = {
                    let cur = Self::ac_load(cx).slow_keys_ms;
                    let at = SLOW_CHOICES.iter().position(|c| *c == cur).unwrap_or(0);
                    SLOW_CHOICES[(at + 1) % SLOW_CHOICES.len()]
                };
                if self.ac_save(cx, &format!("slow keys {next} ms"), |a| a.slow_keys_ms = next).is_some() {
                    self.ok(if next == 0 { "Slow keys off: every key counts at once.".to_string() } else { format!("Slow keys: a key counts after it was held {next} milliseconds. Brushing a key does nothing.") });
                }
            }
            _ => {}
        }
    }

    pub(super) fn access_buttons(&self) -> Vec<Btn> {
        vec![("Screen reader", ACCENT, 'r'), ("Test the voice", TEXT, 't'), ("High contrast", TEXT, 'c'), ("Bigger text", TEXT, 'u'), ("Large pointer", TEXT, 'p'), ("Focus ring", TEXT, 'f'), ("Reduce motion", TEXT, 'm'), ("Sticky keys", TEXT, 'k'), ("Slow keys", TEXT, 'w')]
    }

    pub(super) fn render_access(&mut self, g: &mut Gfx, a: R, s: f32, cx: &AppCx) {
        let cfg = Self::ac_load(cx);
        let hc = acc::high_contrast(cx.services.dir(), &cx.session.user);
        let level = crate::uisize::load(cx.services.dir(), &cx.session.user);
        let (size_name, size_f) = crate::uisize::LEVELS[level.min(crate::uisize::LEVELS.len() - 1)];
        let voice = voice_ready(&cx.services);
        let rows: Vec<(String, String, bool, char)> = vec![
            ("Screen reader".into(), if voice { "Speaks what you move to, messages and errors. Super+Alt+S turns it on or off anywhere, also at sign-in.".into() } else { "No reading voice is installed yet: set one up in Settings, Voice. Super+Alt+S turns it on or off anywhere.".into() }, cfg.screen_reader, 'r'),
            ("Read messages and errors aloud".into(), "Toasts, errors and notices, not only the control you moved to.".into(), cfg.speak_notices, 'n'),
            ("High contrast".into(), "Black and white with a yellow accent and thick borders; every text at least 7 to 1.".into(), hc, 'c'),
            ("Large mouse pointer".into(), "A bigger pointer on the screen.".into(), cfg.large_pointer, 'p'),
            ("Big focus ring".into(), "A thicker ring round the control that has the focus.".into(), cfg.big_ring, 'f'),
            ("Reduce motion".into(), "No moving background, no blinking caret.".into(), cfg.reduce_motion, 'm'),
            ("Sticky keys".into(), "Shift, Ctrl, Alt and Super stay on for the next key.".into(), cfg.sticky_keys, 'k'),
            (if cfg.slow_keys_ms == 0 { "Slow keys: off".into() } else { format!("Slow keys: {} ms", cfg.slow_keys_ms) }, "A key counts only after it was held a moment. W cycles the delay.".into(), cfg.slow_keys_ms > 0, 'w'),
        ];
        let cols = if a.w >= 760.0 * s { 2 } else { 1 };
        let gap = 10.0 * s;
        let cw = (a.w - gap * (cols as f32 - 1.0)) / cols as f32;
        let rh = 62.0 * s;
        let limit = a.y + a.h;
        let mut bottom = a.y;
        for (i, (title, help, on, key)) in rows.iter().enumerate() {
            let (col, row) = (i % cols, i / cols);
            let r = R::new(a.x + col as f32 * (cw + gap), a.y + row as f32 * (rh + gap), cw, rh);
            if r.y + r.h > limit {
                break;
            }
            controls::switch_row(g, r, s, title, help, *on, true, Event::Key(Key::Char(*key)), ACCENT);
            bottom = bottom.max(r.y + r.h);
        }
        let mut y = bottom + 12.0 * s;
        let line = |g: &mut Gfx, y: &mut f32, col: [u8; 4], text: &str| {
            for l in g.wrap(13.0 * s, text, Face::Sans, a.w) {
                if *y + 13.0 * s * 1.3 > limit {
                    return;
                }
                g.text(a.x, *y, 13.0 * s, col, &l, Face::Sans);
                *y += 13.0 * s * 1.45;
            }
        };
        line(g, &mut y, TEXT, &format!("Text size: {size_name} ({}%). u makes text and buttons bigger, U smaller (also in Display).", (size_f * 100.0).round()));
        line(g, &mut y, MUTED, "Key repeat speed and delay are in Keyboard. Without a mouse: Super+Alt+Right and Left move through the controls of the screen, Super+Alt+Enter presses the one marked, Super+Alt+R says the last thing again.");
        line(g, &mut y, DIM, "Not covered: web pages in the Browser, tiles that run as WebAssembly, third-party tiles that do not use the toolkit controls, and Braille. See docs/ACCESSIBILITY.md.");
    }
}
