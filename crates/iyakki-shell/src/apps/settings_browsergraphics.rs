//! Settings -> Browser, "Graphics": how the browser engine draws. Automatic (the graphics card, software by itself after the computer
//! stopped right after the engine started on it), Software (safe) or Hardware (never falls back). The state is shown in plain words
//! ("Automatic: hardware", "Automatic: software because the last session stopped ..."). `g` changes the choice, `t` tries the card
//! again after an automatic fall back; both are buttons too. The logic is in `iyakki_agent::webgraphics`; a change applies the next
//! time the engine starts. Only administrators change it (it is a device setting); everybody sees it.

use super::*;
use iyakki_agent::webgraphics;

impl SettingsApp {
    /// The section; returns the height it used. Buttons only where they fit, otherwise a line that says where the keys are.
    pub(super) fn render_browser_graphics(&mut self, g: &mut Gfx, a: R, y0: f32, s: f32, cx: &AppCx) -> f32 {
        let st = webgraphics::status(cx.services.dir());
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
        y + 6.0 * s - y0
    }

    /// `g` changes the choice (Automatic, Software, Hardware), `t` tries the card again. True when the key was one of these.
    pub(super) fn handle_browser_graphics(&mut self, k: &Key, cx: &AppCx) -> bool {
        if !matches!(k, Key::Char('g') | Key::Char('t')) {
            return false;
        }
        if !cx.session.policy.manage_settings {
            self.fail("Only an administrator changes the browser's graphics mode.");
            return true;
        }
        let data = cx.services.dir();
        let what = match k {
            Key::Char('g') => {
                let next = webgraphics::load_mode(data).0.next();
                match webgraphics::set_mode(data, next) {
                    Ok(()) => format!("Browser graphics: {}. It applies the next time the browser engine starts.", next.label()),
                    Err(e) => {
                        self.fail(e);
                        return true;
                    }
                }
            }
            _ => {
                let st = webgraphics::status(data);
                match webgraphics::try_hardware_again(data, &st.gpu) {
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
}
