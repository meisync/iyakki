//! LEARN: the drawing. Every screen is described as a list of items (heading, text, row, buttons) that are laid out top to bottom with all
//! text wrapped to the width, in one scrolled area under the tab strip, so nothing runs out of the window at any size or text size.
//! Nothing that is the answer key is put in the list for a question that has not been answered.

use iyakki_agent::study::{score, Question, Quiz};

use std::sync::Arc;

use super::learn::*;
use super::learn_lesson::*;
use crate::fx::{self, AMBER};
use crate::gfx::{rgb, Face, Gfx};
use crate::ui::{Event, ERR, MUTED, OK, R, TEXT};

type Btn = (String, Event, [u8; 4]);

pub(super) enum Item {
    Head(String),
    Text(String, [u8; 4]),
    /// A bold title and a muted second part, drawn on a card; `ev` is what a click does.
    Row { title: String, sub: String, accent: [u8; 4], selected: bool, ev: Event },
    Buttons(Vec<Btn>),
    /// A section heading of a lesson; the current one is marked, a click chooses it.
    Sec { title: String, current: bool, jump: bool, ev: Event },
    /// A picture of a lesson with its caption (`bytes` is None while it is still being drawn).
    Pic { id: u64, bytes: Option<Arc<Vec<u8>>>, caption: String },
    /// The box where the question is typed.
    Input,
}

fn letter(i: usize) -> char {
    (b'A' + i as u8) as char
}

fn q_state(q: &Quiz, i: usize, picked: Option<usize>) -> (&'static str, [u8; 4]) {
    if q.questions[i].flagged {
        ("Reported", AMBER)
    } else if picked == Some(q.questions[i].answer) {
        ("Right", OK)
    } else if picked.is_none() {
        ("Skipped", ERR)
    } else {
        ("Wrong", ERR)
    }
}

impl LearnApp {
    pub(super) fn items(&self) -> Vec<Item> {
        let b = |l: &str, id: usize, c: [u8; 4]| (l.to_string(), Event::Tab(id), c);
        let mut v = Vec::new();
        match &self.view {
            View::List if self.tab == Tab::Plans => {
                if self.plans.is_empty() {
                    v.push(Item::Text("No study plans yet. Ask the assistant in the chat, for example: \"I want to learn programming basics in 21 days\" or \"a yoga session every day for a month\".".into(), MUTED));
                }
                for (i, p) in self.plans.iter().enumerate() {
                    let next = p.next_index().map_or("all done".to_string(), |n| format!("next: {} {}", p.sessions[n].date, p.sessions[n].title));
                    v.push(Item::Row { title: p.topic.clone(), sub: format!("{} of {} done · {}{}", p.done_count(), p.sessions.len(), next, if p.events.is_empty() { "" } else { " · in the calendar" }), accent: ACCENT, selected: i == self.sel[0], ev: Event::Row(i) });
                }
                if !self.plans.is_empty() {
                    self.delete_buttons(&mut v, "Delete the selected plan");
                }
            }
            View::List => {
                if self.quizzes.is_empty() {
                    v.push(Item::Text("No quizzes yet. Ask the assistant in the chat, for example: \"quiz me with 10 questions on the solar system\".".into(), MUTED));
                }
                for (i, q) in self.quizzes.iter().enumerate() {
                    v.push(Item::Row { title: q.topic.clone(), sub: format!("{} questions · {}", q.questions.len(), LearnApp::last_line(q)), accent: ACCENT, selected: i == self.sel[1], ev: Event::Row(i) });
                }
                if !self.quizzes.is_empty() {
                    v.push(Item::Buttons(vec![b("Start the selected quiz", T_START, ACCENT)]));
                    self.delete_buttons(&mut v, "Delete the selected quiz");
                }
            }
            View::Plan(id) => {
                let Some(p) = self.plan(id) else { return v };
                v.push(Item::Head(p.topic.clone()));
                v.push(Item::Text(format!("{} of {} sessions done · {} {} · {}", p.done_count(), p.sessions.len(), p.count, p.unit, p.every), MUTED));
                let mut bt = vec![b("Back", T_BACK, MUTED)];
                bt.push(if p.events.is_empty() { b("Add to calendar", T_CAL, ACCENT) } else { b("Remove from calendar", T_UNCAL, AMBER) });
                v.push(Item::Buttons(bt));
                self.delete_buttons(&mut v, "Delete this plan");
                v.push(Item::Text("Click a day (or press Enter) to open its lesson: the assistant writes it for you, with pictures. Space ticks the chosen day done.".into(), MUTED));
                v.push(Item::Buttons(vec![b("Open the lesson of the chosen day", T_OPEN_LESSON, ACCENT), b("Tick the chosen day done", T_TICK, MUTED)]));
                for (i, s) in p.sessions.iter().enumerate() {
                    let mark = if s.done { "[x]" } else { "[ ]" };
                    let sub = if s.notes.is_empty() { format!("{} min", s.minutes) } else { format!("{} min · {}", s.minutes, s.notes.replace('\n', " ")) };
                    v.push(Item::Row { title: format!("{mark} {}. {} {} · {}", i + 1, s.date, s.time, s.title), sub, accent: if s.done { OK } else { ACCENT }, selected: i == self.sel_session, ev: Event::Row(i) });
                }
            }
            View::Lesson(l) => self.lesson_items(l, &mut v),
            View::Quiz(r) => {
                let Some(q) = self.quiz(&r.id) else { return v };
                let qu: &Question = &q.questions[r.idx];
                v.push(Item::Head(format!("{} · question {} of {}", q.topic, r.idx + 1, q.questions.len())));
                v.push(Item::Text(qu.text.clone(), TEXT));
                if qu.flagged {
                    v.push(Item::Text("You reported this question as wrong: it does not count.".into(), AMBER));
                }
                let picked = r.picks[r.idx];
                for (i, o) in qu.options.iter().enumerate() {
                    // the key is only ever coloured in once the question is answered
                    let accent = if !r.answered { ACCENT } else if i == qu.answer { OK } else if picked == Some(i) { ERR } else { MUTED };
                    v.push(Item::Row { title: format!("{}) {}", letter(i), o), sub: String::new(), accent, selected: if r.answered { picked == Some(i) } else { i == r.cursor }, ev: Event::Tab(T_OPTION + i) });
                }
                if r.answered {
                    if picked == Some(qu.answer) {
                        v.push(Item::Text("Right.".into(), OK));
                    } else {
                        v.push(Item::Text(format!("Not quite. The answer is {}) {}", letter(qu.answer), qu.options[qu.answer]), ERR));
                    }
                    v.push(Item::Text(qu.explanation.clone(), TEXT));
                    let last = r.idx + 1 == q.questions.len();
                    v.push(Item::Buttons(vec![b(if last { "Finish" } else { "Next" }, T_NEXT, ACCENT), b(if qu.flagged { "Take back the report" } else { "Report a wrong question" }, T_REPORT, AMBER), b("Stop", T_STOP, MUTED)]));
                } else {
                    v.push(Item::Buttons(vec![b("Stop", T_STOP, MUTED)]));
                }
            }
            View::Result(id, sel) => {
                let Some(q) = self.quiz(id) else { return v };
                let Some(a) = q.attempts.last() else { return v };
                let s = score(q, &a.answers);
                v.push(Item::Head(format!("{}: {} of {} = {}%", q.topic, s.correct, s.counted, s.percent)));
                if s.flagged > 0 {
                    v.push(Item::Text(format!("{} reported question{} left out of the score.", s.flagged, if s.flagged == 1 { " is" } else { "s are" }), AMBER));
                }
                v.push(Item::Buttons(vec![b("Try again", T_AGAIN, ACCENT), b("Done", T_BACK, MUTED), b("Report the chosen question", T_REPORT, AMBER)]));
                v.push(Item::Text("Choose a question to see the answer. A question that is wrong can be reported: it stops counting.".into(), MUTED));
                for (i, qu) in q.questions.iter().enumerate() {
                    let picked = a.answers.get(i).copied().flatten();
                    let (word, c) = q_state(q, i, picked);
                    let mut sub = format!("You picked: {}", picked.map_or("(nothing)".to_string(), |p| format!("{}) {}", letter(p), qu.options[p])));
                    if i == *sel || word != "Right" {
                        sub.push_str(&format!(" · Answer: {}) {} · {}", letter(qu.answer), qu.options[qu.answer], qu.explanation));
                    }
                    v.push(Item::Row { title: format!("{}. {} · {}", i + 1, word, qu.text), sub, accent: c, selected: i == *sel, ev: Event::Row(i) });
                }
            }
        }
        v
    }

    fn lesson_items(&self, l: &LessonState, v: &mut Vec<Item>) {
        let b = |t: &str, id: usize, c: [u8; 4]| (t.to_string(), Event::Tab(id), c);
        let Some(p) = self.plan(&l.plan) else { return };
        let Some(s) = p.sessions.get(l.n - 1) else { return };
        v.push(Item::Head(format!("Day {}: {}", l.n, s.title)));
        v.push(Item::Text(format!("{} {} · {} min · {}{}", s.date, s.time, s.minutes, p.topic, if s.done { " · done" } else { "" }), MUTED));
        let generating = l.generating();
        let has = l.saved.is_some();
        let mut top = vec![b("Back to the plan", T_L_BACK, MUTED), b(if s.done { "Mark not done" } else { "Mark day done" }, T_L_DONE, if s.done { AMBER } else { OK })];
        if !l.sections().is_empty() {
            top.push(b("Previous section", T_L_PREV, MUTED));
            top.push(b("Next section", T_L_NEXT, ACCENT));
        }
        v.push(Item::Buttons(top));
        if generating {
            v.push(Item::Buttons(vec![b("Cancel", T_L_CANCEL, ERR)]));
            v.push(Item::Text(format!("{}…", l.phase), ACCENT));
        } else if l.input.is_none() && !l.confirm_regen {
            let mut act = Vec::new();
            if has {
                act.push(b("Ask a question", T_L_ASK, ACCENT));
            }
            act.push(b("Make it simpler", T_L_SIMPLER, MUTED));
            act.push(b("Go deeper", T_L_DEEPER, MUTED));
            if has {
                act.push(b(if self.reading() { "Stop reading" } else { "Read aloud" }, T_L_READ, MUTED));
                act.push(b("Write it again", T_L_REGEN, AMBER));
            }
            v.push(Item::Buttons(act));
        }
        if l.confirm_regen {
            v.push(Item::Text("Write this lesson again? The saved lesson (and the questions asked about it) is replaced.".into(), AMBER));
            v.push(Item::Buttons(vec![b("Yes, write it again", T_L_REGEN_YES, ERR), b("No", T_L_REGEN_NO, MUTED)]));
        }
        if l.input.is_some() {
            v.push(Item::Input);
            v.push(Item::Buttons(vec![b("Send", T_L_SEND, ACCENT), b("Cancel", T_L_ASK_CANCEL, MUTED)]));
        }
        if let Some(e) = &l.fail {
            v.push(Item::Text(e.clone(), ERR));
            if !generating {
                v.push(Item::Buttons(vec![b(if has { "Write it again" } else { "Retry" }, T_L_RETRY, ACCENT)]));
            }
        }
        // what you will learn today: from the stored title and notes, so it is there before any answer
        v.push(Item::Head("What you will learn today".into()));
        for g in iyakki_agent::studylesson::goals(p, l.n) {
            v.push(Item::Text(format!("• {g}"), TEXT));
        }
        let pics = l.pictures();
        for (i, sec) in l.sections().iter().enumerate() {
            v.push(Item::Sec { title: sec.title.clone(), current: i == l.cur, jump: l.jump && i == l.cur, ev: Event::Tab(T_SEC + i) });
            if !sec.text.is_empty() {
                v.push(Item::Text(sec.text.clone(), TEXT));
            }
            for &pi in &sec.pics {
                if let Some(pic) = pics.get(pi) {
                    let bytes = l.pic_bytes(pi);
                    let id = bytes.as_ref().map_or(0, |b| pic_id(&l.plan, l.n, pi, b));
                    v.push(Item::Pic { id, bytes, caption: pic.caption.clone() });
                }
            }
        }
        if l.sections().is_empty() && !generating && l.fail.is_none() {
            v.push(Item::Text("The lesson is not written yet.".into(), MUTED));
        }
        if let Some(sv) = l.saved.as_ref().filter(|_| !l.writing()) {
            for qa in &sv.qa {
                v.push(Item::Text(format!("You asked: {}", qa.q), MUTED));
                v.push(Item::Text(qa.a.clone(), TEXT));
            }
        }
        if let Some(a) = &l.answer {
            v.push(Item::Text(format!("You asked: {}", l.asked), MUTED));
            v.push(Item::Text(if a.is_empty() { "…".to_string() } else { a.clone() }, TEXT));
        }
    }

    fn delete_buttons(&self, v: &mut Vec<Item>, label: &str) {
        if self.confirm_delete {
            v.push(Item::Text("Delete it for good (a plan's calendar events go with it)?".into(), AMBER));
            v.push(Item::Buttons(vec![("Yes, delete".into(), Event::Tab(T_YES), ERR), ("No".into(), Event::Tab(T_NO), MUTED)]));
        } else {
            v.push(Item::Buttons(vec![(label.into(), Event::Tab(T_DELETE), MUTED)]));
        }
    }

    pub(super) fn draw(&mut self, g: &mut Gfx, area: R, s: f32) {
        let m = 16.0 * s;
        let (x, w) = (area.x + m, (area.w - 2.0 * m).max(40.0));
        let mut y = area.y + 10.0 * s;
        let tabs: Vec<(String, Event)> = vec![("PLANS".into(), Event::Tab(T_PLANS)), ("QUIZZES".into(), Event::Tab(T_QUIZZES))];
        fx::tab_strip(g, x, y, s, w, &tabs, usize::from(self.tab == Tab::Quizzes), ACCENT);
        y += 14.0 * s * 2.1 + 10.0 * s;
        let msg_h = if self.msg.is_some() { 30.0 * s } else { 0.0 };
        let view = R::new(x, y, w, (area.y + area.h - y - msg_h - 6.0 * s).max(0.0));
        self.draw_items(g, view, s);
        if let Some((m, ok)) = self.msg.clone() {
            let my = area.y + area.h - msg_h - 2.0 * s;
            let t = g.fit(14.0 * s, &m, Face::Sans, w - 24.0 * s);
            fx::status_line(g, x, my, w, s, &t, ok);
            g.hit(R::new(x, my - 4.0 * s, w, msg_h), Event::Tab(T_DISMISS), "dismiss message");
        }
    }

    fn draw_items(&mut self, g: &mut Gfx, view: R, s: f32) {
        let items = self.items();
        let bar = 18.0 * s;
        let iw = (view.w - bar).max(40.0 * s);
        let (pad, gap) = (10.0 * s, 6.0 * s);
        let (hs, ts, rs, ss) = (17.0 * s, 14.5 * s, 14.5 * s, 13.0 * s);
        let (hl, tl, rl, sl) = (g.line_height(hs, Face::Bold), g.line_height(ts, Face::Sans), g.line_height(rs, Face::Bold), g.line_height(ss, Face::Sans));
        let bsize = 13.5 * s;
        let bh = bsize * 2.1;

        // measure
        enum Laid {
            Lines(Vec<String>, f32),
            Row(Vec<String>, Vec<String>),
            Btns(Vec<usize>),
            Sec(Vec<String>),
            Pic(Vec<String>, f32, f32),
            Input,
        }
        let mut laid = Vec::new();
        let mut heights = Vec::new();
        for it in &items {
            let (l, h) = match it {
                Item::Head(t) => {
                    let ls = g.wrap(hs, t, Face::Bold, iw);
                    let h = ls.len() as f32 * hl + gap;
                    (Laid::Lines(ls, hs), h)
                }
                Item::Text(t, _) => {
                    let ls = g.wrap(ts, t, Face::Sans, iw);
                    let h = ls.len() as f32 * tl + gap;
                    (Laid::Lines(ls, ts), h)
                }
                Item::Row { title, sub, .. } => {
                    let a = g.wrap(rs, title, Face::Bold, iw - 2.0 * pad - 6.0 * s);
                    let b = if sub.is_empty() { vec![] } else { g.wrap(ss, sub, Face::Sans, iw - 2.0 * pad - 6.0 * s) };
                    let h = 2.0 * pad * 0.8 + a.len() as f32 * rl + b.len() as f32 * sl + gap;
                    (Laid::Row(a, b), h)
                }
                Item::Sec { title, .. } => {
                    let ls = g.wrap(hs * 0.95, title, Face::Bold, iw - 16.0 * s);
                    let h = ls.len() as f32 * hl + gap * 1.5;
                    (Laid::Sec(ls), h)
                }
                Item::Pic { caption, .. } => {
                    let sw = iw.min(560.0 * s);
                    let sh = (sw * 0.6).min(view.h * 0.75).max(80.0 * s).min(sw);
                    let ls = g.wrap(ss, caption, Face::Sans, iw);
                    (Laid::Pic(ls.clone(), sw, sh), sh + gap + ls.len() as f32 * sl + gap)
                }
                Item::Input => (Laid::Input, 38.0 * s + 2.0 * gap),
                Item::Buttons(bs) => {
                    // break the buttons into rows that fit
                    let (mut x, mut rows) = (0.0f32, 1usize);
                    let mut row_of = Vec::new();
                    for (label, _, _) in bs {
                        let label = g.fit(bsize, label, Face::Bold, (iw - bsize * 1.7).max(20.0));
                        let bw = g.text_width(bsize, &label, Face::Bold) + bsize * 1.7;
                        if x > 0.0 && x + bw > iw {
                            x = 0.0;
                            rows += 1;
                        }
                        row_of.push(rows - 1);
                        x += bw + 8.0 * s;
                    }
                    (Laid::Btns(row_of), rows as f32 * (bh + gap) + gap)
                }
            };
            laid.push(l);
            heights.push(h);
        }
        let content: f32 = heights.iter().sum();
        self.list.layout(view, content, 40.0 * s, s);
        // keep the chosen row in view
        let mut top = 0.0;
        for ((it, h), _) in items.iter().zip(&heights).zip(&laid) {
            if matches!(it, Item::Row { selected: true, .. } | Item::Sec { jump: true, .. }) {
                self.list.show(top, top + h);
            }
            top += h;
        }
        let off = self.list.offset;
        g.clipped(view, |g| {
            let mut y = view.y - off;
            for ((it, l), h) in items.iter().zip(&laid).zip(&heights) {
                if y + h >= view.y && y <= view.y + view.h {
                    match (it, l) {
                        (Item::Head(_), Laid::Lines(ls, size)) => {
                            for (k, t) in ls.iter().enumerate() {
                                g.text(view.x, y + k as f32 * hl, *size, TEXT, t, Face::Bold);
                            }
                        }
                        (Item::Text(_, c), Laid::Lines(ls, size)) => {
                            for (k, t) in ls.iter().enumerate() {
                                g.text(view.x, y + k as f32 * tl, *size, *c, t, Face::Sans);
                            }
                        }
                        (Item::Row { accent, selected, ev, .. }, Laid::Row(a, b)) => {
                            let rh = h - gap;
                            let r = R::new(view.x, y, iw, rh);
                            fx::row_at(g, r.x, r.y, r.w, r.h, 8.0 * s, *selected, g.hovered(r), *accent);
                            g.hit(r, ev.clone(), "row");
                            let mut ty = y + pad * 0.8;
                            for t in a {
                                g.text(view.x + pad + 6.0 * s, ty, rs, TEXT, t, Face::Bold);
                                ty += rl;
                            }
                            for t in b {
                                g.text(view.x + pad + 6.0 * s, ty, ss, MUTED, t, Face::Sans);
                                ty += sl;
                            }
                        }
                        (Item::Sec { current, ev, .. }, Laid::Sec(ls)) => {
                            let hh = ls.len() as f32 * hl;
                            g.rrect(view.x, y, 4.0 * s, hh, 2.0 * s, if *current { ACCENT } else { crate::ui::DIM });
                            for (k, t) in ls.iter().enumerate() {
                                g.text(view.x + 12.0 * s, y + k as f32 * hl, hs * 0.95, if *current { ACCENT } else { TEXT }, t, Face::Bold);
                            }
                            g.hit(R::new(view.x, y, iw, hh), ev.clone(), "section");
                        }
                        (Item::Pic { id, bytes, .. }, Laid::Pic(ls, sw, sh)) => {
                            let slot = R::new(view.x, y, *sw, *sh);
                            g.rrect(slot.x, slot.y, slot.w, slot.h, 10.0 * s, rgb(14, 18, 36));
                            let shown = match bytes {
                                Some(b) => {
                                    let ident = (*id, slot.w.round() as i32, slot.h.round() as i32, (s * 1000.0) as i32);
                                    let at = match self.pic_scaled.iter().position(|e| (e.0, e.1, e.2, e.3) == ident) {
                                        Some(i) => Ok(i),
                                        None => self.pic_cache.draw_bytes(g, *id, b, slot.w, s).and_then(|rd| {
                                            let (pw, ph) = (rd.pm.width() as f32, rd.pm.height() as f32);
                                            let k = (slot.w / pw).min(slot.h / ph);
                                            let (iw2, ih2) = ((pw * k).round().max(1.0) as u32, (ph * k).round().max(1.0) as u32);
                                            let mut scaled = tiny_skia::Pixmap::new(iw2, ih2).ok_or("the picture is too small to show")?;
                                            let paint = tiny_skia::PixmapPaint { quality: tiny_skia::FilterQuality::Bilinear, ..Default::default() };
                                            scaled.draw_pixmap(0, 0, rd.pm.as_ref().as_ref(), &paint, tiny_skia::Transform::from_scale(iw2 as f32 / pw, ih2 as f32 / ph), None);
                                            if self.pic_scaled.len() >= 8 {
                                                self.pic_scaled.remove(0);
                                            }
                                            self.pic_scaled.push((ident.0, ident.1, ident.2, ident.3, scaled));
                                            Ok(self.pic_scaled.len() - 1)
                                        }),
                                    };
                                    match at {
                                        Ok(i) => {
                                            let sc = &self.pic_scaled[i].4;
                                            let (dx, dy) = ((slot.x + (slot.w - sc.width() as f32) / 2.0).round() as i32, (slot.y + (slot.h - sc.height() as f32) / 2.0).round() as i32);
                                            g.pm.draw_pixmap(dx, dy, sc.as_ref(), &tiny_skia::PixmapPaint::default(), tiny_skia::Transform::identity(), None);
                                            true
                                        }
                                        Err(e) => {
                                            let m = g.fit(12.0 * s, &format!("The picture cannot be shown: {e}"), Face::Sans, slot.w - 16.0 * s);
                                            g.text(slot.x + 8.0 * s, slot.y + slot.h / 2.0, 12.0 * s, ERR, &m, Face::Sans);
                                            true
                                        }
                                    }
                                }
                                None => false,
                            };
                            if !shown {
                                let m = g.fit(13.0 * s, "Drawing the picture…", Face::Sans, slot.w - 16.0 * s);
                                g.text(slot.x + 10.0 * s, slot.y + slot.h / 2.0 - 7.0 * s, 13.0 * s, MUTED, &m, Face::Sans);
                            }
                            g.rrect_outline(slot.x, slot.y, slot.w, slot.h, 10.0 * s, 1.0, crate::fx::with_alpha(ACCENT, 90));
                            for (k, t) in ls.iter().enumerate() {
                                g.text(view.x, y + slot.h + gap * 0.5 + k as f32 * sl, ss, MUTED, t, Face::Sans);
                            }
                        }
                        (Item::Input, Laid::Input) => {
                            if let View::Lesson(l) = &self.view {
                                if let Some(f) = &l.input {
                                    f.render(g, R::new(view.x, y + gap, iw, 38.0 * s), true, s, true, Face::Sans, FIELD_ID);
                                }
                            }
                        }
                        (Item::Buttons(bs), Laid::Btns(row_of)) => {
                            let mut x = 0.0f32;
                            let mut cur = 0usize;
                            for ((label, ev, c), row) in bs.iter().zip(row_of) {
                                if *row != cur {
                                    cur = *row;
                                    x = 0.0;
                                }
                                let label = g.fit(bsize, label, Face::Bold, (iw - bsize * 1.7).max(20.0));
                                x += g.button(view.x + x, y + gap * 0.5 + *row as f32 * (bh + gap), bsize, &label, *c, ev.clone()) + 8.0 * s;
                            }
                        }
                        _ => {}
                    }
                }
                y += h;
            }
        });
        if let View::Lesson(l) = &mut self.view {
            l.jump = false;
        }
        self.list.draw_bar(g, s, A_LIST, ACCENT);
    }
}
