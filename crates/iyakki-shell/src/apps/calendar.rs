//! CALENDAR: the tile itself lives in the `iyakki-tile-calendar` crate (it does not depend on the agent). This file is the shell's side of
//! it: the real alarm and event store plus the machine's time zone behind the tile's `CalendarSource`, the conversion of the agent's
//! shapes, the audit sink, and the adapter that makes the tile crate's `App` an `App` of the shell. See
//! docs/development/COMPILE-SPLIT-PILOT.md ("Third step").

use iyakki_agent::calendar as agent;
use iyakki_agent::clock::Zone;
use iyakki_tile_api::calendar as api;
use iyakki_tile_api::{App as TileApp, AuditSink, TileCx};

use super::usage::ShellUsage;
use super::{App, AppCx, Flow};
use crate::gfx::{Gfx, Icon};
use crate::ui::{Event, R};

pub use iyakki_tile_calendar::Tab;

/// The real calendar store, in the machine's time zone.
pub struct ShellCalendar<'a> {
    pub store: &'a agent::CalendarStore,
    pub zone: &'a Zone,
}

fn repeat_to_api(r: agent::Repeat) -> api::Repeat {
    match r {
        agent::Repeat::None => api::Repeat::None,
        agent::Repeat::Daily => api::Repeat::Daily,
        agent::Repeat::Weekly => api::Repeat::Weekly,
        agent::Repeat::Monthly => api::Repeat::Monthly,
        agent::Repeat::Yearly => api::Repeat::Yearly,
    }
}

fn repeat_to_agent(r: api::Repeat) -> agent::Repeat {
    match r {
        api::Repeat::None => agent::Repeat::None,
        api::Repeat::Daily => agent::Repeat::Daily,
        api::Repeat::Weekly => agent::Repeat::Weekly,
        api::Repeat::Monthly => agent::Repeat::Monthly,
        api::Repeat::Yearly => agent::Repeat::Yearly,
    }
}

fn alarm_from_agent(a: agent::Alarm) -> api::Alarm {
    api::Alarm { id: a.id, label: a.label, hour: a.hour, minute: a.minute, days: a.days, enabled: a.enabled, by: a.by }
}

fn alarm_occ_from_agent(a: agent::AlarmOccurrence) -> api::AlarmOccurrence {
    api::AlarmOccurrence { id: a.id, label: a.label, at: a.at, days: a.days, enabled: a.enabled }
}

impl api::CalendarSource for ShellCalendar<'_> {
    fn local(&self, unix: i64) -> api::Local {
        let l = self.zone.local(unix);
        api::Local { year: l.year, month: l.month, day: l.day, hour: l.hour, minute: l.minute, second: l.second, weekday: l.weekday, offset: l.offset, abbrev: l.abbrev }
    }
    fn local_to_unix(&self, y: i64, m: u32, d: u32, hh: u32, mm: u32, ss: u32) -> i64 {
        self.zone.local_to_unix(y, m, d, hh, mm, ss)
    }
    fn events_in(&self, user: &str, from: i64, to: i64) -> Vec<api::Occurrence> {
        self.store
            .events_in(user, from, to, self.zone)
            .into_iter()
            .map(|o| api::Occurrence { id: o.id, title: o.title, start: o.start, end: o.end, all_day: o.all_day, repeat: repeat_to_api(o.repeat), notes: o.notes, by: o.by, remind: o.remind })
            .collect()
    }
    fn alarms_in(&self, user: &str, from: i64, to: i64) -> Vec<api::AlarmOccurrence> {
        self.store.alarms_in(user, from, to, self.zone).into_iter().map(alarm_occ_from_agent).collect()
    }
    fn add_event(&self, user: &str, title: &str, start: i64, minutes: u32, all_day: bool, repeat: api::Repeat, notes: &str, by: &str, remind: Option<u32>) -> Result<api::Added, String> {
        self.store.add_event(user, title, start, minutes, all_day, repeat_to_agent(repeat), notes, by, remind).map(|e| api::Added { title: e.title, start: e.start })
    }
    fn remove_event(&self, user: &str, id: &str) -> Result<(), String> {
        self.store.remove_event(user, id)
    }
    fn alarms(&self, user: &str) -> Vec<api::Alarm> {
        self.store.alarms(user).into_iter().map(alarm_from_agent).collect()
    }
    fn add_alarm(&self, user: &str, label: &str, hour: u32, minute: u32, days: u8, now: i64, by: &str) -> Result<api::Alarm, String> {
        self.store.add_alarm(user, label, hour, minute, days, now, self.zone, by).map(alarm_from_agent)
    }
    fn remove_alarm(&self, user: &str, id: &str) -> Result<(), String> {
        self.store.remove_alarm(user, id)
    }
    fn set_alarm_enabled(&self, user: &str, id: &str, on: bool, now: i64) -> Result<(), String> {
        self.store.set_alarm_enabled(user, id, on, now, self.zone)
    }
    fn next_alarm(&self, user: &str, now: i64) -> Option<api::AlarmOccurrence> {
        self.store.next_alarm(user, now, self.zone).map(alarm_occ_from_agent)
    }
}

/// The shell's audit log behind the tile's sink.
pub struct ShellAudit<'a>(pub &'a crate::accounts::AuditLog);

impl AuditSink for ShellAudit<'_> {
    fn record(&self, user: &str, action: &str, subject: &str) {
        self.0.record(user, action, subject);
    }
}

/// The tile crate's app, as an app of the shell. The time zone is read once, when the tile opens, as before.
pub struct CalendarApp {
    inner: iyakki_tile_calendar::CalendarApp,
    zone: Zone,
}

impl CalendarApp {
    pub fn new(cx: &AppCx) -> CalendarApp {
        let zone = Zone::system();
        let inner = Self::with_zone(cx, &zone, iyakki_tile_calendar::CalendarApp::new);
        CalendarApp { inner, zone }
    }

    pub fn tab(&self) -> Tab {
        self.inner.tab()
    }

    pub fn selected_date(&self) -> (i64, u32, u32) {
        self.inner.selected_date()
    }

    fn with_zone<T>(cx: &AppCx, zone: &Zone, f: impl FnOnce(&TileCx) -> T) -> T {
        let usage = ShellUsage(&cx.services);
        let cal = ShellCalendar { store: &cx.services.calendar, zone };
        let audit = ShellAudit(&cx.audit);
        f(&TileCx { user: cx.session.user.as_str(), manage_settings: cx.session.policy.manage_settings, usage: &usage, calendar: &cal, audit: &audit })
    }
}

impl App for CalendarApp {
    fn title(&self) -> &'static str {
        self.inner.title()
    }
    fn icon(&self) -> Icon {
        self.inner.icon()
    }
    fn accent(&self) -> [u8; 4] {
        self.inner.accent()
    }
    fn hint(&self) -> String {
        self.inner.hint()
    }
    fn help(&self) -> Vec<String> {
        self.inner.help()
    }
    fn handle(&mut self, ev: &Event, cx: &AppCx) -> Flow {
        match Self::with_zone(cx, &self.zone, |t| self.inner.handle(ev, t)) {
            iyakki_tile_api::Flow::Stay => Flow::Stay,
            iyakki_tile_api::Flow::Back => Flow::Back,
        }
    }
    fn render(&mut self, g: &mut Gfx, area: R, s: f32, cx: &AppCx, tick: u64) {
        Self::with_zone(cx, &self.zone, |t| self.inner.render(g, area, s, t, tick));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::apps::tests::{cx_with_admin_store, services_in, temp_dir, Offline};
    use crate::ui::Key;
    use api::CalendarSource;
    use std::sync::Arc;

    fn admin_cx(tag: &str) -> AppCx {
        let mut cx = cx_with_admin_store(tag);
        let d = temp_dir(&format!("{tag}-svc"));
        cx.services = services_in(&d, Arc::new(Offline));
        cx.services.on_login("root", "rootpass12", true);
        cx
    }

    /// The shell's adapter: an event and an alarm added through the tile crate's form land in the real store (and the audit log); the
    /// real store's answers reach the tile; and the tile reaches the calendar through the shell's `App` trait.
    #[test]
    fn the_shell_adapter_connects_the_tile_to_the_real_calendar_and_audit_log() {
        let cx = admin_cx("calshell1");
        let mut a = CalendarApp::new(&cx);
        let zone = Zone::system();
        let src = ShellCalendar { store: &cx.services.calendar, zone: &zone };
        let now = iyakki_agent::clock::now_unix();
        let l = src.local(now);
        assert_eq!((l.year, l.month, l.day, l.hour, l.minute), {
            let z = zone.local(now);
            (z.year, z.month, z.day, z.hour, z.minute)
        });
        assert_eq!(src.local_to_unix(2030, 6, 3, 12, 30, 0), zone.local_to_unix(2030, 6, 3, 12, 30, 0));
        // through the source: add, read back converted, remove
        let added = src.add_event("root", "Team lunch", src.local_to_unix(2030, 6, 3, 12, 30, 0), 30, false, api::Repeat::Weekly, "", "user", Some(10)).unwrap();
        assert_eq!(added.title, "Team lunch");
        let day = src.events_in("root", src.local_to_unix(2030, 6, 10, 0, 0, 0), src.local_to_unix(2030, 6, 11, 0, 0, 0));
        assert_eq!((day.len(), day[0].repeat, day[0].remind), (1, api::Repeat::Weekly, Some(10)));
        assert_eq!(cx.services.calendar.events("root").len(), 1);
        src.add_alarm("root", "Stand up", 10, 0, 0x7f, now, "assistant").unwrap();
        let al = src.alarms("root");
        assert_eq!((al.len(), al[0].label.as_str(), al[0].days, al[0].by.as_str()), (1, "Stand up", 0x7f, "assistant"));
        assert!(src.next_alarm("root", now).is_some());
        src.set_alarm_enabled("root", &al[0].id, false, now).unwrap();
        assert!(!src.alarms("root")[0].enabled && src.next_alarm("root", now).is_none());
        // through the shell App trait: draws, handles keys, switches tab, and a delete is audited
        let mut g = Gfx::new(1200, 700);
        a.render(&mut g, R::new(0.0, 46.0, 1200.0, 654.0), 1.0, &cx, 0);
        assert_eq!(a.title(), "Calendar");
        assert!(matches!(a.handle(&Event::Key(Key::Tab), &cx), Flow::Stay));
        assert_eq!(a.tab(), Tab::Clock);
        a.render(&mut g, R::new(0.0, 46.0, 1200.0, 654.0), 1.0, &cx, 1);
        assert!(matches!(a.handle(&Event::Key(Key::Char('d')), &cx), Flow::Stay));
        assert!(matches!(a.handle(&Event::Key(Key::Char('y')), &cx), Flow::Stay));
        assert!(cx.services.calendar.alarms("root").is_empty());
        let log = std::fs::read_to_string(cx.audit.path()).unwrap();
        assert!(log.contains("alarm.delete"), "{log}");
        assert!(matches!(a.handle(&Event::Key(Key::Esc), &cx), Flow::Back));
    }

    /// The tile crate carries its own copies of the date and time parsers and the civil-date maths (it cannot depend on the agent). They
    /// must agree with the agent's on a table of inputs.
    #[test]
    fn the_tile_crates_parsers_agree_with_the_agents() {
        let zone = Zone::from_posix("PST8PDT,M3.2.0,M11.1.0").unwrap();
        let store_dir = temp_dir("calparity");
        let store = agent::CalendarStore::new(&store_dir);
        let src = ShellCalendar { store: &store, zone: &zone };
        for t in ["", "7:30", "07:30", "7:30pm", "7pm", "12am", "12pm", "18:05", "24:00", "7", "13pm", "x", "9:60", "0:00"] {
            assert_eq!(api::parse_hm(t), agent::parse_hm(t), "hm {t}");
        }
        for d in ["", "once", "daily", "everyday", "weekdays", "weekends", "mon,wed,fri", "Sat Sun", "funday", "23", "tue/thu", "mon  wed"] {
            assert_eq!(api::parse_days(d), agent::parse_days(d), "days {d}");
        }
        for s in ["2026-10-12 15:30", "2026-10-12T07:05", "2026-10-12", "", "tomorrow", "2026-13-01", "2026-02-30", "2026-10-12 25:00", "2026-10-12 9", "10/12/2026", "1969-01-01", "2026-10-12 12:60", "2028-02-29", "2027-02-29", "2026-03-08 02:30", "2026-11-01 01:30"] {
            assert_eq!(api::parse_local(&src, s), agent::parse_local(&zone, s), "local {s}");
        }
        for days in [-1000i64, -1, 0, 1, 59, 60, 365, 11_000, 20_000, 24_000, 40_000] {
            assert_eq!(api::civil(days), iyakki_agent::clock::civil(days), "civil {days}");
            let (y, m, d) = api::civil(days);
            assert_eq!(api::days_from_civil(y, m, d), iyakki_agent::clock::days_from_civil(y, m, d));
        }
        for r in agent::Repeat::ALL {
            assert_eq!(repeat_to_api(r).label(), r.label());
            assert_eq!(repeat_to_agent(repeat_to_api(r)), r);
        }
        let al = agent::Alarm { id: "a".into(), label: "x".into(), hour: 1, minute: 2, days: 0, once_at: None, enabled: true, by: "user".into() };
        for days in [0u8, 0x7f, 0b0111110, 0b1000001, 0b0101010] {
            let a = agent::Alarm { days, ..al.clone() };
            assert_eq!(alarm_from_agent(a.clone()).days_label(), a.days_label());
        }
    }
}
