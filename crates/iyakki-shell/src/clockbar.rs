//! The clock in the top bar: the time, the date beside it when there is room, and a tooltip with the full date, the time
//! zone and a warning when the machine's clock was never set (a Mac mini without its clock battery starts in 1970).
//! The same code draws it in the framebuffer console, the desktop window and the browser, because they all show the shell's
//! picture. All the text comes from [`iyakki_agent::clock`], which finds the zone without any zone files on the machine.

use iyakki_agent::clock::{self, Zone};

/// What the person chose in Settings -> Time, and the zone it resolves to.
pub struct ClockState {
    zone: Zone,
    zone_name: Option<String>,
    pub hour12: bool,
    pub seconds: bool,
    loaded_at: Option<u64>,
    /// A fixed moment instead of the machine's clock (tests).
    pub fixed: Option<i64>,
}

impl Default for ClockState {
    fn default() -> ClockState {
        ClockState { zone: Zone::utc(), zone_name: None, hour12: false, seconds: false, loaded_at: None, fixed: None }
    }
}

/// How the clock fits: what is drawn at a given width.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Fit {
    /// Time and date.
    Full,
    /// Only the time.
    TimeOnly,
    /// No room: hidden (the date is still in the Devices-style tooltip of nothing; the clock page is in Settings).
    Hidden,
}

/// Choose what to draw from the room there is and how wide each part is.
pub fn fit(avail: f32, time_w: f32, date_w: f32, gap: f32) -> Fit {
    if avail >= time_w.max(date_w) && avail >= time_w + gap + date_w {
        Fit::Full
    } else if avail >= time_w {
        Fit::TimeOnly
    } else {
        Fit::Hidden
    }
}

impl ClockState {
    /// Read the settings again (cheap; the shell calls it a few times a second).
    pub fn refresh(&mut self, tick: u64, settings: Option<&iyakki_agent::settings::TimeSettings>, person: Option<(bool, bool)>) {
        if let Some((h12, secs)) = person {
            self.hour12 = h12;
            self.seconds = secs;
        }
        if self.loaded_at.is_some_and(|t| tick.saturating_sub(t) < 15) {
            return;
        }
        self.loaded_at = Some(tick);
        if let Some(t) = settings {
            if self.zone_name.as_deref() != Some(t.zone.as_str()) {
                self.zone_name = Some(t.zone.clone());
                // the chosen zone, else the machine's own (`TZ`, `/etc/localtime`, UTC)
                self.zone = Zone::system_with((!t.zone.is_empty()).then(|| t.zone.clone()));
            }
        }
    }

    /// Use the zone the machine itself has (no choice made yet).
    pub fn use_system_zone(&mut self) {
        self.zone = Zone::system();
    }

    pub fn now(&self) -> i64 {
        self.fixed.unwrap_or_else(clock::now_unix)
    }

    pub fn time_text(&self) -> String {
        clock::bar_time(&self.zone, self.now(), self.hour12, self.seconds)
    }

    pub fn date_text(&self) -> String {
        clock::bar_date(&self.zone, self.now(), false)
    }

    /// The lines of the tooltip: the long date, the zone, and what is wrong with the clock, if anything.
    pub fn tooltip(&self) -> Vec<(String, bool)> {
        let now = self.now();
        let l = self.zone.local(now);
        let mut out = vec![(clock::bar_date(&self.zone, now, true), false)];
        let off = l.offset;
        let zone = match self.zone_name.as_deref() {
            Some(n) if !n.is_empty() => format!("{n} ({})", l.abbrev),
            _ => l.abbrev.clone(),
        };
        out.push((format!("{zone} · UTC{}{:02}:{:02}", if off < 0 { '-' } else { '+' }, off.abs() / 3600, off.abs() % 3600 / 60), false));
        if clock::looks_wrong(now) {
            out.push(("The clock looks wrong: this computer has not set its time (no clock battery, or no network yet).".to_string(), true));
        }
        let st = iyakki_agent::ntp::status();
        if !st.message.is_empty() {
            out.push((st.message, st.pending_back.is_some()));
        }
        out.push(("Click to open Settings, Time.".to_string(), false));
        out
    }

    pub fn looks_wrong(&self) -> bool {
        clock::looks_wrong(self.now())
    }
}
