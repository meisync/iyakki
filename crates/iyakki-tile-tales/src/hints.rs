//! TALES, hints, the open menus and the Drop of the tile.

use super::*;

impl StoryApp {
    /// The options with what the idea text says filled in, except what the person chose (that always wins) or removed.
    pub fn with_hints(&self, base: &StoryOptions) -> StoryOptions {
        let mut o = base.clone();
        let h: Hints = st::parse_hints(&base.topic);
        let ok = |k: usize, said: &str| !self.explicit[k] && self.dismissed[k].as_deref() != Some(said);
        if let Some((l, said)) = h.length {
            if ok(H_LEN, &said) {
                o.length = l;
            }
        }
        if let Some((l, said)) = h.language {
            if ok(H_LANG, &said) {
                o.language = l;
            }
        }
        if let Some((g, said)) = h.genre {
            if ok(H_GENRE, &said) {
                o.genre = g;
            }
        }
        if let Some((t, said)) = h.tone {
            if ok(H_TONE, &said) {
                o.tone = t;
            }
        }
        o
    }

    /// The options as they stand now, with the text's suggestions (what Send would use, pictures still as chosen).
    pub fn shown_options(&self) -> StoryOptions {
        let mut b = self.opts.clone();
        b.topic = self.idea.text.clone();
        self.with_hints(&b)
    }

    /// The suggestions in the idea text that have not been removed, for the chips under the composer.
    pub fn hint_chips(&self) -> Vec<HintChip> {
        let h = st::parse_hints(&self.idea.text);
        let mut v = Vec::new();
        let mut add = |k: usize, name: &'static str, value: String, said: String, chosen: String| {
            if self.dismissed[k].as_deref() != Some(said.as_str()) {
                v.push(HintChip { k, name, value, said, applied: self.explicit.get(k).map_or(true, |e| !e), chosen });
            }
        };
        if let Some((l, said)) = h.length {
            add(H_LEN, "Length", l.label().to_string(), said, self.opts.length.label().to_string());
        }
        if let Some((l, said)) = h.language {
            add(H_LANG, "Language", l, said, self.opts.language.clone());
        }
        if let Some((g, said)) = h.genre {
            add(H_GENRE, "Kind", g.label().to_string(), said, self.opts.genre.label().to_string());
        }
        if let Some((t, said)) = h.tone {
            add(H_TONE, "Tone", t.label().to_string(), said, self.opts.tone.label().to_string());
        }
        if let Some((st, said)) = h.style {
            add(H_STYLE, "Picture style", st.label().to_string(), said, String::new());
        }
        v
    }

    // ---- the agent

    /// The agent that will write, if one is chosen and still configured.
    pub fn agent_id(&self, port: &dyn TalesPort) -> Option<String> {
        self.prefs.agent.clone().filter(|id| port.agent_known(id))
    }

    pub fn agent_label(&self, port: &dyn TalesPort) -> Option<String> {
        let id = self.agent_id(port)?;
        port.agent_label(&id)
    }

    /// The run of an external agent for these messages: `Ok(None)` when our own model writes; `Err` says why a chosen agent
    /// cannot be used (the story is then written by our own model). The request denies every tool and works in an empty folder.
    pub fn agent_request(&self, port: &dyn TalesPort, msgs: &[iyakki_chat_model::Message]) -> Result<Option<AgentRun>, String> {
        let Some(id) = self.prefs.agent.clone() else { return Ok(None) };
        let Some(label) = port.agent_label(&id) else { return Err("The agent you chose is not set up any more (Settings → Agents).".into()) };
        if !port.may_run_commands() {
            return Err(format!("{label} is off for your account (it may not run commands)."));
        }
        port.agent_check(&id).map_err(|fix| format!("{label} is not ready: {fix}"))?;
        let advisor = port.agent_advisor(&id);
        let key_ref = port.agent_key_ref(&id);
        let key = if key_ref.is_empty() { None } else { Some(Arc::new(port.resolve_secret(&key_ref).map_err(|e| format!("The API key `{key_ref}` of {label} could not be read from the vault: {e}."))?)) };
        let workspace = port.data_dir().join("tales-agent");
        std::fs::create_dir_all(&workspace).map_err(|e| format!("{label} has no folder to work in: {e}."))?;
        let (system, prompt) = (msgs.first().map(|m| m.content.clone()).unwrap_or_default(), msgs.get(1).map(|m| m.content.clone()).unwrap_or_default());
        let (audit, usage) = (port.audit_fn(), port.usage_fn());
        let model = self.prefs.agent_model.clone();
        let shown = if model.is_empty() { label.clone() } else { format!("{label} · {model}") };
        let req = AgentRunReq { agent: id.clone(), workspace, prompt, system, model, key, audit, on_usage: usage, exe: std::env::current_exe().unwrap_or_default() };
        Ok(Some((req, label, shown, advisor)))
    }

    // ---- menus

    pub fn close_menu(&mut self) {
        if self.menu == Some(Menu::Language) {
            self.menu = Some(Menu::Options);
            return;
        }
        self.menu = None;
        if self.page == Page::Story && self.modal.is_none() {
            self.set_focus(Focus::Idea);
        }
    }

    /// Open one of the composer's menus, or close it when it is the one open.
    pub fn toggle_menu(&mut self, m: Menu, port: &dyn TalesPort) {
        if self.menu == Some(m) {
            self.close_menu();
            return;
        }
        match m {
            Menu::Kind | Menu::Tone => {
                let o = self.shown_options();
                self.menu_sel = if m == Menu::Kind { Genre::ALL.iter().position(|g| *g == o.genre) } else { Tone::ALL.iter().position(|t| *t == o.tone) }.unwrap_or(0);
                self.menu = Some(m);
            }
            Menu::Options => {
                self.refresh_tts(port);
                self.cscroll = 0.0;
                self.menu = Some(m);
            }
            Menu::Language => {
                self.menu_sel = LANGUAGES.iter().position(|l| *l == self.shown_options().language).unwrap_or(0);
                self.menu = Some(m);
            }
            Menu::Version => {
                if self.ver_count < 2 {
                    return;
                }
                self.menu_sel = (self.ver as usize).saturating_sub(1);
                self.menu = Some(m);
            }
            Menu::How => {
                self.refresh_tts(port);
                self.menu = Some(m);
            }
        }
        if self.menu.is_some() && !matches!(self.menu, Some(Menu::Options)) && self.focus != Focus::None {
            self.focus = Focus::None;
        }
    }

    /// What the Pictures chip near the composer says: what will really be done.
    pub fn pictures_status(&self, port: &dyn TalesPort) -> String {
        let picture_name = || match self.picture_plan(port) {
            ModelPlan::Chosen(c) => c.model,
            ModelPlan::SameAsChat { .. } => self.image_models.first().map_or("the picture model".to_string(), |m| m.1.clone()),
        };
        match self.resolve_pictures(self.opts.pictures, self.opts.drawing) {
            PictureMode::Off => "Pictures: off".into(),
            PictureMode::ImageModel if self.image_models.is_empty() && self.prefs.picture_model.is_none() => "Pictures: images (no picture model set up)".into(),
            PictureMode::ImageModel => format!("Pictures: images via {}", self.resolved_for(&self.pic_now(), port).map_or_else(picture_name, |r| r.model)),
            _ => {
                let (rich, who) = self.draws_rich(port);
                if rich { format!("Pictures: rich SVG by {who}") } else { "Pictures: cartoon (built-in)".into() }
            }
        }
    }

    /// What Auto pictures really are: images when a picture model exists, except when the person chose Drawing: Rich SVG or
    /// Built-in cartoon, which are drawings by definition (found by the live run: the choice was ignored while a picture model existed).
    pub fn resolve_pictures(&self, pictures: PictureMode, drawing: st::DrawingMode) -> PictureMode {
        if pictures == PictureMode::Auto && matches!(drawing, st::DrawingMode::Rich | st::DrawingMode::Cartoon) {
            return PictureMode::Drawings;
        }
        pictures.resolve(!self.image_models.is_empty())
    }

    /// Whether drawings are made as rich SVG (a strong writer, or the person said so) and by whom; else the built-in cartoon.
    pub fn draws_rich(&self, port: &dyn TalesPort) -> (bool, String) {
        let agent = self.agent_label(port);
        let model = match self.model_plan(port) {
            ModelPlan::Chosen(c) => c.model,
            ModelPlan::SameAsChat { .. } => port.chat_choice().map_or(String::new(), |c| c.model),
        };
        let who = agent.clone().unwrap_or(model.clone());
        let rich = match self.opts.drawing {
            st::DrawingMode::Rich => true,
            st::DrawingMode::Cartoon => false,
            st::DrawingMode::Auto | st::DrawingMode::Image => scene::rich::is_strong(&model, agent.is_some()),
        };
        (rich, who)
    }

    /// The line above the controls: who writes.
    pub fn writer_line(&self, port: &dyn TalesPort) -> String {
        match self.agent_label(port) {
            Some(l) => {
                let m = if self.prefs.agent_model.is_empty() { "default model".to_string() } else { self.prefs.agent_model.clone() };
                let mode = self.agent_id(port).map_or("", |id| if port.agent_advisor(&id) { " · advisor, text only" } else { " · no tools" });
                format!("agent: {l} · {m}{mode}")
            }
            None => format!("model: {}", self.model_label(port).0),
        }
    }
}

impl Drop for StoryApp {
    fn drop(&mut self) {
        // leaving the tile ends the reading and the writing: nothing keeps running unseen
        self.reader.stop();
        if let Some(g) = &self.gen {
            g.stop.store(true, Ordering::SeqCst);
        }
        if let Some(j) = &self.audio_job {
            j.cancel.store(true, Ordering::SeqCst);
        }
        self.pic_gen += 1;
    }
}
