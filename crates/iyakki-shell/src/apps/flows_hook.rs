//! FLOWS, the Webhook panel of the open flow (F-FLOW-15): the flow's webhooks, whether each is on, the last 20 deliveries, and the
//! buttons that make one, make its address again, switch it on or off and open it to the public for an hour. A new secret address
//! (and shared secret) is shown here **once**, in this panel only; closing the panel forgets it, and the assistant never sees it.

use iyakki_agent::flows::hookenv::{Created, HookOpts, HookView};
use iyakki_agent::flows::hookstore::Reveal;

use super::flows::*;
use super::flows_view::{ago_text, GfxFit};
use crate::fx::{self, AMBER, CYAN};
use crate::gfx::{Face, Gfx};
use crate::ui::{Event, Key, DIM, ERR, MUTED, OK, R, TEXT};

pub(super) struct HookPanel {
    pub views: Vec<HookView>,
    pub sel: usize,
    /// The address and secret just made, shown until the panel closes.
    pub reveal: Option<Reveal>,
    pub note: String,
    pub note_ok: bool,
    /// `p` was pressed: waiting for `y` to open the selected hook to the public for an hour.
    pub ask_public: bool,
}

impl HookPanel {
    pub(super) fn selected(&self) -> Option<&HookView> {
        self.views.get(self.sel)
    }
}

impl FlowsApp {
    pub(super) fn open_hooks(&mut self) {
        let Some(id) = self.cur.clone() else { return };
        let reveal = iyakki_agent::flows::hookstore::take_reveal(&self.env.user);
        let views = self.env.hook_views(Some(&id));
        self.modal = Some(Modal::Webhook(HookPanel { sel: views.len().saturating_sub(1), views, reveal, note: String::new(), note_ok: true, ask_public: false }));
    }

    fn shown(&self, c: Created) -> Reveal {
        Reveal { hook: c.id, flow: c.flow, path: c.path, secret: c.secret }
    }

    /// Keys of the panel; `false` closes it.
    pub(super) fn hook_event(&mut self, p: &mut HookPanel, ev: &Event) -> bool {
        let flow = self.cur.clone().unwrap_or_default();
        let who = self.env.who();
        let done = |p: &mut HookPanel, r: Result<String, String>| match r {
            Ok(m) => {
                p.note = m;
                p.note_ok = true;
            }
            Err(e) => {
                p.note = e;
                p.note_ok = false;
            }
        };
        let was_ask = std::mem::take(&mut p.ask_public);
        match ev {
            Event::Key(Key::Esc) => return false,
            Event::Key(Key::Up) if p.sel > 0 => p.sel -= 1,
            Event::Key(Key::Down) if p.sel + 1 < p.views.len() => p.sel += 1,
            Event::Row(i) if *i < p.views.len() => p.sel = *i,
            Event::Key(Key::Char(k @ ('n' | 'N'))) => {
                let signed = *k == 'n';
                let r = self.env.hook_create(&flow, &HookOpts { signed, ..Default::default() }, &who).map(|c| {
                    p.reveal = Some(self.shown(c));
                    format!("Made a webhook{}. It is OFF: publish the flow, then switch it on (o).", if signed { "" } else { " that is NOT signed: the secret address is its only protection" })
                });
                done(p, r);
            }
            Event::Key(Key::Char('r')) => {
                let id = p.selected().map(|h| h.id.clone());
                let r = match id {
                    Some(id) => self.env.hook_regenerate(&id, &who).map(|c| {
                        p.reveal = Some(self.shown(c));
                        "Made the address new. The old one stopped working.".to_string()
                    }),
                    None => Err("Nothing selected.".into()),
                };
                done(p, r);
            }
            Event::Key(Key::Char('o')) => {
                let cur = p.selected().map(|h| (h.id.clone(), h.enabled));
                let r = match cur {
                    Some((id, true)) => self.env.hook_disable(&id, &who).map(|()| "Switched off: it answers 404.".to_string()),
                    Some((id, false)) => self.env.hook_enable(&id, &who).map(|()| "Switched on.".to_string()),
                    None => Err("Nothing selected.".into()),
                };
                done(p, r);
            }
            Event::Key(Key::Char('p')) => match p.selected().map(|h| (h.id.clone(), h.public_until > iyakki_agent::clock::now_unix())) {
                Some((id, true)) => {
                    let r = self.env.hook_public(&id, 0, &who).map(|_| "Closed to the public: local network only.".to_string());
                    done(p, r);
                }
                Some(_) => {
                    p.ask_public = true;
                    p.note = "Open to senders OUTSIDE your local network for 1 hour? Press y to open it, anything else keeps it local.".into();
                    p.note_ok = false;
                }
                None => done(p, Err("Nothing selected.".into())),
            },
            Event::Key(Key::Char('y')) if was_ask => {
                let id = p.selected().map(|h| h.id.clone());
                let r = match id {
                    Some(id) => self.env.hook_public(&id, 60, &who).map(|_| "Open to the public for 1 hour (the firewall still decides what reaches the web port).".to_string()),
                    None => Err("Nothing selected.".into()),
                };
                done(p, r);
            }
            _ => {}
        }
        let keep_sel = p.selected().map(|h| h.id.clone());
        p.views = self.env.hook_views(Some(&flow));
        if let Some(id) = keep_sel {
            if let Some(i) = p.views.iter().position(|h| h.id == id) {
                p.sel = i;
            }
        }
        p.sel = p.sel.min(p.views.len().saturating_sub(1));
        true
    }

    pub(super) fn draw_hooks(&self, p: &HookPanel, g: &mut Gfx, area: R, s: f32) {
        let m = fx::modal(g, area, s, 800.0 * s, 540.0 * s, ACCENT, false);
        let y0 = fx::modal_title(g, m, s, "Webhooks of this flow", ACCENT);
        let x = m.x + 22.0 * s;
        let w = m.w - 44.0 * s;
        let mut y = y0;
        let now = iyakki_agent::clock::now_unix();
        if let Some(r) = &p.reveal {
            g.text(x, y, 12.0 * s, AMBER, "Shown once. Copy it now: closing this panel forgets it, and nobody else (not the assistant) has seen it.", Face::Bold);
            y += 18.0 * s;
            g.text_fit(x, y, 11.0 * s, MUTED, "Address (https://<this computer>:<web port>, the same port as Settings, Remote access):", Face::Sans, w);
            y += 15.0 * s;
            // the 64 hex digits of the token are drawn whole, on their own line
            let (head, tail) = r.path.rsplit_once('/').unwrap_or(("", &r.path));
            g.text_fit(x, y, 11.5 * s, TEXT, &format!("{head}/"), Face::Mono, w);
            y += 15.0 * s;
            g.text_fit(x, y, 11.5 * s, CYAN, tail, Face::Mono, w);
            y += 17.0 * s;
            if let Some(sec) = &r.secret {
                g.text_fit(x, y, 11.0 * s, MUTED, "Shared secret for X-Iyakki-Signature: sha256=HMAC-SHA256(\"<timestamp>.<body>\"), with X-Iyakki-Timestamp:", Face::Sans, w);
                y += 15.0 * s;
                g.text_fit(x, y, 11.5 * s, CYAN, sec, Face::Mono, w);
                y += 20.0 * s;
            }
        }
        let list_w = 300.0 * s;
        let row = 34.0 * s;
        if p.views.is_empty() {
            g.text_fit(x, y + 4.0 * s, 12.0 * s, DIM, "This flow has no webhook yet. n makes one (signed; N: not signed). It is off until you switch it on.", Face::Sans, w);
        }
        for (i, h) in p.views.iter().enumerate().take(5) {
            let rr = R::new(x, y + i as f32 * row, list_w, row - 4.0 * s);
            fx::list_row(g, rr, s, i == p.sel, g.hovered(rr), ACCENT);
            let state = if h.enabled { "ON" } else { "off" };
            g.text_fit(rr.x + 8.0 * s, rr.y + 3.0 * s, 12.0 * s, if h.enabled { OK } else { TEXT }, &format!("{} · {state}{}", h.id, if h.signed { " · signed" } else { " · not signed" }), Face::Bold, rr.w - 16.0 * s);
            let sub = if !h.published { "publish the flow to make it live".to_string() } else if h.public_until > now { format!("open to the public for {} more min", (h.public_until - now + 59) / 60) } else { "local network only".to_string() };
            g.text_fit(rr.x + 8.0 * s, rr.y + 18.0 * s, 10.5 * s, if h.public_until > now { AMBER } else { DIM }, &sub, Face::Sans, rr.w - 16.0 * s);
            g.hit(rr, Event::Row(i), &h.id);
        }
        // the last 20 deliveries of the selected hook
        let dx = x + list_w + 16.0 * s;
        let dw = m.x + m.w - 22.0 * s - dx;
        if let Some(h) = p.selected() {
            g.text(dx, y, 11.5 * s, MUTED, &format!("Last {} deliveries", h.deliveries.len()), Face::Bold);
            let max = ((m.y + m.h - 130.0 * s - y) / (14.0 * s)).max(1.0) as usize;
            let mut dy = y + 18.0 * s;
            for d in h.deliveries.iter().rev().take(max.min(20)) {
                let c = if d.outcome == "accepted" { OK } else if matches!(d.outcome.as_str(), "rate_limited" | "too_large" | "stale") { AMBER } else { ERR };
                g.text_fit(dx, dy, 11.0 * s, c, &format!("{:<14} {} {:>10}  {} B", d.outcome.replace('_', " "), d.http, ago_text((now - d.at) * 1000), d.bytes), Face::Mono, dw);
                dy += 14.0 * s;
            }
        }
        if !p.note.is_empty() {
            g.text_fit(x, m.y + m.h - 84.0 * s, 11.5 * s, if p.note_ok { OK } else { ERR }, &p.note, Face::Sans, w);
        }
        let by = m.y + m.h - 46.0 * s;
        g.button(x, by, 12.0 * s, "New (n)", ACCENT, Event::Key(Key::Char('n')));
        g.button(x + 100.0 * s, by, 12.0 * s, "New address (r)", AMBER, Event::Key(Key::Char('r')));
        g.button(x + 260.0 * s, by, 12.0 * s, if p.selected().is_some_and(|h| h.enabled) { "Switch off (o)" } else { "Switch on (o)" }, OK, Event::Key(Key::Char('o')));
        g.button(x + 420.0 * s, by, 12.0 * s, if p.selected().is_some_and(|h| h.public_until > now) { "Close to public (p)" } else { "Open to public (p)" }, AMBER, Event::Key(Key::Char('p')));
        g.button(x + 620.0 * s, by, 12.0 * s, "Close (Esc)", MUTED, Event::Key(Key::Esc));
    }
}
