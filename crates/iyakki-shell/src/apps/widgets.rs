//! Modal forms, a filterable picker, and a small drop-down select (closed box and open list), shared by the apps.

use crate::safeclamp::SafeClamp;
use crate::gfx::{rgb, Face, Gfx};
use crate::ui::{Event, Key, TextField, DIM, ERR, MUTED, R, TEXT};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FieldKind {
    Text,
    /// Shown as bullets; the value is never echoed back after saving.
    Secret,
    Choice(Vec<String>),
    /// A path: a text box with a Browse button and a live chip (exists / file / folder), see `filepicker.rs`.
    Path(crate::filepicker::PathKind, Vec<String>),
}

#[derive(Clone, Debug)]
pub struct FormField {
    pub label: String,
    pub kind: FieldKind,
    pub tf: TextField,
    pub choice: usize,
    pub hint: String,
    /// Example words shown under a text field as clickable chips: rows of (row label, words). Choosing one adds it to the
    /// field's comma separated words (choosing it again takes it out).
    pub chips: Vec<(String, Vec<String>)>,
    /// The chip the keyboard is on (Ctrl+Left / Ctrl+Right move it, Ctrl+E picks it), counted across all rows.
    pub chip_sel: usize,
}

/// `Event::Tab(CHIP_BASE + field * 100 + n)` is a click on chip number `n` under field `field`.
pub const CHIP_BASE: usize = 4000;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FormEvent {
    None,
    Submit,
    Cancel,
    /// A choice field changed to this index (lets the owner refresh dependent defaults).
    Changed(usize),
}

#[derive(Clone, Debug)]
pub struct Form {
    pub title: String,
    pub fields: Vec<FormField>,
    pub focus: usize,
    pub error: Option<String>,
    /// Words under the title that explain the form (how to get a key, for example); wrapped to the width.
    pub note: String,
    /// The file picker opened from a path field (it takes every event while open).
    pub picker: std::cell::RefCell<Option<Box<crate::filepicker::FilePicker>>>,
    /// The last check of each path field: (the text checked, the answer).
    pcache: std::cell::RefCell<Vec<Option<(String, crate::filepicker::Check)>>>,
}

impl Form {
    pub fn new(title: &str) -> Form {
        Form { title: title.into(), fields: vec![], focus: 0, error: None, note: String::new(), picker: Default::default(), pcache: Default::default() }
    }

    pub fn with_note(mut self, note: &str) -> Form {
        self.note = note.into();
        self
    }

    pub fn text(mut self, label: &str, init: &str, hint: &str) -> Form {
        let mut tf = TextField::new("", false);
        tf.set(init); // caret goes to the end of the pre-filled text
        tf.max = 4096;
        self.fields.push(FormField { label: label.into(), kind: FieldKind::Text, tf, choice: 0, hint: hint.into(), chips: vec![], chip_sel: 0 });
        self
    }

    /// A path field: the text box of [`Form::text`] plus Browse and a live check. `exts` limits files (empty: any).
    pub fn path(mut self, label: &str, init: &str, hint: &str, kind: crate::filepicker::PathKind, exts: &[&str]) -> Form {
        self = self.text(label, init, hint);
        if let Some(f) = self.fields.last_mut() {
            f.kind = FieldKind::Path(kind, exts.iter().map(|e| e.to_string()).collect());
        }
        self
    }

    /// The check of path field `i` for what it holds now (looked at again only when the text changed).
    pub fn path_check(&self, i: usize) -> Option<crate::filepicker::Check> {
        let f = self.fields.get(i)?;
        let FieldKind::Path(kind, exts) = &f.kind else { return None };
        let mut cache = self.pcache.borrow_mut();
        if cache.len() <= i {
            cache.resize(i + 1, None);
        }
        if let Some((t, c)) = &cache[i] {
            if *t == f.tf.value {
                return Some(c.clone());
            }
        }
        let c = crate::filepicker::check(&crate::filepicker::current_scope(), *kind, exts, &f.tf.value);
        cache[i] = Some((f.tf.value.clone(), c.clone()));
        Some(c)
    }

    /// Open the picker for path field `i`, starting where the box points.
    pub fn browse(&mut self, i: usize) {
        let Some(f) = self.fields.get(i) else { return };
        let FieldKind::Path(kind, exts) = &f.kind else { return };
        self.focus = i;
        let title = match kind {
            crate::filepicker::PathKind::Folder => "Choose a folder",
            crate::filepicker::PathKind::File => "Choose a file",
            crate::filepicker::PathKind::Save => "Save as",
        };
        *self.picker.get_mut() = Some(Box::new(crate::filepicker::FilePicker::new(*kind, exts, title, f.tf.value.trim(), crate::filepicker::current_scope())));
    }

    /// Fill path field `i` (a drop, or the picker's answer) and forget the old check.
    fn fill_path(&mut self, i: usize, p: &str) {
        self.fields[i].tf.set(p);
        self.focus = i;
    }

    pub fn secret(mut self, label: &str, hint: &str) -> Form {
        self.fields.push(FormField { label: label.into(), kind: FieldKind::Secret, tf: TextField::new("", true), choice: 0, hint: hint.into(), chips: vec![], chip_sel: 0 });
        self
    }

    pub fn choice(mut self, label: &str, options: &[&str], selected: usize) -> Form {
        self.fields.push(FormField { label: label.into(), kind: FieldKind::Choice(options.iter().map(|s| s.to_string()).collect()), tf: TextField::new("", false), choice: selected, hint: "← → to change".into(), chips: vec![], chip_sel: 0 });
        self
    }

    /// Show example words under field `i` (see [`FormField::chips`]).
    pub fn with_chips(mut self, i: usize, rows: Vec<(String, Vec<String>)>) -> Form {
        self.set_chips(i, rows);
        self
    }

    pub fn set_chips(&mut self, i: usize, rows: Vec<(String, Vec<String>)>) {
        if let Some(f) = self.fields.get_mut(i) {
            if f.chips != rows {
                f.chips = rows;
                f.chip_sel = 0;
            }
        }
    }

    /// All chip words of field `i`, in the order they are drawn.
    pub fn chip_words(&self, i: usize) -> Vec<String> {
        self.fields.get(i).map(|f| f.chips.iter().flat_map(|r| r.1.iter().cloned()).collect()).unwrap_or_default()
    }

    /// Choose chip `n` of field `i`: its word is added to the field's words, or taken out when it is there already.
    pub fn pick_chip(&mut self, i: usize, n: usize) -> bool {
        let Some(word) = self.chip_words(i).get(n).cloned() else { return false };
        let f = &mut self.fields[i];
        let mut words: Vec<String> = f.tf.value.split(',').map(|w| w.trim().to_string()).filter(|w| !w.is_empty()).collect();
        match words.iter().position(|w| w.eq_ignore_ascii_case(&word)) {
            Some(p) => {
                words.remove(p);
            }
            None => words.push(word),
        }
        f.tf.set(&words.join(", "));
        true
    }

    pub fn set_label(&mut self, i: usize, label: &str) {
        if let Some(f) = self.fields.get_mut(i) {
            if f.label != label {
                f.label = label.to_string();
            }
        }
    }

    pub fn value(&self, i: usize) -> String {
        self.fields[i].tf.value.trim().to_string()
    }

    /// Secret values keep their exact text (no trimming of intentional spaces other than ends).
    pub fn raw(&self, i: usize) -> String {
        self.fields[i].tf.value.clone()
    }

    pub fn set_choice(&mut self, i: usize, n: usize) {
        if let Some(f) = self.fields.get_mut(i) {
            f.choice = n;
        }
    }

    pub fn choice_idx(&self, i: usize) -> usize {
        self.fields[i].choice
    }

    pub fn set_value(&mut self, i: usize, v: &str) {
        self.fields[i].tf.set(v);
    }

    /// Keyboard, paste and mouse events for the dialog.
    pub fn handle_event(&mut self, ev: &Event) -> FormEvent {
        if let Some(p) = self.picker.get_mut().as_mut() {
            match p.handle_event(ev) {
                crate::filepicker::PickOut::Stay => {}
                crate::filepicker::PickOut::Cancel => *self.picker.get_mut() = None,
                crate::filepicker::PickOut::Chosen(path) => {
                    *self.picker.get_mut() = None;
                    let i = self.focus.min(self.fields.len().saturating_sub(1));
                    if self.fields.get(i).is_some() {
                        self.fill_path(i, &path.to_string_lossy());
                        return FormEvent::Changed(i);
                    }
                }
            }
            return FormEvent::None;
        }
        if let Some(d) = crate::filepicker::dropped_text(ev) {
            // a dropped file or folder fills the path field that has the keyboard, or the first one
            let at = if matches!(self.fields.get(self.focus).map(|f| &f.kind), Some(FieldKind::Path(..))) { Some(self.focus) } else { self.fields.iter().position(|f| matches!(f.kind, FieldKind::Path(..))) };
            if let Some(i) = at {
                let mut text = d.to_string();
                if matches!(self.fields[i].kind, FieldKind::Path(crate::filepicker::PathKind::Folder, _)) && std::path::Path::new(&text).is_file() {
                    text = std::path::Path::new(&text).parent().map_or(text.clone(), |p| p.to_string_lossy().into_owned());
                }
                self.fill_path(i, &text);
                return FormEvent::Changed(i);
            }
            return FormEvent::None;
        }
        match ev {
            Event::Tab(n) if (crate::filepicker::BROWSE_TAB..crate::filepicker::BROWSE_TAB + 100).contains(n) => {
                self.browse(*n - crate::filepicker::BROWSE_TAB);
                FormEvent::None
            }
            Event::Key(Key::Ctrl('o')) if matches!(self.fields.get(self.focus).map(|f| &f.kind), Some(FieldKind::Path(..))) => {
                self.browse(self.focus);
                FormEvent::None
            }
            Event::Key(k) => self.handle(k),
            Event::Paste(t) => {
                if let Some(f) = self.fields.get_mut(self.focus) {
                    if matches!(f.kind, FieldKind::Text | FieldKind::Secret | FieldKind::Path(..)) {
                        f.tf.paste(t);
                    }
                }
                FormEvent::None
            }
            Event::FocusAt(i, x) => {
                if *i < self.fields.len() {
                    self.focus = *i;
                    if matches!(self.fields[*i].kind, FieldKind::Text | FieldKind::Secret | FieldKind::Path(..)) {
                        self.fields[*i].tf.click(*x);
                    }
                }
                FormEvent::None
            }
            // a click on an example chip
            Event::Tab(n) if *n >= CHIP_BASE => {
                let (i, c) = ((*n - CHIP_BASE) / 100, (*n - CHIP_BASE) % 100);
                if i < self.fields.len() && self.pick_chip(i, c) {
                    self.focus = i;
                    self.fields[i].chip_sel = c;
                    return FormEvent::Changed(i);
                }
                FormEvent::None
            }
            // clicks on a choice field's arrows: Tab(2i) = previous, Tab(2i+1) = next
            Event::Tab(n) => {
                let (i, next) = (n / 2, n % 2 == 1);
                self.focus = i.min(self.fields.len().saturating_sub(1));
                self.handle(&if next { Key::Right } else { Key::Left })
            }
            _ => FormEvent::None,
        }
    }

    pub fn handle(&mut self, k: &Key) -> FormEvent {
        let n = self.fields.len().max(1);
        match k {
            Key::Esc => return FormEvent::Cancel,
            Key::Ctrl('s') => return FormEvent::Submit,
            Key::Enter => {
                if self.focus + 1 < n {
                    self.focus += 1; // Enter walks through the fields, and submits on the last
                    return FormEvent::None;
                }
                return FormEvent::Submit;
            }
            Key::Nav { nav: crate::ui::Nav::Right | crate::ui::Nav::Left, shift: false, ctrl: true } if self.fields.get(self.focus).is_some_and(|f| !f.chips.is_empty()) => {
                let words = self.chip_words(self.focus).len().max(1);
                let f = &mut self.fields[self.focus];
                f.chip_sel = if matches!(k, Key::Nav { nav: crate::ui::Nav::Right, .. }) { (f.chip_sel + 1) % words } else { (f.chip_sel + words - 1) % words };
            }
            Key::Ctrl('e') if self.fields.get(self.focus).is_some_and(|f| !f.chips.is_empty()) => {
                let (i, c) = (self.focus, self.fields[self.focus].chip_sel);
                if self.pick_chip(i, c) {
                    return FormEvent::Changed(i);
                }
            }
            Key::Tab | Key::Down => self.focus = (self.focus + 1) % n,
            Key::BackTab | Key::Up => self.focus = (self.focus + n - 1) % n,
            Key::Left | Key::Right => {
                if let Some(FormField { kind: FieldKind::Choice(opts), choice, .. }) = self.fields.get_mut(self.focus) {
                    let len = opts.len();
                    *choice = if *k == Key::Right { (*choice + 1) % len } else { (*choice + len - 1) % len };
                    return FormEvent::Changed(self.focus);
                }
                if let Some(f) = self.fields.get_mut(self.focus) {
                    f.tf.handle(k); // caret movement inside text fields
                }
            }
            other => {
                if let Some(f) = self.fields.get_mut(self.focus) {
                    if !matches!(f.kind, FieldKind::Text | FieldKind::Secret | FieldKind::Path(..)) {
                        return FormEvent::None;
                    }
                    f.tf.max = 4096;
                    f.tf.handle(other);
                }
            }
        }
        FormEvent::None
    }

    /// Where the chips of field `f` go, relative to the field's left edge and the top of the chip area, for a width `w`:
    /// (row label and its y, then each chip as x, y, width, number), and the height used.
    #[allow(clippy::type_complexity)]
    fn chip_layout(&self, g: &mut Gfx, s: f32, w: f32, f: &FormField, row_h: f32, with_labels: bool) -> (Vec<(String, f32)>, Vec<(f32, f32, f32, usize)>, f32) {
        let (mut labels, mut chips) = (vec![], vec![]);
        let (mut y, mut n) = (0.0, 0usize);
        for (label, words) in &f.chips {
            let mut x = 0.0;
            if with_labels && !label.is_empty() {
                labels.push((label.clone(), y));
                x = g.text_width(11.0 * s, label, Face::Bold).max(54.0 * s) + 10.0 * s;
            }
            for word in words {
                let cw = g.text_width(12.0 * s, word, Face::Bold) + 16.0 * s;
                if x + cw > w && x > 0.0 {
                    x = if with_labels && !label.is_empty() { g.text_width(11.0 * s, label, Face::Bold).max(54.0 * s) + 10.0 * s } else { 0.0 };
                    y += row_h;
                }
                chips.push((x, y, cw, n));
                x += cw + 6.0 * s;
                n += 1;
            }
            y += row_h;
        }
        (labels, chips, y)
    }

    pub fn render(&self, g: &mut Gfx, area: R, s: f32, accent: [u8; 4], blink: bool) {
        // vertical room: the spacing of the fields shrinks, then the chips lose their row labels, when the window is small
        let avail = area.h - 16.0 * s;
        let inner_w = (640.0 * s).min(area.w - 40.0 * s) - 52.0 * s;
        let chip_row = 28.0 * s;
        let mut pitch = 78.0 * s;
        let mut labels_on = true;
        let note_h = |g: &mut Gfx, me: &Form| -> f32 {
            if me.note.is_empty() {
                0.0
            } else {
                g.wrap(13.0 * s, &me.note, Face::Sans, inner_w).len() as f32 * 13.0 * s * 1.45 + 10.0 * s
            }
        };
        let ideal = |g: &mut Gfx, pitch: f32, labels_on: bool, me: &Form| -> f32 {
            let chips: f32 = me.fields.iter().map(|f| if f.chips.is_empty() { 0.0 } else { me.chip_layout(g, s, inner_w, f, chip_row, labels_on).2 + 4.0 * s }).sum();
            note_h(g, me) + 112.0 * s + pitch * me.fields.len() as f32 + chips + if me.error.is_some() { 30.0 * s } else { 0.0 }
        };
        let mut h = ideal(g, pitch, labels_on, self);
        while h > avail && pitch > 62.0 * s {
            pitch -= 4.0 * s;
            h = ideal(g, pitch, labels_on, self);
        }
        if h > avail {
            labels_on = false;
            h = ideal(g, pitch, labels_on, self);
        }
        let p = crate::fx::modal(g, area, s, 640.0 * s, h, accent, false);
        let (x, y, w, h) = (p.x, p.y, p.w, p.h);
        crate::fx::modal_title(g, p, s, &self.title, accent);
        let mut fy = y + 62.0 * s;
        if !self.note.is_empty() {
            for line in g.wrap(13.0 * s, &self.note, Face::Sans, inner_w) {
                g.text(x + 26.0 * s, fy - 2.0 * s, 13.0 * s, MUTED, &line, Face::Sans);
                fy += 13.0 * s * 1.45;
            }
            fy += 10.0 * s;
        }
        for (i, f) in self.fields.iter().enumerate() {
            let focused = i == self.focus;
            g.text(x + 26.0 * s, fy, 13.0 * s, if focused { TEXT } else { MUTED }, &f.label, Face::Bold);
            let r = R::new(x + 26.0 * s, fy + 20.0 * s, w - 52.0 * s, 38.0 * s);
            match &f.kind {
                FieldKind::Choice(opts) => {
                    crate::fx::field_frame(g, r, s, focused, accent);
                    g.text(r.x + 14.0 * s, r.y + 8.0 * s, 17.0 * s, TEXT, &format!("◀  {}  ▶", opts[f.choice]), Face::Sans);
                    // left half steps back, right half steps forward
                    g.hit(R::new(r.x, r.y, r.w / 2.0, r.h), Event::Tab(i * 2), "choice-prev");
                    g.hit(R::new(r.x + r.w / 2.0, r.y, r.w / 2.0, r.h), Event::Tab(i * 2 + 1), "choice-next");
                }
                FieldKind::Path(kind, _) => {
                    // the box, a Browse button at its right, and the live check on the label's line
                    crate::filepicker::note_path_field();
                    let bw = g.text_width(13.0 * s, "Browse…", Face::Bold) + 24.0 * s;
                    let bw = bw.min((r.w / 2.0).max(0.0));
                    let tr = R::new(r.x, r.y, (r.w - bw - 6.0 * s).max(0.0), r.h);
                    f.tf.render(g, tr, focused, s, blink, Face::Sans, i);
                    let br = R::new(tr.x + tr.w + 6.0 * s, r.y, bw, r.h);
                    let hot = g.hovered(br);
                    crate::fx::pill(g, br, s, accent, hot, true);
                    let lbl = g.fit(13.0 * s, "Browse…", Face::Bold, (bw - 8.0 * s).max(0.0));
                    g.text(br.x + 12.0 * s, br.y + (br.h - g.line_height(13.0 * s, Face::Bold)) / 2.0, 13.0 * s, TEXT, &lbl, Face::Bold);
                    g.hit(br, Event::Tab(crate::filepicker::BROWSE_TAB + i), "Browse");
                    if let Some(c) = self.path_check(i) {
                        crate::filepicker::draw_chip(g, r.x + r.w, fy + 1.0 * s, s, &c, *kind);
                    }
                }
                _ => f.tf.render(g, r, focused, s, blink, Face::Sans, i),
            }
            if focused && !f.hint.is_empty() && f.tf.value.is_empty() && matches!(f.kind, FieldKind::Text | FieldKind::Secret | FieldKind::Path(..)) {
                let hint = g.fit(14.0 * s, &f.hint, Face::Sans, r.w - 60.0 * s);
                g.text(r.x + 14.0 * s, r.y + 9.0 * s, 14.0 * s, DIM, &hint, Face::Sans);
            }
            fy += pitch;
            if !f.chips.is_empty() {
                // example chips under the field: a click puts the word in the field
                let (labs, chips, ch) = self.chip_layout(g, s, r.w, f, chip_row, labels_on);
                let top = r.y + r.h + 4.0 * s;
                let words: Vec<String> = f.tf.value.split(',').map(|w| w.trim().to_lowercase()).collect();
                for (label, ly) in &labs {
                    g.text(r.x, top + ly + 6.0 * s, 11.0 * s, DIM, label, Face::Bold);
                }
                let all = self.chip_words(i);
                for (cx, cy, cw, n) in chips {
                    let rect = R::new(r.x + cx, top + cy + 2.0 * s, cw, 22.0 * s);
                    let on = words.contains(&all[n].to_lowercase());
                    let kb = focused && n == f.chip_sel;
                    let hot = g.hovered(rect);
                    g.rrect(rect.x, rect.y, rect.w, rect.h, 11.0 * s, if on { crate::fx::with_alpha(accent, 70) } else if hot { rgb(40, 50, 86) } else { rgb(24, 31, 58) });
                    g.rrect_outline(rect.x, rect.y, rect.w, rect.h, 11.0 * s, if kb { 1.8 * s } else { 1.0 * s }, if on || kb { accent } else { rgb(70, 82, 120) });
                    g.text(rect.x + 8.0 * s, rect.y + 4.0 * s, 12.0 * s, if on { TEXT } else { MUTED }, &all[n], Face::Bold);
                    g.hit(rect, Event::Tab(CHIP_BASE + i * 100 + n), &all[n]);
                }
                fy += ch + 4.0 * s;
            }
        }
        if let Some(e) = &self.error {
            let e = g.fit(14.0 * s, e, Face::Sans, w - 52.0 * s);
            g.text(x + 26.0 * s, fy - 6.0 * s, 14.0 * s, ERR, &e, Face::Sans);
        }
        let by = y + h - 50.0 * s;
        let sw = g.button(x + w - 26.0 * s - 90.0 * s, by, 14.0 * s, "Save", accent, Event::Key(Key::Ctrl('s')));
        let _ = sw;
        g.button(x + w - 26.0 * s - 90.0 * s - 100.0 * s, by, 14.0 * s, "Cancel", MUTED, Event::Key(Key::Esc));
        let help = if self.fields.get(self.focus).is_some_and(|f| !f.chips.is_empty()) { "Tab/click to move · click an example or Ctrl+←/→ then Ctrl+E · Ctrl+V paste" } else { if self.fields.get(self.focus).is_some_and(|f| matches!(f.kind, FieldKind::Path(..))) { "Tab/click to move · Ctrl+V paste or drop a file · Ctrl+O browse" } else { "Tab/click to move · Ctrl+V paste · Ctrl+Shift+V history" } };
        let help = g.fit(12.0 * s, help, Face::Sans, w - 52.0 * s - 210.0 * s);
        g.text(x + 26.0 * s, by + 8.0 * s, 12.0 * s, DIM, &help, Face::Sans);
        // the file picker, when Browse opened it, sits on top of the whole form
        if let Some(p) = self.picker.borrow_mut().as_mut() {
            p.render(g, area, s, accent, blink);
        }
    }
}


/// Yes/No confirmation drawn as a centred dialog with clickable buttons. `y`/Esc work too.
pub fn render_confirm(g: &mut Gfx, area: R, s: f32, text: &str, yes_label: &str) {
    let p = crate::fx::modal(g, area, s, 540.0 * s, 170.0 * s, ERR, true);
    let (bx, by, bw, bh) = (p.x, p.y, p.w, p.h);
    let t = g.fit(19.0 * s, text, Face::Bold, bw - 48.0 * s);
    g.text(bx + 24.0 * s, by + 28.0 * s, 19.0 * s, TEXT, &t, Face::Bold);
    g.text(bx + 24.0 * s, by + 70.0 * s, 14.0 * s, MUTED, "Press y to confirm, or Esc to cancel.", Face::Sans);
    g.button(bx + 24.0 * s, by + bh - 58.0 * s, 14.0 * s, yes_label, ERR, Event::Key(Key::Char('y')));
    g.button(bx + 24.0 * s + 150.0 * s, by + bh - 58.0 * s, 14.0 * s, "Cancel", MUTED, Event::Key(Key::Esc));
}

/// A closed drop-down: a framed box of the control height with the label and a caret. Registers a click that sends `ev`.
/// Returns its rectangle.
#[allow(clippy::too_many_arguments)]
pub fn select_box(g: &mut Gfx, x: f32, y: f32, w: f32, s: f32, label: &str, open: bool, accent: [u8; 4], ev: Event, hit: &str) -> R {
    let m = super::ide_chrome::cur();
    let sz = m.font_small * s + 1.0 * s;
    let r = R::new(x, y, w, super::ide_chrome::tool(s));
    let hot = g.hovered(r);
    g.rrect(r.x, r.y, r.w, r.h, 5.0 * s, if hot || open { rgb(34, 44, 78) } else { crate::fx::WELL });
    g.rrect_outline(r.x, r.y, r.w, r.h, 5.0 * s, if open { 1.5 * s } else { 1.0 }, if open { accent } else if hot { crate::fx::with_alpha(accent, 130) } else { rgb(50, 60, 92) });
    let caret = g.text_width(sz, "▾", Face::Bold);
    let t = g.fit(sz, label, Face::Bold, (w - caret - 22.0 * s).max(10.0));
    let lh = g.line_height(sz, Face::Bold);
    g.text(r.x + 8.0 * s, r.y + (r.h - lh) / 2.0, sz, crate::fx::readable(TEXT, crate::fx::WELL, 4.5), &t, Face::Bold);
    g.text(r.x + r.w - caret - 8.0 * s, r.y + (r.h - lh) / 2.0, sz, crate::fx::readable(MUTED, crate::fx::WELL, 4.5), "▾", Face::Bold);
    g.hit(r, ev, hit);
    r
}

/// The natural width of a select box showing `label`.
pub fn select_width(g: &mut Gfx, s: f32, label: &str) -> f32 {
    let sz = super::ide_chrome::cur().font_small * s + 1.0 * s;
    g.text_width(sz, label, Face::Bold) + g.text_width(sz, "▾", Face::Bold) + 28.0 * s
}

/// An open list of choices next to `anchor` (above it when there is no room below, inside `bounds`). A click anywhere else
/// sends `close`. Each row is (label, chosen, click). Register it last: the later hit wins. Returns the list's rectangle.
#[allow(clippy::too_many_arguments)]
pub fn menu_list(g: &mut Gfx, anchor: R, bounds: R, s: f32, rows: &[(String, bool, Event)], close: Event, accent: [u8; 4]) -> R {
    let sz = super::ide_chrome::cur().font_small * s + 2.0 * s;
    let rh = (super::ide_chrome::tool(s) + 4.0 * s).max(sz * 1.9);
    let w = rows.iter().map(|r| g.text_width(sz, &r.0, Face::Bold)).fold(0.0f32, f32::max) + 40.0 * s;
    let w = w.max(anchor.w).min(bounds.w - 8.0 * s);
    let h = (rows.len() as f32 * rh + 10.0 * s).min(bounds.h - 8.0 * s);
    let x = anchor.x.min(bounds.x + bounds.w - w - 4.0 * s).max(bounds.x + 4.0 * s);
    let below = anchor.y + anchor.h + 4.0 * s;
    let y = if below + h <= bounds.y + bounds.h { below } else { (anchor.y - h - 4.0 * s).max(bounds.y + 4.0 * s) };
    g.hit(bounds, close, "close the menu");
    let r = R::new(x, y, w, h);
    g.rrect(r.x, r.y, r.w, r.h, 8.0 * s, rgb(16, 20, 36));
    g.rrect_outline(r.x, r.y, r.w, r.h, 8.0 * s, 1.5 * s, accent);
    let fit = ((h - 10.0 * s) / rh) as usize;
    for (i, (label, on, ev)) in rows.iter().enumerate().take(fit.max(1)) {
        let rr = R::new(x + 5.0 * s, y + 5.0 * s + i as f32 * rh, w - 10.0 * s, rh - 2.0 * s);
        let hot = g.hovered(rr);
        if hot || *on {
            g.rrect(rr.x, rr.y, rr.w, rr.h, 5.0 * s, if hot { rgb(34, 44, 78) } else { crate::fx::over(crate::fx::WELL, crate::fx::with_alpha(accent, 50)) });
        }
        let t = g.fit(sz, label, Face::Bold, rr.w - 20.0 * s);
        g.text(rr.x + 10.0 * s, rr.y + (rr.h - g.line_height(sz, Face::Bold)) / 2.0, sz, if *on { accent } else { TEXT }, &t, if *on { Face::Bold } else { Face::Sans });
        g.hit(rr, ev.clone(), label);
    }
    r
}

#[derive(Clone, Debug)]
pub struct Picker {
    pub title: String,
    items: Vec<String>,
    filter: String,
    sel: usize,
    /// A small picture for items that are pictures (same order as `items`).
    thumbs: Vec<Option<std::rc::Rc<tiny_skia::Pixmap>>>,
    /// Items that Delete may mark ("don't use this one"), the text that says so in the footer, and the item marked last (`take_removed`).
    deletable: Vec<String>,
    delete_hint: String,
    removed: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PickEvent {
    None,
    Chosen(String),
    Cancel,
}

impl Picker {
    pub fn new(title: &str, items: Vec<String>) -> Picker {
        Picker { title: title.into(), items, filter: String::new(), sel: 0, thumbs: Vec::new(), deletable: Vec::new(), delete_hint: String::new(), removed: None }
    }

    /// Show a thumbnail beside the items that have one, with a Save button (`Event::Tab(1000 + n)` for the item whose label
    /// starts with `n.`).
    pub fn with_thumbs(mut self, thumbs: Vec<Option<std::rc::Rc<tiny_skia::Pixmap>>>) -> Picker {
        self.thumbs = thumbs;
        self
    }

    /// Let Delete mark the highlighted item when it is one of `items` (the footer shows `hint`). The chooser stays open; the caller reads
    /// the marked item with [`Picker::take_removed`] after every event.
    pub fn with_deletable(mut self, items: Vec<String>, hint: &str) -> Picker {
        self.deletable = items;
        self.delete_hint = hint.to_string();
        self
    }

    /// The item Delete marked since the last call, if any.
    pub fn take_removed(&mut self) -> Option<String> {
        self.removed.take()
    }

    pub fn visible(&self) -> Vec<&String> {
        let f = self.filter.to_lowercase();
        self.items.iter().filter(|i| f.is_empty() || i.to_lowercase().contains(&f)).collect()
    }

    /// Keyboard, paste, wheel and click events.
    pub fn handle_event(&mut self, ev: &Event) -> PickEvent {
        match ev {
            Event::Key(k) => self.handle(k),
            Event::Paste(t) => {
                for c in t.chars().filter(|c| !c.is_control()) {
                    self.filter.push(c);
                }
                self.sel = 0;
                PickEvent::None
            }
            Event::Scroll(d) => {
                let n = self.visible().len();
                self.sel = (self.sel as i32 - *d).clamp_safe(0, n.saturating_sub(1) as i32) as usize;
                PickEvent::None
            }
            // a click on row `i` chooses it
            Event::Row(i) => match self.visible().get(*i) {
                Some(item) => PickEvent::Chosen((*item).clone()),
                None => PickEvent::None,
            },
            _ => PickEvent::None,
        }
    }

    /// Index (in the original item list) of the currently highlighted entry.
    pub fn selected_index(&self) -> Option<usize> {
        let v = self.visible();
        let item = v.get(self.sel)?;
        self.items.iter().position(|i| std::ptr::eq(i, *item))
    }

    pub fn handle(&mut self, k: &Key) -> PickEvent {
        let n = self.visible().len();
        match k {
            Key::Esc => return PickEvent::Cancel,
            Key::Enter => {
                if let Some(i) = self.visible().get(self.sel) {
                    return PickEvent::Chosen((*i).clone());
                }
            }
            Key::Up => self.sel = self.sel.saturating_sub(1),
            Key::Down => self.sel = (self.sel + 1).min(n.saturating_sub(1)),
            Key::PageUp => self.sel = self.sel.saturating_sub(8),
            Key::PageDown => self.sel = (self.sel + 8).min(n.saturating_sub(1)),
            Key::Backspace => {
                self.filter.pop();
                self.sel = 0;
            }
            Key::Delete => {
                if let Some(i) = self.visible().get(self.sel) {
                    if self.deletable.contains(*i) {
                        self.removed = Some((*i).clone());
                    }
                }
            }
            Key::Char(c) if !c.is_control() => {
                self.filter.push(*c);
                self.sel = 0;
            }
            _ => {}
        }
        PickEvent::None
    }

    pub fn render(&self, g: &mut Gfx, area: R, s: f32, accent: [u8; 4]) {
        let p = crate::fx::modal(g, area, s, 620.0 * s, 560.0 * s, accent, false);
        let (x, y, w, h) = (p.x, p.y, p.w, p.h);
        crate::fx::modal_title(g, p, s, &self.title, accent);
        let shown = if self.filter.is_empty() { "type to filter…".to_string() } else { format!("filter: {}", self.filter) };
        g.text(x + 26.0 * s, y + 52.0 * s, 14.0 * s, if self.filter.is_empty() { DIM } else { accent }, &shown, Face::Sans);
        let vis = self.visible();
        let has_thumbs = self.thumbs.iter().any(Option::is_some);
        let row_h = if has_thumbs { 54.0 * s } else { 34.0 * s };
        let rows = (((h - 120.0 * s) / row_h) as usize).max(1);
        let start = self.sel.saturating_sub(rows - 1).min(vis.len().saturating_sub(rows));
        for (i, item) in vis.iter().enumerate().skip(start).take(rows) {
            let ry = y + 82.0 * s + (i - start) as f32 * row_h;
            let hot = g.hovered(R::new(x + 16.0 * s, ry, w - 32.0 * s, row_h - 4.0 * s));
            if i == self.sel || hot {
                crate::fx::row_at(g, x + 16.0 * s, ry, w - 32.0 * s, row_h - 4.0 * s, 8.0 * s, i == self.sel, hot, accent);
            }
            let orig = self.items.iter().position(|x| std::ptr::eq(x, *item)).unwrap_or(i);
            let thumb = self.thumbs.get(orig).and_then(|t| t.as_ref());
            let mut tx = x + 28.0 * s;
            if let Some(pm) = thumb {
                let (th, tw) = (row_h - 12.0 * s, (row_h - 12.0 * s) * 1.5);
                let k = (tw / pm.width() as f32).min(th / pm.height() as f32);
                let (dw, dh) = (pm.width() as f32 * k, pm.height() as f32 * k);
                let paint = tiny_skia::PixmapPaint { quality: tiny_skia::FilterQuality::Bilinear, ..Default::default() };
                g.pm.draw_pixmap(0, 0, pm.as_ref().as_ref(), &paint, tiny_skia::Transform::from_translate(tx + (tw - dw) / 2.0, ry + 4.0 * s + (th - dh) / 2.0).pre_scale(k, k), None);
                g.rrect_outline(tx, ry + 4.0 * s, tw, th, 4.0 * s, 1.0 * s, rgb(70, 82, 120));
                tx += tw + 14.0 * s;
            }
            let t = g.fit(16.0 * s, item, Face::Mono, w - 64.0 * s - (tx - x - 28.0 * s) - if thumb.is_some() { 90.0 * s } else { 0.0 });
            g.text(tx, ry + (row_h - 4.0 * s - 20.0 * s) / 2.0, 16.0 * s, TEXT, &t, Face::Mono);
            g.hit(R::new(x + 16.0 * s, ry, w - 32.0 * s, row_h - 4.0 * s), Event::Row(i), item);
            if thumb.is_some() {
                let n = item.trim_start().split('.').next().and_then(|n| n.parse::<usize>().ok()).unwrap_or(0);
                g.button(x + w - 16.0 * s - 74.0 * s, ry + 10.0 * s, 13.0 * s, "Save", accent, Event::Tab(1000 + n));
            }
        }
        if vis.is_empty() {
            g.text(x + 26.0 * s, y + 90.0 * s, 15.0 * s, MUTED, "nothing matches", Face::Sans);
        }
        g.text(x + 26.0 * s, y + h - 34.0 * s, 12.0 * s, DIM, &format!("{} of {} · click or Enter to select{}", vis.len(), self.items.len(), if self.delete_hint.is_empty() { String::new() } else { format!(" · {}", self.delete_hint) }), Face::Sans);
        g.button(x + w - 26.0 * s - 90.0 * s, y + h - 50.0 * s, 14.0 * s, "Cancel", MUTED, Event::Key(Key::Esc));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn typ(f: &mut Form, s: &str) {
        for c in s.chars() {
            f.handle(&Key::Char(c));
        }
    }

    #[test]
    fn form_navigation_and_values() {
        let mut f = Form::new("Add").text("id", "", "").choice("type", &["A", "B", "C"], 0).secret("key", "");
        typ(&mut f, " local ");
        assert_eq!(f.value(0), "local", "trimmed");
        assert_eq!(f.handle(&Key::Enter), FormEvent::None);
        assert_eq!(f.focus, 1);
        assert_eq!(f.handle(&Key::Right), FormEvent::Changed(1));
        assert_eq!(f.handle(&Key::Right), FormEvent::Changed(1));
        assert_eq!(f.handle(&Key::Right), FormEvent::Changed(1), "wraps");
        assert_eq!(f.choice_idx(1), 0);
        assert_eq!(f.handle(&Key::Left), FormEvent::Changed(1));
        assert_eq!(f.choice_idx(1), 2);
        typ(&mut f, "ignored on a choice field");
        assert_eq!(f.fields[1].tf.value, "");
        f.handle(&Key::Tab);
        typ(&mut f, "sk-secret");
        assert_eq!(f.raw(2), "sk-secret");
        assert_eq!(f.handle(&Key::Enter), FormEvent::Submit, "Enter on the last field submits");
        assert_eq!(f.handle(&Key::Esc), FormEvent::Cancel);
        f.handle(&Key::Backspace);
        assert_eq!(f.raw(2), "sk-secre");
        f.handle(&Key::BackTab);
        assert_eq!(f.focus, 1);
    }

    #[test]
    fn picker_filters_and_selects() {
        let items: Vec<String> = ["gpt-4o", "gpt-4o-mini", "claude-sonnet", "llama3.2:3b"].iter().map(|s| s.to_string()).collect();
        let mut p = Picker::new("Models", items);
        assert_eq!(p.visible().len(), 4);
        for c in "GPT".chars() {
            p.handle(&Key::Char(c));
        }
        assert_eq!(p.visible(), vec!["gpt-4o", "gpt-4o-mini"], "case-insensitive substring");
        p.handle(&Key::Down);
        assert_eq!(p.handle(&Key::Enter), PickEvent::Chosen("gpt-4o-mini".into()));
        for _ in 0..3 {
            p.handle(&Key::Backspace);
        }
        p.handle(&Key::Char('z'));
        assert_eq!(p.handle(&Key::Enter), PickEvent::None, "nothing to choose");
        assert_eq!(p.handle(&Key::Esc), PickEvent::Cancel);
        let mut big = Picker::new("t", (0..500).map(|i| format!("model-{i}")).collect());
        big.handle(&Key::PageDown);
        big.handle(&Key::PageDown);
        assert_eq!(big.handle(&Key::Enter), PickEvent::Chosen("model-16".into()));
    }

    #[test]
    fn dialogs_render() {
        let mut g = Gfx::new(900, 600);
        g.clear(rgb(0, 0, 0));
        let mut f = Form::new("Add provider").text("id", "x", "hint").choice("type", &["Ollama", "OpenAI"], 1).secret("API key", "paste key");
        f.error = Some("bad".into());
        f.render(&mut g, R::new(0.0, 70.0, 900.0, 530.0), 1.0, rgb(255, 0, 0), true);
        Picker::new("Models", (0..50).map(|i| format!("m{i}")).collect()).render(&mut g, R::new(0.0, 70.0, 900.0, 530.0), 1.0, rgb(255, 0, 0));
        render_confirm(&mut g, R::new(0.0, 70.0, 900.0, 530.0), 1.0, "Delete?", "Delete");
    }

    #[test]
    fn form_mouse_and_paste() {
        let mut f = Form::new("Add").text("id", "", "").choice("type", &["A", "B", "C"], 0).secret("key", "");
        let mut g = Gfx::new(900, 600);
        f.render(&mut g, R::new(0.0, 70.0, 900.0, 530.0), 1.0, rgb(255, 0, 0), true);
        // the backdrop swallows clicks outside the dialog; fields and buttons are registered on top
        let labels: Vec<&str> = g.hits.iter().map(|h| h.label.as_str()).collect();
        for want in ["backdrop", "field", "choice-prev", "choice-next", "Save", "Cancel"] {
            assert!(labels.contains(&want), "{want} in {labels:?}");
        }
        assert_eq!(labels[0], "backdrop", "nothing from the UI behind the dialog survives");
        // click the third field, paste into it, caret keeps working
        assert_eq!(f.handle_event(&Event::FocusAt(2, 14.0)), FormEvent::None);
        assert_eq!(f.focus, 2);
        f.handle_event(&Event::Paste("sk-pasted-key\n".into()));
        assert_eq!(f.raw(2), "sk-pasted-key");
        // choice arrows
        assert_eq!(f.handle_event(&Event::Tab(2 * 1 + 1)), FormEvent::Changed(1));
        assert_eq!(f.choice_idx(1), 1);
        f.handle_event(&Event::Tab(2 * 1));
        f.handle_event(&Event::Tab(2 * 1));
        assert_eq!(f.choice_idx(1), 2, "wrapped backwards");
        // pasting while a choice field has focus does nothing
        f.handle_event(&Event::Paste("zzz".into()));
        assert_eq!(f.fields[1].tf.value, "");
        // Save button = Ctrl+S, Cancel = Esc
        assert_eq!(f.handle_event(&Event::Key(Key::Ctrl('s'))), FormEvent::Submit);
        assert_eq!(f.handle_event(&Event::Key(Key::Esc)), FormEvent::Cancel);
        // arrows move the caret inside a text field
        f.focus = 0;
        f.handle_event(&Event::Paste("abc".into()));
        f.handle(&Key::Left);
        f.handle(&Key::Char('X'));
        assert_eq!(f.value(0), "abXc");
    }

    #[test]
    fn picker_click_wheel_and_paste_filter() {
        let items: Vec<String> = (0..30).map(|i| format!("model-{i}")).collect();
        let mut p = Picker::new("Models", items);
        let mut g = Gfx::new(900, 600);
        p.render(&mut g, R::new(0.0, 70.0, 900.0, 530.0), 1.0, rgb(255, 0, 0));
        assert!(g.hits.iter().any(|h| h.label == "model-3" && h.ev == Event::Row(3)));
        assert_eq!(p.handle_event(&Event::Row(3)), PickEvent::Chosen("model-3".into()));
        assert_eq!(p.handle_event(&Event::Row(99)), PickEvent::None);
        p.handle_event(&Event::Scroll(-3)); // wheel down 3
        assert_eq!(p.handle_event(&Event::Key(Key::Enter)), PickEvent::Chosen("model-3".into()));
        p.handle_event(&Event::Paste("model-2".into()));
        assert_eq!(p.visible().len(), 11, "model-2 and model-20..29");
        assert_eq!(p.selected_index(), Some(2));
    }
}

/// The private form for putting a credential in the person's own vault: a name (suggested, may be changed) and the secret,
/// shown as bullets. `value` pre-fills the secret (the chat's credential guard offers what it found).
pub fn secret_form(title: &str, name: &str, value: Option<&str>) -> Form {
    let mut f = Form::new(title).text("Vault name", name, "lowercase letters, digits, . _ - ; kept as uvault:<name>").secret("Secret", "typed here, stored in your vault, never shown to the assistant");
    if let Some(v) = value {
        f.set_value(1, v);
    }
    f
}
