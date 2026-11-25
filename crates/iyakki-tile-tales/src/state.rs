//! TALES, the tile's state: building it, what it reports, the speech engine, the models (text, picture, agent) and their menus.
use super::*;

impl StoryApp {
    /// The tile over any port (a fake in tests).
    pub fn with_port(port: &dyn TalesPort) -> StoryApp {
        let store = port.story_store();
        let prefs = store.load_prefs();
        let mut opts = prefs.options.clone();
        opts.topic = String::new();
        normalize_options(&mut opts);
        let mut search = TextField::new("search your stories", false);
        search.max = 80;
        let (pic_tx, pic_rx) = channel();
        let (health_tx, health_rx) = channel();
        let mut app = StoryApp {
            reword: port.rewording(),
            page: Page::Story,
            focus: Focus::Idea,
            opts,
            idea: new_idea(),
            search,
            store,
            prefs,
            tts: TtsStatus::NoEngine,
            quick: Vec::new(),
            voices: Vec::new(),
            takes_speed: false,
            browser_voice: false,
            direct: None,
            no_audio_file: false,
            request: None,
            turning_on: false,
            image_models: Vec::new(),
            raw: String::new(),
            story: Story::default(),
            story_opts: StoryOptions::default(),
            saved: None,
            gen: None,
            error: None,
            note: None,
            pics: Vec::new(),
            sheet: None,
            pic_gen: 0,
            clock: port.clock(),
            pic_pending: Vec::new(),
            alt_jobs: std::collections::VecDeque::new(),
            alt_running: None,
            scene_seed: HashMap::new(),
            confirm_secs: illus::CONFIRM_SECS,
            gal_hits: Vec::new(),
            install: None,
            pic_started: HashMap::new(),
            pic_target: None,
            health_tx,
            health_rx,
            health_busy: false,
            health_last: None,
            server_state: String::new(),
            last_wait_log: None,
            wait_text: None,
            test_timing: None,
            pic_attempts: HashMap::new(),
            pic_tx,
            pic_rx,
            pic_cache: PicCache::default(),
            reader: {
                // the browser's "Stop it" dialog can name TALES while it reads and stop it cleanly (soundhold.rs)
                let r = port.new_reader();
                r.announce_holder();
                r
            },
            chunks: Vec::new(),
            chunked_units: 0,
            live: false,
            saved_audio: None,
            level: 0.0,
            audio_job: None,
            cscroll: 0.0,
            cmax: 0.0,
            tscroll: 0.0,
            tmax: 0.0,
            manual_unit: None,
            stick_bottom: true,
            pane: Pane::new(TEXT_PANE),
            highlight: Highlight::default(),
            text_area: R::new(0.0, 0.0, 0.0, 0.0),
            line_map: Vec::new(),
            click_read: None,
            pending_read: None,
            word_vis: None,
            word_rect: None,
            sentence_rects: Vec::new(),
            line_tops: Vec::new(),
            lay: Vec::new(),
            lay_key: (0, 0),
            anim: None,
            live_out: Vec::new(),
            tick_off: 0,
            pic_scaled: Vec::new(),
            pane_open: true,
            menu: None,
            menu_sel: 0,
            panel_hover: false,
            anchors: [R::new(0.0, 0.0, 0.0, 0.0); 7],
            explicit: [false; 4],
            dismissed: [None, None, None, None, None],
            pic_auto: false,
            agent_note: None,
            convo: Vec::new(),
            view: ChatView::new(),
            tool_ok: HashMap::new(),
            scroll_px: 0.0,
            max_off: 0.0,
            thinking: String::new(),
            think_since: None,
            step_seq: 0,
            ver: 1,
            ver_count: 1,
            queued: None,
            rev: None,
            write_call: None,
            panel_w: 380.0,
            drag_o: None,
            div_rect: R::new(0.0, 0.0, 0.0, 0.0),
            area_rect: R::new(0.0, 0.0, 0.0, 0.0),
            scale: 1.0,
            wide: true,
            lib: Vec::new(),
            lib_sel: 0,
            lib_top: 0,
            modal: None,
            host_save: None,
            ticks: 0,
            seen_word: None,
            polled_word: None,
            frame_due_at: (u64::MAX, false),
            dirty: true,
            bar_rect: None,
            pic_buttons: Vec::new(),
            held: false,
            setting: None,
            #[cfg(any(test, feature = "testing"))]
            test_memory: None,
            idea_geo: None,
            last_status: Status::Idle,
            auto_pending: false,
            run_model: String::new(),
            #[cfg(any(test, feature = "testing"))]
            test_backend: None,
            #[cfg(any(test, feature = "testing"))]
            test_drawers: None,
        };
        let d = StoryOptions::default();
        app.explicit = [app.opts.length != d.length, app.opts.language != d.language, app.opts.genre != d.genre, app.opts.tone != d.tone];
        app.refresh_tts(port);
        app
    }

    // ------------------------------------------------------------------------------------------------ small helpers

    pub fn page_name(&self) -> &'static str {
        match self.page {
            Page::Story => "story",
            Page::Library => "library",
        }
    }
    pub fn tts_status(&self) -> &TtsStatus {
        &self.tts
    }
    pub fn story_text(&self) -> String {
        self.story.plain()
    }
    pub fn is_writing(&self) -> bool {
        self.gen.is_some()
    }
    pub fn reader_status(&self) -> Status {
        self.reader.status()
    }
    pub fn error_text(&self) -> Option<&str> {
        self.error.as_deref()
    }
    pub fn note_text(&self) -> Option<&str> {
        self.note.as_ref().map(|n| n.0.as_str())
    }
    pub fn focus_name(&self) -> &'static str {
        match self.focus {
            Focus::None => "none",
            Focus::Idea => "idea",
            Focus::Search => "search",
        }
    }
    pub fn saved_id(&self) -> Option<String> {
        self.saved.as_ref().map(|e| e.id.clone())
    }
    pub fn library_ids(&self) -> Vec<String> {
        self.lib.iter().map(|e| e.id.clone()).collect()
    }
    pub fn options(&self) -> &StoryOptions {
        &self.opts
    }
    pub fn take_pending_save(&mut self) -> Option<(String, Vec<u8>)> {
        self.host_save.take()
    }

    pub fn refresh_tts(&mut self, port: &dyn TalesPort) {
        let v = port.voice_settings();
        let host = HostVoice::current();
        // the engines of this computer are looked for once, in the background, the first time somebody might read
        port.host_tts().ensure();
        let lang = hostvoice::lang_tag(&self.story_opts.language);
        let auto = if v.tts.is_none() { port.host_tts().auto_engine(Some(lang)) } else { None };
        let engine = st::effective_engine_auto(&v, &host, auto.as_ref());
        self.browser_voice = engine == Some(SpeechEngine::Browser);
        if self.browser_voice {
            // the window front end checks its engine only when somebody wants to read
            host.want_probe();
        }
        self.direct = engine.as_ref().filter(|e| port.speaks_directly(e)).and_then(|e| port.direct_voice(e));
        self.no_audio_file = self.browser_voice || engine.as_ref().is_some_and(|e| !port.can_save_audio(e));
        self.tts = st::tts_status_auto(&v, port.voice_allowed(), &path_env(), &host, auto.as_ref());
        self.voices = if self.browser_voice {
            self.browser_voices(&host)
        } else if matches!(engine, Some(SpeechEngine::Host { .. })) {
            self.host_voices(port, &engine, lang)
        } else {
            engine.as_ref().map(st::voice_choices).unwrap_or_default()
        };
        self.takes_speed = engine.as_ref().is_some_and(st::engine_takes_speed);
        self.image_models = port.image_models();
        self.quick = if matches!(self.tts, TtsStatus::NoEngine) && port.is_admin() {
            port.quick_engines(&path_env(), &voice_dirs(port))
        } else {
            Vec::new()
        };
    }

    /// "automatic" (the engine picks a voice for the story's language) and the voices of the engine of this computer that speak it,
    /// or all of them when none does.
    pub fn host_voices(&self, port: &dyn TalesPort, engine: &Option<SpeechEngine>, lang: &str) -> Vec<String> {
        let Some(SpeechEngine::Host { engine: id, .. }) = engine else { return Vec::new() };
        let Some(report) = port.host_tts().report() else { return vec![String::new()] };
        let Some(e) = hostengines::Kind::from_id(id).and_then(|k| report.get(k)) else { return vec![String::new()] };
        let mut mine: Vec<String> = e.voices.iter().filter(|v| v.fit(lang) > 0).map(|v| v.id.clone()).collect();
        if mine.is_empty() {
            mine = e.voices.iter().map(|v| v.id.clone()).collect();
        }
        mine.truncate(60);
        std::iter::once(String::new()).chain(mine).collect()
    }

    /// "automatic" (the browser picks a voice for the story's language) and the browser's voices for that language, or all of them
    /// when none speaks it.
    pub fn browser_voices(&self, host: &HostVoice) -> Vec<String> {
        let all = host.voices();
        let mut mine = hostvoice::voices_for(&all, hostvoice::lang_tag(&self.opts.language));
        if mine.is_empty() {
            mine = all;
        }
        std::iter::once(String::new()).chain(mine.into_iter().map(|v| v.name)).collect()
    }

    pub fn can_read(&self) -> bool {
        matches!(self.tts, TtsStatus::Ready { .. }) || self.saved_audio.is_some()
    }

    pub fn sync_fields(&mut self) {
        self.opts.topic = self.idea.text.clone();
    }

    pub fn set_focus(&mut self, f: Focus) {
        self.focus = f;
        // the idea box is only reachable with the panel in view
        if f == Focus::Idea && !self.wide {
            self.pane_open = true;
        }
    }

    pub fn voice_key(&self) -> String {
        if self.saved_audio.is_some() {
            return "saved".into();
        }
        format!("{}{}@{:.2}", if self.browser_voice { "browser:" } else if self.direct.is_some() { "direct:" } else { "" }, self.prefs.voice.clone().unwrap_or_default(), self.prefs.speed)
    }

    pub fn save_prefs(&mut self) {
        self.prefs.options = self.opts.clone();
        self.prefs.options.topic.clear();
        let _ = self.store.save_prefs(&self.prefs);
    }

    pub fn go(&mut self, p: Page, port: &dyn TalesPort) {
        self.page = p;
        self.modal = None;
        self.menu = None;
        self.focus = Focus::None;
        if p == Page::Library {
            self.reload_library();
        }
        self.refresh_tts(port);
    }

    /// Show the form and put the keyboard in the idea box.
    pub fn open_form(&mut self, port: &dyn TalesPort) {
        self.page = Page::Story;
        self.modal = None;
        self.pane_open = true;
        self.refresh_tts(port);
        self.set_focus(Focus::Idea);
    }

    // ------------------------------------------------------------------------------------------------ writing

    // ------------------------------------------------------------------------------------------------ the model

    pub fn model_plan(&self, port: &dyn TalesPort) -> ModelPlan {
        st::plan_model(self.prefs.model.as_ref(), &port.text_models(), "model")
    }

    /// The picture model to use, if one was chosen and is still there, and a note when it is not.
    pub fn picture_plan(&self, port: &dyn TalesPort) -> ModelPlan {
        let found = if self.image_models.is_empty() { port.image_models() } else { self.image_models.clone() };
        st::plan_model(self.prefs.picture_model.as_ref(), &found, "picture model")
    }

    /// How a model is named on the page: `qwen3 · home`.
    pub fn model_name(c: &ModelChoice) -> String {
        format!("{} · {}", c.model, c.provider)
    }

    /// What the Model chip says, and the note to show when a remembered choice could not be used.
    pub fn model_label(&self, port: &dyn TalesPort) -> (String, Option<String>) {
        let chat = port.chat_choice().map_or("no chat model yet".to_string(), |c| Self::model_name(&c));
        match self.model_plan(port) {
            ModelPlan::Chosen(c) => (Self::model_name(&c), None),
            ModelPlan::SameAsChat { note } => (format!("same as chat ({chat})"), note),
        }
    }

    pub fn picture_label(&self, port: &dyn TalesPort) -> (String, Option<String>) {
        match self.picture_plan(port) {
            ModelPlan::Chosen(c) => (Self::model_name(&c), None),
            ModelPlan::SameAsChat { note } => ("automatic".to_string(), note),
        }
    }

    /// The backend that writes: the chosen model, or the chat's.
    pub fn backend(&self, port: &dyn TalesPort) -> Arc<dyn ChatBackend> {
        match self.model_plan(port) {
            ModelPlan::Chosen(c) => {
                #[cfg(any(test, feature = "testing"))]
                if let Some(f) = &self.test_backend {
                    return f(&c);
                }
                port.chat_backend_for(c)
            }
            ModelPlan::SameAsChat { .. } => port.chat(),
        }
    }

    /// The one choice of who writes, from the remembered preferences: a chosen agent that is still set up wins, then a model of
    /// a provider that is still there, else the chat's model. (Older versions kept the model and the agent as two choices;
    /// both fields are still read, and the agent is what answered then too.)
    pub fn choice(&self, port: &dyn TalesPort) -> Choice {
        if let Some(id) = self.agent_id(port) {
            return Choice::Agent(id, self.prefs.agent_model.clone());
        }
        match self.model_plan(port) {
            ModelPlan::Chosen(c) => Choice::Provider(c),
            ModelPlan::SameAsChat { .. } => Choice::SameAsChat,
        }
    }

    /// The name of an agent's model as the chooser shows it: `Sonnet`, `Default`, or the id that was typed.
    pub fn agent_model_name(&self, port: &dyn TalesPort, id: &str, model: &str) -> String {
        if model.is_empty() {
            return "Default".into();
        }
        port.agent_model_label(id, model)
    }

    /// What the Model chip says: `Model: Sonnet · Claude Code`, `Model: qwen3:8b · nvidia_ollama` or `Model: same as chat`.
    pub fn chip_text(&self, port: &dyn TalesPort) -> String {
        let t = match self.choice(port) {
            Choice::SameAsChat => "same as chat".to_string(),
            Choice::Provider(c) => Self::model_name(&c),
            Choice::Agent(id, model) => format!("{} · {}", self.agent_model_name(port, &id, &model), self.agent_label(port).unwrap_or(id).replace(" (agent)", "")),
        };
        let t = if t.chars().count() > 36 { format!("{}…", t.chars().take(35).collect::<String>()) } else { t };
        format!("Model: {t}")
    }

    /// An agent as the chooser lists it (for the notes after choosing it).
    pub fn agent_row(&self, port: &dyn TalesPort, id: &str) -> Option<AgentRow> {
        let e = port.agent_entry(id)?;
        let advisor = port.agent_advisor(id);
        Some(AgentRow { label: e.label.replace(" (agent)", ""), chip: e.chip, ready: e.ready, advisor })
    }

    /// Open the model chooser (Ctrl+P, Ctrl+G or the chip): the chat's list with "Same as chat" first, and the agents. With
    /// `picture`, the picture models only. Anyone may choose here: it changes only this tile's own preference.
    pub fn open_model_pick(&mut self, picture: bool, port: &dyn TalesPort) {
        if self.gen.is_some() {
            self.note = Some(("Wait until the story is written, then choose the model.".into(), false));
            return;
        }
        let plain = if picture { if self.image_models.is_empty() { port.image_models() } else { self.image_models.clone() } } else { port.text_models() };
        let (top, current, title, hint) = if picture {
            let cur = self.prefs.picture_model.clone();
            (
                TopRow { label: "Automatic".into(), note: "the first picture model found".into() },
                Current { same: cur.is_none(), plain: cur.map(|c| (c.provider, c.model)), agent: None },
                "Choose the picture model",
                "Enter or click chooses · Esc closes · only for pictures made by a picture model.",
            )
        } else {
            let chat = port.chat_choice().map_or("no chat model yet".to_string(), |c| Self::model_name(&c));
            let current = match self.choice(port) {
                Choice::SameAsChat => Current { same: true, ..Default::default() },
                Choice::Provider(c) => Current { plain: Some((c.provider, c.model)), ..Default::default() },
                Choice::Agent(id, model) => Current { agent: Some((id, model)), ..Default::default() },
            };
            (
                TopRow { label: "Same as chat".into(), note: chat },
                current,
                "Choose the model",
                "Enter or click chooses · Esc closes · only for TALES; the chat keeps its own model · Settings → Models adds providers, Settings → Agents sets up Claude Code and others.",
            )
        };
        let list = port.model_list(plain, Some(top), !picture, current, title, hint);
        // the first line is always there; with nothing after it there is nothing to choose
        let Some(list) = list.filter(|l| l.items.len() > 1) else {
            self.note = Some((if picture { "No picture model is available. Settings → Models: connect a server with a model such as FLUX or Stable Diffusion." } else { "No models yet. An administrator can open Settings → Models, add a provider and press t to fetch its model list." }.into(), false));
            return;
        };
        self.focus = Focus::None;
        self.note = None;
        self.menu = None;
        self.modal = Some(Modal::Model(TalesPick { list, picture }));
    }

    /// Make the picked line the choice and remember it.
    pub fn choose_model(&mut self, pick: Pick, picture: bool, port: &dyn TalesPort) {
        match pick {
            Pick::Top | Pick::Provider(..) => {
                let choice = match pick {
                    Pick::Provider(p, m) => Some(ModelChoice { provider: p, model: m }),
                    _ => None,
                };
                if picture {
                    self.prefs.picture_model = choice.clone();
                } else {
                    // a provider's model (or "same as chat") replaces an agent: one choice
                    self.prefs.model = choice.clone();
                    if self.prefs.agent.take().is_some() {
                        port.audit("story.agent", "own model");
                    }
                    self.prefs.agent_model = String::new();
                }
                let _ = self.store.save_prefs(&self.prefs);
                port.audit(if picture { "story.picture_model" } else { "story.model" }, &choice.map_or("same as chat".to_string(), |c| format!("{} {}", c.provider, c.model)));
                self.set_focus(Focus::Idea);
            }
            // pictures are never made by an agent
            Pick::Agent(_) | Pick::AgentModel { .. } if picture => {}
            Pick::Agent(id) => {
                let keep = if self.prefs.agent.as_deref() == Some(id.as_str()) { self.prefs.agent_model.clone() } else { String::new() };
                self.choose_agent(&id, keep, false, port);
            }
            Pick::AgentModel { agent, model, custom } => self.choose_agent(&agent, model, custom, port),
        }
    }

    /// An agent (and its model; empty = its own default) now writes the stories. An agent that is not ready is still chosen: the
    /// exact fix is said, and our own model writes until it is ready.
    pub fn choose_agent(&mut self, id: &str, model: String, custom: bool, port: &dyn TalesPort) {
        let Some(row) = self.agent_row(port, id) else { return };
        if !port.may_run_commands() {
            self.note = Some(("Your account may not run commands, so external agents are off for it.".into(), false));
            return;
        }
        if custom {
            port.model_remember_custom(id, &model);
        }
        self.prefs.agent = Some(id.to_string());
        self.prefs.agent_model = model;
        let _ = self.store.save_prefs(&self.prefs);
        port.audit("story.agent", id);
        port.audit("story.agent_model", &format!("{id} {}", self.prefs.agent_model));
        self.note = if !row.ready {
            Some((format!("{} is {}: the story is written with our own model until it is ready (Settings → Agents).", row.label, row.chip), false))
        } else if row.advisor {
            Some((format!("{} is an advisor here (read-only): it can only write the story as text, which is all a story needs. It cannot save files; TALES saves the story.", row.label), true))
        } else {
            None
        };
        self.set_focus(Focus::Idea);
    }

    /// A key, click or paste in the chooser.
    pub fn model_event(&mut self, ev: &Event, port: &dyn TalesPort) {
        let Some(Modal::Model(m)) = self.modal.as_mut() else { return };
        let picture = m.picture;
        match model_picker::key(&mut m.list, ev) {
            model_picker::Out::Stay => {}
            model_picker::Out::Close => {
                self.modal = None;
                self.set_focus(Focus::Idea);
            }
            model_picker::Out::Bad(e) => self.note = Some((e, false)),
            model_picker::Out::Pick(p) => {
                self.modal = None;
                self.choose_model(p, picture, port);
            }
        }
    }

    // ------------------------------------------------------------------------------------------------ variants and the gallery

}
