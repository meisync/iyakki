//! Settings → Cloud mode: whether this machine hides what needs a local screen, speakers or camera (`iyakki_agent::cloudmode`).
//! Automatic (the default), On or Off, kept in the machine's settings (administrators). The page says what the shell decided,
//! why, and what is hidden. The mode is read when someone signs in; the sound and camera services follow it at the next start.

use super::*;

impl SettingsApp {
    pub(super) fn handle_cloud(&mut self, k: &Key, cx: &AppCx) {
        if matches!(k, Key::Char('a')) && self.admin(cx) {
            let svc = Self::svc(cx);
            let next = !svc.show_all_ai();
            match svc.set_show_all_ai(next) {
                Ok(()) => {
                    cx.audit.record(&cx.session.user, "settings.show_all_ai", if next { "on" } else { "off" });
                    self.ok(format!("Show all AI tools: {}. {}", if next { "on" } else { "off" }, if next { "Tools this machine's hardware does not suit are listed as not recommended." } else { "Tools this machine's hardware does not suit are hidden again." }));
                }
                Err(e) => self.fail(e),
            }
            return;
        }
        if !matches!(k, Key::Char('c') | Key::Enter | Key::Char(' ')) || !self.admin(cx) {
            return;
        }
        let svc = Self::svc(cx);
        let next = svc.cloud_setting().next();
        match svc.set_cloud_setting(next) {
            Ok(()) => {
                cx.audit.record(&cx.session.user, "settings.cloud", next.label());
                self.ok(format!("Cloud mode: {}. The menus follow it when someone signs in; the sound and camera services at the next start.", next.label()));
            }
            Err(e) => self.fail(e),
        }
    }

    /// (label, value) rows of the page.
    pub(super) fn cloud_rows(&self, cx: &AppCx) -> Vec<(String, String)> {
        let c = &self.cloud;
        let join = |v: Vec<String>| if v.is_empty() { "nothing".to_string() } else { v.join(", ") };
        vec![
            ("Setting".into(), Self::svc(cx).cloud_setting().label().into()),
            ("Mode now".into(), if c.on { "Cloud mode is on".into() } else { "Desktop (cloud mode is off)".to_string() }),
            ("Why".into(), c.why.clone()),
            ("Hidden apps".into(), join(c.hidden_apps.iter().map(|a| a.label()).collect())),
            ("Hidden Settings pages".into(), join(c.hidden_settings_pages.iter().map(|s| s.to_string()).collect())),
            ("Hidden Devices rows".into(), join(c.hidden_device_rows.iter().map(|s| s.to_string()).collect())),
            ("Fn and media keys".into(), if c.fn_keys_off { "off".into() } else { "on".to_string() }),
            ("This machine (AI)".into(), iyakki_agent::aiprofile::this_machine(iyakki_agent::modelhost::Hosts::new(Self::svc(cx).dir()).list().len()).summary()),
            ("Show all AI tools".into(), format!("{} (press A to change). AI tools this hardware does not suit are hidden unless this is on.", if Self::svc(cx).show_all_ai() { "On" } else { "Off" })),
            ("Boot parameter".into(), "iyakki.mode=cloud or iyakki.mode=desktop beats the setting".into()),
        ]
    }

    pub(super) fn render_cloud(&mut self, g: &mut Gfx, a: R, s: f32, cx: &AppCx) {
        let mut y = a.y;
        let label_w = (210.0 * s).min(a.w * 0.38);
        for (label, value) in self.cloud_rows(cx) {
            g.text(a.x, y, 14.0 * s, MUTED, &label, Face::Sans);
            let lines = g.wrap(14.0 * s, &value, Face::Bold, a.w - label_w);
            for (i, l) in lines.iter().take(3).enumerate() {
                g.text(a.x + label_w, y + i as f32 * 18.0 * s, 14.0 * s, TEXT, l, Face::Bold);
            }
            y += 24.0 * s + lines.len().clamp(1, 3).saturating_sub(1) as f32 * 18.0 * s;
        }
    }
}

