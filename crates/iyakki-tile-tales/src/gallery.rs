//! TALES, The pictures of a story: extra pictures, the gallery and the picture log.
use super::*;

impl StoryApp {
    /// Queue the extra pictures of the scenes in `idxs`: one per further style and per further picture of a style, scene by
    /// scene, style by style. The first picture of the first style is the scene's main picture and is not queued here.
    pub fn queue_alts(&mut self, idxs: &[usize]) {
        let pic = self.story_opts.pic.clone();
        let (styles, count) = (pic.styles_list(), pic.count());
        for &idx in idxs {
            if self.pics.get(idx).is_none_or(|p| p.mode != PictureMode::ImageModel) {
                continue;
            }
            for (si, look) in styles.iter().enumerate() {
                for n in 0..count {
                    if si == 0 && n == 0 {
                        continue;
                    }
                    self.alt_jobs.push_back(AltJob { idx, look: *look, n });
                }
            }
        }
    }

    /// How many pictures the job makes for `scenes` scenes and the estimated seconds in all (0.0 when the model's steps are not
    /// known, so no estimate is made).
    pub fn job_totals(&self, port: &dyn TalesPort, scenes: usize) -> (usize, f64) {
        let pic = self.pic_now();
        let cpu_only = port.cpu_only();
        let mut total = 0.0;
        for look in pic.styles_list() {
            match self.resolved_look(&pic, look, port).and_then(|r| r.steps.map(|st| illus::est_secs(r.width, r.height, st, cpu_only))) {
                Some(each) => total += each * pic.count() as f64 * scenes as f64,
                None => return (illus::job_estimate(scenes, pic.styles_list().len(), pic.count(), 0.0).0, 0.0),
            }
        }
        (illus::job_estimate(scenes, pic.styles_list().len(), pic.count(), 0.0).0, total)
    }

    /// The total line for the job, before it starts.
    pub fn job_line(&self, port: &dyn TalesPort, scenes: usize) -> Option<String> {
        let (n, total) = self.job_totals(port, scenes);
        let pic = self.pic_now();
        (total > 0.0 && n > 0).then(|| illus::job_text(scenes, pic.styles_list().len(), pic.count(), total / n as f64))
    }

    /// Make extra picture `k` of scene `idx` its main picture (the reader, the cover and the export use the main one); the old
    /// main picture takes its place in the gallery.
    pub fn choose_main(&mut self, idx: usize, k: usize) {
        let Some(p) = self.pics.get_mut(idx) else { return };
        if k >= p.alts.len() || p.state != PicState::Ready {
            return;
        }
        let a = p.alts[k].clone();
        let old = Alt { look: p.look, seed: p.seed, provider: p.provider.clone(), model: p.model.clone(), size: p.size.clone(), ms: p.ms, ext: p.ext.clone(), bytes: p.bytes.clone().unwrap_or_default(), prompt: p.prompt.clone(), key: p.key };
        (p.look, p.seed, p.provider, p.model, p.ms) = (a.look, a.seed, a.provider.clone(), a.model.clone(), a.ms);
        (p.size, p.ext, p.prompt, p.key, p.bytes) = (a.size.clone(), a.ext.clone(), a.prompt.clone(), a.key, Some(a.bytes.clone()));
        p.how = format!("images via {}", a.model);
        p.alts[k] = old;
        if self.saved.is_some() {
            self.persist_pictures();
        }
        self.note = Some((format!("The {} picture is now the main one of scene {}.", a.look.label(), idx + 1), true));
        self.dirty = true;
    }

    pub fn delete_alt(&mut self, idx: usize, k: usize) {
        if self.pics.get(idx).is_none_or(|p| k >= p.alts.len()) {
            return;
        }
        self.pics[idx].alts.remove(k);
        if self.saved.is_some() {
            self.persist_pictures();
        }
        self.note = Some(("That picture was deleted; the others are as they were.".into(), true));
        self.dirty = true;
    }

    /// Remove every picture that is not the chosen main one of its scene.
    pub fn keep_only_chosen(&mut self) {
        let n: usize = self.pics.iter().map(|p| p.alts.len()).sum();
        for p in &mut self.pics {
            p.alts.clear();
        }
        self.alt_jobs.clear();
        if self.saved.is_some() {
            self.persist_pictures();
        }
        self.note = Some((format!("Kept only the chosen picture of each scene ({n} removed)."), true));
        self.dirty = true;
    }

    /// Draw one more picture of scene `idx` in the next style (a new seed), without redrawing the others.
    pub fn more_alt(&mut self, idx: usize, port: &dyn TalesPort) {
        let Some(p) = self.pics.get(idx) else { return };
        if p.state != PicState::Ready || p.mode != PictureMode::ImageModel {
            return;
        }
        let styles = self.story_opts.pic.styles_list();
        let look = styles[(p.alts.len() + 1) % styles.len()];
        let n = (p.alts.iter().filter(|a| a.look == look).count() + usize::from(p.look == look)) as u32;
        self.alt_jobs.push_back(AltJob { idx, look, n });
        self.held = false;
        self.pump_pictures(port);
        self.dirty = true;
    }

    /// The height of the gallery under the picture of scene `pi` (0 when the scene has only its main picture).
    pub fn gallery_h(&self, pi: usize, slot_w: f32, s: f32) -> f32 {
        let n = self.pics.get(pi).map_or(0, |p| p.alts.len());
        if n == 0 {
            return 0.0;
        }
        let per_row = ((slot_w / (118.0 * s)) as usize).max(1);
        ((n + 1).div_ceil(per_row)) as f32 * 104.0 * s + 34.0 * s
    }

    /// The row of thumbnails under a scene's picture: the main one (outlined) and the others with their style, model and time.
    /// A click on another picture makes it the main one.
    pub fn draw_gallery(&mut self, g: &mut Gfx, slot: R, s: f32, idx: usize) {
        let h = self.gallery_h(idx, slot.w, s);
        if h <= 0.0 {
            return;
        }
        let (tw, th) = (110.0 * s, 62.0 * s);
        let per_row = ((slot.w / (118.0 * s)) as usize).max(1);
        let items: Vec<(Option<usize>, illus::Look, String, u64, u64, Arc<Vec<u8>>)> = {
            let p = &self.pics[idx];
            let mut v = vec![(None, p.look, p.model.clone(), p.ms, p.key, p.bytes.clone().unwrap_or_default())];
            v.extend(p.alts.iter().enumerate().map(|(k, a)| (Some(k), a.look, a.model.clone(), a.ms, a.key, a.bytes.clone())));
            v
        };
        let y0 = slot.y + slot.h + 6.0 * s;
        for (i, (alt, look, model, ms, key, bytes)) in items.iter().enumerate() {
            let r = R::new(slot.x + (i % per_row) as f32 * 118.0 * s, y0 + (i / per_row) as f32 * 104.0 * s, tw, th);
            g.rrect(r.x, r.y, r.w, r.h, 8.0 * s, rgb(14, 18, 36));
            if let Ok(rd) = self.pic_cache.draw_bytes(g, *key, bytes, tw, s) {
                let (pw, ph) = (rd.pm.width() as f32, rd.pm.height() as f32);
                let k = (tw / pw).min(th / ph);
                let paint = tiny_skia::PixmapPaint { quality: tiny_skia::FilterQuality::Bilinear, ..Default::default() };
                g.pm_mut().draw_pixmap(0, 0, rd.pm.as_ref().as_ref(), &paint, tiny_skia::Transform::from_translate(r.x + (tw - pw * k) / 2.0, r.y + (th - ph * k) / 2.0).pre_scale(k, k), None);
            }
            g.rrect_outline(r.x, r.y, r.w, r.h, 8.0 * s, if alt.is_none() { 2.2 * s } else { 1.0 }, if alt.is_none() { ACCENT } else { with_alpha(MUTED, 120) });
            let cap = format!("{}{}", if alt.is_none() { "main \u{b7} " } else { "" }, look.label());
            let cap = g.fit(11.0 * s, &cap, Face::Bold, tw);
            g.text(r.x, r.y + th + 2.0 * s, 11.0 * s, if alt.is_none() { ACCENT } else { TEXT }, &cap, Face::Bold);
            let sub = g.fit(10.0 * s, &format!("{model} \u{b7} {} s", ms / 1000), Face::Sans, tw);
            g.text(r.x, r.y + th + 2.0 * s + g.line_height(11.0 * s, Face::Bold), 10.0 * s, MUTED, &sub, Face::Sans);
            if let Some(k) = alt {
                self.gal_hits.push((r, T_ALT_PICK + idx * 24 + k, format!("Make the {} picture the main one", look.label())));
                let x = R::new(r.x + r.w - 18.0 * s, r.y + 2.0 * s, 16.0 * s, 16.0 * s);
                g.rrect(x.x, x.y, x.w, x.h, 8.0 * s, [6, 8, 20, 210]);
                g.text(x.x + 4.5 * s, x.y + 1.0 * s, 11.0 * s, ERR, "x", Face::Bold);
                self.gal_hits.push((x, T_ALT_DEL + idx * 24 + k, format!("Delete this {} picture", look.label())));
            }
        }
        let styles = self.story_opts.pic.styles_list();
        let next = styles[(self.pics[idx].alts.len() + 1) % styles.len()];
        let rows = items.len().div_ceil(per_row);
        let by = y0 + rows as f32 * 104.0 * s - 2.0 * s;
        if self.gen.is_none() && self.alt_running.is_none() {
            self.gal_hits.push((R::new(slot.x, by, 0.0, 0.0), T_ALT_MORE + idx, format!("Draw another in {}", next.label())));
        }
    }

    /// The picture log window: the events of the story, newest at the bottom, scrolling, with Copy log, Clear log and Verbose.
    pub fn draw_pic_log(&self, g: &mut Gfx, area: R, s: f32) {
        let Some(Modal::PicLog(v)) = &self.modal else { return };
        let p = fx::modal(g, area, s, 920.0 * s, area.h - 24.0 * s, ACCENT, false);
        let events = self.pic_log_events();
        let top = fx::modal_title(g, p, s, &format!("Picture log · {}", self.story_name()), ACCENT);
        let sub = format!("{} event{} · stays on this computer (it holds your story text) · the key is never written · g, Esc or Close leaves · Up, Down, PageUp, PageDown scroll", events.len(), if events.len() == 1 { "" } else { "s" });
        let sub = g.fit(12.0 * s, &sub, Face::Sans, p.w - 52.0 * s);
        g.text(p.x + 26.0 * s, top - 2.0 * s, 12.0 * s, MUTED, &sub, Face::Sans);
        let (bsz, by) = (13.0 * s, top + 20.0 * s);
        let mut bx = p.x + 26.0 * s;
        let verbose = self.pic_now().verbose;
        let clear = if v.confirm_clear { "Press again to clear" } else { "Clear log" };
        for (label, code, col) in [("Copy log".to_string(), T_LOG_COPY, ACCENT), (clear.to_string(), T_LOG_CLEAR, if v.confirm_clear { ERR } else { MUTED }), (format!("Verbose: {}", if verbose { "on" } else { "off" }), T_LOG_VERBOSE, if verbose { ACCENT } else { MUTED }), ("Close".to_string(), T_LOG_CLOSE, MUTED)] {
            bx += g.button(bx, by, bsz, &label, col, Event::Tab(code)) + 8.0 * s;
        }
        let inner = R::new(p.x + 26.0 * s, by + bsz * 2.1 + 12.0 * s, p.w - 52.0 * s - 10.0 * s, p.y + p.h - (by + bsz * 2.1 + 12.0 * s) - 16.0 * s);
        let (sz, face) = (11.5 * s, Face::Mono);
        let lh = g.line_height(sz, face);
        let text = if events.is_empty() { "Nothing yet. Every picture request, skip, cap and fall back to SVG is written here.".to_string() } else { piclog::log_text(&events) };
        let mut lines: Vec<String> = Vec::new();
        for l in text.split('\n') {
            if l.is_empty() {
                lines.push(String::new());
            } else {
                lines.extend(g.wrap(sz, l, face, inner.w));
            }
        }
        let rows = ((inner.h / lh) as usize).max(1);
        let max = lines.len().saturating_sub(rows);
        let at = if v.at_end.get() { max } else { v.top.get().min(max) };
        v.top.set(at);
        v.total.set(lines.len());
        v.rows.set(rows);
        g.clipped(inner, |g| {
            for (i, l) in lines.iter().skip(at).take(rows + 1).enumerate() {
                g.text(inner.x, inner.y + i as f32 * lh, sz, if l.starts_with("20") { TEXT } else { MUTED }, l, face);
            }
        });
        fx::scrollbar(g, R::new(p.x + p.w - 22.0 * s, inner.y, 6.0 * s, inner.h), s, at, rows, lines.len(), ACCENT);
    }

    pub fn draw_model_pick(&mut self, g: &mut Gfx, area: R, s: f32, port: &dyn TalesPort) {
        let Some(Modal::Model(m)) = &mut self.modal else { return };
        port.model_draw(g, area, s, &mut m.list);
    }

}
