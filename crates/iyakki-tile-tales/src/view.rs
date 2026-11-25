//! TALES drawing: the story view, pictures, words, player bar, status line and the library page.

use super::*;

impl StoryApp {
    pub fn story_view(&mut self, g: &mut Gfx, area: R, s: f32, tick: u64) {
        let m = if area.w < 700.0 * s { 14.0 * s } else { 20.0 * s };
        let x = area.x + m;
        let w = area.w - 2.0 * m;
        let mut y = area.y + 8.0 * s;
        y += self.tabs(g, x, y, w, s) + 10.0 * s;
        let empty = self.story.is_empty() && self.gen.is_none();
        if empty {
            let r = R::new(area.x, y, area.w, area.y + area.h - y);
            if let Some(e) = &self.error {
                fx::empty_state(g, r, s, Icon::Tales, "The story could not be written", e, ACCENT);
            } else {
                let hint = if self.pane_open || self.wide { "Describe it in the box below and press Send." } else { "Open Idea & options, describe it and press Send." };
                fx::empty_state(g, r, s, Icon::Tales, "Your story will appear here", hint, ACCENT);
            }
            return;
        }
        let snap = self.reader.snapshot();
        let reading = matches!(snap.status, Status::Playing | Status::Paused) || (matches!(snap.status, Status::Buffering) && self.live);
        let ph = if area.h < 500.0 { 58.0 * s } else { 70.0 * s };
        // one short hint line under the player when no voice is ready, with How for the longer text
        let chip_on = self.chip_wanted();
        let hint_h = if chip_on { 34.0 * s } else if self.read_hint().is_some() { 20.0 * s } else { 0.0 };
        let player = R::new(x, area.y + area.h - ph - 8.0 * s - hint_h, w, ph);
        if chip_on {
            // one actionable chip instead of a message: it looks for a speech engine here, offers the browser voice helper
            // and otherwise opens Settings → Voice
            let fs = 12.0 * s;
            let ty = player.y + player.h + 3.0 * s;
            let label = if self.turning_on { "Looking for a voice…" } else { "Turn on reading aloud" };
            let cw = (g.text_width(13.0 * s, label, Face::Bold) + 13.0 * s * 1.9).min(w);
            chip(g, x + 4.0 * s, ty, s, label, self.turning_on, Event::Tab(T_TURN_ON));
            let hw = g.text_width(fs, "How", Face::Bold) + fs * 1.4;
            let room = w - cw - hw - 28.0 * s;
            let mut tx = x + 4.0 * s + cw + 10.0 * s;
            if room > 70.0 * s {
                let t = g.fit(fs, "Text only for now", Face::Sans, room);
                g.text(tx, ty + (28.0 * s - g.line_height(fs, Face::Sans)) / 2.0, fs, fx::AMBER, &t, Face::Sans);
                tx += g.text_width(fs, &t, Face::Sans) + 8.0 * s;
            }
            let r = R::new(tx, ty, hw, 28.0 * s);
            let hot = g.hovered(r);
            g.text(tx + fs * 0.2, ty + (r.h - g.line_height(fs, Face::Bold)) / 2.0, fs, if hot { ACCENT } else { fx::readable(CYAN, fx::PANEL, 4.5) }, "How", Face::Bold);
            g.rect(tx + fs * 0.2, ty + r.h - 7.0 * s, hw - fs * 0.4, 1.0, if hot { ACCENT } else { with_alpha(CYAN, 120) });
            g.hit(r, Event::Tab(T_HOW), "How");
        } else if let Some((text, col)) = self.read_hint() {
            let fs = 12.0 * s;
            let hw = g.text_width(fs, "How", Face::Bold) + fs * 1.4;
            let ty = player.y + player.h + 2.0 * s;
            let t = g.fit(fs, &text, Face::Sans, (w - hw - 12.0 * s).max(40.0 * s));
            g.text(x + 4.0 * s, ty + (hint_h - 2.0 * s - g.line_height(fs, Face::Sans)) / 2.0, fs, col, &t, Face::Sans);
            let tx = x + 4.0 * s + g.text_width(fs, &t, Face::Sans) + 8.0 * s;
            let r = R::new(tx, ty, hw, hint_h - 2.0 * s);
            let hot = g.hovered(r);
            g.text(tx + fs * 0.2, ty + (r.h - g.line_height(fs, Face::Bold)) / 2.0, fs, if hot { ACCENT } else { fx::readable(CYAN, fx::PANEL, 4.5) }, "How", Face::Bold);
            g.rect(tx + fs * 0.2, ty + r.h - 2.0 * s, hw - fs * 0.4, 1.0, if hot { ACCENT } else { with_alpha(CYAN, 120) });
            g.hit(r, Event::Tab(T_HOW), "How");
        }
        let hh = (area.h * 0.19).clamp_safe(72.0 * s, 150.0 * s);
        let hero = R::new(x, y, w, hh);
        self.hero(g, hero, s, if reading { Some(snap.unit) } else { None });
        y += hh + 10.0 * s;
        let card = R::new(x, y, w, (player.y - 8.0 * s - y).max(40.0 * s));
        let glow = if matches!(snap.status, Status::Playing) { 0.3 + 0.7 * self.level } else { 0.15 };
        let ring = glow_card_keep(g, card, s, ACCENT, MAGENTA, glow);
        let pad = 18.0 * s;
        let mut ty = card.y + 12.0 * s;
        let tw = card.w - 2.0 * pad - 12.0 * s;
        let nh = self.banner(g, card.x + pad, ty, tw, s);
        ty += nh;
        let area_txt = R::new(card.x + pad, ty, tw, (card.y + card.h - ty - 12.0 * s).max(20.0));
        let size = 16.0 * s;
        self.ensure_layout(g, size, area_txt.w);
        let lh = g.line_height(size, Face::Sans);
        // heights: the units, with a picture slot before each unit that has one
        let gap = 0.7 * lh;
        let (slot_w, slot_h) = Self::pic_slot(area_txt.w, s);
        let mut tops = Vec::with_capacity(self.lay.len());
        let mut slots: Vec<(usize, f32)> = Vec::new(); // (picture index, y offset)
        let mut slot_rects: Vec<(f32, f32)> = Vec::new(); // (y offset, height with the gallery)
        let mut total = 0.0;
        for (i, l) in self.lay.iter().enumerate() {
            if l.kind == UKind::Hidden {
                tops.push(total);
                continue;
            }
            if l.kind == UKind::Heading {
                total += gap * 0.5;
            }
            for (pi, p) in self.pics.iter().enumerate() {
                if p.scene.place_before == i {
                    slots.push((pi, total));
                    slot_rects.push((total, slot_h + self.gallery_h(pi, slot_w, s)));
                    total += slot_h + self.gallery_h(pi, slot_w, s) + gap;
                }
            }
            tops.push(total);
            let hl = if l.kind == UKind::Heading { g.line_height(size * 1.18, Face::Bold) } else { lh };
            total += l.lines.len() as f32 * hl + if l.kind == UKind::Heading { gap * 0.6 } else { gap };
        }
        if self.gen.is_some() {
            total += lh;
        }
        self.tmax = (total - area_txt.h).max(0.0);
        // follow what is being read, or the end of what is being written
        let hl_range = if reading { Some((snap.unit, snap.range)) } else { None };
        // the word being said (inside the sentence), when the setting asks for it
        let cur_word = if reading && self.highlight.word() { snap.word } else { None };
        if let Some((u, (a, _))) = hl_range {
            if self.manual_unit != Some(u) {
                if let Some(l) = self.lay.get(u) {
                    // follow the word when there is one (a long sentence wraps over several lines), else the sentence
                    let a = cur_word.map_or(a, |w| w.0);
                    let k = l.lines.iter().rposition(|(_, st0)| *st0 <= a).unwrap_or(0);
                    let target = (tops[u] + k as f32 * lh - area_txt.h * 0.3).clamp_safe(0.0, self.tmax);
                    self.tscroll += (target - self.tscroll) * 0.25;
                    if (target - self.tscroll).abs() < 1.0 {
                        self.tscroll = target;
                    }
                }
            }
        } else if self.stick_bottom && self.gen.is_some() {
            self.tscroll = self.tmax;
        }
        self.tscroll = self.tscroll.clamp_safe(0.0, self.tmax);
        let scroll = self.tscroll;
        let lay = std::mem::take(&mut self.lay);
        self.pane.begin(area_txt);
        self.text_area = area_txt;
        self.line_map.clear();
        self.line_tops.clear();
        self.word_rect = None;
        self.sentence_rects.clear();
        if cur_word.is_none() {
            self.word_vis = None;
        }
        self.pic_buttons.clear();
        self.gal_hits.clear();
        let base_y = area_txt.y - scroll;
        let tc = TextCtx { area: area_txt, base_y, size, s, hl_range, cur_word, lit: self.highlight.sentence(), tick };
        let word = self.ease_word(g, &lay, &tops, &tc);
        g.clipped(area_txt, |g| {
            for (pi, off) in &slots {
                let slot = R::new(area_txt.x, base_y + off, slot_w, slot_h);
                if slot.y + slot.h >= area_txt.y && slot.y <= area_txt.y + area_txt.h {
                    self.draw_pic(g, slot, s, *pi, tick);
                }
            }
            for (i, l) in lay.iter().enumerate() {
                if l.kind == UKind::Hidden {
                    continue;
                }
                let (sz, face) = if l.kind == UKind::Heading { (size * 1.18, Face::Bold) } else { (size, Face::Sans) };
                let hl = g.line_height(sz, face);
                let ypos = base_y + tops[i];
                if tc.hl_range.is_some_and(|(u, _)| u == i) && tc.lit {
                    // a soft bar along the paragraph being read
                    g.rrect(area_txt.x - 10.0 * s, ypos - 2.0 * s, 4.0 * s, l.lines.len() as f32 * hl + 4.0 * s, 2.0 * s, ACCENT);
                }
                for (li, (line, ls)) in l.lines.iter().enumerate() {
                    let ly = ypos + li as f32 * hl;
                    if ly + hl >= area_txt.y - hl && ly <= area_txt.y + area_txt.h + hl {
                        self.draw_line(g, l, i, li, ly, &tc, &word);
                    }
                    self.line_map.push((i, *ls));
                    self.line_tops.push(ly);
                    self.pane.line(g, area_txt.x, ly, hl, sz, face, line, i as u32, i as u32);
                }
            }
            if self.gen.is_some() && lay.is_empty() {
                g.text(area_txt.x, area_txt.y, size, MUTED, &format!("The storyteller is thinking{}", ".".repeat(1 + (tick / 10 % 3) as usize)), Face::Sans);
            }
        });
        self.lay = lay;
        self.pane.end(g, ACCENT);
        // the buttons of the pictures go on top of the text area's press region
        let gal = std::mem::take(&mut self.gal_hits);
        g.clipped(area_txt, |g| {
            for (r, code, label) in &gal {
                if *code >= T_ALT_MORE {
                    g.button(r.x, r.y, 12.0 * s, label, ACCENT, Event::Tab(*code));
                } else {
                    g.hit(*r, Event::Tab(*code), label);
                }
            }
        });
        let btns = std::mem::take(&mut self.pic_buttons);
        g.clipped(area_txt, |g| {
            for (r, label, idx, col) in &btns {
                g.button(r.x, r.y, 12.0 * s, label, *col, Event::Tab(if *idx >= 100 { T_PIC_DELETE + idx - 100 } else { T_PIC_REDO + idx }));
            }
        });
        if let Some(e) = &self.error {
            // the failure, under the text (or alone)
            let es = g.fit(13.0 * s, e, Face::Sans, area_txt.w);
            g.rrect(area_txt.x - 6.0 * s, area_txt.y + area_txt.h - 24.0 * s, area_txt.w + 12.0 * s, 24.0 * s, 8.0 * s, fx::over(PANEL, with_alpha(ERR, 60)));
            g.text(area_txt.x, area_txt.y + area_txt.h - 20.0 * s, 13.0 * s, ERR, &es, Face::Sans);
        }
        if self.tmax > 0.0 {
            fx::scrollbar(g, R::new(card.x + card.w - 14.0 * s, area_txt.y, 6.0 * s, area_txt.h), s, (self.tscroll / lh) as usize, (area_txt.h / lh) as usize, (total / lh) as usize, ACCENT);
        }
        // the frames after this one repaint only what moves while the story is read aloud, when nothing else is going on
        let bar_rect = glow_bounds(player, s);
        let keep = matches!(snap.status, Status::Playing) && reading && self.partial_possible() && ring.as_ref().is_none_or(|r| r.depth() as f32 <= 8.0 * s) && {
            // the glow of the text card and the one of the player are repainted separately: they must not share pixels
            let c = glow_bounds(card, s);
            c.0 + c.2 <= bar_rect.0 || bar_rect.0 + bar_rect.2 <= c.0 || c.1 + c.3 <= bar_rect.1 || bar_rect.1 + bar_rect.3 <= c.1
        };
        self.tick_off = tick.wrapping_sub(self.ticks);
        let under = if keep { g.grab(bar_rect) } else { Vec::new() };
        self.player_bar(g, player, s, &snap, tick);
        self.anim = if keep {
            let caret = self.focus == Focus::Idea && (self.wide || self.pane_open);
            let slot_screen = slot_rects.iter().map(|(off, h)| R::new(area_txt.x, base_y + off, slot_w, *h)).collect();
            Some(Anim { text: area_txt, base_y, size, s, scroll, tops, unit: snap.unit, range: snap.range, word: cur_word, hl: self.highlight, slots: slot_screen, ring, ring_alpha: glow_alphas(glow), player, bar: bar_rect, bar_under: under, caret, blink: blink(tick), sig: self.view_sig(), streak: 0 })
        } else {
            None
        };
    }

    /// The frames between two whole ones while the story is read aloud: repaint the lines of the sentence and word that were
    /// lit and are lit now, the glow of the text card when the sound level moved it, and the player bar, over the whole frame
    /// drawn last. Returns the rectangles painted, or `None` when something else changed (the whole frame is drawn then).
    pub fn live_animate(&mut self, g: &mut Gfx) -> Option<Vec<(i32, i32, i32, i32)>> {
        let mut a = self.anim.take()?;
        let lay = std::mem::take(&mut self.lay);
        let out = self.animate_in(g, &mut a, &lay);
        self.lay = lay;
        if out.is_some() {
            a.streak += 1;
            self.anim = Some(a);
        }
        out
    }

    pub fn animate_in(&mut self, g: &mut Gfx, a: &mut Anim, lay: &[ULay]) -> Option<Vec<(i32, i32, i32, i32)>> {
        if !self.partial_possible() || !matches!(self.reader.status(), Status::Playing) || a.sig != self.view_sig() || a.streak >= 40 || self.highlight != a.hl {
            return None;
        }
        let tick = self.ticks.wrapping_add(self.tick_off);
        if a.caret && blink(tick) != a.blink {
            return None;
        }
        let snap = self.reader.snapshot();
        let u = a.unit;
        let l = lay.get(u).filter(|l| l.kind != UKind::Hidden)?;
        if snap.unit != u || a.tops.len() != lay.len() || lay.iter().map(|l| l.lines.len()).sum::<usize>() != self.line_map.len() {
            return None;
        }
        let cur_word = if self.highlight.word() { snap.word } else { None };
        // the page scrolls toward the word: that moves every line, a whole frame does it
        let lh = g.line_height(a.size, Face::Sans);
        if self.tscroll != a.scroll {
            return None;
        }
        if self.manual_unit != Some(u) {
            let at = cur_word.map_or(snap.range.0, |w| w.0);
            let k = l.lines.iter().rposition(|(_, st0)| *st0 <= at).unwrap_or(0);
            let target = (a.tops[u] + k as f32 * lh - a.text.h * 0.3).clamp_safe(0.0, self.tmax);
            if target != self.tscroll {
                return None;
            }
        }
        let (sz, face) = if l.kind == UKind::Heading { (a.size * 1.18, Face::Bold) } else { (a.size, Face::Sans) };
        let hl = g.line_height(sz, face);
        // the lines whose lit part changes: where the sentence and the word were, and where they are
        let touching = |from: (usize, usize)| -> Vec<usize> { l.lines.iter().enumerate().filter(|(_, (line, ls))| from.0 < ls + line.chars().count() && from.1 > *ls).map(|(k, _)| k).collect() };
        let mut dirty: Vec<usize> = Vec::new();
        if self.highlight.sentence() {
            dirty.extend(touching(a.range));
            dirty.extend(touching(snap.range));
        }
        if self.highlight.word() {
            for w in [a.word, cur_word].into_iter().flatten() {
                dirty.extend(touching(w));
            }
        }
        dirty.sort_unstable();
        dirty.dedup();
        let top = a.base_y + a.tops[u];
        let mut bands: Vec<R> = Vec::new();
        for &k in &dirty {
            let ly = top + k as f32 * hl;
            let (y0, y1) = ((ly - 2.0).max(a.text.y).floor(), (ly + hl + 2.0).min(a.text.y + a.text.h).ceil());
            if y1 <= y0 {
                continue; // scrolled out of view
            }
            let band = R::new(a.text.x.floor(), y0, a.text.w.ceil(), y1 - y0);
            if a.slots.iter().any(|sl| sl.y < band.y + band.h && sl.y + sl.h > band.y) {
                return None; // next to a picture: not repainted alone
            }
            bands.push(band);
        }
        let tc = TextCtx { area: a.text, base_y: a.base_y, size: a.size, s: a.s, hl_range: Some((u, snap.range)), cur_word, lit: self.highlight.sentence(), tick };
        if cur_word.is_none() {
            self.word_vis = None;
        }
        let word = self.ease_word(g, lay, &a.tops, &tc);
        self.word_rect = None;
        self.sentence_rects.clear();
        let mut rects: Vec<(i32, i32, i32, i32)> = Vec::new();
        for band in bands {
            g.clipped_near(band, hl + 8.0 + 12.0 * a.s, |g| {
                g.rect(band.x, band.y, band.w, band.h, PANEL);
                // every line that touches the band (the neighbours too: the fill covered a little of them)
                for (i, ul) in lay.iter().enumerate() {
                    if ul.kind == UKind::Hidden {
                        continue;
                    }
                    let uh = if ul.kind == UKind::Heading { g.line_height(a.size * 1.18, Face::Bold) } else { lh };
                    let utop = a.base_y + a.tops[i];
                    if utop > band.y + band.h {
                        break;
                    }
                    if utop + ul.lines.len() as f32 * uh < band.y {
                        continue;
                    }
                    for li in 0..ul.lines.len() {
                        let ly = utop + li as f32 * uh;
                        if ly + uh >= band.y - 2.0 && ly - 1.0 <= band.y + band.h {
                            self.draw_line(g, ul, i, li, ly, &tc, &word);
                        }
                    }
                }
            });
            rects.push((band.x as i32, band.y as i32, band.w as i32, band.h as i32));
        }
        // the glow of the text card follows the sound level
        let glow = 0.3 + 0.7 * self.level;
        let alpha = glow_alphas(glow);
        if alpha != a.ring_alpha {
            if let Some(r) = &a.ring {
                glow_ring_again(g, r, ACCENT, MAGENTA, glow);
                rects.extend(r.rects());
            }
            a.ring_alpha = alpha;
        }
        // the player bar: its bars and progress move every frame
        let bar = a.bar;
        let bar_r = R::new(bar.0 as f32, bar.1 as f32, bar.2 as f32, bar.3 as f32);
        g.clipped_near(bar_r, 8.0 * a.s, |g| {
            g.put(bar, &a.bar_under);
            self.player_bar(g, a.player, a.s, &snap, tick);
        });
        rects.push(bar);
        a.unit = u;
        a.range = snap.range;
        a.word = cur_word;
        Some(rects)
    }

    /// Nothing but the reading itself moves on this screen: no story being written, no picture or audio being made, no
    /// menu, dialog, error or selection.
    pub fn partial_possible(&self) -> bool {
        matches!(self.page, Page::Story)
            && self.gen.is_none()
            && self.audio_job.is_none()
            && self.alt_running.is_none()
            && self.alt_jobs.is_empty()
            && !self.pics.iter().any(|p| matches!(p.state, PicState::Working | PicState::Queued))
            && self.error.is_none()
            && self.modal.is_none()
            && self.menu.is_none()
            && self.drag_o.is_none()
            && !self.pane.has_selection()
    }

    /// A summary of what is on the screen besides the reading, to notice a change the partial frames cannot show.
    pub fn view_sig(&self) -> u64 {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        (self.story.title.as_str(), self.story.blocks.len(), self.story.partial.len(), self.story.words(), self.pics.len(), self.held, self.ver, self.ver_count, self.pane_open, self.wide, std::mem::discriminant(&self.focus)).hash(&mut h);
        for p in &self.pics {
            (p.key, p.alts.len(), std::mem::discriminant(&p.state)).hash(&mut h);
        }
        (self.saved.as_ref().map(|e| (e.id.as_str(), e.favourite, e.has_audio)), self.saved_audio.is_some(), self.note.as_ref().map(|n| (n.0.as_str(), n.1)), self.run_model.as_str(), self.turning_on, self.install.is_some()).hash(&mut h);
        (self.chip_wanted(), self.read_hint().map(|x| x.0), self.voices.len(), self.takes_speed, self.prefs.speed.to_bits(), self.prefs.voice.as_deref()).hash(&mut h);
        h.finish()
    }

    /// Where the word being said sits on its line, and the fill that eases toward it (kept for the next frame). Call once per
    /// frame, before the lines are drawn.
    pub fn ease_word(&mut self, g: &mut Gfx, lay: &[ULay], tops: &[f32], tc: &TextCtx) -> Option<WordAt> {
        let (Some((u, _)), Some((wa, wb))) = (tc.hl_range, tc.cur_word) else { return None };
        let l = lay.get(u).filter(|l| l.kind != UKind::Hidden)?;
        let (sz, face) = if l.kind == UKind::Heading { (tc.size * 1.18, Face::Bold) } else { (tc.size, Face::Sans) };
        let hl = g.line_height(sz, face);
        for (li, (line, ls)) in l.lines.iter().enumerate() {
            let ly = tc.base_y + tops.get(u).copied()? + li as f32 * hl;
            if !(ly + hl >= tc.area.y - hl && ly <= tc.area.y + tc.area.h + hl) {
                continue;
            }
            if let Some((c0, c1, off, mw)) = span_in_line(g, line, *ls, wa, wb, sz, face) {
                let (ex, ew) = match self.word_vis {
                    Some((uu, l0, x, w)) if uu == u && l0 == li => (x + (off - x) * 0.6, w + (mw - w) * 0.6),
                    _ => (off, mw),
                };
                self.word_vis = Some((u, li, ex, ew));
                return Some(WordAt { unit: u, li, c0, c1, off, ex, ew });
            }
        }
        None
    }

    /// One line of the story: the lit sentence and word behind it, then its text.
    #[allow(clippy::too_many_arguments)]
    pub fn draw_line(&mut self, g: &mut Gfx, l: &ULay, i: usize, li: usize, ly: f32, tc: &TextCtx, word: &Option<WordAt>) {
        let (s, area_txt) = (tc.s, tc.area);
        let (sz, face) = if l.kind == UKind::Heading { (tc.size * 1.18, Face::Bold) } else { (tc.size, Face::Sans) };
        let hl = g.line_height(sz, face);
        let (line, ls) = (&l.lines[li].0, l.lines[li].1);
        let current = tc.hl_range.filter(|(u, _)| *u == i);
        let reading = tc.hl_range.is_some();
        let base = if l.kind == UKind::Heading { ACCENT } else if l.kind == UKind::Partial { MUTED } else if reading && tc.lit && current.is_none() { MUTED } else { TEXT };
        // the characters of this line that are inside the range (sentence, then word), as pixels
        if let (true, Some((_, (a, b)))) = (tc.lit, current) {
            if let Some((_, _, off, mw)) = span_in_line(g, line, ls, a, b, sz, face) {
                g.rrect(area_txt.x + off - 3.0 * s, ly - 1.0, mw + 6.0 * s, hl + 2.0, 5.0 * s, with_alpha(ACCENT, 70));
                let r = R::new(area_txt.x + off - 3.0 * s, ly - 1.0, mw + 6.0 * s, hl + 2.0);
                if !self.sentence_rects.contains(&r) {
                    self.sentence_rects.push(r);
                }
            }
        }
        // the word being said: a solid rounded fill under dark text, easing from the word before it
        let word_at = word.as_ref().filter(|w| w.unit == i && w.li == li);
        if let Some(w) = word_at {
            // the width of the word itself, again (the eased fill has its own)
            g.rrect(area_txt.x + w.ex - 3.0 * s, ly - 1.0, w.ew + 6.0 * s, hl + 2.0, 5.0 * s, WORD_FILL);
            self.word_rect = Some(R::new(area_txt.x + w.ex - 3.0 * s, ly - 1.0, w.ew + 6.0 * s, hl + 2.0));
        }
        let dots;
        let shown: &str = if l.kind == UKind::Partial && li + 1 == l.lines.len() {
            dots = format!("{line}{}", ".".repeat(1 + (tc.tick / 10 % 3) as usize));
            &dots
        } else {
            line
        };
        match word_at {
            Some(w) => {
                // three runs, so the word can be dark on its fill: before, the word, after
                let chars: Vec<char> = shown.chars().collect();
                let (c1, c0) = (w.c1.min(chars.len()), w.c0.min(chars.len()));
                let (a, m, z): (String, String, String) = (chars[..c0].iter().collect(), chars[c0..c1].iter().collect(), chars[c1..].iter().collect());
                g.text(area_txt.x, ly, sz, base, &a, face);
                g.text(area_txt.x + w.off, ly, sz, WORD_INK, &m, face);
                let zx = g.text_width(sz, &format!("{a}{m}"), face);
                g.text(area_txt.x + zx, ly, sz, base, &z, face);
            }
            None => {
                g.text(area_txt.x, ly, sz, base, shown, face);
            }
        }
    }

    pub fn player_bar(&mut self, g: &mut Gfx, r: R, s: f32, snap: &st::Snapshot, tick: u64) {
        let active = matches!(snap.status, Status::Playing);
        glow_card(g, r, s, CYAN, MAGENTA, if active { 0.3 + 0.7 * self.level } else { 0.1 });
        let d = (r.h * 0.5).max(30.0 * s);
        let cy = r.y + r.h / 2.0;
        let mut x = r.x + 16.0 * s;
        let btn = |g: &mut Gfx, x: f32, dd: f32, glyph: Glyph, lit: Option<[u8; 4]>, code: usize, label: &str| {
            let rr = R::new(x, cy - dd / 2.0, dd, dd);
            round_button(g, rr, s, glyph, lit);
            g.hit(rr, Event::Tab(code), label);
        };
        btn(g, x, d, Glyph::Previous, None, T_PREV, "Previous part");
        x += d + 8.0 * s;
        // play / pause is the big one
        let pd = d + 12.0 * s;
        let rr = R::new(x, cy - pd / 2.0, pd, pd);
        let lit = if active { MAGENTA } else { CYAN };
        round_button(g, rr, s, Glyph::Play, Some(lit));
        if matches!(snap.status, Status::Playing | Status::Buffering) {
            // cover the triangle with two bars
            g.circle(rr.x + rr.w / 2.0, rr.y + rr.h / 2.0, rr.w * 0.3, fx::over(PANEL, with_alpha(lit, 120)));
            let k = rr.w * 0.17;
            g.rect(rr.x + rr.w / 2.0 - k * 0.95, rr.y + rr.h / 2.0 - k, k * 0.7, k * 2.0, TEXT);
            g.rect(rr.x + rr.w / 2.0 + k * 0.25, rr.y + rr.h / 2.0 - k, k * 0.7, k * 2.0, TEXT);
        }
        g.hit(rr, Event::Tab(T_PLAY), if matches!(snap.status, Status::Playing | Status::Buffering) { "Pause" } else { "Play" });
        x += pd + 8.0 * s;
        btn(g, x, d, Glyph::Stop, None, T_STOP, "Stop");
        x += d + 8.0 * s;
        btn(g, x, d, Glyph::Next, None, T_NEXT, "Next part");
        x += d + 14.0 * s;
        // replay as a small text button when it fits
        let rs = 12.0 * s;
        let rw = g.text_width(rs, "Replay", Face::Bold) + rs * 1.7;
        let right_min = 250.0 * s;
        if x + rw + right_min < r.x + r.w {
            g.button(x, cy - rs * 1.05, rs, "Replay", MUTED, Event::Tab(T_REPLAY));
            x += rw + 12.0 * s;
        }
        // the right side: voice and speed chips (when the engine has them), the level bars
        let mut rx = r.x + r.w - 14.0 * s;
        let eng = matches!(self.tts, TtsStatus::Ready { .. }) && self.saved_audio.is_none();
        if self.saved_audio.is_some() {
            let w = chip_w(g, s, "saved audio");
            rx -= w;
            fx::chip(g, rx, cy - 12.0 * s, s, "saved audio", OK, false);
            rx -= 10.0 * s;
        }
        if eng && self.takes_speed {
            let label = format!("{:.2}x", self.prefs.speed).replace(".00x", ".0x");
            let w = g.text_width(13.0 * s, &label, Face::Bold) + 13.0 * s * 1.9;
            if rx - w > x + 220.0 * s {
                rx -= w;
                chip(g, rx, cy - 14.0 * s, s, &label, false, Event::Tab(T_SPEED));
                rx -= 8.0 * s;
            }
        }
        if eng && self.voices.len() > 1 {
            let v = self.prefs.voice.clone().unwrap_or_else(|| self.voices[0].clone());
            let label = format!("Voice: {}", voice_label(&v));
            let w = g.text_width(13.0 * s, &label, Face::Bold) + 13.0 * s * 1.9;
            if rx - w > x + 220.0 * s {
                rx -= w;
                chip(g, rx, cy - 14.0 * s, s, &label, false, Event::Tab(T_VOICE));
                rx -= 8.0 * s;
            }
        } else if eng && !self.voices.is_empty() {
            let label = format!("Voice: {}", voice_label(&self.voices[0]));
            let w = g.text_width(12.0 * s, &label, Face::Sans);
            if rx - w > x + 220.0 * s {
                rx -= w;
                g.text(rx, cy - g.line_height(12.0 * s, Face::Sans) / 2.0, 12.0 * s, DIM, &label, Face::Sans);
                rx -= 10.0 * s;
            }
        }
        // waveform
        let bars = 18usize;
        let ww = (r.w * 0.11).clamp_safe(60.0 * s, 150.0 * s);
        let wavex = rx - ww;
        if wavex > x + 160.0 * s {
            let maxh = r.h * 0.6;
            for i in 0..bars {
                let t = i as f32 / (bars - 1) as f32;
                let bw = ww / bars as f32;
                let wob = (tick as f32 * 0.35 + i as f32 * 0.9).sin() * 0.5 + 0.5;
                let lvl = match snap.status {
                    Status::Playing => (self.level * (0.35 + 0.65 * wob)).max(0.06),
                    Status::Buffering => 0.08 + 0.12 * ((tick as f32 * 0.12 + i as f32 * 0.5).sin() * 0.5 + 0.5),
                    _ => 0.05,
                };
                let hgt = (lvl * maxh).max(3.0 * s);
                let c = mix(CYAN, MAGENTA, t);
                g.rrect(wavex + i as f32 * bw, cy - hgt / 2.0, bw - 2.0 * s, hgt, 2.0 * s, if matches!(snap.status, Status::Playing | Status::Buffering) { c } else { with_alpha(c, 90) });
            }
            rx = wavex - 14.0 * s;
        }
        // progress and what is going on
        let px = x;
        let pw = (rx - px).max(40.0 * s);
        let (txt, col) = self.status_line(snap);
        let sz = 13.0 * s;
        let txt = g.fit(sz, &txt, Face::Sans, pw);
        let th = g.line_height(sz, Face::Sans);
        g.text(px, cy - th - 6.0 * s, sz, col, &txt, Face::Sans);
        let bar = R::new(px, cy + 4.0 * s, pw, 8.0 * s);
        fx::progress_bar(g, bar, snap.fraction, if active { MAGENTA } else { CYAN }, s);
        g.hit(R::new(bar.x, bar.y - 8.0 * s, bar.w, bar.h + 16.0 * s), Event::Area(BAR_AREA), "progress");
        self.bar_rect = Some(bar);
    }

    pub fn status_line(&self, snap: &st::Snapshot) -> (String, [u8; 4]) {
        if let Status::Failed(m) = &snap.status {
            return (m.clone(), ERR);
        }
        if let Some(j) = &self.audio_job {
            return (format!("Saving the audio… {} of {}", j.done.load(Ordering::SeqCst), j.total), MAGENTA);
        }
        let more = if self.gen.is_some() { " (still writing)" } else { "" };
        match snap.status {
            // where the lit word's times come from, said quietly (native, aligned, from the voice, estimated)
            Status::Playing if self.highlight.word() && snap.word.is_some() => (format!("Reading part {} of {}{more} · words {}", snap.chunk + 1, snap.chunks, snap.timing.label()), CYAN),
            Status::Playing => (format!("Reading part {} of {}{more}", snap.chunk + 1, snap.chunks), CYAN),
            Status::Buffering if self.live => (format!("Getting the voice ready… part {} of {}{more}", snap.chunk + 1, snap.chunks.max(1)), MUTED),
            Status::Paused => (format!("Paused at part {} of {}", snap.chunk + 1, snap.chunks), MUTED),
            Status::Done => ("Finished. Press play to listen again.".into(), OK),
            _ if self.gen.is_some() => (if self.run_model.is_empty() { "Writing the story…".into() } else { format!("Writing the story with {}…", self.run_model) }, MAGENTA),
            _ if self.can_read() => (if self.saved_audio.is_some() { "Saved audio is ready. Press play.".into() } else { "Press play to read it aloud.".into() }, MUTED),
            _ => ("Text only: reading aloud is off. Use Turn on reading aloud under the player.".into(), fx::AMBER),
        }
    }

    /// The story page: the story view on the left and the conversation panel on the right (below 900 px the panel is a toggle).
    pub fn story_screen(&mut self, g: &mut Gfx, area: R, s: f32, port: &dyn TalesPort, tick: u64) {
        let hl = port.voice_settings().highlight;
        if hl != self.highlight {
            self.highlight = hl;
            self.reader.set_words(hl.word());
        }
        self.wide = area.w >= WIDE_PX;
        self.area_rect = area;
        self.scale = s;
        self.panel_hover = false;
        let show = self.wide || self.pane_open;
        let pw = (self.panel_w * s).clamp_safe(270.0 * s, area.w * if self.wide { 0.6 } else { 0.65 });
        let (main, panel) = if show { (R::new(area.x, area.y, area.w - pw, area.h), R::new(area.x + area.w - pw, area.y, pw, area.h)) } else { (area, R::new(area.x + area.w, area.y, 0.0, area.h)) };
        self.story_view(g, main, s, tick);
        if show {
            self.panel(g, panel, s, port, tick, area);
            // the divider, to drag the panel wider or narrower
            let dw = (10.0 * s).max(9.0);
            let d = R::new(panel.x - dw / 2.0, area.y, dw, area.h);
            self.div_rect = d;
            let hot = g.hovered(d) || self.drag_o.is_some() && iyakki_uikit::ui::dragging();
            if hot {
                g.rrect(d.x + d.w / 2.0 - 1.5 * s, d.y + 8.0 * s, 3.0 * s, d.h - 16.0 * s, 1.5 * s, with_alpha(ACCENT, 190));
            }
            g.hit(d, Event::Area(DIV_AREA), "divider");
        }
        // the export menu
        if let Some(Modal::Export(p, _)) = &self.modal {
            p.render(g, area, s, ACCENT);
        }
        self.draw_menus(g, area, panel, s, port);
        self.draw_model_pick(g, area, s, port);
        self.draw_pic_log(g, area, s);
    }

    // ---------------------------------------------------------------------------------------------- library page

    pub fn library_page(&mut self, g: &mut Gfx, area: R, s: f32, tick: u64) {
        let m = if area.w < 700.0 * s { 14.0 * s } else { 24.0 * s };
        let x = area.x + m;
        let w = area.w - 2.0 * m;
        let mut y = area.y + 8.0 * s;
        y += self.tabs(g, x, y, w, s) + 12.0 * s;
        let fr = R::new(x, y, w.min(520.0 * s), 38.0 * s);
        self.search.render(g, fr, self.focus == Focus::Search && self.modal.is_none(), s, blink(tick), Face::Sans, SEARCH_FIELD);
        let count = format!("{} {}", self.lib.len(), if self.lib.len() == 1 { "story" } else { "stories" });
        g.text(fr.x + fr.w + 14.0 * s, fr.y + (fr.h - g.line_height(13.0 * s, Face::Sans)) / 2.0, 13.0 * s, MUTED, &count, Face::Sans);
        y += fr.h + 10.0 * s;
        let nh = self.banner(g, x, y, w, s);
        y += nh;
        if self.lib.is_empty() {
            let r = R::new(area.x, y, area.w, area.y + area.h - y);
            if self.search.value.trim().is_empty() {
                fx::empty_state(g, r, s, Icon::Tales, "No saved stories yet", "Stories you create are kept here, with their pictures and audio, so you can listen again.", ACCENT);
            } else {
                fx::empty_state(g, r, s, Icon::Tales, "Nothing matches", "Try fewer words, or clear the search.", ACCENT);
            }
        } else {
            let rh = 60.0 * s;
            let fit = (((area.y + area.h - y - 8.0 * s) / (rh + 6.0 * s)).floor() as usize).max(1);
            self.lib_top = self.lib_top.min(self.lib.len().saturating_sub(1));
            if self.lib_sel < self.lib_top {
                self.lib_top = self.lib_sel;
            } else if self.lib_sel >= self.lib_top + fit {
                self.lib_top = self.lib_sel + 1 - fit;
            }
            let rows = self.lib.clone();
            for (i, e) in rows.iter().enumerate().skip(self.lib_top).take(fit) {
                let rect = R::new(x, y, w - 12.0 * s, rh);
                let sel = i == self.lib_sel;
                let hot = g.hovered(rect);
                fx::list_row(g, rect, s, sel, hot, ACCENT);
                g.hit(rect, Event::Row(i), &e.title);
                let cv = R::new(rect.x + 8.0 * s, rect.y + 7.0 * s, rh - 14.0 * s, rh - 14.0 * s);
                cover(g, cv, s * 0.6, &e.title);
                let tx = cv.x + cv.w + 14.0 * s;
                // the actions of the chosen row on the right; the title gets the rest
                let mut right = rect.x + rect.w - 10.0 * s;
                if sel {
                    let mut acts: Vec<(&str, usize, [u8; 4])> = vec![("Listen", T_LIB_LISTEN, ACCENT), ("Open", T_LIB_OPEN, MUTED), (if e.favourite { "Unfavourite" } else { "Favourite" }, T_LIB_FAV, MUTED), ("Export…", T_LIB_EXPORT, MUTED), ("Delete", T_LIB_DELETE, ERR)];
                    let bsz = 12.0 * s;
                    let mut keep = Vec::new();
                    let mut used = 0.0;
                    let room = rect.w * 0.52;
                    for a in acts.drain(..) {
                        let bw = g.text_width(bsz, a.0, Face::Bold) + bsz * 1.7 + 6.0 * s;
                        if used + bw <= room {
                            used += bw;
                            keep.push((a, bw));
                        }
                    }
                    for (a, bw) in keep.into_iter().rev() {
                        right -= bw;
                        g.button(right, rect.y + (rect.h - bsz * 2.1) / 2.0, bsz, a.0, a.2, Event::Tab(a.1));
                    }
                    right -= 6.0 * s;
                } else {
                    if e.has_audio {
                        let cw = chip_w(g, s, "audio");
                        right -= cw;
                        fx::chip(g, right, rect.y + (rect.h - 22.0 * s) / 2.0, s, "audio", OK, false);
                        right -= 8.0 * s;
                    }
                    if e.pictures > 0 {
                        let cw = chip_w(g, s, "pictures");
                        right -= cw;
                        fx::chip(g, right, rect.y + (rect.h - 22.0 * s) / 2.0, s, "pictures", CYAN, false);
                        right -= 8.0 * s;
                    }
                    if e.favourite {
                        right -= 20.0 * s;
                        heart(g, right + 10.0 * s, rect.y + rect.h / 2.0 - 2.0 * s, 16.0 * s, MAGENTA, true);
                    }
                }
                let tw = (right - tx - 10.0 * s).max(30.0);
                let t = g.fit(16.0 * s, &e.title, Face::Bold, tw);
                if e.favourite && sel {
                    heart(g, tx + 6.0 * s, rect.y + 17.0 * s, 13.0 * s, MAGENTA, true);
                    let t2 = g.fit(16.0 * s, &e.title, Face::Bold, tw - 18.0 * s);
                    g.text(tx + 18.0 * s, rect.y + 8.0 * s, 16.0 * s, TEXT, &t2, Face::Bold);
                } else {
                    g.text(tx, rect.y + 8.0 * s, 16.0 * s, TEXT, &t, Face::Bold);
                }
                let aud = Audience::from_id(&e.audience).map_or("", |a| a.label());
                let gen = Genre::from_id(&e.genre).map_or("", |a| a.label());
                let meta = format!("{aud} · {gen} · {} · {} words · {}", e.language, e.words, ymd(e.created));
                let meta = g.fit(12.0 * s, &meta, Face::Sans, tw);
                g.text(tx, rect.y + 34.0 * s, 12.0 * s, fx::readable(MUTED, rgb(30, 38, 68), 4.5), &meta, Face::Sans);
                y += rh + 6.0 * s;
            }
            if self.lib.len() > fit {
                fx::scrollbar(g, R::new(x + w - 6.0 * s, area.y + 90.0 * s, 6.0 * s, area.h - 100.0 * s), s, self.lib_top, fit, self.lib.len(), ACCENT);
            }
        }
        match &self.modal {
            Some(Modal::ConfirmDelete(_, title)) => {
                let shown = g.fit(19.0 * s, title, Face::Bold, 300.0 * s);
                iyakki_uikit::widgets::render_confirm(g, area, s, &format!("Delete \"{shown}\" and everything saved with it?"), "Delete");
            }
            Some(Modal::FetchLink(_, url)) => {
                let host = iyakki_scene::reply::host_of(url);
                iyakki_uikit::widgets::render_confirm(g, area, s, &format!("Fetch one picture from {host}? Your computer asks that site for the file at the link; nothing else is sent."), "Fetch");
            }
            Some(Modal::Export(p, _)) => p.render(g, area, s, ACCENT),
            Some(Modal::Model(_)) | Some(Modal::PicLog(_)) | None => {}
        }
    }
}
