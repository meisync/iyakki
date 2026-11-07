//! The one-time question about the time zone: after an administrator signs in and the machine has a network, a card asks whether to set
//! the zone from the network (Settings -> Time "Detect automatically", which asks an internet service and shows your public IP address
//! to it), to use UTC, to choose a zone by hand, or to leave it. It is asked once; whatever the answer, it is remembered
//! (`TimeSettings::zone_asked`) and every choice stays in Settings -> Time. Nothing is sent until the person presses the first button.

use super::*;

pub(super) const AUTO: usize = 0x4D40;
pub(super) const UTC: usize = 0x4D41;
pub(super) const PICK: usize = 0x4D42;
pub(super) const LATER: usize = 0x4D43;

#[derive(Default)]
pub(super) struct TzAsk {
    pub open: bool,
}

pub(super) fn is_click(n: usize) -> bool {
    (AUTO..=LATER).contains(&n)
}

/// Whether the question is due: an administrator, never asked, no zone chosen and no automatic detection, and a network.
pub(super) fn due(t: &iyakki_agent::settings::TimeSettings, admin: bool, online: bool) -> bool {
    admin && online && !t.zone_asked && !t.zone_auto && t.zone.trim().is_empty()
}

impl Shell {
    pub(super) fn tzask_tick(&mut self) {
        // not while the screen saver or the dim is showing (the card would be waiting on a desktop nobody sees)
        if self.tzask.open || self.tick % 60 != 20 || self.screen_stage() != crate::saver::Stage::Awake {
            return;
        }
        let admin = self.session.as_ref().is_some_and(|s| s.policy.manage_settings);
        let t = self.env.services.settings.lock().map(|s| s.get().time.clone()).unwrap_or_default();
        let online = crate::loginbar::network_chip(self.devpop.snapshot().as_ref()).1;
        if due(&t, admin, online) {
            self.tzask.open = true;
            self.stale = true;
        }
    }

    pub(super) fn tzask_event(&mut self, id: usize) {
        let Some(user) = self.session.as_ref().map(|s| s.user.clone()) else { return };
        self.tzask.open = false;
        self.stale = true;
        let save = |sh: &mut Shell, f: &dyn Fn(&mut iyakki_agent::settings::TimeSettings)| {
            let _ = sh.env.services.settings.lock().map(|mut s| s.update(|st| f(&mut st.time)));
        };
        match id {
            AUTO => {
                save(self, &|t| {
                    t.zone_asked = true;
                    t.zone_auto = true;
                });
                self.env.audit.record(&user, "time.zone.auto", "asked at first network");
                iyakki_agent::tzdetect::refresh_in_background(&self.env.services, true, Arc::new(|z: &str| iyakki_agent::clock::set_chosen_zone(z)));
                self.say_info("Looking up the time zone from this network. Settings, Time shows the result and can turn it off.");
            }
            UTC => {
                save(self, &|t| {
                    t.zone_asked = true;
                    t.zone_auto = false;
                    t.zone = "UTC".into();
                });
                // (the zone of the whole process: tests leave it alone, other tests compare clock text)
                if !cfg!(test) {
                    iyakki_agent::clock::set_chosen_zone("UTC");
                }
                self.env.audit.record(&user, "time.zone.set", "UTC");
                self.say_info("The time zone is UTC. Settings, Time can change it.");
            }
            PICK => {
                save(self, &|t| t.zone_asked = true);
                self.say_info("Choose the time zone in Settings, Time.");
            }
            _ => {
                save(self, &|t| t.zone_asked = true);
                self.say_info("The time zone stays as it is. Settings, Time can change it.");
            }
        }
    }

    pub(super) fn draw_tzask(&mut self, g: &mut Gfx, w: u32, h: u32, s: f32) {
        if !self.tzask.open || self.session.is_none() {
            return;
        }
        let bw = (420.0 * s).min(w as f32 - 16.0);
        let bh = 178.0 * s;
        let (x, y) = (w as f32 - bw - 12.0 * s, h as f32 - bh - 12.0 * s);
        crate::fx::modal_panel(g, R::new(x, y, bw, bh), s, crate::fx::CYAN, false);
        g.text(x + 18.0 * s, y + 14.0 * s, 17.0 * s, TEXT, "Set the time zone from the network?", Face::Bold);
        let line = g.fit(13.0 * s, "Detect automatically asks ipapi.co which zone your public IP address is in. That service sees the address; nothing else is sent.", Face::Sans, bw - 36.0 * s);
        g.text(x + 18.0 * s, y + 44.0 * s, 13.0 * s, MUTED, &line, Face::Sans);
        let (mut bx, by) = (x + 18.0 * s, y + 96.0 * s);
        for (label, id, accent) in [("Detect automatically", AUTO, crate::fx::CYAN), ("Use UTC", UTC, TEXT)] {
            bx += g.button(bx, by, 14.0 * s, label, accent, Event::Tab(id)) + 10.0 * s;
        }
        let by2 = by + 40.0 * s;
        let mut bx = x + 18.0 * s;
        for (label, id) in [("Choose in Settings", PICK), ("Not now", LATER)] {
            bx += g.button(bx, by2, 14.0 * s, label, MUTED, Event::Tab(id)) + 10.0 * s;
        }
    }
}
