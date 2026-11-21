//! Settings -> Browser, "Graphics": how the browser engine draws. Automatic (the graphics card, software by itself after the computer
//! stopped right after the engine started on it), Software (safe) or Hardware (never falls back). The state is shown in plain words
//! ("Automatic: hardware", "Automatic: software because the last session stopped ..."). `g` changes the choice, `t` tries the card
//! again after an automatic fall back; both are buttons too. The logic is in `iyakki_agent::webgraphics`; a change applies the next
//! time the engine starts. Only administrators change it (it is a device setting); everybody sees it.

use super::*;
use crate::safeclamp::SafeClamp;

impl SettingsApp {
    /// The section; returns the height it used. Buttons only where they fit, otherwise a line that says where the keys are.
    pub(super) fn render_browser_graphics(&mut self, g: &mut Gfx, a: R, y0: f32, s: f32, cx: &AppCx) -> f32 {
        let st = cx.agent.web_graphics().status(cx.data_dir());
        let admin = cx.session.policy.manage_settings;
        let need = 62.0 + 6.0;
        // the page does not scroll: when the row does not fit, it is a line where there is room for one and nothing where there is not
        if y0 + need * s > a.y + a.h - 4.0 * s {
            if y0 + 22.0 * s < a.y + a.h {
                let line = g.fit(13.0 * s, &format!("Graphics: {}. {}", st.line, if admin { "g changes it." } else { "" }), Face::Sans, a.w - 8.0 * s);
                g.text(a.x + 4.0 * s, y0, 13.0 * s, DIM, &line, Face::Sans);
                return 22.0 * s;
            }
            return 0.0;
        }
        let mut y = y0;
        let size = 13.0 * s;
        let labels: Vec<&str> = if !admin { vec![] } else if st.fallback { vec!["Change (g)", "Try hardware again (t)"] } else { vec!["Change (g)"] };
        let btn_w = labels.iter().map(|l| g.text_width(size, l, Face::Bold) + size * 1.7).fold(0.0, f32::max);
        let info_w = (a.w - btn_w - if btn_w > 0.0 { 10.0 * s } else { 0.0 }).max(120.0 * s);
        let mut help = st.detail.clone();
        if !admin {
            help.push_str(" An administrator changes this.");
        } else if let Some(why) = &st.read_only {
            help = why.clone();
        }
        let r = R::new(a.x, y, info_w, 58.0 * s);
        crate::fx::glass(g, r.x, r.y, r.w, r.h, 14.0 * s, crate::fx::CYAN);
        let tw = info_w - 36.0 * s;
        let title = g.fit(15.0 * s, &format!("Graphics: {}", st.line), Face::Bold, tw);
        g.text(r.x + 18.0 * s, r.y + 5.0 * s, 15.0 * s, TEXT, &title, Face::Bold);
        let mut lines = g.wrap(12.0 * s, &help, Face::Sans, tw);
        if lines.len() > 2 {
            let rest = lines[1..].join(" ");
            lines.truncate(1);
            lines.push(g.fit(12.0 * s, &rest, Face::Sans, tw));
        }
        for (i, l) in lines.iter().enumerate() {
            g.text(r.x + 18.0 * s, r.y + (25.0 + i as f32 * 15.0) * s, 12.0 * s, DIM, l, Face::Sans);
        }
        if st.read_only.is_none() {
            for (i, l) in labels.iter().enumerate() {
                let key = if i == 0 { 'g' } else { 't' };
                g.button(a.x + a.w - btn_w, y + (2.0 + i as f32 * 29.0) * s, size, l, ACCENT, Event::Key(Key::Char(key)));
            }
        }
        y += 58.0 * s;
        y += 6.0 * s;
        y += self.render_browser_accel(g, a, y, s, cx);
        y - y0
    }

    /// One row under the graphics mode: whether the graphics card speeds the browser up (compositing, drawing, video decoding, WebGL) for this
    /// person, with a button for her switch (`G`) and one for the administrator's override of the whole computer (`C`). Drawn only where it fits.
    fn render_browser_accel(&mut self, g: &mut Gfx, a: R, y0: f32, s: f32, cx: &AppCx) -> f32 {
        let h = 34.0 * s;
        if y0 + h > a.y + a.h - 4.0 * s {
            return 0.0;
        }
        let admin = cx.session.policy.manage_settings;
        let (pol, _) = iyakki_shared::webaccel::load(&cx.data_dir().join("web-profile"));
        let me = iyakki_shared::webengine::safe_name(&cx.session.user);
        let v = pol.verdict(&me, "");
        let line = if v.allowed { "Graphics acceleration of the full browser: on where the computer has a working graphics card".to_string() } else { format!("Graphics acceleration of the full browser: off, {}", v.why) };
        let size = 13.0 * s;
        let mine_off = pol.people.get(&me).is_some_and(|p| p.off);
        let mut labels: Vec<(String, char)> = vec![(if mine_off { "Turn on for me (G)".into() } else { "Turn off for me (G)".into() }, 'G')];
        if admin {
            labels.push((if pol.admin_off { "Turn on for all (C)".into() } else { "Turn off for all (C)".into() }, 'C'));
        }
        let widths: Vec<f32> = labels.iter().map(|(l, _)| g.text_width(size, l, Face::Bold) + size * 1.7).collect();
        let btn_w: f32 = widths.iter().sum::<f32>() + 6.0 * s * widths.len() as f32;
        let info_w = (a.w - btn_w - 10.0 * s).max(120.0 * s);
        let r = R::new(a.x, y0, info_w, 30.0 * s);
        crate::fx::glass(g, r.x, r.y, r.w, r.h, 12.0 * s, crate::fx::CYAN);
        let t = g.fit(12.0 * s, &line, Face::Sans, info_w - 28.0 * s);
        g.text(r.x + 14.0 * s, r.y + 8.0 * s, 12.0 * s, DIM, &t, Face::Sans);
        let mut x = a.x + a.w - btn_w;
        for ((l, k), w) in labels.iter().zip(&widths) {
            g.button(x, y0 + 2.0 * s, size, l, ACCENT, Event::Key(Key::Char(*k)));
            x += w + 6.0 * s;
        }
        h + 4.0 * s
    }

    /// The arrow, Page, Home and End keys scroll the Browser page (it is taller than a small window). True when the key was one of these.
    pub(super) fn browser_scroll_key(&mut self, k: &Key) -> bool {
        let s = self.nav_s.max(0.5);
        let page = (self.bh.page_h.max(120.0)) * 0.85;
        let to = match k {
            Key::Up => self.bh.scroll - 40.0 * s,
            Key::Down => self.bh.scroll + 40.0 * s,
            Key::PageUp => self.bh.scroll - page,
            Key::PageDown => self.bh.scroll + page,
            Key::Home => 0.0,
            Key::End => self.bh.max,
            _ => return false,
        };
        self.bh.scroll = to.clamp_safe(0.0, self.bh.max);
        true
    }

    /// `g` changes the choice (Automatic, Software, Hardware), `t` tries the card again. True when the key was one of these.
    pub(super) fn handle_browser_graphics(&mut self, k: &Key, cx: &AppCx) -> bool {
        if matches!(k, Key::Char('G') | Key::Char('C')) {
            return self.handle_browser_accel(k, cx);
        }
        if !matches!(k, Key::Char('g') | Key::Char('t')) {
            return false;
        }
        if !cx.session.policy.manage_settings {
            self.fail("Only an administrator changes the browser's graphics mode.");
            return true;
        }
        let data = cx.data_dir();
        let what = match k {
            Key::Char('g') => {
                let next = cx.agent.web_graphics().load_mode(data).0.next();
                match cx.agent.web_graphics().set_mode(data, next) {
                    Ok(()) => format!("Browser graphics: {}. It applies the next time the browser engine starts.", next.label()),
                    Err(e) => {
                        self.fail(e);
                        return true;
                    }
                }
            }
            _ => {
                let st = cx.agent.web_graphics().status(data);
                match cx.agent.web_graphics().try_hardware_again(data, &st.gpu) {
                    Ok(()) => "Browser graphics: trying the graphics card again. It applies the next time the browser engine starts.".to_string(),
                    Err(e) => {
                        self.fail(e);
                        return true;
                    }
                }
            }
        };
        cx.audit.record(&cx.session.user, "settings.browser", &what);
        self.ok(what);
        true
    }

    /// `G` turns graphics acceleration off or on for this person, `C` (administrators) for the whole computer. It applies the next time the engine starts.
    fn handle_browser_accel(&mut self, k: &Key, cx: &AppCx) -> bool {
        let dir = cx.data_dir().join("web-profile");
        let (mut pol, _) = iyakki_shared::webaccel::load(&dir);
        let me = iyakki_shared::webengine::safe_name(&cx.session.user);
        let what = if matches!(k, Key::Char('C')) {
            if !cx.session.policy.manage_settings {
                self.fail("Only an administrator changes this for the whole computer.");
                return true;
            }
            let off = !pol.admin_off;
            pol.set_admin_off(off);
            format!("Browser graphics acceleration {} for the computer", if off { "off" } else { "on" })
        } else {
            let off = !pol.people.get(&me).is_some_and(|p| p.off);
            pol.set_person_off(&me, off);
            format!("Browser graphics acceleration {} for the person", if off { "off" } else { "on" })
        };
        if let Err(e) = iyakki_shared::webaccel::save(&dir, &pol) {
            self.fail(e);
            return true;
        }
        cx.audit.record(&cx.session.user, "settings.browser", &what);
        self.ok(format!("{what}. It applies the next time the browser engine starts."));
        true
    }
}
