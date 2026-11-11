//! CALENDAR: a month calendar with a day's agenda, and a clock with alarms. Events and alarms are the person's own and
//! the same ones the chat reads and plans with (the assistant's entries are marked), so "put the dentist on Tuesday
//! at 10" in the chat shows up here, and an alarm set here can be read by the assistant. The calendar can show or hide
//! the alarm schedule; alarms ring anywhere in the system while it is running.

use iyakki_tile_api::calendar::{civil, days_from_civil, now_unix, parse_days, parse_hm, parse_local, Alarm, Repeat};
use iyakki_tile_api::{App, Flow, TileCx};
use iyakki_uikit::blink::blink;
use iyakki_uikit::gfx::{rgb, Face, Gfx, Icon};
use iyakki_uikit::safeclamp::SafeClamp;
use iyakki_uikit::ui::{Event, Key, DIM, MUTED, R, TEXT};
use iyakki_uikit::widgets::{render_confirm, Form, FormEvent};
use iyakki_uikit::fx;

const ACCENT: [u8; 4] = rgb(110, 190, 255);
const AMBER: [u8; 4] = rgb(255, 196, 94);
const WEEKDAYS: [&str; 7] = ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"];
const MONTHS: [&str; 12] = ["January", "February", "March", "April", "May", "June", "July", "August", "September", "October", "November", "December"];

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Tab {
    Calendar,
    Clock,
}

enum Mode {
    Browse,
    Event(Form),
    Alarm(Form),
    Confirm(Item),
}

#[derive(Clone, Debug)]
struct Item {
    alarm: bool,
    id: String,
    when: String,
    title: String,
    sub: String,
    off: bool,
    by_assistant: bool,
}

pub struct CalendarApp {
    tab: Tab,
    /// The selected date.
    cur: (i64, u32, u32),
    show_alarms: bool,
    sel_item: usize,
    sel_alarm: usize,
    mode: Mode,
    status: Option<(String, bool)>,
}

fn weekday_mon0(days: i64) -> u32 {
    (days + 3).rem_euclid(7) as u32
}

impl CalendarApp {
    pub fn new(cx: &TileCx) -> CalendarApp {
        let l = cx.calendar.local(now_unix());
        CalendarApp { tab: Tab::Calendar, cur: (l.year, l.month, l.day), show_alarms: true, sel_item: 0, sel_alarm: 0, mode: Mode::Browse, status: None }
    }

    pub fn tab(&self) -> Tab {
        self.tab
    }
    pub fn selected_date(&self) -> (i64, u32, u32) {
        self.cur
    }
    pub fn status_text(&self) -> Option<&str> {
        self.status.as_ref().map(|s| s.0.as_str())
    }

    fn days(&self) -> i64 {
        days_from_civil(self.cur.0, self.cur.1, self.cur.2)
    }

    fn set_days(&mut self, d: i64) {
        self.cur = civil(d);
        self.sel_item = 0;
    }

    fn shift_month(&mut self, by: i64) {
        let m0 = self.cur.0 * 12 + self.cur.1 as i64 - 1 + by;
        let (y, m) = (m0.div_euclid(12), (m0.rem_euclid(12) + 1) as u32);
        let dim = {
            let next = if m == 12 { days_from_civil(y + 1, 1, 1) } else { days_from_civil(y, m + 1, 1) };
            (next - days_from_civil(y, m, 1)) as u32
        };
        self.cur = (y, m, self.cur.2.min(dim));
        self.sel_item = 0;
    }

    fn ok(&mut self, m: impl Into<String>) {
        self.status = Some((m.into(), true));
    }
    fn fail(&mut self, m: impl Into<String>) {
        self.status = Some((m.into(), false));
    }

    fn day_start(&self, cx: &TileCx, days: i64) -> i64 {
        let (y, m, d) = civil(days);
        cx.calendar.local_to_unix(y, m, d, 0, 0, 0)
    }

    /// Everything on one day, soonest first: events, and alarms when they are shown.
    fn items(&self, cx: &TileCx, days: i64) -> Vec<Item> {
        let (from, to) = (self.day_start(cx, days), self.day_start(cx, days + 1));
        let mut v: Vec<(i64, Item)> = Vec::new();
        for o in cx.calendar.events_in(cx.user, from, to) {
            let (when, sub) = if o.all_day {
                ("all day".to_string(), String::new())
            } else {
                let s = cx.calendar.local(o.start);
                let e = cx.calendar.local(o.end);
                (format!("{:02}:{:02}", s.hour, s.minute), if o.end > o.start { format!("until {:02}:{:02}", e.hour, e.minute) } else { String::new() })
            };
            let mut sub = sub;
            if o.repeat != Repeat::None {
                sub = format!("{}{}repeats {}", sub, if sub.is_empty() { "" } else { " · " }, o.repeat.label());
            }
            if let Some(r) = o.remind {
                sub = format!("{}{}reminder {} min before", sub, if sub.is_empty() { "" } else { " · " }, r);
            }
            v.push((o.start, Item { alarm: false, id: o.id, when, title: o.title, sub, off: false, by_assistant: o.by == "assistant" }));
        }
        if self.show_alarms {
            for a in cx.calendar.alarms_in(cx.user, from, to) {
                let l = cx.calendar.local(a.at);
                v.push((a.at, Item { alarm: true, id: a.id, when: format!("{:02}:{:02}", l.hour, l.minute), title: a.label, sub: format!("alarm · {}", a.days), off: !a.enabled, by_assistant: false }));
            }
        }
        v.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.title.cmp(&b.1.title)));
        v.into_iter().map(|x| x.1).collect()
    }

    fn event_form(&self) -> Form {
        let (y, m, d) = self.cur;
        Form::new("New event")
            .text("Title", "", "what is it?")
            .text("Date", &format!("{y}-{m:02}-{d:02}"), "YYYY-MM-DD")
            .text("Starts at", "09:00", "HH:MM, empty for an all-day event")
            .text("Minutes", "60", "how long")
            .choice("Repeat", &["once", "daily", "weekly", "monthly", "yearly"], 0)
            .text("Remind", "10", "minutes before; empty for none")
            .text("Notes", "", "optional")
    }

    fn alarm_form(&self) -> Form {
        Form::new("New alarm").text("Time", "07:00", "HH:MM, or 7:00am").text("Label", "", "what is it for?").choice("Rings", &["once", "daily", "weekdays", "weekends"], 0).text("Or on days", "", "optional: mon,wed,fri")
    }

    fn submit_event(&mut self, cx: &TileCx, f: &Form) -> Result<(), String> {
        let title = f.value(0);
        let time = f.value(2);
        let when = if time.is_empty() { f.value(1) } else { format!("{} {}", f.value(1), time) };
        let (start, all_day) = parse_local(cx.calendar, &when)?;
        let minutes = if all_day { 0 } else if f.value(3).is_empty() { 0 } else { f.value(3).parse::<u32>().map_err(|_| "Minutes must be a number".to_string())? };
        let remind = if f.value(5).is_empty() { None } else { Some(f.value(5).parse::<u32>().map_err(|_| "Remind must be a number of minutes".to_string())?) };
        let e = cx.calendar.add_event(cx.user, &title, start, minutes, all_day, Repeat::ALL[f.choice_idx(4).min(4)], &f.value(6), "user", remind)?;
        cx.audit.record(cx.user, "calendar.add", &e.title);
        let l = cx.calendar.local(e.start + if all_day { 3600 } else { 0 });
        self.cur = (l.year, l.month, l.day);
        Ok(())
    }

    fn submit_alarm(&mut self, cx: &TileCx, f: &Form) -> Result<(), String> {
        let (h, m) = parse_hm(&f.value(0)).ok_or("The time must look like 07:30 or 7:30am")?;
        let days = if f.value(3).is_empty() { [0u8, 0x7f, 0b0111110, 0b1000001][f.choice_idx(2).min(3)] } else { parse_days(&f.value(3))? };
        let a = cx.calendar.add_alarm(cx.user, &f.value(1), h, m, days, now_unix(), "user")?;
        cx.audit.record(cx.user, "alarm.add", &a.label);
        Ok(())
    }

    fn delete(&mut self, cx: &TileCx, it: &Item) {
        let r = if it.alarm { cx.calendar.remove_alarm(cx.user, &it.id) } else { cx.calendar.remove_event(cx.user, &it.id) };
        match r {
            Ok(()) => {
                cx.audit.record(cx.user, if it.alarm { "alarm.delete" } else { "calendar.delete" }, &it.title);
                self.ok(format!("Deleted “{}”.", it.title));
            }
            Err(e) => self.fail(e),
        }
        self.sel_item = 0;
        self.sel_alarm = 0;
    }

    fn alarms(&self, cx: &TileCx) -> Vec<Alarm> {
        cx.calendar.alarms(cx.user)
    }
}

impl App for CalendarApp {
    fn title(&self) -> &'static str {
        "Calendar"
    }
    fn icon(&self) -> Icon {
        Icon::Calendar
    }
    fn accent(&self) -> [u8; 4] {
        ACCENT
    }
    fn hint(&self) -> String {
        match self.tab {
            Tab::Calendar => "←→ day · ↑↓ week · PgUp/PgDn month · T today · N new event · A alarms shown/hidden · D delete · Tab clock · Esc back".into(),
            Tab::Clock => "N new alarm · ↑↓ choose · Space on/off · D delete · Tab calendar · Esc back".into(),
        }
    }
    fn help(&self) -> Vec<String> {
        [
            "Calendar: pick a day on the month; the list on the right is that day's plan.",
            "Arrow keys move the day (left/right) or week (up/down); PgUp / PgDn change the month; T goes to today.",
            "N · a new event (a time, or empty for all day; repeats daily, weekly, monthly or yearly; a reminder that rings before it starts).",
            "A · show or hide the alarm schedule on the calendar.   D · delete the chosen entry (asks first).   Enter · choose the next entry.",
            "Clock tab (Tab key): the time in this machine's zone and your alarms. N adds one: once, daily, weekdays, weekends or chosen days.",
            "Alarms and reminders ring wherever you are in the system while it is running: Enter dismisses, S snoozes for five minutes.",
            "The assistant reads and plans in the same calendar: entries it makes are marked \"assistant\".",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect()
    }

    fn handle(&mut self, ev: &Event, cx: &TileCx) -> Flow {
        match &mut self.mode {
            Mode::Event(f) | Mode::Alarm(f) => {
                match f.handle_event(ev) {
                    FormEvent::Cancel => self.mode = Mode::Browse,
                    FormEvent::Submit => {
                        let alarm = matches!(self.mode, Mode::Alarm(_));
                        let form = match std::mem::replace(&mut self.mode, Mode::Browse) {
                            Mode::Event(f) | Mode::Alarm(f) => f,
                            _ => unreachable!(),
                        };
                        let r = if alarm { self.submit_alarm(cx, &form) } else { self.submit_event(cx, &form) };
                        match r {
                            Ok(()) => self.ok(if alarm { "Alarm set." } else { "Event added." }),
                            Err(e) => {
                                let mut form = form;
                                form.error = Some(e);
                                self.mode = if alarm { Mode::Alarm(form) } else { Mode::Event(form) };
                            }
                        }
                    }
                    _ => {}
                }
                return Flow::Stay;
            }
            Mode::Confirm(it) => {
                let it = it.clone();
                if matches!(ev, Event::Key(Key::Char('y')) | Event::Key(Key::Enter)) {
                    self.mode = Mode::Browse;
                    self.delete(cx, &it);
                } else if matches!(ev, Event::Key(_)) {
                    self.mode = Mode::Browse;
                }
                return Flow::Stay;
            }
            Mode::Browse => {}
        }
        match ev {
            Event::Tab(0) => self.tab = Tab::Calendar,
            Event::Tab(1) => self.tab = Tab::Clock,
            Event::Tab(20) => self.shift_month(-1),
            Event::Tab(21) => self.shift_month(1),
            Event::Tab(22) => {
                let l = cx.calendar.local(now_unix());
                self.cur = (l.year, l.month, l.day);
            }
            Event::Tab(40) => self.show_alarms = !self.show_alarms,
            Event::Tab(41) => self.mode = Mode::Event(self.event_form()),
            Event::Tab(42) => self.mode = Mode::Alarm(self.alarm_form()),
            Event::Tab(n) if (300..400).contains(n) => {
                if let Some(a) = self.alarms(cx).get(*n - 300) {
                    let now = now_unix();
                    if let Err(e) = cx.calendar.set_alarm_enabled(cx.user, &a.id, !a.enabled, now) {
                        self.fail(e);
                    }
                }
            }
            Event::Tab(n) if *n >= 1000 => {
                // a day on the month grid
                let first = days_from_civil(self.cur.0, self.cur.1, 1);
                let grid0 = first - weekday_mon0(first) as i64;
                self.set_days(grid0 + (*n - 1000) as i64);
            }
            Event::Row(i) => {
                if self.tab == Tab::Clock {
                    self.sel_alarm = *i;
                } else {
                    self.sel_item = *i;
                }
            }
            Event::Key(Key::Esc) => return Flow::Back,
            Event::Key(Key::Tab) => self.tab = if self.tab == Tab::Calendar { Tab::Clock } else { Tab::Calendar },
            Event::Key(Key::Char('n')) => self.mode = if self.tab == Tab::Calendar { Mode::Event(self.event_form()) } else { Mode::Alarm(self.alarm_form()) },
            Event::Key(Key::Char('a')) if self.tab == Tab::Calendar => self.show_alarms = !self.show_alarms,
            Event::Key(k) if self.tab == Tab::Calendar => match k {
                Key::Left => self.set_days(self.days() - 1),
                Key::Right => self.set_days(self.days() + 1),
                Key::Up => self.set_days(self.days() - 7),
                Key::Down => self.set_days(self.days() + 7),
                Key::PageUp => self.shift_month(-1),
                Key::PageDown => self.shift_month(1),
                Key::Char('t') => {
                    let l = cx.calendar.local(now_unix());
                    self.cur = (l.year, l.month, l.day);
                }
                Key::Enter => {
                    let n = self.items(cx, self.days()).len();
                    self.sel_item = if n == 0 { 0 } else { (self.sel_item + 1) % n };
                }
                Key::Char('d') | Key::Delete => {
                    if let Some(it) = self.items(cx, self.days()).get(self.sel_item).cloned() {
                        self.mode = Mode::Confirm(it);
                    }
                }
                _ => {}
            },
            Event::Key(k) => match k {
                Key::Up => self.sel_alarm = self.sel_alarm.saturating_sub(1),
                Key::Down => self.sel_alarm = (self.sel_alarm + 1).min(self.alarms(cx).len().saturating_sub(1)),
                Key::Char(' ') | Key::Enter => {
                    if let Some(a) = self.alarms(cx).get(self.sel_alarm) {
                        let now = now_unix();
                        let _ = cx.calendar.set_alarm_enabled(cx.user, &a.id, !a.enabled, now);
                    }
                }
                Key::Char('d') | Key::Delete => {
                    if let Some(a) = self.alarms(cx).get(self.sel_alarm) {
                        self.mode = Mode::Confirm(Item { alarm: true, id: a.id.clone(), when: format!("{:02}:{:02}", a.hour, a.minute), title: a.label.clone(), sub: String::new(), off: !a.enabled, by_assistant: false });
                    }
                }
                _ => {}
            },
            _ => {}
        }
        Flow::Stay
    }

    fn render(&mut self, g: &mut Gfx, area: R, s: f32, cx: &TileCx, tick: u64) {
        let x = area.x + 24.0 * s;
        let w = area.w - 48.0 * s;
        let mut y = area.y + 10.0 * s;
        // tabs and actions
        let tabs = [("Calendar".to_string(), Event::Tab(0)), ("Clock & alarms".to_string(), Event::Tab(1))];
        fx::tab_strip(g, x, y, s, w * 0.5, &tabs, if self.tab == Tab::Calendar { 0 } else { 1 }, ACCENT);
        let mut rx = x + w;
        if self.tab == Tab::Calendar {
            let l = format!("+ Event (n)");
            let bw = g.text_width(14.0 * s, &l, Face::Bold) + 14.0 * s * 1.7;
            g.button(rx - bw, y, 14.0 * s, &l, ACCENT, Event::Tab(41));
            rx -= bw + 8.0 * s;
            let l = if self.show_alarms { "Alarms: shown (a)" } else { "Alarms: hidden (a)" };
            let bw = g.text_width(14.0 * s, l, Face::Bold) + 14.0 * s * 1.7;
            g.button(rx - bw, y, 14.0 * s, l, if self.show_alarms { AMBER } else { MUTED }, Event::Tab(40));
        } else {
            let l = "+ Alarm (n)";
            let bw = g.text_width(14.0 * s, l, Face::Bold) + 14.0 * s * 1.7;
            g.button(rx - bw, y, 14.0 * s, l, AMBER, Event::Tab(42));
        }
        y += 50.0 * s;
        if let Some((m, ok)) = &self.status {
            fx::status_line(g, x, y - 8.0 * s, w, s, m, *ok);
            y += 18.0 * s;
        }
        let body = R::new(x, y, w, area.y + area.h - y - 6.0 * s);
        match self.tab {
            Tab::Calendar => self.draw_calendar(g, body, s, cx),
            Tab::Clock => self.draw_clock(g, body, s, cx),
        }
        match &self.mode {
            Mode::Event(f) | Mode::Alarm(f) => f.render(g, area, s, ACCENT, blink(tick)),
            Mode::Confirm(it) => render_confirm(g, area, s, &format!("Delete the {} “{}”?", if it.alarm { "alarm" } else { "event" }, it.title), "Delete"),
            Mode::Browse => {}
        }
    }
}

impl CalendarApp {
    fn draw_calendar(&mut self, g: &mut Gfx, body: R, s: f32, cx: &TileCx) {
        let now = now_unix();
        let today = {
            let l = cx.calendar.local(now);
            days_from_civil(l.year, l.month, l.day)
        };
        let grid_w = (body.w * 0.6).min(760.0 * s);
        let (y, m) = (self.cur.0, self.cur.1);
        // month header
        let title = format!("{} {}", MONTHS[m as usize - 1], y);
        g.text(body.x, body.y + 4.0 * s, 24.0 * s, TEXT, &title, Face::Bold);
        let mut hx = body.x + grid_w;
        for (l, ev) in [("›", 21usize), ("Today", 22), ("‹", 20)] {
            let bw = g.text_width(14.0 * s, l, Face::Bold) + 14.0 * s * 1.7;
            hx -= bw + 6.0 * s;
            g.button(hx, body.y, 14.0 * s, l, MUTED, Event::Tab(ev));
        }
        let gy = body.y + 46.0 * s;
        let cw = grid_w / 7.0;
        for (i, d) in WEEKDAYS.iter().enumerate() {
            g.text(body.x + i as f32 * cw + 8.0 * s, gy, 12.0 * s, DIM, d, Face::Bold);
        }
        let first = days_from_civil(y, m, 1);
        let grid0 = first - weekday_mon0(first) as i64;
        let rows = 6;
        let ch = ((body.h - 46.0 * s - 24.0 * s) / rows as f32).clamp_safe(40.0 * s, 90.0 * s);
        let (from, to) = (self.day_start(cx, grid0), self.day_start(cx, grid0 + 42));
        let events = cx.calendar.events_in(cx.user, from, to);
        let alarms = if self.show_alarms { cx.calendar.alarms_in(cx.user, from, to) } else { vec![] };
        let day_of = |t: i64| {
            let l = cx.calendar.local(t);
            days_from_civil(l.year, l.month, l.day)
        };
        let cur_days = self.days();
        for i in 0..42i64 {
            let (col, row) = ((i % 7) as f32, (i / 7) as f32);
            let r = R::new(body.x + col * cw + 2.0 * s, gy + 22.0 * s + row * ch, cw - 4.0 * s, ch - 4.0 * s);
            let days = grid0 + i;
            let (_, mm, dd) = civil(days);
            let sel = days == cur_days;
            let hot = g.hovered(r);
            fx::row_at(g, r.x, r.y, r.w, r.h, 10.0 * s, sel, hot, ACCENT);
            g.hit(r, Event::Tab(1000 + i as usize), &format!("{dd}"));
            if days == today {
                g.rrect_outline(r.x, r.y, r.w, r.h, 10.0 * s, 2.0 * s, ACCENT);
            }
            let in_month = mm == m;
            g.text(r.x + 9.0 * s, r.y + 6.0 * s, 15.0 * s, if in_month { TEXT } else { DIM }, &dd.to_string(), if sel || days == today { Face::Bold } else { Face::Sans });
            let n_ev = events.iter().filter(|o| day_of(o.start) <= days && days <= day_of((o.end - 1).max(o.start))).count();
            for k in 0..n_ev.min(4) {
                g.circle(r.x + 14.0 * s + k as f32 * 11.0 * s, r.y + r.h - 12.0 * s, 3.6 * s, if in_month { ACCENT } else { DIM });
            }
            if n_ev > 4 {
                g.text(r.x + 14.0 * s + 4.0 * 11.0 * s - 4.0 * s, r.y + r.h - 20.0 * s, 11.0 * s, MUTED, "+", Face::Bold);
            }
            if alarms.iter().any(|a| a.enabled && day_of(a.at) == days) {
                // an alarm clock: a ring with two small feet
                let (cx0, cy0) = (r.x + r.w - 15.0 * s, r.y + 14.0 * s);
                g.ring(cx0, cy0, 5.5 * s, 1.6 * s, if in_month { AMBER } else { DIM });
                g.line(cx0, cy0, cx0, cy0 - 3.0 * s, 1.3 * s, if in_month { AMBER } else { DIM });
                g.line(cx0 - 6.0 * s, cy0 - 6.0 * s, cx0 - 3.5 * s, cy0 - 8.0 * s, 1.3 * s, if in_month { AMBER } else { DIM });
                g.line(cx0 + 6.0 * s, cy0 - 6.0 * s, cx0 + 3.5 * s, cy0 - 8.0 * s, 1.3 * s, if in_month { AMBER } else { DIM });
            }
        }
        // the chosen day's plan
        let px = body.x + grid_w + 18.0 * s;
        let pw = body.x + body.w - px;
        if pw < 180.0 * s {
            return;
        }
        let (cy_, cm, cd) = self.cur;
        let wd = weekday_mon0(cur_days);
        // the long date ("Wednesday 11 November") is wider than the pane at the largest text size on some days: it is fitted to the pane, with an ellipsis
        // as the last resort, so how wide today's names are never decides whether the heading overflows
        let long = format!("{} {} {}", ["Monday", "Tuesday", "Wednesday", "Thursday", "Friday", "Saturday", "Sunday"][wd as usize], cd, MONTHS[cm as usize - 1]);
        let heading = g.fit(20.0 * s, &long, Face::Bold, pw);
        g.text(px, body.y + 4.0 * s, 20.0 * s, TEXT, &heading, Face::Bold);
        g.text(px, body.y + 30.0 * s, 13.0 * s, MUTED, &format!("{cy_}{}", if cur_days == today { "  ·  today" } else { "" }), Face::Sans);
        let items = self.items(cx, cur_days);
        self.sel_item = self.sel_item.min(items.len().saturating_sub(1));
        let mut iy = body.y + 60.0 * s;
        if items.is_empty() {
            fx::empty_state(g, R::new(px - 10.0 * s, iy, body.w - (px - body.x) , 220.0 * s), s, Icon::Calendar, "Nothing planned", "Press N to add an event, or ask the assistant to plan it.", ACCENT);
        }
        let rh = 58.0 * s;
        let fit = (((body.y + body.h - iy) / rh).floor() as usize).max(1);
        for (i, it) in items.iter().enumerate().take(fit) {
            let r = R::new(px, iy, pw, rh - 6.0 * s);
            fx::row_at(g, r.x, r.y, r.w, r.h, 10.0 * s, i == self.sel_item, false, ACCENT);
            g.hit(r, Event::Row(i), &it.title);
            let col = if it.alarm { AMBER } else { ACCENT };
            g.rect(r.x, r.y + 8.0 * s, 4.0 * s, r.h - 16.0 * s, if it.off { DIM } else { col });
            g.text(r.x + 14.0 * s, r.y + 8.0 * s, 14.0 * s, if it.off { DIM } else { col }, &it.when, Face::Bold);
            let tw = g.text_width(14.0 * s, &it.when, Face::Bold);
            let title = g.fit(15.0 * s, &it.title, Face::Bold, r.w - tw - 60.0 * s);
            g.text(r.x + 26.0 * s + tw, r.y + 7.0 * s, 15.0 * s, if it.off { DIM } else { TEXT }, &title, Face::Bold);
            let mut sub = it.sub.clone();
            if it.off {
                sub = format!("{sub} · off");
            }
            if it.by_assistant {
                sub = format!("{sub}{}planned by the assistant", if sub.is_empty() { "" } else { " · " });
            }
            let sub = g.fit(12.0 * s, &sub, Face::Sans, r.w - 28.0 * s);
            g.text(r.x + 14.0 * s, r.y + 31.0 * s, 12.0 * s, MUTED, &sub, Face::Sans);
            iy += rh;
        }
        if items.len() > fit {
            g.text(px, iy, 12.0 * s, DIM, &format!("+{} more", items.len() - fit), Face::Sans);
        }
    }

    fn draw_clock(&mut self, g: &mut Gfx, body: R, s: f32, cx: &TileCx) {
        let now = now_unix();
        let l = cx.calendar.local(now);
        // the clock face
        let left_w = (body.w * 0.42).min(460.0 * s);
        let r = (left_w.min(body.h - 120.0 * s) / 2.0 - 10.0 * s).max(70.0 * s);
        let (ccx, ccy) = (body.x + left_w / 2.0, body.y + r + 14.0 * s);
        g.radial_glow(ccx, ccy, r * 1.2, [ACCENT[0], ACCENT[1], ACCENT[2], 36]);
        g.circle(ccx, ccy, r + 8.0 * s, rgb(10, 14, 30));
        g.ring(ccx, ccy, r + 8.0 * s, 1.0 * s, [ACCENT[0], ACCENT[1], ACCENT[2], 70]);
        g.ring(ccx, ccy, r, 2.5 * s, ACCENT);
        for i in 0..60 {
            let a = i as f32 * std::f32::consts::TAU / 60.0;
            let (sn, cs) = a.sin_cos();
            let (len, wd, col) = if i % 5 == 0 { (r * 0.12, 3.0 * s, TEXT) } else { (r * 0.05, 1.5 * s, DIM) };
            g.line(ccx + sn * (r - 6.0 * s), ccy - cs * (r - 6.0 * s), ccx + sn * (r - 6.0 * s - len), ccy - cs * (r - 6.0 * s - len), wd, col);
        }
        let hand = |g: &mut Gfx, frac: f32, len: f32, wd: f32, col: [u8; 4]| {
            let a = frac * std::f32::consts::TAU;
            g.line(ccx, ccy, ccx + a.sin() * len, ccy - a.cos() * len, wd, col);
        };
        hand(g, ((l.hour % 12) as f32 + l.minute as f32 / 60.0) / 12.0, r * 0.5, 5.0 * s, TEXT);
        hand(g, (l.minute as f32 + l.second as f32 / 60.0) / 60.0, r * 0.76, 3.5 * s, ACCENT);
        hand(g, l.second as f32 / 60.0, r * 0.84, 1.8 * s, rgb(255, 120, 150));
        g.circle(ccx, ccy, 6.0 * s, rgb(255, 120, 150));
        let ty = ccy + r + 22.0 * s;
        let t = format!("{:02}:{:02}:{:02}", l.hour, l.minute, l.second);
        let tw = g.text_width(44.0 * s, &t, Face::Bold);
        g.text(ccx - tw / 2.0, ty, 44.0 * s, TEXT, &t, Face::Bold);
        let d = format!("{}, {} {} {}", ["Sunday", "Monday", "Tuesday", "Wednesday", "Thursday", "Friday", "Saturday"][l.weekday as usize], l.day, MONTHS[l.month as usize - 1], l.year);
        let dw = g.text_width(16.0 * s, &d, Face::Sans);
        g.text(ccx - dw / 2.0, ty + 54.0 * s, 16.0 * s, MUTED, &d, Face::Sans);
        let z = format!("{} (UTC{}{:02}:{:02})", l.abbrev, if l.offset < 0 { '-' } else { '+' }, l.offset.abs() / 3600, l.offset.abs() % 3600 / 60);
        let zw = g.text_width(13.0 * s, &z, Face::Sans);
        g.text(ccx - zw / 2.0, ty + 78.0 * s, 13.0 * s, DIM, &z, Face::Sans);
        // the alarms
        let px = body.x + left_w + 24.0 * s;
        let pw = body.x + body.w - px;
        if pw < 200.0 * s {
            return;
        }
        g.text(px, body.y + 4.0 * s, 20.0 * s, TEXT, "Alarms", Face::Bold);
        match cx.calendar.next_alarm(cx.user, now) {
            Some(a) => {
                let mins = (a.at - now + 59) / 60;
                let left = if mins >= 60 * 24 { format!("{}d {}h", mins / 1440, mins % 1440 / 60) } else if mins >= 60 { format!("{}h {}m", mins / 60, mins % 60) } else { format!("{mins} min") };
                g.text(px, body.y + 32.0 * s, 14.0 * s, AMBER, &format!("Next: “{}” in {left}", a.label), Face::Bold);
            }
            None => {
                g.text(px, body.y + 32.0 * s, 14.0 * s, MUTED, "No alarm is set.", Face::Sans);
            }
        }
        let alarms = self.alarms(cx);
        self.sel_alarm = self.sel_alarm.min(alarms.len().saturating_sub(1));
        let rh = 62.0 * s;
        let mut y = body.y + 64.0 * s;
        let fit = (((body.y + body.h - y) / rh).floor() as usize).max(1);
        if alarms.is_empty() {
            g.text(px, y + 6.0 * s, 15.0 * s, MUTED, "Press N to set one, or ask the assistant: \"wake me at 6:30 on weekdays\".", Face::Sans);
        }
        for (i, a) in alarms.iter().enumerate().take(fit) {
            let r = R::new(px, y, pw, rh - 6.0 * s);
            fx::row_at(g, r.x, r.y, r.w, r.h, 12.0 * s, i == self.sel_alarm, false, ACCENT);
            g.hit(r, Event::Row(i), &a.label);
            let t = format!("{:02}:{:02}", a.hour, a.minute);
            g.text(r.x + 16.0 * s, r.y + 8.0 * s, 30.0 * s, if a.enabled { TEXT } else { DIM }, &t, Face::Bold);
            let tw = g.text_width(30.0 * s, &t, Face::Bold);
            let label = g.fit(15.0 * s, &a.label, Face::Bold, r.w - tw - 150.0 * s);
            g.text(r.x + 32.0 * s + tw, r.y + 8.0 * s, 15.0 * s, if a.enabled { TEXT } else { DIM }, &label, Face::Bold);
            let sub = format!("{}{}", a.days_label(), if a.by == "assistant" { " · set by the assistant" } else { "" });
            let sub = g.fit(12.0 * s, &sub, Face::Sans, r.w - tw - 150.0 * s);
            g.text(r.x + 32.0 * s + tw, r.y + 32.0 * s, 12.0 * s, MUTED, &sub, Face::Sans);
            // the on/off switch
            let (sx, sy) = (r.x + r.w - 62.0 * s, r.y + r.h / 2.0 - 11.0 * s);
            fx::switch(g, R::new(sx, sy, 46.0 * s, 22.0 * s), a.enabled, true, AMBER, s);
            g.hit(R::new(sx - 6.0 * s, sy - 6.0 * s, 58.0 * s, 34.0 * s), Event::Tab(300 + i), "switch");
            y += rh;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use iyakki_tile_api::calendar::{AlarmOccurrence, Added, CalendarSource, Local, Occurrence};
    use iyakki_tile_api::{usage, AuditSink};
    use std::cell::RefCell;

    #[derive(Clone)]
    struct StoredEvent {
        id: String,
        title: String,
        start: i64,
        minutes: u32,
        all_day: bool,
        repeat: Repeat,
        notes: String,
        by: String,
        remind: Option<u32>,
    }

    /// An in-test calendar in UTC: events (once, daily or weekly) and alarms in memory.
    #[derive(Default)]
    struct FakeCal {
        events: RefCell<Vec<StoredEvent>>,
        alarms: RefCell<Vec<Alarm>>,
        n: RefCell<u32>,
    }

    impl FakeCal {
        fn events(&self) -> Vec<StoredEvent> {
            self.events.borrow().clone()
        }
        fn id(&self) -> String {
            *self.n.borrow_mut() += 1;
            format!("id{}", self.n.borrow())
        }
        fn alarm_times(a: &Alarm, from: i64, to: i64) -> Vec<i64> {
            let mut out = vec![];
            for day in from.div_euclid(86_400)..=to.div_euclid(86_400) {
                let wd = (day + 4).rem_euclid(7) as u32;
                let t = day * 86_400 + a.hour as i64 * 3600 + a.minute as i64 * 60;
                if (a.days == 0 || a.days & (1 << wd) != 0) && t >= from && t < to {
                    out.push(t);
                }
            }
            out
        }
    }

    impl CalendarSource for FakeCal {
        fn local(&self, unix: i64) -> Local {
            let days = unix.div_euclid(86_400);
            let secs = unix.rem_euclid(86_400) as u32;
            let (year, month, day) = civil(days);
            Local { year, month, day, hour: secs / 3600, minute: secs % 3600 / 60, second: secs % 60, weekday: (days + 4).rem_euclid(7) as u32, offset: 0, abbrev: "UTC".into() }
        }
        fn local_to_unix(&self, y: i64, m: u32, d: u32, hh: u32, mm: u32, ss: u32) -> i64 {
            days_from_civil(y, m, d) * 86_400 + (hh * 3600 + mm * 60 + ss) as i64
        }
        fn events_in(&self, _user: &str, from: i64, to: i64) -> Vec<Occurrence> {
            let mut out = vec![];
            for e in self.events.borrow().iter() {
                let step = match e.repeat {
                    Repeat::None => 0,
                    Repeat::Daily => 86_400,
                    Repeat::Weekly => 7 * 86_400,
                    _ => 0,
                };
                let mut t = e.start;
                while t < to {
                    if t >= from {
                        out.push(Occurrence { id: e.id.clone(), title: e.title.clone(), start: t, end: t + e.minutes as i64 * 60, all_day: e.all_day, repeat: e.repeat, notes: e.notes.clone(), by: e.by.clone(), remind: e.remind });
                    }
                    if step == 0 {
                        break;
                    }
                    t += step;
                }
            }
            out
        }
        fn alarms_in(&self, _user: &str, from: i64, to: i64) -> Vec<AlarmOccurrence> {
            let mut out = vec![];
            for a in self.alarms.borrow().iter() {
                for t in Self::alarm_times(a, from, to) {
                    out.push(AlarmOccurrence { id: a.id.clone(), label: a.label.clone(), at: t, days: a.days_label(), enabled: a.enabled });
                }
            }
            out
        }
        fn add_event(&self, _user: &str, title: &str, start: i64, minutes: u32, all_day: bool, repeat: Repeat, notes: &str, by: &str, remind: Option<u32>) -> Result<Added, String> {
            self.events.borrow_mut().push(StoredEvent { id: self.id(), title: title.into(), start, minutes, all_day, repeat, notes: notes.into(), by: by.into(), remind });
            Ok(Added { title: title.into(), start })
        }
        fn remove_event(&self, _user: &str, id: &str) -> Result<(), String> {
            let mut v = self.events.borrow_mut();
            let n = v.len();
            v.retain(|e| e.id != id);
            if v.len() == n { Err("no such event".into()) } else { Ok(()) }
        }
        fn alarms(&self, _user: &str) -> Vec<Alarm> {
            self.alarms.borrow().clone()
        }
        fn add_alarm(&self, _user: &str, label: &str, hour: u32, minute: u32, days: u8, _now: i64, by: &str) -> Result<Alarm, String> {
            let a = Alarm { id: self.id(), label: label.into(), hour: hour as u8, minute: minute as u8, days, enabled: true, by: by.into() };
            self.alarms.borrow_mut().push(a.clone());
            Ok(a)
        }
        fn remove_alarm(&self, _user: &str, id: &str) -> Result<(), String> {
            self.alarms.borrow_mut().retain(|a| a.id != id);
            Ok(())
        }
        fn set_alarm_enabled(&self, _user: &str, id: &str, on: bool, _now: i64) -> Result<(), String> {
            match self.alarms.borrow_mut().iter_mut().find(|a| a.id == id) {
                Some(a) => {
                    a.enabled = on;
                    Ok(())
                }
                None => Err("no such alarm".into()),
            }
        }
        fn next_alarm(&self, _user: &str, now: i64) -> Option<AlarmOccurrence> {
            self.alarms_in("", now, now + 8 * 86_400).into_iter().filter(|a| a.enabled).min_by_key(|a| a.at)
        }
    }

    #[derive(Default)]
    struct FakeAudit(RefCell<Vec<String>>);
    impl AuditSink for FakeAudit {
        fn record(&self, user: &str, action: &str, subject: &str) {
            self.0.borrow_mut().push(format!("{user} {action} {subject}"));
        }
    }

    struct NoUsage;
    impl usage::UsageSource for NoUsage {
        fn now_unix(&self) -> i64 {
            0
        }
        fn series(&self, _: usage::Range, _: i64, _: Option<&str>) -> usage::Series {
            usage::Series { models: vec![], buckets: vec![], totals: vec![], input: 0, output: 0, calls: 0 }
        }
    }

    struct Fx {
        cal: FakeCal,
        audit: FakeAudit,
        usage: NoUsage,
    }

    impl Fx {
        fn new() -> Fx {
            Fx { cal: FakeCal::default(), audit: FakeAudit::default(), usage: NoUsage }
        }
        fn cx(&self) -> TileCx<'_> {
            TileCx { user: "root", manage_settings: true, usage: &self.usage, calendar: &self.cal, audit: &self.audit, remote: &iyakki_tile_api::remote::NoRemote, cameras: &iyakki_tile_api::cameras::NoCameras }
        }
    }

    /// Key and click helpers over one app and one context.
    struct Th<'a, 'b> {
        app: CalendarApp,
        cx: &'a TileCx<'b>,
    }

    impl Th<'_, '_> {
        fn send(&mut self, ev: Event) {
            self.app.handle(&ev, self.cx);
        }
        fn key(&mut self, k: Key) {
            self.send(Event::Key(k));
        }
    }

    fn draw(a: &mut CalendarApp, cx: &TileCx, w: u32, h: u32) -> Gfx {
        let mut g = Gfx::new(w, h);
        g.clear(rgb(9, 11, 20));
        a.render(&mut g, R::new(0.0, 46.0, w as f32, h as f32 - 46.0), 1.0, cx, 3);
        g
    }

    fn type_into(a: &mut CalendarApp, cx: &TileCx, text: &str) {
        for c in text.chars() {
            a.handle(&Event::Key(Key::Char(c)), cx);
        }
    }

    #[test]
    fn moving_around_the_month_selects_days_and_changes_months_correctly() {
        let fx = Fx::new();
        let cx = fx.cx();
        let mut t = Th { app: CalendarApp::new(&cx), cx: &cx };
        t.app.cur = (2026, 1, 31);
        t.key(Key::PageDown);
        assert_eq!(t.app.selected_date(), (2026, 2, 28), "31 January + a month is the end of February");
        t.key(Key::PageUp);
        t.key(Key::PageUp);
        assert_eq!(t.app.selected_date(), (2025, 12, 28));
        t.key(Key::Right);
        t.key(Key::Down);
        assert_eq!(t.app.selected_date(), (2026, 1, 5));
        t.key(Key::Left);
        t.key(Key::Up);
        assert_eq!(t.app.selected_date(), (2025, 12, 28));
        // clicking a day on the grid selects it (grid starts on the Monday on or before the 1st)
        t.app.cur = (2026, 10, 15);
        t.send(Event::Tab(1000));
        assert_eq!(t.app.selected_date(), (2026, 9, 28), "1 October 2026 is a Thursday, so the grid starts on Monday 28 September");
        t.app.cur = (2026, 10, 15);
        t.send(Event::Tab(1000 + 13));
        assert_eq!(t.app.selected_date(), (2026, 10, 11));
        t.key(Key::Char('t'));
        let today = fx.cal.local(now_unix());
        assert_eq!(t.app.selected_date(), (today.year, today.month, today.day));
        // every window size draws
        for (w, h) in [(1280u32, 720u32), (700, 500), (400, 400), (1920, 1080)] {
            draw(&mut t.app, &cx, w, h);
        }
    }

    #[test]
    fn an_event_is_added_with_the_form_shows_in_the_plan_repeats_and_is_deleted_after_asking() {
        let fx = Fx::new();
        let cx = fx.cx();
        let mut a = CalendarApp::new(&cx);
        a.cur = (2030, 6, 3);
        a.handle(&Event::Key(Key::Char('n')), &cx);
        draw(&mut a, &cx, 1280, 720);
        type_into(&mut a, &cx, "Team lunch");
        // tab through: date (kept), start (09:00 → 12:30), minutes, repeat weekly, remind, notes
        a.handle(&Event::Key(Key::Tab), &cx);
        a.handle(&Event::Key(Key::Tab), &cx);
        for _ in 0..5 {
            a.handle(&Event::Key(Key::Backspace), &cx);
        }
        type_into(&mut a, &cx, "12:30");
        a.handle(&Event::Key(Key::Tab), &cx);
        a.handle(&Event::Key(Key::Tab), &cx);
        a.handle(&Event::Key(Key::Right), &cx);
        a.handle(&Event::Key(Key::Right), &cx);
        a.handle(&Event::Key(Key::Enter), &cx);
        // Enter on the last field or Ctrl-S submits; use the form's own submit via Enter on every field if needed
        for _ in 0..4 {
            if !matches!(a.mode, Mode::Browse) {
                a.handle(&Event::Key(Key::Enter), &cx);
            }
        }
        assert!(matches!(a.mode, Mode::Browse), "{:?}", a.status_text());
        let ev = fx.cal.events();
        assert_eq!(ev.len(), 1, "{:?}", a.status_text());
        assert_eq!((ev[0].title.as_str(), ev[0].by.as_str(), ev[0].remind), ("Team lunch", "user", Some(10)));
        assert_eq!(ev[0].repeat, Repeat::Weekly);
        let l = fx.cal.local(ev[0].start);
        assert_eq!((l.year, l.month, l.day, l.hour, l.minute), (2030, 6, 3, 12, 30));
        // it shows on its day and a week later, not the day after
        assert_eq!(a.items(&cx, days_from_civil(2030, 6, 3)).len(), 1);
        assert_eq!(a.items(&cx, days_from_civil(2030, 6, 10)).len(), 1);
        assert!(a.items(&cx, days_from_civil(2030, 6, 4)).is_empty());
        let it = &a.items(&cx, days_from_civil(2030, 6, 3))[0];
        assert!(it.sub.contains("repeats weekly") && it.sub.contains("reminder 10 min before"), "{it:?}");
        draw(&mut a, &cx, 1280, 720);
        // delete asks first
        a.cur = (2030, 6, 3);
        a.handle(&Event::Key(Key::Char('d')), &cx);
        assert!(matches!(a.mode, Mode::Confirm(_)));
        draw(&mut a, &cx, 1280, 720);
        a.handle(&Event::Key(Key::Char('x')), &cx);
        assert_eq!(fx.cal.events().len(), 1, "anything but y keeps it");
        a.handle(&Event::Key(Key::Char('d')), &cx);
        a.handle(&Event::Key(Key::Char('y')), &cx);
        assert!(fx.cal.events().is_empty());
        let log = fx.audit.0.borrow().join("\n");
        assert!(log.contains("calendar.add") && log.contains("calendar.delete"), "{log}");
        // a bad date stays in the form with the reason
        a.handle(&Event::Key(Key::Char('n')), &cx);
        type_into(&mut a, &cx, "x");
        a.handle(&Event::Key(Key::Tab), &cx);
        for _ in 0..10 {
            a.handle(&Event::Key(Key::Backspace), &cx);
        }
        type_into(&mut a, &cx, "2030-02-31");
        for _ in 0..8 {
            if matches!(a.mode, Mode::Event(_)) {
                a.handle(&Event::Key(Key::Enter), &cx);
            }
        }
        match &a.mode {
            Mode::Event(f) => assert!(f.error.as_deref().unwrap_or("").contains("is not a date"), "{:?}", f.error),
            _ => panic!("the form should still be open"),
        }
    }

    #[test]
    fn the_calendar_can_show_or_hide_the_alarm_schedule_and_alarms_are_set_switched_and_deleted_on_the_clock_tab() {
        let fx = Fx::new();
        let cx = fx.cx();
        let mut a = CalendarApp::new(&cx);
        let now = now_unix();
        // a daily alarm shows on every day's plan while alarms are shown
        fx.cal.add_alarm("root", "Stand up", 10, 0, 0x7f, now, "assistant").unwrap();
        let today = {
            let l = fx.cal.local(now);
            days_from_civil(l.year, l.month, l.day)
        };
        let day = a.items(&cx, today + 3);
        assert_eq!(day.len(), 1);
        assert!(day[0].alarm && day[0].title == "Stand up" && day[0].when == "10:00");
        let g = draw(&mut a, &cx, 1280, 720);
        assert!(g.hits.iter().any(|h| h.label == "Stand up"));
        a.handle(&Event::Key(Key::Char('a')), &cx);
        assert!(!a.show_alarms);
        assert!(a.items(&cx, today + 3).is_empty(), "hidden alarms leave the calendar");
        draw(&mut a, &cx, 1280, 720);
        a.handle(&Event::Tab(40), &cx);
        assert!(a.show_alarms);
        // the clock tab
        a.handle(&Event::Key(Key::Tab), &cx);
        assert_eq!(a.tab(), Tab::Clock);
        let g = draw(&mut a, &cx, 1280, 720);
        assert!(g.hits.iter().any(|h| h.label == "switch"), "each alarm has a switch");
        a.handle(&Event::Tab(300), &cx);
        assert!(!fx.cal.alarms("root")[0].enabled, "the switch turned it off");
        assert!(a.items(&cx, today + 3)[0].off);
        a.handle(&Event::Key(Key::Char(' ')), &cx);
        assert!(fx.cal.alarms("root")[0].enabled);
        // a new alarm from the form: 06:45 on weekdays
        a.handle(&Event::Key(Key::Char('n')), &cx);
        for _ in 0..5 {
            a.handle(&Event::Key(Key::Backspace), &cx);
        }
        type_into(&mut a, &cx, "6:45");
        a.handle(&Event::Key(Key::Tab), &cx);
        type_into(&mut a, &cx, "Work");
        a.handle(&Event::Key(Key::Tab), &cx);
        a.handle(&Event::Key(Key::Right), &cx);
        a.handle(&Event::Key(Key::Right), &cx);
        for _ in 0..4 {
            if !matches!(a.mode, Mode::Browse) {
                a.handle(&Event::Key(Key::Enter), &cx);
            }
        }
        let all = fx.cal.alarms("root");
        let work = all.iter().find(|x| x.label == "Work").expect("the alarm was created");
        assert_eq!((work.hour, work.minute, work.days, work.by.as_str()), (6, 45, 0b0111110, "user"));
        draw(&mut a, &cx, 1280, 720);
        // delete the chosen one after asking
        a.sel_alarm = 0;
        a.handle(&Event::Key(Key::Char('d')), &cx);
        assert!(matches!(a.mode, Mode::Confirm(_)));
        a.handle(&Event::Key(Key::Char('y')), &cx);
        assert_eq!(fx.cal.alarms("root").len(), 1);
        for (w, h) in [(1280u32, 720u32), (700, 500), (400, 400)] {
            draw(&mut a, &cx, w, h);
        }
    }

    /// `IYAKKI_SHOT=/path.png cargo test -p iyakki-tile-calendar calendar_picture -- --ignored`
    #[test]
    #[ignore]
    fn calendar_picture() {
        let Ok(out) = std::env::var("IYAKKI_SHOT") else { return };
        let fx = Fx::new();
        let cx = fx.cx();
        let mut a = CalendarApp::new(&cx);
        let z = &fx.cal;
        let now = now_unix();
        let l = z.local(now);
        let c = &fx.cal;
        let at = |d: i64, h: u32, m: u32| z.local_to_unix(l.year, l.month, l.day, h, m, 0) + d * 86_400;
        c.add_event("root", "Standup", at(0, 9, 30), 15, false, Repeat::Daily, "", "user", Some(5)).unwrap();
        c.add_event("root", "Dentist", at(0, 16, 0), 45, false, Repeat::None, "bring the form", "assistant", Some(30)).unwrap();
        c.add_event("root", "Plan the release", at(2, 11, 0), 90, false, Repeat::None, "", "assistant", None).unwrap();
        c.add_event("root", "Mum's birthday", at(5, 0, 0), 0, true, Repeat::Yearly, "", "user", None).unwrap();
        c.add_event("root", "Review", at(3, 14, 0), 60, false, Repeat::Weekly, "", "user", Some(10)).unwrap();
        c.add_alarm("root", "Wake up", 6, 30, 0b0111110, now, "user").unwrap();
        c.add_alarm("root", "Take medicine", 20, 0, 0x7f, now, "assistant").unwrap();
        let mut g = draw(&mut a, &cx, 1280, 760);
        g.save_png(std::path::Path::new(&out)).unwrap();
        a.handle(&Event::Key(Key::Tab), &cx);
        g = draw(&mut a, &cx, 1280, 760);
        g.save_png(std::path::Path::new(&format!("{out}.clock.png"))).unwrap();
    }
}
