//! Settings -> Browser -> "Proxy" (F-BROX-22): the HTTP proxy the browser's pages go through (`host:port`, a CONNECT tunnel, TLS
//! stays end to end), on the page and not only through the assistant. Key N opens a form (empty turns the proxy off). It uses the same
//! functions as the chat tools (`iyakki_proxy::proxyset`), is audited with the proxy address only, and applies the next time the
//! browser starts. No sign-in is kept (D-BROXY-AUTH): a proxy that wants a name and a password is refused in words.

use iyakki_proxy::proxyset;

use super::*;
use super::browser_port::BrowserSettingsPort;

impl SettingsApp {
    pub(super) fn render_browser_proxy(&mut self, g: &mut Gfx, a: R, y0: f32, s: f32, p: &dyn BrowserSettingsPort) -> f32 {
        let t = proxyset::text(p.data_root());
        let state = if t.is_empty() { "none (direct)".to_string() } else { t };
        let rh = 70.0 * s;
        if y0 + rh > a.y + a.h - 4.0 * s {
            if y0 + 22.0 * s < a.y + a.h {
                g.text(a.x + 4.0 * s, y0, 13.0 * s, DIM, &format!("Browser proxy: {state}. N changes it."), Face::Sans);
            }
            return 22.0 * s;
        }
        let help = format!("Pages go through this HTTP proxy (host:port): it sees which sites you visit, not what is in them. Now: {state}. Applies the next time the browser starts.");
        g.text(a.x + 4.0 * s, y0, 15.0 * s, TEXT, "Browser proxy", Face::Sans);
        g.text(a.x + 4.0 * s, y0 + 22.0 * s, 13.0 * s, DIM, &help, Face::Sans);
        g.button(a.x + 4.0 * s, y0 + 42.0 * s, 15.0 * s, "Change (N)", ACCENT, Event::Key(Key::Char('N')));
        rh + 10.0 * s
    }

    /// N of the Browser tab.
    pub(super) fn handle_browser_proxy(&mut self, k: &Key, p: &dyn BrowserSettingsPort) -> bool {
        if !matches!(k, Key::Char('N')) {
            return false;
        }
        let now = proxyset::text(p.data_root());
        self.open_form(FormKind::BrowserProxy, Form::new("Browser proxy").text("Proxy", &now, "host:port, for example proxy.example:3128; empty turns it off"));
        true
    }

    pub(super) fn browser_proxy_submit(&mut self, p: &dyn BrowserSettingsPort, form: &Form) -> Result<String, String> {
        let kept = proxyset::save(p.data_root(), form.value(0).trim())?;
        if kept.is_empty() {
            p.audit("settings.browser", "Browser proxy: off");
            Ok("The browser connects to sites directly; it applies the next time the browser starts.".into())
        } else {
            p.audit("settings.browser", &format!("Browser proxy: {kept}"));
            Ok(format!("The browser will use the proxy {kept} from the next time it starts."))
        }
    }
}
