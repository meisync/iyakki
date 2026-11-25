//! Settings -> Browser -> "Check a site's certificate" (key K, F-BROW-98): type a site's address and see the certificate it presents, as the
//! assistant's `browser_certificate_view` tool shows it: who it was issued to and by, its dates, its fingerprint, whether its names fit and
//! whether it leads to a trusted authority, with warnings. It connects to the site and does only the TLS handshake (nothing is requested).
//! The probe runs on a job thread; the answer opens as a list of lines (Esc closes it). The account's host rules apply as for pages.

use super::*;
use super::browser_port::BrowserSettingsPort;

impl SettingsApp {
    pub(super) fn render_browser_cert(&mut self, g: &mut Gfx, a: R, y0: f32, s: f32, _p: &dyn BrowserSettingsPort) -> f32 {
        let rh = 56.0 * s;
        if y0 + rh > a.y + a.h - 4.0 * s {
            if y0 + 22.0 * s < a.y + a.h {
                g.text(a.x + 4.0 * s, y0, 13.0 * s, DIM, "Check a site's certificate: press K.", Face::Sans);
            }
            return 22.0 * s;
        }
        let bw = g.text_width(15.0 * s, "Check (K)", Face::Bold) + 15.0 * s * 1.7;
        controls::info_row(g, R::new(a.x, y0, a.w - bw - 10.0 * s, rh), s, "Check a site's certificate", "Shows who a site's certificate was issued to and by, its dates and whether it can be trusted. Only the TLS handshake is made; nothing is requested from the site.");
        g.button(a.x + a.w - bw, y0 + 10.0 * s, 15.0 * s, "Check (K)", ACCENT, Event::Key(Key::Char('K')));
        rh + 10.0 * s
    }

    /// K of the Browser tab.
    pub(super) fn handle_browser_cert(&mut self, k: &Key, _p: &dyn BrowserSettingsPort) -> bool {
        if !matches!(k, Key::Char('K')) {
            return false;
        }
        self.open_form(FormKind::BrowserCert, Form::new("Certificate of a site").text("Address", "", "for example www.example.org or example.org:8443"));
        true
    }

    /// The typed address as (host, port); anything else is refused with the reason.
    pub(super) fn cert_target(text: &str) -> Result<(String, u16), String> {
        let t = text.trim().trim_start_matches("https://").trim_end_matches('/').trim_end_matches('.');
        let (host, port) = match t.rsplit_once(':') {
            Some((h, p)) => (h, p.parse::<u16>().ok().filter(|p| *p > 0).ok_or("The port must be 1 to 65535")?),
            None => (t, 443),
        };
        let host = host.to_ascii_lowercase();
        if host.is_empty() || host.len() > 253 || !host.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '.' | '_')) {
            return Err("That is not a site name (write it like www.example.org)".into());
        }
        Ok((host, port))
    }

    pub(super) fn browser_cert_submit(&mut self, p: &dyn BrowserSettingsPort, form: &Form) -> Result<String, String> {
        let (host, port) = Self::cert_target(&form.value(0))?;
        if !p.acting_policy().allows_host(&host) {
            return Err(format!("Your account may not open pages from {host}."));
        }
        p.audit("browser.certificate_view", &host);
        self.start("Reading the certificate", move || JobOut::Cert(crate::apps::chat_settings::cert_probe(&host, port).map(|v| v.text().lines().map(str::to_string).collect())));
        Ok("Asking the site for its certificate.".into())
    }

    pub(super) fn cert_job_done(&mut self, r: Result<Vec<String>, String>) {
        match r {
            Ok(lines) => self.modal = Some(Modal::Pick(PickKind::CertView, Picker::new("Certificate (Esc closes)", lines))),
            Err(e) => self.fail(e),
        }
    }
}
