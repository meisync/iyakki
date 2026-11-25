//! TALES, Drawing the pictures: the queue, the workers, links, installs of picture models, readiness and the look.
use super::*;

impl StoryApp {
    pub fn pump_pictures(&mut self, port: &dyn TalesPort) {
        if self.held || self.alt_running.is_some() || self.pics.iter().any(|p| p.state == PicState::Working) {
            return;
        }
        // the main pictures first; then the extra ones (other styles and seeds), scene by scene
        let mut alt: Option<AltJob> = None;
        let idx = match self.pics.iter().position(|p| p.state == PicState::Queued) {
            Some(i) => i,
            None => {
                while let Some(j) = self.alt_jobs.pop_front() {
                    if self.pics.get(j.idx).is_some_and(|p| p.state == PicState::Ready && p.mode == PictureMode::ImageModel) {
                        alt = Some(j);
                        break;
                    }
                }
                let Some(j) = alt else { return };
                j.idx
            }
        };
        if self.pics[idx].mode == PictureMode::ImageModel {
            if let Some(why) = self.low_memory(port) {
                // no picture job starts while the machine is short of memory; the text is untouched and Try again works later
                let mut evs = Vec::new();
                for i in 0..self.pics.len() {
                    if self.pics[i].state == PicState::Queued && self.pics[i].mode == PictureMode::ImageModel {
                        self.pics[i].state = PicState::Failed(why.clone());
                        self.pic_result(i, false, &why);
                        evs.push(self.pic_decision(plk::SKIPPED, i, &format!("skipped: free memory low. {why}")));
                    }
                }
                if alt.is_some() {
                    evs.push(self.pic_decision(plk::SKIPPED, idx, &format!("skipped (another picture of this scene): free memory low. {why}")));
                    self.alt_jobs.clear();
                }
                self.log_pic_events(evs);
                self.note = Some((why, false));
                self.dirty = true;
                return;
            }
        }
        if alt.is_none() {
            self.pics[idx].state = PicState::Working;
        } else {
            self.alt_running = Some(idx);
        }
        let (gen, tx) = (self.pic_gen, self.pic_tx.clone());
        let title = if self.story.title.is_empty() { st::title_from_topic(&self.story_opts.topic) } else { self.story.title.clone() };
        let (mut o, scene, mode) = (self.story_opts.clone(), self.pics[idx].scene.clone(), self.pics[idx].mode);
        o.style = self.pics[idx].style;
        let (look, variant_n) = alt.map_or((self.pics[idx].look, 0), |j| (j.look, j.n));
        o.pic.look = look;
        let svc = port.worker();
        let clock = self.clock.clone();
        let drawers = self.drawers(port);
        let workdir = port.data_dir().join("tales-agent");
        let (story_text, known_sheet, feedback, known_setting) = (self.story.plain(), self.sheet.clone(), if alt.is_some() { String::new() } else { self.pics[idx].feedback.clone() }, self.setting.clone());
        // the model that draws is named (the chosen one, else the first found) so its name is kept with the picture
        let resolved = self.resolved_look(&o.pic, look, port);
        let picture_model = resolved.as_ref().map(|r| (r.provider.clone(), r.model.clone()));
        let family = illus::Family::of(&resolved.as_ref().map_or(String::new(), |r| r.model.clone()));
        // a new seed for every picture, or the story's own when it keeps one look (made with the first picture and kept)
        let attempt = {
            let n = self.pic_attempts.entry(idx).or_insert(0);
            *n += 1;
            *n
        };
        // one seed family per scene: every style of it starts from the same seed, each further picture of a style from another
        let base = match self.scene_seed.get(&idx).copied().filter(|_| alt.is_some()) {
            Some(b) => b,
            None => {
                let b = illus::seed_for(&o.pic, illus::fresh_seed(idx as u64 * 31 + attempt as u64));
                self.scene_seed.insert(idx, b);
                b
            }
        };
        let seed = illus::PictureOpts::variant_seed(base, variant_n);
        let (rsteps, rsize, rguid) = resolved.as_ref().map_or((None, None, None), |r| (r.steps, Some((r.width, r.height)), r.guidance));
        let realistic = o.pic.is_realistic();
        if o.pic.keep_look && self.story_opts.pic.seed.is_none() {
            self.story_opts.pic.seed = Some(base);
            if let Some(id) = self.saved_id() {
                let _ = self.store.save_picture_opts(&id, &self.story_opts.pic);
            }
        }
        let story_name = title.clone();
        self.pic_started.insert(idx, Instant::now());
        self.pic_target = picture_model.clone();
        self.last_wait_log = None;
        port.audit("story.picture", &format!("{} scene {}", mode.id(), idx + 1));
        std::thread::spawn(move || {
            // the character sheet is made once per story, by the first drawer, and kept
            let mut sheet = known_sheet;
            let mut made_sheet = None;
            let mut made_setting: Option<String> = None;
            let mut need_sheet = |drawer: &Drawer| -> CharacterSheet {
                if let Some(s) = &sheet {
                    return s.clone();
                }
                let s = scene::sheet::character_sheet(drawer.chat.as_ref(), &o, &story_text);
                made_sheet = Some(s.clone());
                sheet = Some(s.clone());
                s
            };
            let mut events: Vec<PicEvent> = Vec::new();
            let mut missing: Option<MissingModel> = None;
            let res: Result<PicOut, String> = match mode {
                PictureMode::ImageModel => 'img: {
                    // ask the server now whether the model is on it: a call for a model that is not downloaded would start a
                    // download of several GB inside the call, with no progress and no end in sight
                    if let Some((p, m)) = &picture_model {
                        if svc.provider_is_lemonade(p) {
                            let size = match svc.lemonade_live_model(p, m) {
                                Ok(Some(info)) if !info.downloaded => Some(info.size_text()),
                                Ok(None) => Some("size unknown".to_string()),
                                _ => None,
                            };
                            if let Some(size) = size {
                                let text = illus::not_downloaded_text(m, p, &size);
                                let mut ev = piclog::decision_event(clock(), plk::NOT_DOWNLOADED, &story_name, idx + 1, &scene.title, &format!("{text} Nothing was asked of the server."));
                                (ev.provider, ev.model, ev.attempt) = (p.clone(), m.clone(), attempt);
                                events.push(ev);
                                missing = Some(MissingModel { provider: p.clone(), model: m.clone(), size, text: text.clone() });
                                break 'img Err(text);
                            }
                        }
                    }
                    let sh = need_sheet(&drawers[0]);
                    // the story model writes the setting note once and a short visual brief for this scene; when it cannot,
                    // the scene's own text is used, so a picture is never lost to this step
                    let setting = known_setting.clone().or_else(|| {
                        let n = drawers[0].chat.complete(&illus::setting_messages(&title, &o, &story_text)).ok().and_then(|t| illus::clean_description(&t));
                        made_setting = n.clone();
                        n
                    });
                    let brief = drawers[0].chat.complete(&illus::visual_messages_for(&title, &o, &scene, &sh, setting.as_deref().unwrap_or(""), Some(family))).ok().and_then(|t| illus::clean_description(&t));
                    let mut desc = brief.unwrap_or_else(|| scene.text.chars().take(300).collect());
                    if !feedback.trim().is_empty() {
                        desc.push_str(&format!(" Change requested by the person: {}.", feedback.trim()));
                    }
                    let built = illus::compose(&o, &scene, Some(&desc), &sh, setting.as_deref().unwrap_or(""), Some(family));
                    let prompt = built.prompt.clone();
                    let (fw, fh) = family.fallback_size();
                    let params = iyakki_provider_kind::ImageParams { seed: Some(seed), negative: illus::API_TAKES_NEGATIVE_PROMPT.then(|| built.negative.clone()), verbose: o.pic.verbose, steps: rsteps, size: rsize, guidance: rguid };
                    let (r, trace) = svc.generate_image(picture_model.as_ref().map(|(p, m)| (p.as_str(), m.as_str())), &prompt, &format!("{fw}x{fh}"), &params);
                    // what the picture model sends is judged by its first bytes, never by its claimed type: size caps, a pixel
                    // cap from the header, a real decode, metadata stripped
                    let mut rejected = None;
                    let out = match r {
                        Ok((_, _, bytes, _)) => match scene::reply::validate(bytes, o.video, "the picture model") {
                            Ok(f) => {
                                let size = scene::reply::dimensions(&f.bytes, f.ext).map(|(w, h)| format!("{w}x{h}")).unwrap_or_default();
                                let log = format!("=== image model ===\nPROMPT\n{prompt}\nNEGATIVE (sent: {})\n{}\nSEED {seed}\nRESULT: {} accepted\n", built.negative_sent, built.negative, f.ext);
                                Ok(PicOut { bytes: f.bytes, ext: f.ext.to_string(), prompt: prompt.clone(), how: "image".into(), who: picture_model.as_ref().map_or("the picture model".into(), |m| m.1.clone()), note: None, log, sheet: None, setting: None, size, made: Some((look, Some(seed), trace.provider.clone(), trace.model.clone(), trace.ms)), links: Vec::new() })
                            }
                            Err(e) => {
                                rejected = Some(e.clone());
                                Err(e)
                            }
                        },
                        Err(e) => Err(e),
                    };
                    let info = piclog::RequestInfo { ts: clock(), story: &story_name, picture: idx + 1, scene: &scene.title, attempt, prompt: &prompt, negative: &built.negative, negative_sent: built.negative_sent, verbose: o.pic.verbose, style: look.id(), variant: variant_n + 1, realistic, guidance: rguid };
                    events.push(piclog::request_event(&trace, &info, rejected.as_deref(), ""));
                    out
                }
                _ => {
                    // the writer draws (a rich SVG when it is strong, else it picks a scene for the built-in cartoon); when the
                    // chosen agent cannot be reached the person's own model takes over
                    let mut last = "no drawing".to_string();
                    let mut out = None;
                    for d in &drawers {
                        let sh = need_sheet(d);
                        match scene::draw_picture(d.chat.as_ref(), &title, &o, &scene, &sh, &scene::DrawCfg { strong: d.strong, story_text: &story_text, feedback: &feedback, workdir: Some(workdir.as_path()), allow_video: o.video }) {
                            Ok(x) => {
                                out = Some(PicOut { bytes: x.bytes, ext: x.ext.into(), prompt: x.prompt, how: x.how.into(), who: d.who.clone(), note: x.note, log: x.log, sheet: None, setting: None, size: String::new(), made: None, links: x.links });
                                break;
                            }
                            Err(e) => last = format!("{}: {e}", d.who),
                        }
                    }
                    out.ok_or(last)
                }
            };
            let res = res.map(|mut r| {
                r.sheet = made_sheet.take();
                r.setting = made_setting.take();
                r
            });
            let _ = tx.send(PicMsg { gen, alt, idx, res, events, missing, linked: false });
        });
    }

    /// Who draws: the external agent that was chosen to write (when it is usable), then the writing model, each with whether
    /// it is strong enough to draw a rich SVG by itself. The agent is tried first and the model is the fallback.
    pub fn drawers(&self, port: &dyn TalesPort) -> Vec<Drawer> {
        let mut v = Vec::new();
        if self.prefs.agent.is_some() {
            if let Ok(Some((req, label, _, _))) = self.agent_request(port, &[]) {
                let Some(chat) = port.scene_agent_chat(&req) else { return v };
                v.push(Drawer { chat, strong: true, who: label });
            }
        }
        let model = match self.model_plan(port) {
            ModelPlan::Chosen(c) => c.model,
            ModelPlan::SameAsChat { .. } => port.chat_choice().map_or(String::new(), |c| c.model),
        };
        v.push(Drawer { chat: self.backend(port), strong: scene::rich::is_strong(&model, false), who: if model.is_empty() { "the chat model".into() } else { model } });
        #[cfg(any(test, feature = "testing"))]
        if let Some(f) = &self.test_drawers {
            return f(v);
        }
        v
    }

    pub fn poll_pictures(&mut self, port: &dyn TalesPort) {
        self.poll_install(port);
        self.update_wait(port);
        let mut changed = false;
        while let Ok(mut m) = self.pic_rx.try_recv() {
            // the log keeps what was asked even when the answer comes after a cancel
            let evs = std::mem::take(&mut m.events);
            self.log_pic_events(evs);
            if m.gen != self.pic_gen || m.idx >= self.pics.len() {
                continue;
            }
            self.pic_started.remove(&m.idx);
            if m.missing.is_some() {
                self.alt_running = None;
                self.alt_jobs.clear();
            }
            if let Some(miss) = m.missing.take() {
                for i in 0..self.pics.len() {
                    if (m.alt.is_none() && i == m.idx) || (self.pics[i].state == PicState::Queued && self.pics[i].mode == PictureMode::ImageModel) {
                        self.pics[i].state = PicState::Failed(miss.text.clone());
                        self.pic_result(i, false, &miss.text);
                    }
                }
                self.note = Some((miss.text.clone(), false));
                self.install = Some(Install { provider: miss.provider, model: miss.model, size: miss.size, started: false });
                changed = true;
                continue;
            }
            // one more picture of a scene (another style or seed): it goes into the scene's gallery, never over the main one
            if let Some(job) = m.alt {
                self.alt_running = None;
                match m.res {
                    Ok(out) => match self.cap_error_alt(out.bytes.len() as u64) {
                        Ok(()) => {
                            let (look, seed, provider, model, ms) = out.made.clone().unwrap_or((job.look, None, String::new(), out.who.clone(), 0));
                            let key = hash(&format!("{}{}{}{}", self.pic_gen, m.idx, job.look.id(), out.bytes.len())) ^ out.bytes.len() as u64;
                            self.pics[m.idx].alts.push(Alt { look, seed, provider, model, size: out.size.clone(), ms, ext: out.ext.clone(), prompt: out.prompt.clone(), key, bytes: Arc::new(out.bytes) });
                            self.note = Some((format!("Another picture of scene {} is ready ({}).", m.idx + 1, look.label()), true));
                        }
                        Err(e) => {
                            // the limit: nothing that exists is dropped; the rest of the job stops, and it says so
                            let left = self.alt_jobs.len();
                            self.alt_jobs.clear();
                            let ev = self.pic_decision(plk::NOT_KEPT, m.idx, &format!("not kept: {e}. The {left} pictures still to be drawn were stopped."));
                            self.log_pic_events(vec![ev]);
                            self.note = Some((format!("Stopped: {e}. Your pictures are all still there; {left} more were not drawn."), false));
                        }
                    },
                    Err(e) => self.note = Some((format!("Another picture of scene {} could not be drawn: {e}", m.idx + 1), false)),
                }
                changed = true;
                continue;
            }
            match m.res {
                Ok(out) => {
                    if let Err(e) = self.cap_error(m.idx, out.bytes.len() as u64) {
                        let why = format!("Not kept: {e}.");
                        let ev = self.pic_decision(plk::NOT_KEPT, m.idx, &format!("not kept: {e}"));
                        self.log_pic_events(vec![ev]);
                        self.pics[m.idx].state = PicState::Failed(why.clone());
                        self.pic_result(m.idx, false, &why);
                        changed = true;
                        continue;
                    }
                    if self.setting.is_none() && out.setting.is_some() {
                        self.setting = out.setting.clone();
                        if let Some(id) = self.saved_id() {
                            let _ = self.store.save_setting(&id, self.setting.as_deref().unwrap_or(""));
                        }
                    }
                    if self.sheet.is_none() {
                        self.sheet = out.sheet.clone();
                    }
                    let how = match out.how.as_str() {
                        "image" => format!("images via {}", out.who),
                        "rich svg" => format!("rich SVG drawing by {}", out.who),
                        "image reply" => format!("image by {}", out.who),
                        "video reply" => format!("video clip by {}", out.who),
                        "linked" => format!("picture from {} (you approved the link)", out.who),
                        "composed" => "cartoon (built-in)".to_string(),
                        "model svg" => "an SVG drawing".to_string(),
                        _ => "built-in sketch".to_string(),
                    };
                    let said = out.note.clone().map_or(format!("Picture {} is ready ({how}).", m.idx + 1), |n| format!("Picture {} is ready ({how}). {n}", m.idx + 1));
                    let p = &mut self.pics[m.idx];
                    p.key = p.key.wrapping_add(1).wrapping_mul(1_000_003) ^ out.bytes.len() as u64;
                    p.bytes = Some(Arc::new(out.bytes));
                    p.ext = out.ext;
                    p.prompt = out.prompt;
                    p.how = how;
                    p.size = out.size;
                    p.links = out.links;
                    if let Some((look, seed, provider, model, ms)) = out.made {
                        (p.look, p.seed, p.provider, p.model, p.ms) = (look, seed, provider, model, ms);
                    }
                    p.log = out.log;
                    p.state = PicState::Ready;
                    self.pic_result(m.idx, true, &said);
                }
                Err(e) if m.linked => {
                    // the approved link did not give a picture: the scene keeps what it has, and the person is told why
                    self.note = Some((format!("The picture from the link could not be used: {e}"), false));
                }
                Err(e) if self.pic_auto && self.pics[m.idx].mode == PictureMode::ImageModel => {
                    // images are preferred; when they do not work this scene and the rest are drawn as SVG, said once
                    self.pic_auto = false;
                    self.alt_jobs.clear();
                    self.pic_result(m.idx, false, &format!("image generation did not work: {e}"));
                    let mut ev = self.pic_decision(plk::SVG_FALLBACK, m.idx, "this picture and the rest are drawn as SVG");
                    ev.svg_fallback = format!("image generation did not work: {e}");
                    self.log_pic_events(vec![ev]);
                    let title = if self.story.title.is_empty() { st::title_from_topic(&self.story_opts.topic) } else { self.story.title.clone() };
                    let mut again = Vec::new();
                    for i in 0..self.pics.len() {
                        if i == m.idx || self.pics[i].state == PicState::Queued {
                            let mut o = self.story_opts.clone();
                            o.style = self.pics[i].style;
                            let q = &mut self.pics[i];
                            q.mode = PictureMode::Drawings;
                            q.state = PicState::Queued;
                            q.prompt = format!("SVG drawing: {}", st::image_prompt(&title, &o, &q.scene));
                            q.call = None;
                            again.push(i);
                        }
                    }
                    self.push_draw_calls(&again);
                    self.note = Some((format!("Image generation did not work ({e}), so the remaining pictures are drawn as SVG instead."), false));
                }
                Err(e) => {
                    self.pics[m.idx].state = PicState::Failed(e.clone());
                    self.pic_result(m.idx, false, &e);
                }
            }
            changed = true;
        }
        if changed {
            // a story that is saved keeps its pictures up to date
            if self.saved.is_some() {
                self.persist_pictures();
            }
            self.persist_chat();
            self.pump_pictures(port);
            self.dirty = true;
        }
    }

    /// The person said yes to this address for this scene's picture: fetched in the background through the account's own host rules,
    /// judged like any picture, and shown as the scene's picture when it passes.
    pub fn fetch_link(&mut self, idx: usize, url: &str, port: &dyn TalesPort) {
        let Some(p) = self.pics.get(idx) else { return };
        let prompt = p.prompt.clone();
        let allow_video = self.story_opts.video;
        let (gen, tx, url) = (self.pic_gen, self.pic_tx.clone(), url.to_string());
        let host = iyakki_scene::reply::host_of(&url);
        let worker = port.worker();
        port.audit("story.picture.link", &host);
        self.note = Some((format!("Fetching the picture from {host}..."), true));
        std::thread::spawn(move || {
            let res = worker.fetch_link(&url, allow_video).map(|f| PicOut { bytes: f.bytes, ext: f.ext.to_string(), prompt, how: "linked".into(), who: host, note: None, log: String::new(), sheet: None, setting: None, size: String::new(), made: None, links: Vec::new() });
            let _ = tx.send(PicMsg { gen, alt: None, idx, res, events: Vec::new(), missing: None, linked: true });
        });
    }

    pub fn redo_picture(&mut self, idx: usize, port: &dyn TalesPort) {
        let Some(p) = self.pics.get_mut(idx) else { return };
        if p.state == PicState::Working {
            return;
        }
        p.state = PicState::Queued;
        self.held = false;
        self.push_draw_calls(&[idx]);
        self.pump_pictures(port);
    }

    /// Stop the picture job: the one being made is dropped when it comes back, the waiting ones are not started. The text and
    /// the finished pictures stay; a cancelled picture can be tried again.
    pub fn cancel_pictures(&mut self) {
        self.pic_gen += 1;
        self.held = false;
        self.alt_jobs.clear();
        self.alt_running = None;
        let mut evs = Vec::new();
        for i in 0..self.pics.len() {
            if matches!(self.pics[i].state, PicState::Working | PicState::Queued) {
                evs.push(self.pic_decision(plk::SKIPPED, i, if self.pics[i].state == PicState::Working { "cancelled while it was being drawn (an answer that comes later is only logged)" } else { "cancelled before it started" }));
                self.pics[i].state = PicState::Failed("cancelled".into());
                self.pic_result(i, false, "cancelled");
            }
        }
        self.log_pic_events(evs);
        if self.saved.is_some() {
            self.persist_pictures();
        }
        self.persist_chat();
        self.note = Some(("The pictures were cancelled; the story is as it was.".into(), true));
        self.dirty = true;
    }

    /// Remove one picture from the story (and from its folder when saved).
    pub fn delete_picture(&mut self, idx: usize) {
        if idx >= self.pics.len() {
            return;
        }
        if self.pics.iter().any(|p| p.state == PicState::Working) {
            self.note = Some(("Wait for the picture being drawn, or press Cancel pictures, then delete.".into(), false));
            return;
        }
        self.pics.remove(idx);
        if self.saved.is_some() {
            self.persist_pictures();
        }
        self.note = Some(("The picture was deleted.".into(), true));
        self.dirty = true;
    }

    // ------------------------------------------------------------------------------------------------ waiting, time limit, install

    /// The time limit for one picture, the pause between health calls and between "waiting" lines of the log.
    pub fn timing(&self) -> (Duration, Duration, Duration) {
        self.test_timing.unwrap_or((Duration::from_secs(60 * self.pic_now().limit_min() as u64), Duration::from_secs(5), Duration::from_secs(30)))
    }

    pub fn secs_text(d: Duration) -> String {
        let s = d.as_secs();
        if s >= 120 { format!("{} min {} s", s / 60, s % 60) } else { format!("{s} s") }
    }

    /// While a picture is being drawn: how long it has taken, what the server says (asked every few seconds), a line in the log
    /// now and then, and the time limit.
    pub fn update_wait(&mut self, port: &dyn TalesPort) {
        while let Ok(r) = self.health_rx.try_recv() {
            self.health_busy = false;
            let target = self.pic_target.as_ref().map_or(String::new(), |t| t.1.clone());
            self.server_state = match r {
                Ok((text, loaded)) => format!("server: {text}; {target} is {}", if loaded.iter().any(|m| *m == target) { "loaded" } else { "not loaded yet (loading, or downloading)" }),
                Err(e) => format!("server: its health call failed ({e})"),
            };
        }
        let Some(idx) = self.pics.iter().position(|p| p.state == PicState::Working).or(self.alt_running) else {
            self.wait_text = None;
            self.server_state.clear();
            return;
        };
        let Some(t0) = self.pic_started.get(&idx).copied() else { return };
        let el = t0.elapsed();
        let (limit, every, log_every) = self.timing();
        if el >= limit {
            self.time_out(idx, el, limit);
            return;
        }
        if let Some((p, _)) = self.pic_target.clone().filter(|(p, _)| port.provider_is_lemonade(p)) {
            if !self.health_busy && self.health_last.is_none_or(|l| l.elapsed() >= every) {
                self.health_busy = true;
                self.health_last = Some(Instant::now());
                let (svc, tx) = (port.worker(), self.health_tx.clone());
                std::thread::spawn(move || {
                    let _ = tx.send(svc.lemonade_health(&p));
                });
            }
        }
        let state = if self.server_state.is_empty() { String::new() } else { format!(" ({})", self.server_state) };
        let text = format!("Drawing picture {}: {} so far. The model is loading or generating; on a CPU this can take minutes. The limit is {}; Cancel pictures stops it{state}.", idx + 1, Self::secs_text(el), Self::secs_text(limit));
        self.wait_text = Some(text);
        if self.last_wait_log.is_none_or(|l| l.elapsed() >= log_every) && el >= log_every.min(Duration::from_secs(30)) {
            self.last_wait_log = Some(Instant::now());
            let mut ev = self.pic_decision(plk::WAITING, idx, &format!("waiting {}{}", Self::secs_text(el), if self.server_state.is_empty() { String::new() } else { format!(" · {}", self.server_state) }));
            if let Some((p, m)) = &self.pic_target {
                (ev.provider, ev.model) = (p.clone(), m.clone());
            }
            self.log_pic_events(vec![ev]);
        }
        self.dirty = true;
    }

    /// The picture took longer than the limit: stop waiting for it (an answer that comes later is dropped, and only logged).
    pub fn time_out(&mut self, idx: usize, el: Duration, limit: Duration) {
        let why = format!("Stopped after {} (the time limit for a picture is {}). The server may still be working on it; try again, choose a smaller picture model, or raise the limit in Options.", Self::secs_text(el), Self::secs_text(limit));
        let mut evs = vec![self.pic_decision(plk::TIMEOUT, idx, &format!("timeout after {}: {}", Self::secs_text(el), if self.server_state.is_empty() { "no server state" } else { &self.server_state }))];
        if let Some((p, m)) = &self.pic_target {
            (evs[0].provider, evs[0].model) = (p.clone(), m.clone());
        }
        self.pic_gen += 1;
        self.held = false;
        self.alt_jobs.clear();
        self.alt_running = None;
        for i in 0..self.pics.len() {
            if matches!(self.pics[i].state, PicState::Working | PicState::Queued) {
                self.pics[i].state = PicState::Failed(why.clone());
                self.pic_result(i, false, &why);
            }
        }
        self.pic_started.clear();
        self.log_pic_events(std::mem::take(&mut evs));
        if self.saved.is_some() {
            self.persist_pictures();
        }
        self.persist_chat();
        self.note = Some((why, false));
        self.dirty = true;
    }

    /// Start the download of the picture model that is not on the server (the same download as Settings → Models, with progress).
    pub fn start_install(&mut self, port: &dyn TalesPort) {
        let Some((p, m, size)) = self.install.as_ref().filter(|i| !i.started).map(|i| (i.provider.clone(), i.model.clone(), i.size.clone())) else { return };
        match port.pull_model(&p, &m) {
            Ok(()) => {
                if let Some(i) = self.install.as_mut() {
                    i.started = true;
                }
                let mut ev = piclog::decision_event((self.clock)(), plk::PULL, &self.story_name(), 0, "", &format!("pull started: {m} on {p} ({size})"));
                (ev.provider, ev.model) = (p, m);
                self.log_pic_events(vec![ev]);
                port.audit("story.picture_install", &self.pic_log_name());
            }
            Err(e) => self.note = Some((format!("The download could not start: {e}"), false)),
        }
    }

    /// Follow the download: progress as a note, then, when it is there, the waiting pictures are drawn.
    pub fn poll_install(&mut self, port: &dyn TalesPort) {
        let Some((p, m, started)) = self.install.as_ref().map(|i| (i.provider.clone(), i.model.clone(), i.started)) else { return };
        if !started {
            return;
        }
        let Some(st) = port.pull_status().into_iter().find(|s| s.provider == p && s.model == m) else { return };
        let log = |a: &mut StoryApp, d: String| {
            let mut ev = piclog::decision_event((a.clock)(), plk::PULL, &a.story_name(), 0, "", &d);
            (ev.provider, ev.model) = (p.clone(), m.clone());
            a.log_pic_events(vec![ev]);
        };
        if let Some(e) = &st.error {
            self.install = self.install.take().map(|i| Install { started: false, ..i });
            log(self, format!("pull failed: {e}"));
            self.note = Some((format!("The download of {m} failed: {e}"), false));
        } else if st.done {
            self.install = None;
            self.image_models = port.image_models();
            log(self, "pull finished".into());
            self.note = Some((format!("{m} is installed. Drawing the pictures now."), true));
            let again: Vec<usize> = (0..self.pics.len()).filter(|&i| self.pics[i].mode == PictureMode::ImageModel && matches!(&self.pics[i].state, PicState::Failed(w) if w.contains("is not downloaded"))).collect();
            for &i in &again {
                self.pics[i].state = PicState::Queued;
            }
            self.held = false;
            self.push_draw_calls(&again);
            self.pump_pictures(port);
        } else {
            let pct = if st.total > 0 { format!(" {}%", st.completed * 100 / st.total) } else { String::new() };
            self.note = Some((format!("Downloading {m}{pct}: {}", st.describe()), true));
        }
        self.dirty = true;
    }

    /// The picture choices in force: the story's when one is on the page, else the defaults.
    pub fn pic_now(&self) -> illus::PictureOpts {
        if self.story.is_empty() && self.gen.is_none() { self.opts.pic.clone() } else { self.story_opts.pic.clone() }
    }

    /// The picture models that are there, as (provider, model).
    pub fn found_images(&self, port: &dyn TalesPort) -> Vec<(String, String)> {
        if self.image_models.is_empty() { port.image_models() } else { self.image_models.clone() }
    }

    /// The picture model that `pic` leads to (the story's own choice, Quality, the selected one) with its size and steps.
    pub fn resolved_for(&self, pic: &illus::PictureOpts, port: &dyn TalesPort) -> Option<illus::Resolved> {
        self.resolved_look(pic, pic.styles_list()[0], port)
    }

    /// [`Self::resolved_for`] for drawing in `look` (the Photorealistic style takes the most realistic installed model).
    pub fn resolved_look(&self, pic: &illus::PictureOpts, look: illus::Look, port: &dyn TalesPort) -> Option<illus::Resolved> {
        let cands = port.picture_choices(&self.found_images(port));
        let avail = self.memory_now(port).filter(|m| m.total > 0).map(|m| m.available >> 20);
        illus::resolve_for(pic, look, self.prefs.picture_model.as_ref(), &cands, avail)
    }

    /// Change a picture choice for the defaults and for the story on the page, and keep it (with the story when it is saved).
    pub fn set_pic(&mut self, f: impl Fn(&mut illus::PictureOpts)) {
        f(&mut self.opts.pic);
        self.opts.pic.seed = None;
        if !self.story.is_empty() {
            f(&mut self.story_opts.pic);
            if !self.story_opts.pic.keep_look {
                self.story_opts.pic.seed = None;
            }
            if let Some(id) = self.saved_id() {
                let _ = self.store.save_picture_opts(&id, &self.story_opts.pic);
            }
        }
        self.save_prefs();
        self.dirty = true;
    }

    /// The next picture model for this story: automatic, then each picture model of every provider, round and round.
    pub fn cycle_story_model(&mut self, port: &dyn TalesPort) {
        let found: Vec<(String, String)> = port.picture_choices(&self.found_images(port)).into_iter().map(|c| (c.provider, c.model)).collect();
        let cur = self.pic_now().model;
        let at = cur.and_then(|c| found.iter().position(|(p, m)| *p == c.provider && *m == c.model));
        let next = match at {
            None if self.pic_now().model.is_none() => found.first(),
            None => None,
            Some(i) => found.get(i + 1),
        };
        let next = next.map(|(p, m)| ModelChoice { provider: p.clone(), model: m.clone() });
        self.set_pic(move |o| o.model = next.clone());
    }

    /// Note picture log events; they go to the story's folder as soon as the story is saved.
    pub fn log_pic_events(&mut self, evs: Vec<PicEvent>) {
        if evs.is_empty() {
            return;
        }
        self.pic_pending.extend(evs);
        self.flush_pic_log();
    }

    pub fn flush_pic_log(&mut self) {
        let Some(id) = self.saved_id() else {
            let n = self.pic_pending.len();
            if n > 500 {
                self.pic_pending.drain(..n - 500);
            }
            return;
        };
        if !self.pic_pending.is_empty() && self.store.append_picture_log(&id, &self.pic_pending).is_ok() {
            self.pic_pending.clear();
        }
    }

    pub fn pic_log_name(&self) -> String {
        self.install.as_ref().map_or(String::new(), |i| format!("{} {}", i.provider, i.model))
    }

    pub fn story_name(&self) -> String {
        if self.story.title.is_empty() { st::title_from_topic(&self.story_opts.topic) } else { self.story.title.clone() }
    }

    /// A decision about picture `idx` for the log (skipped, not kept, cancelled).
    pub fn pic_decision(&self, kind: &str, idx: usize, decision: &str) -> PicEvent {
        piclog::decision_event((self.clock)(), kind, &self.story_name(), idx + 1, self.pics.get(idx).map_or("", |p| p.scene.title.as_str()), decision)
    }

    /// Every picture log event of the story on the page, oldest first.
    pub fn pic_log_events(&self) -> Vec<PicEvent> {
        let mut v = self.saved_id().map(|id| self.store.load_picture_log(&id)).unwrap_or_default();
        v.extend(self.pic_pending.iter().cloned());
        v
    }

    pub fn open_pic_log(&mut self) {
        self.menu = None;
        self.focus = Focus::None;
        self.modal = Some(Modal::PicLog(PicLogView { top: Cell::new(usize::MAX), at_end: Cell::new(true), total: Cell::new(0), rows: Cell::new(0), confirm_clear: false }));
    }

    /// Scroll the picture log by `d` wrapped lines (negative: towards the oldest).
    pub fn log_scroll(&mut self, d: i64) {
        if let Some(Modal::PicLog(v)) = &self.modal {
            let max = v.total.get().saturating_sub(v.rows.get());
            let at = v.top.get().min(max) as i64;
            let to = (at + d).clamp_safe(0, max as i64) as usize;
            v.top.set(to);
            v.at_end.set(to == max);
            self.dirty = true;
        }
    }

    /// The machine's memory now (tests set their own).
    pub fn memory_now(&self, port: &dyn TalesPort) -> Option<iyakki_sysinfo_model::Memory> {
        #[cfg(any(test, feature = "testing"))]
        if self.test_memory.is_some() {
            return self.test_memory;
        }
        port.memory()
    }

    /// The picture model that would be used: the chosen one, else the first found, with its Lemonade entry when it has one.
    pub fn picture_info(&self, port: &dyn TalesPort) -> Option<iyakki_provider_kind::lemonade::Model> {
        let r = self.resolved_for(&self.pic_now(), port)?;
        port.lemonade_model(&r.provider, &r.model)
    }

    /// Whether the picture step can run now, and why not.
    pub fn readiness(&self, port: &dyn TalesPort) -> Readiness {
        let found = self.found_images(port);
        // the memory of this computer matters only for a model that loads on this computer
        let remote = self.resolved_for(&self.pic_now(), port).is_some_and(|r| r.remote);
        illus::readiness(&found, &port.lemonade_images_missing(), self.picture_info(port).as_ref(), if remote { None } else { self.memory_now(port) }.as_ref())
    }

    /// The sentence about memory when a picture job must wait, else `None`.
    pub fn low_memory(&self, port: &dyn TalesPort) -> Option<String> {
        match self.readiness(port) {
            r @ Readiness::LowMemory { .. } => illus::reason(&r),
            _ => None,
        }
    }

    /// The honest timing line for `n` pictures with the picture model that would be used.
    pub fn timing_line(&self, port: &dyn TalesPort, n: usize) -> String {
        let info = self.picture_info(port);
        let name = self.resolved_for(&self.pic_now(), port).map_or("The picture model".to_string(), |r| r.model);
        let cpu_only = port.cpu_only();
        illus::timing_text(&name, info.and_then(|m| m.image_defaults), n, cpu_only)
    }

    /// May a picture of `new` bytes be kept (the story's cap and the cap over all stories)?
    pub fn cap_error(&self, idx: usize, new: u64) -> Result<(), String> {
        // every picture of the story counts, the extra ones of a scene too (the one being replaced does not)
        let story: u64 = self.story_pic_bytes(Some(idx));
        let all = if self.saved.is_some() { self.store.pictures_bytes().max(story) } else { story };
        illus::check_caps(story, all, new)
    }

    /// May one more picture of `new` bytes be kept beside the others?
    pub fn cap_error_alt(&self, new: u64) -> Result<(), String> {
        let story = self.story_pic_bytes(None);
        let all = if self.saved.is_some() { self.store.pictures_bytes().max(story) } else { story };
        illus::check_caps(story, all, new)
    }

    /// The bytes of the pictures of the story on the page: main and extra ones, without the main one of `skip`.
    pub fn story_pic_bytes(&self, skip: Option<usize>) -> u64 {
        self.pics
            .iter()
            .enumerate()
            .map(|(i, p)| {
                let main = if Some(i) != skip && p.state == PicState::Ready { p.bytes.as_ref().map_or(0, |b| b.len() as u64) } else { 0 };
                main + p.alts.iter().map(|a| a.bytes.len() as u64).sum::<u64>()
            })
            .sum()
    }

    // ------------------------------------------------------------------------------------------------ reading

    pub fn synth(&self, port: &dyn TalesPort) -> Arc<dyn st::Synth> {
        match &self.saved_audio {
            Some(p) => p.clone(),
            None if self.browser_voice => browser_synth(),
            None if self.direct.is_some() => self.direct.as_ref().map(|d| d.synth()).unwrap_or_else(|| self.file_synth(port)),
            None => self.file_synth(port),
        }
    }

    /// The engine writing audio files (what Save audio uses, also for an engine that speaks by itself when reading).
    pub fn file_synth(&self, port: &dyn TalesPort) -> Arc<dyn st::Synth> {
        port.reading_synth(SpeakOpts { voice: self.prefs.voice.clone(), speed: self.prefs.speed, lang: Some(hostvoice::lang_tag(&self.story_opts.language).to_string()) })
    }

}
