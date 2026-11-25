//! TALES, Writing: the idea box, the model's turn, revisions, versions, saving and the PDF.
use super::*;

impl StoryApp {
    /// Send: a new story when there is none on the page, a correction of it when there is. While a story is being written a
    /// correction stops it where it is and is applied next.
    pub fn submit(&mut self, port: &dyn TalesPort) {
        self.sync_fields();
        let text = self.idea.text.trim().to_string();
        if self.gen.is_some() {
            if text.is_empty() {
                self.note = Some(("Type what to change: the story stops where it is and your change is applied.".into(), false));
                return;
            }
            self.queued = Some(text);
            self.idea.set("");
            self.sync_fields();
            self.stop_writing();
            self.note = Some(("Stopping here; your change is applied next.".into(), true));
            return;
        }
        if self.story.blocks.is_empty() {
            self.start_create(port);
        } else if let Some((scene, url)) = port.asks_for_link(&text, self.pics.len()) {
            // "use https://... for picture 2": the same question as the button; nothing is fetched before the person says yes
            self.idea.set("");
            self.sync_fields();
            self.modal = Some(Modal::FetchLink(scene, url));
        } else if port.asks_for_pdf(&text) {
            // "make it a PDF": the same writer as the button, shown as a step of the conversation
            self.idea.set("");
            self.sync_fields();
            self.save_pdf(port, Some(&text));
        } else {
            self.start_revision(text, port);
        }
    }

    /// Save the story as `Stories/<title>/story.pdf` beside `story.md`: the text, the headings and the pictures that are ready, written
    /// by the in-house PDF writer (not by the model). The story is saved first when it is not yet. `asked` is the person's message when
    /// the PDF was asked for in the conversation: it and the result then show as a step on the timeline.
    pub fn save_pdf(&mut self, port: &dyn TalesPort, asked: Option<&str>) {
        if self.gen.is_some() {
            self.note = Some(("Wait until the story is written, then save the PDF.".into(), false));
            return;
        }
        if self.story.blocks.is_empty() {
            self.note = Some(("There is no story to put in a PDF yet.".into(), false));
            return;
        }
        let step = asked.map(|t| {
            self.convo.push(Message::user(t.to_string()));
            self.step("PDF", "story.pdf")
        });
        let result = self.make_pdf(port);
        match &result {
            Ok(m) => self.note = Some((m.clone(), true)),
            Err(e) => self.note = Some((e.clone(), false)),
        }
        if let Some(c) = step {
            self.step_done(&c, result.is_ok(), &result.unwrap_or_else(|e| e));
        }
    }

    pub fn make_pdf(&mut self, port: &dyn TalesPort) -> Result<String, String> {
        if self.saved.is_none() {
            self.save_story(port, "");
        }
        let id = self.saved_id().ok_or_else(|| self.note.as_ref().map(|n| n.0.clone()).unwrap_or_else(|| "The story could not be saved, so there is no folder for its PDF.".into()))?;
        let (story, opts, pics, _) = self.export_source(None)?;
        let made = port.story_pdf(&story, &opts, &pics, (self.clock)() as i64)?;
        let (path, replaced) = self.store.save_pdf(&id, &made.bytes)?;
        port.audit("story.pdf", &id);
        let _ = path;
        Ok(format!(
            "Saved Stories/{id}/story.pdf ({} page{}, {} picture{}{}){}.",
            made.pages,
            if made.pages == 1 { "" } else { "s" },
            made.pictures,
            if made.pictures == 1 { "" } else { "s" },
            if made.left_out > 0 { format!(", {} left out", made.left_out) } else { String::new() },
            if replaced { "; the earlier copy was replaced" } else { "" }
        ))
    }

    /// A step of the conversation: shown as a dot on the timeline of the panel until `step_done` gives its result.
    pub fn step(&mut self, name: &str, input: &str) -> ToolCall {
        self.step_seq += 1;
        let c = ToolCall { id: format!("tales-{}", self.step_seq), name: name.into(), args: serde_json::json!({ "prompt": input }) };
        self.convo.push(Message::assistant_calls("", vec![c.clone()]));
        c
    }

    pub fn step_done(&mut self, c: &ToolCall, ok: bool, text: &str) {
        self.tool_ok.insert(c.id.clone(), ok);
        self.convo.push(Message::tool_result(c, if ok { text.to_string() } else { format!("Failed: {text}") }));
    }

    /// Several drawing steps at once (one message), each the step of one picture.
    pub fn push_draw_calls(&mut self, idxs: &[usize]) {
        let mut calls = Vec::new();
        for &i in idxs {
            self.step_seq += 1;
            let id = format!("tales-{}", self.step_seq);
            let title = self.pics[i].scene.title.clone();
            calls.push(ToolCall { id: id.clone(), name: "Draw".into(), args: serde_json::json!({ "prompt": format!("picture {}: {}", i + 1, title) }) });
            self.pics[i].call = Some(id);
        }
        if !calls.is_empty() {
            self.convo.push(Message::assistant_calls("", calls));
        }
    }

    /// The result of the drawing step of picture `idx`.
    pub fn pic_result(&mut self, idx: usize, ok: bool, text: &str) {
        if let Some(id) = self.pics.get_mut(idx).and_then(|p| p.call.take()) {
            self.step_done(&ToolCall { id, name: "Draw".into(), args: serde_json::Value::Null }, ok, text);
        }
    }

    /// What the person asked for so far (the user messages of the conversation), oldest first.
    pub fn user_requests(&self) -> Vec<String> {
        self.convo.iter().filter(|m| m.role == iyakki_chat_model::Role::User).map(|m| m.content.clone()).collect()
    }

    /// The line of the "Plan" step: what was understood.
    pub fn plan_text(&self, o: &StoryOptions, port: &dyn TalesPort) -> String {
        let pictures = match o.pictures {
            PictureMode::Off => "off".to_string(),
            PictureMode::ImageModel => format!("images via {}", self.picture_name(port)),
            _ => format!("{}, {}", match o.drawing {
                st::DrawingMode::Auto => "drawings (a strong model draws SVG or an image itself, a small one uses the built-in cartoon)",
                st::DrawingMode::Image => "an image model, else drawings",
                st::DrawingMode::Rich => "rich SVG by the model",
                st::DrawingMode::Cartoon => "cartoon (built-in)",
            }, o.style.label().to_lowercase()),
        };
        format!("For {} · {} · {} · {} · {}. Pictures: {pictures}.", o.audience.label(), o.genre.label(), o.length.label(), o.language, o.tone.label())
    }

    pub fn picture_name(&self, port: &dyn TalesPort) -> String {
        match self.picture_plan(port) {
            ModelPlan::Chosen(c) => c.model,
            ModelPlan::SameAsChat { .. } => self.image_models.first().map_or("the picture model".to_string(), |m| m.1.clone()),
        }
    }

    pub fn start_create(&mut self, port: &dyn TalesPort) {
        if self.gen.is_some() {
            return;
        }
        self.sync_fields();
        let base = match self.opts.validated() {
            Ok(o) => o,
            Err(e) => {
                self.note = Some((e, false));
                self.error = None;
                return;
            }
        };
        // what the text says fills the options the person did not choose; the pictures follow what is available
        self.refresh_tts(port);
        let mut o = self.with_hints(&base);
        o.pictures = self.resolve_pictures(base.pictures, base.drawing);
        o.style = self.style_for(&base.topic, o.audience, o.genre);
        let msgs = match st::messages(&o) {
            Ok(m) => m,
            Err(e) => {
                self.note = Some((e, false));
                return;
            }
        };
        self.reader.stop();
        self.reset_story();
        self.pic_auto = base.pictures == PictureMode::Auto && o.pictures == PictureMode::ImageModel && o.drawing != st::DrawingMode::Image;
        self.story_opts = o.clone();
        self.opts = base;
        self.menu = None;
        self.save_prefs();
        self.ver = 1;
        self.ver_count = 1;
        self.convo.push(Message::user(o.topic.clone()));
        let plan = self.step("Plan", &o.topic);
        let text = self.plan_text(&o, port);
        self.step_done(&plan, true, &text);
        self.launch(port, msgs, "story.create");
        self.idea.set("");
        self.sync_fields();
        self.dismissed = Default::default();
    }

    /// The look of the pictures: a style named in the text (unless its chip was removed), else the one for the kind of story.
    pub fn style_for(&self, text: &str, audience: Audience, genre: Genre) -> PictureStyle {
        match st::parse_hints(text).style {
            Some((s, said)) if self.dismissed[H_STYLE].as_deref() != Some(said.as_str()) => s,
            _ => st::auto_style(audience, genre),
        }
    }

    /// Start the model (or the agent) on `msgs`: the story streams into the page. `what` is the audit line.
    pub fn launch(&mut self, port: &dyn TalesPort, msgs: Vec<Message>, what: &str) {
        let (label, note) = self.model_label(port);
        let own_name = match self.model_plan(port) {
            ModelPlan::Chosen(c) => Self::model_name(&c),
            ModelPlan::SameAsChat { .. } => port.chat_choice().map_or(label, |c| Self::model_name(&c)),
        };
        self.note = note.map(|n| (n, false));
        // an external agent writes it when one is chosen and usable; otherwise our own model, with the reason
        let ext = match self.agent_request(port, &msgs) {
            Ok(x) => x,
            Err(why) => {
                let n = format!("{why} Writing it with {own_name} instead.");
                self.agent_note = Some(n.clone());
                self.note = Some((n, false));
                None
            }
        };
        self.run_model = ext.as_ref().map_or(own_name.clone(), |e| e.2.clone());
        let o = &self.story_opts;
        port.audit(what, &format!("{} {} {} {} pictures {} topic {} chars model {}", o.audience.id(), o.genre.id(), o.length.id(), o.language, o.pictures.id(), o.topic.chars().count(), if ext.is_some() { format!("agent {}", self.prefs.agent.clone().unwrap_or_default()) } else { self.run_model_for_audit(port) }));
        let write = self.step("Write", &format!("{} {} story in {}", o.length.label(), o.genre.label().to_lowercase(), o.language));
        self.write_call = Some(write);
        self.thinking.clear();
        self.think_since = Some(Instant::now());
        let (tx, rx) = channel();
        let stop = Arc::new(AtomicBool::new(false));
        let (chat, stop2) = (self.backend(port), stop.clone());
        let svc_run = port.worker();
        std::thread::spawn(move || {
            fn own(chat: &Arc<dyn ChatBackend>, msgs: &[Message], tx: &Sender<GenMsg>, stop: &Arc<AtomicBool>) -> Result<String, String> {
                // no tools are ever passed: the model can only write
                chat.complete_tools_stream(msgs, &[], &mut |piece| {
                    if !piece.is_empty() {
                        let _ = tx.send(GenMsg::Delta(piece.to_string()));
                    }
                    !stop.load(Ordering::SeqCst)
                })
                .map(|t| t.text)
            }
            let r = match ext {
                None => own(&chat, &msgs, &tx, &stop2),
                Some((req, label, _shown, advisor)) => {
                    let h = svc_run.start_agent(req);
                    let (txs, txt) = (tx.clone(), tx.clone());
                    let got = st::follow_run(
                        h.as_ref(),
                        &|| stop2.load(Ordering::SeqCst),
                        &mut |t| {
                            let _ = txs.send(GenMsg::Snapshot(t.to_string()));
                        },
                        &mut |t| {
                            let _ = txt.send(GenMsg::Thinking(t.to_string()));
                        },
                    );
                    match got {
                        Ok(t) if !t.trim().is_empty() => Ok(t),
                        other if stop2.load(Ordering::SeqCst) => other.map(|_| String::new()),
                        other => {
                            let why = other.err().unwrap_or_else(|| "it sent no story".into());
                            let who = if advisor { format!("{label} runs as an advisor here (read-only) and could not write the story: {why}") } else { format!("{label} could not write the story: {why}") };
                            let _ = tx.send(GenMsg::Snapshot(String::new()));
                            let _ = tx.send(GenMsg::Note(format!("{who} Writing it with {own_name} instead.")));
                            let _ = tx.send(GenMsg::Who(own_name.clone()));
                            own(&chat, &msgs, &tx, &stop2)
                        }
                    }
                }
            };
            let _ = tx.send(GenMsg::Done(r));
        });
        self.gen = Some(Gen { rx, stop });
        self.auto_pending = true;
        self.page = Page::Story;
        self.focus = Focus::None;
        self.pane_open = false;
        self.stick_bottom = true;
        self.scroll_px = 0.0;
    }

    /// Correct the story on the page: the model gets the story as data with the requests so far and writes it again, keeping
    /// what the request does not touch; only the pictures whose scene changed are drawn again; the result is a new version.
    pub fn start_revision(&mut self, text: String, port: &dyn TalesPort) {
        if self.gen.is_some() {
            return;
        }
        if text.trim().is_empty() {
            self.note = Some(("Say what to change, for example \"make it shorter\" or \"change the fox's name to Beeru\".".into(), false));
            return;
        }
        if let Some(req) = st::parse_picture_request(&text) {
            return self.redraw_by_request(req, &text, port);
        }
        self.refresh_tts(port);
        let earlier = self.user_requests();
        let mut o = self.story_opts.clone();
        let h = st::parse_revision_hints(&text);
        if let Some((l, _)) = h.length {
            o.length = l;
        }
        if let Some((l, _)) = h.language {
            o.language = l;
        }
        if let Some((g, _)) = h.genre {
            o.genre = g;
        }
        if let Some((t, _)) = h.tone {
            o.tone = t;
        }
        let new_style = h.style.is_some();
        if let Some((s, _)) = h.style {
            o.style = s;
        } else if h.genre.is_some() && self.story_opts.style == st::auto_style(self.story_opts.audience, self.story_opts.genre) {
            o.style = st::auto_style(o.audience, o.genre);
        }
        o.pictures = self.resolve_pictures(self.opts.pictures, self.opts.drawing);
        let msgs = match st::revision_messages(&o, &self.story, &earlier, &text) {
            Ok(m) => m,
            Err(e) => {
                self.note = Some((e, false));
                return;
            }
        };
        self.reader.stop();
        self.pic_auto = self.opts.pictures == PictureMode::Auto && o.pictures == PictureMode::ImageModel && o.drawing != st::DrawingMode::Image;
        let rev = Rev { old_story: std::mem::take(&mut self.story), old_raw: std::mem::take(&mut self.raw), old_opts: std::mem::replace(&mut self.story_opts, o.clone()), old_pics: std::mem::take(&mut self.pics), new_style };
        self.rev = Some(rev);
        self.clear_view();
        self.pic_gen += 1;
        self.error = None;
        self.note = None;
        self.agent_note = None;
        self.convo.push(Message::user(text.clone()));
        let plan = self.step("Plan", &text);
        let plan_text = format!("Revise version {}: keep what the request does not touch. {}", self.ver, self.plan_text(&o, port));
        self.step_done(&plan, true, &plan_text);
        self.launch(port, msgs, "story.revise");
        self.idea.set("");
        self.sync_fields();
        self.dismissed = Default::default();
    }

    /// "Redraw picture 2 as a comic": only that picture is made again; the story text stays.
    pub fn redraw_by_request(&mut self, req: st::PictureRequest, text: &str, port: &dyn TalesPort) {
        let idx = req.index - 1;
        if self.pics.is_empty() {
            self.note = Some(("There are no pictures to redraw (they are switched off, see Options).".into(), false));
            return;
        }
        if idx >= self.pics.len() {
            self.note = Some((format!("There is no picture {}: this story has {}.", req.index, self.pics.len()), false));
            return;
        }
        self.convo.push(Message::user(text.to_string()));
        if let Some(style) = req.style {
            self.pics[idx].style = style;
        }
        let fb = st::picture_feedback(text);
        if !fb.is_empty() {
            self.pics[idx].feedback = fb;
        }
        self.pics[idx].state = PicState::Queued;
        self.push_draw_calls(&[idx]);
        self.idea.set("");
        self.sync_fields();
        self.note = None;
        self.pump_pictures(port);
        self.persist_chat();
    }

    /// The revision has been written (or stopped, or failed): keep it as the next version and match the pictures to it.
    pub fn finish_revision(&mut self, rev: Rev, stopped: bool, port: &dyn TalesPort) {
        let write = self.write_call.take();
        if self.story.blocks.is_empty() {
            // nothing came back: the story stays as it was
            let why = self.error.take().unwrap_or_else(|| "the model returned no story".into());
            self.story = rev.old_story;
            self.raw = rev.old_raw;
            self.story_opts = rev.old_opts;
            self.pics = rev.old_pics;
            self.chunks = st::chunks_for(&self.story.units(), 0, st::MAX_CHUNK_CHARS);
            self.chunked_units = self.story.units().len();
            self.note = Some((format!("The story was not changed: {why}"), false));
            if let Some(c) = write {
                self.step_done(&c, false, &format!("the story was not changed: {why}"));
            }
            return;
        }
        let before = self.pics.len();
        let _ = before;
        self.reconcile_pictures(rev.old_pics, rev.new_style, port);
        // a saved story keeps the earlier text as a version of its own; an unsaved one only moves on
        let mut saved_as: Option<String> = None;
        match self.saved.clone() {
            Some(e) => match self.store.save_revision(&e.id, &self.story, &self.story_opts) {
                Ok(n) => {
                    self.ver = n;
                    self.ver_count = n.max(self.ver_count);
                    if let Ok(d) = self.store.load(&e.id) {
                        self.saved = Some(d.entry);
                    }
                    saved_as = Some(format!("Saved as version {n} in Stories/{}/; version {} is kept.", e.id, n - 1));
                    port.audit("story.version", &format!("{} v{n}", e.id));
                }
                Err(err) => self.note = Some((format!("The new version could not be saved: {err}"), false)),
            },
            None => {
                self.ver += 1;
                self.ver_count = self.ver_count.max(self.ver);
            }
        }
        if self.saved.is_some() {
            self.persist_pictures();
        }
        let words = self.story.words();
        if let Some(c) = write {
            self.step_done(&c, true, &format!("Wrote \"{}\": {words} words{}. This is version {}.", self.display_title(), if stopped { " (stopped early)" } else { "" }, self.ver));
        }
        self.convo.push(Message::assistant(format!("Version {} is on the left{}. Keep telling me what to change; the earlier versions stay under \"Version\".", self.ver, if stopped { " (stopped where it was)" } else { "" })));
        if let Some(n) = saved_as {
            self.note = Some((format!("{}{n}", self.agent_note.take().map(|a| format!("{a} ")).unwrap_or_default()), true));
        }
    }

    /// Give the story the pictures that still fit (same scene text), draw the others, and say which in the conversation.
    pub fn reconcile_pictures(&mut self, old: Vec<Pic>, all_new: bool, port: &dyn TalesPort) {
        self.pics.clear();
        self.pic_gen += 1;
        self.held = false;
        self.alt_jobs.clear();
        self.alt_running = None;
        if self.story_opts.pictures == PictureMode::Off {
            return;
        }
        let o = self.story_opts.clone();
        let title = if self.story.title.is_empty() { st::title_from_topic(&o.topic) } else { self.story.title.clone() };
        let scenes = illus::pick_scenes(st::scenes(&self.story, st::MAX_PICTURES), o.coverage);
        let old_scenes: Vec<st::Scene> = old.iter().map(|p| if p.state == PicState::Ready { p.scene.clone() } else { st::Scene { text: String::new(), ..p.scene.clone() } }).collect();
        let reuse = if all_new { vec![None; scenes.len()] } else { st::reuse_pictures(&old_scenes, &scenes) };
        let mut kept = 0;
        let mut fresh = Vec::new();
        for (sc, r) in scenes.into_iter().zip(reuse) {
            match r {
                Some(j) => {
                    let mut p = old[j].clone();
                    p.scene = sc;
                    p.call = None;
                    kept += 1;
                    self.pics.push(p);
                }
                None => {
                    let prompt = match o.pictures {
                        PictureMode::ImageModel => st::image_prompt(&title, &o, &sc),
                        _ => format!("SVG drawing: {}", st::image_prompt(&title, &o, &sc)),
                    };
                    fresh.push(self.pics.len());
                    self.pics.push(Pic { scene: sc, state: PicState::Queued, bytes: None, ext: "svg".into(), prompt, mode: o.pictures, style: o.style, key: self.pic_gen, call: None, how: String::new(), log: String::new(), feedback: String::new(), size: String::new(), look: o.pic.styles_list()[0], seed: None, provider: String::new(), model: String::new(), ms: 0, alts: Vec::new(), links: Vec::new() });
                }
            }
        }
        if kept > 0 {
            let c = self.step("Keep", &format!("{kept} picture{}", if kept == 1 { "" } else { "s" }));
            self.step_done(&c, true, &format!("{kept} picture{} still fit{} the new text and {} kept; {} drawn again.", if kept == 1 { "" } else { "s" }, if kept == 1 { "s" } else { "" }, if kept == 1 { "is" } else { "are" }, fresh.len()));
        }
        self.queue_alts(&fresh);
        let job = if o.pictures == PictureMode::ImageModel { self.job_line(port, fresh.len()) } else { None };
        let (_, secs) = self.job_totals(port, fresh.len());
        let long = o.pictures == PictureMode::ImageModel && secs > self.confirm_secs && !fresh.is_empty();
        if (o.coverage == Coverage::Ask || long) && !fresh.is_empty() {
            // nothing is drawn until the person says yes: the text is complete and readable meanwhile
            self.held = true;
            let n = fresh.len();
            let total = job.unwrap_or_else(|| self.timing_line(port, n));
            self.convo.push(Message::assistant(format!("I can draw {n} picture{} for this story. {total} Press \"Make the pictures\" to start, or \"No pictures\" to cancel.", if n == 1 { "" } else { "s" })));
            return;
        }
        self.push_draw_calls(&fresh);
        self.pump_pictures(port);
    }

    /// Choose version `n` of the saved story again: its text comes back, and the pictures are matched to it.
    pub fn switch_version(&mut self, n: u32, port: &dyn TalesPort) {
        if self.gen.is_some() {
            self.note = Some(("Wait until the story is written, or stop it, then choose a version.".into(), false));
            return;
        }
        let Some(id) = self.saved_id() else {
            self.note = Some(("Save the story first: versions are kept with a saved story.".into(), false));
            return;
        };
        if n == self.ver {
            return;
        }
        let doc = match self.store.select_version(&id, n) {
            Ok(d) => d,
            Err(e) => {
                self.note = Some((e, false));
                return;
            }
        };
        self.reader.stop();
        let old = std::mem::take(&mut self.pics);
        self.clear_view();
        self.story = doc.story.clone();
        self.raw = doc.story.markdown();
        self.story_opts = self.options_of(&doc.entry);
        self.story_opts.pictures = self.resolve_pictures(self.opts.pictures, self.opts.drawing);
        self.story_opts.style = old.first().map_or_else(|| st::auto_style(self.story_opts.audience, self.story_opts.genre), |p| p.style);
        self.saved = Some(doc.entry.clone());
        self.ver = n;
        self.chunks = st::chunks_for(&self.story.units(), 0, st::MAX_CHUNK_CHARS);
        self.chunked_units = self.story.units().len();
        self.pic_auto = self.opts.pictures == PictureMode::Auto && self.story_opts.pictures == PictureMode::ImageModel && self.story_opts.drawing != st::DrawingMode::Image;
        self.reconcile_pictures(old, false, port);
        self.persist_pictures();
        self.convo.push(Message::assistant(format!("Back to version {n} of {}. Your later versions are still there.", self.ver_count)));
        self.persist_chat();
        self.menu = None;
        port.audit("story.version", &format!("{id} switched to v{n}"));
        self.note = Some((format!("Version {n} is in use again."), true));
    }

    /// The options a saved story was made with (from its header).
    pub fn options_of(&self, e: &StoryEntry) -> StoryOptions {
        StoryOptions {
            topic: e.topic.clone(),
            audience: Audience::from_id(&e.audience).unwrap_or(Audience::Kids),
            genre: Genre::from_id(&e.genre).unwrap_or(Genre::Bedtime),
            length: Length::from_id(&e.length).unwrap_or(Length::Short),
            language: if e.language.is_empty() { "English".into() } else { e.language.clone() },
            tone: Tone::from_id(&e.tone).unwrap_or(Tone::Calm),
            characters: e.characters.clone(),
            pictures: PictureMode::Off,
            style: PictureStyle::default(),
            drawing: st::DrawingMode::default(),
            video: false,
            coverage: self.opts.coverage,
            pic: self.store.load_picture_opts(&e.id).unwrap_or_else(|| illus::PictureOpts { seed: None, ..self.opts.pic.clone() }),
        }
    }

    /// Keep the conversation beside the saved story.
    pub fn persist_chat(&mut self) {
        if let Some(id) = self.saved_id() {
            let _ = self.store.save_chat(&id, &st::StoryChat { version: self.ver, messages: self.convo.clone() });
        }
    }

    /// A new story: the page, the pictures and the conversation start empty.
    pub fn new_story(&mut self, port: &dyn TalesPort) {
        if self.gen.is_some() {
            self.stop_writing();
        }
        self.queued = None;
        self.reader.stop();
        self.reset_story();
        self.ver = 1;
        self.ver_count = 1;
        self.open_form(port);
    }

    pub fn run_model_for_audit(&self, port: &dyn TalesPort) -> String {
        match self.model_plan(port) {
            ModelPlan::Chosen(c) => format!("{} {}", c.provider, c.model),
            ModelPlan::SameAsChat { .. } => "same as chat".to_string(),
        }
    }

    /// Empty the story view (the text, the reading, the layout) without touching the conversation or the saved story.
    pub fn clear_view(&mut self) {
        self.raw.clear();
        self.story = Story::default();
        self.chunks.clear();
        self.chunked_units = 0;
        self.live = false;
        self.saved_audio = None;
        self.auto_pending = false;
        self.tscroll = 0.0;
        self.manual_unit = None;
        self.pane.clear();
        self.lay.clear();
        self.pics.clear();
        self.sheet = None;
        self.setting = None;
        self.held = false;
        self.pic_gen += 1;
        if let Some(j) = self.audio_job.take() {
            j.cancel.store(true, Ordering::SeqCst);
        }
    }

    pub fn reset_story(&mut self) {
        self.run_model.clear();
        self.agent_note = None;
        self.saved = None;
        self.error = None;
        self.note = None;
        self.convo.clear();
        self.tool_ok.clear();
        self.write_call = None;
        self.rev = None;
        self.thinking.clear();
        self.scroll_px = 0.0;
        self.pic_pending.clear();
        self.pic_attempts.clear();
        self.clear_view();
    }

    pub fn stop_writing(&mut self) {
        if let Some(g) = &self.gen {
            g.stop.store(true, Ordering::SeqCst);
        }
    }

    /// Take what the model has sent so far.
    pub fn poll_writing(&mut self, port: &dyn TalesPort) {
        let mut finished: Option<Result<String, String>> = None;
        if let Some(g) = &self.gen {
            while let Ok(m) = g.rx.try_recv() {
                match m {
                    GenMsg::Delta(t) => self.raw.push_str(&t),
                    GenMsg::Snapshot(t) => self.raw = t,
                    GenMsg::Thinking(t) => self.thinking = t,
                    GenMsg::Who(w) => self.run_model = w,
                    GenMsg::Note(n) => {
                        self.agent_note = Some(n.clone());
                        self.note = Some((n, false));
                    }
                    GenMsg::Done(r) => {
                        finished = Some(r);
                        break;
                    }
                }
            }
        } else {
            return;
        }
        let stopped = self.gen.as_ref().is_some_and(|g| g.stop.load(Ordering::SeqCst));
        if let Some(r) = finished {
            self.gen = None;
            match r {
                Ok(full) => {
                    if !full.trim().is_empty() {
                        self.raw = full;
                    }
                }
                Err(e) if self.raw.trim().is_empty() => self.error = Some(e),
                Err(e) => self.error = Some(format!("The model stopped early: {e}")),
            }
            self.story = st::parse_story(&self.raw, true);
            if self.story.blocks.is_empty() && self.error.is_none() {
                self.error = Some("The model returned no story. Try again, or choose another model (Settings → Models).".into());
            }
            self.thinking.clear();
            self.think_since = None;
            self.feed_reader(true);
            match self.rev.take() {
                Some(rev) => self.finish_revision(rev, stopped, port),
                None => {
                    if !self.story.blocks.is_empty() {
                        self.start_pictures(port);
                        if self.prefs.auto_save {
                            self.save_story(port, if stopped { "Stopped early. " } else { "" });
                        } else {
                            self.note = Some((format!("{}{}The story is not saved yet. Press Save story to keep it.", self.agent_note.take().map(|n| format!("{n} ")).unwrap_or_default(), if stopped { "Stopped early. " } else { "" }), true));
                        }
                    }
                    if let Some(c) = self.write_call.take() {
                        if self.story.blocks.is_empty() {
                            let why = self.error.clone().unwrap_or_else(|| "no story came back".into());
                            self.step_done(&c, false, &why);
                        } else {
                            let t = format!("Wrote \"{}\": {} words{}.", self.display_title(), self.story.words(), if stopped { " (stopped early)" } else { "" });
                            self.step_done(&c, true, &t);
                            self.convo.push(Message::assistant("Version 1 is on the left. Tell me what to change: shorter, a different name, funnier, another language, or \"redraw picture 2 as a comic\"."));
                        }
                    }
                }
            }
            self.persist_chat();
            if let Some(q) = self.queued.take() {
                self.idea.set(&q);
                self.sync_fields();
                self.submit(port);
            }
        } else {
            self.story = st::parse_story(&self.raw, false);
            self.feed_reader(false);
        }
    }

    /// Give the reader the paragraphs that are finished; start it by itself when "read while writing" is on.
    pub fn feed_reader(&mut self, done: bool) {
        let units = self.story.units();
        if units.len() <= self.chunked_units && !(done && self.live) {
            return;
        }
        let fresh = st::chunks_for(&units, self.chunked_units, st::MAX_CHUNK_CHARS);
        self.chunked_units = units.len();
        if fresh.is_empty() && !done {
            return;
        }
        if self.live {
            if !fresh.is_empty() {
                self.chunks.extend(fresh.clone());
                self.reader.append(fresh);
            }
            if done {
                self.reader.close_input();
            }
        } else {
            self.chunks.extend(fresh);
        }
    }

    pub fn maybe_autostart(&mut self, port: &dyn TalesPort) {
        if !self.auto_pending {
            return;
        }
        if self.live || !self.prefs.read_while_writing || !matches!(self.tts, TtsStatus::Ready { .. }) {
            self.auto_pending = false;
        } else if !self.chunks.is_empty() {
            self.auto_pending = false;
            self.start_reading(0, port);
        } else if self.gen.is_none() {
            self.auto_pending = false;
        }
    }

    // ------------------------------------------------------------------------------------------------ saving

    /// Save the story into `Stories/<title>/` with what is ready of its pictures and its audio, and show it in Listen again.
    pub fn save_story(&mut self, port: &dyn TalesPort, prefix: &str) {
        if self.story.blocks.is_empty() {
            self.note = Some(("There is no story to save yet.".into(), false));
            return;
        }
        let entry = match self.saved.clone() {
            Some(e) => e,
            None => match self.store.save(&self.story, &self.story_opts, (self.clock)()) {
                Ok(e) => {
                    port.audit("story.save", &e.id);
                    // a story that was revised before it was saved starts its kept versions from this text
                    self.ver = 1;
                    self.ver_count = 1;
                    e
                }
                Err(e) => {
                    self.note = Some((format!("The story could not be saved: {e}"), false));
                    return;
                }
            },
        };
        self.saved = Some(entry.clone());
        let mut parts = vec!["text".to_string()];
        let n = self.persist_pictures();
        if n > 0 {
            parts.push(format!("{n} picture{}", if n == 1 { "" } else { "s" }));
        }
        if self.persist_audio_if_ready() {
            parts.push("audio".into());
        }
        if let Ok(d) = self.store.load(&entry.id) {
            self.saved = Some(d.entry);
        }
        let said = self.agent_note.take().map(|n| format!("{n} ")).unwrap_or_default();
        self.note = Some((format!("{said}{prefix}Saved in Stories/{}/ ({}).", entry.id, parts.join(", ")), true));
        self.persist_chat();
    }

    /// The other pictures of a scene as the store keeps them.
    pub fn stored_variants(p: &Pic) -> Vec<StoredVariant> {
        p.alts.iter().map(|a| StoredVariant { ext: a.ext.clone(), bytes: a.bytes.as_ref().clone(), style: a.look.id().into(), seed: a.seed, provider: a.provider.clone(), model: a.model.clone(), size: a.size.clone(), ms: a.ms, prompt: a.prompt.clone() }).collect()
    }

    /// Keep the pictures that are ready next to the saved story (their places are those of the saved text).
    pub fn persist_pictures(&mut self) -> usize {
        let Some(id) = self.saved_id() else { return 0 };
        // a story the model gave no title is saved with one, which shifts every unit by one
        let shift = usize::from(self.story.title.is_empty());
        let list: Vec<StoredPic> = self
            .pics
            .iter()
            .filter(|p| p.state == PicState::Ready)
            .filter_map(|p| {
                Some(StoredPic {
                    variants: Self::stored_variants(p),
                    look: p.look.id().into(),
                    seed: p.seed,
                    provider: p.provider.clone(),
                    model: p.model.clone(),
                    ms: p.ms,
                    size: p.size.clone(),
                    source: p.how.clone(),
                    scene_text: p.scene.text.clone(),
                    unit: p.scene.place_before + shift,
                    title: p.scene.title.clone(),
                    prompt: p.prompt.clone(),
                    style: p.style.id().into(),
                    mode: p.mode.id().into(),
                    ext: p.ext.clone(),
                    bytes: p.bytes.as_ref()?.as_ref().clone(),
                })
            })
            .collect();
        let mut log = String::new();
        for (i, p) in self.pics.iter().enumerate().filter(|(_, p)| !p.log.is_empty()) {
            log.push_str(&format!("######## Picture {} ({}): {}\nstyle {} · made as: {}\n{}\n", i + 1, p.scene.title, p.how, p.style.id(), p.ext, p.log));
        }
        self.flush_pic_log();
        let _ = self.store.save_picture_opts(&id, &self.story_opts.pic);
        let _ = self.store.save_notes(&id, self.sheet.as_ref().map(|c| c.to_json()).as_deref(), &log);
        if let Some(t) = &self.setting {
            let _ = self.store.save_setting(&id, t);
        }
        match self.store.save_pictures(&id, &list) {
            Ok(()) => list.len(),
            Err(e) => {
                self.note = Some((format!("The pictures could not be saved: {e}"), false));
                0
            }
        }
    }

    /// Save the audio when every piece of the story has already been spoken (nothing is made just for this).
    pub fn persist_audio_if_ready(&mut self) -> bool {
        let Some(id) = self.saved_id() else { return false };
        if !self.live || self.gen.is_some() {
            return false;
        }
        if self.browser_voice || self.direct.is_some() {
            return false; // the browser (or the engine) said it aloud; there is nothing to keep
        }
        let (chunks, wavs) = self.reader.sounds();
        if chunks.is_empty() || chunks.len() != self.chunks.len() || wavs.iter().any(|w| w.is_none()) {
            return false;
        }
        let all: Vec<Arc<Vec<u8>>> = wavs.into_iter().flatten().collect();
        match st::assemble_audio_timed(&chunks, &all).and_then(|(wav, starts)| self.store.save_audio_indexed(&id, &wav, &chunks, &starts)) {
            Ok(_) => true,
            Err(_) => false,
        }
    }

    // ------------------------------------------------------------------------------------------------ pictures

    /// Plan the illustrations of the finished story (one per scene) and start making them one after another in the
    /// background; reading never waits for them.
    pub fn start_pictures(&mut self, port: &dyn TalesPort) {
        self.reconcile_pictures(Vec::new(), true, port);
    }

}
