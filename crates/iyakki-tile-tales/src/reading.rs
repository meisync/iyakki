//! TALES, Reading aloud and the library: the player, speed and voice, saving audio, saved stories, export.
use super::*;

impl StoryApp {
    pub fn start_reading(&mut self, from: usize, port: &dyn TalesPort) {
        if !self.can_read() {
            self.note = Some((st::status_text(&self.tts), false));
            return;
        }
        if self.chunks.is_empty() {
            self.chunks = st::chunks_for(&self.story.units(), 0, st::MAX_CHUNK_CHARS);
            self.chunked_units = self.story.units().len();
        }
        if self.chunks.is_empty() {
            return;
        }
        port.audit("story.read", &format!("{} parts", self.chunks.len()));
        self.highlight = port.voice_settings().highlight;
        self.reader.set_words(self.highlight.word());
        self.reader.set_speed(self.prefs.speed);
        let sink: Arc<dyn st::Sink> = if self.browser_voice && self.saved_audio.is_none() {
            browser_sink(HostVoice::current(), self.prefs.voice.clone().filter(|v| !v.is_empty()), Some(hostvoice::lang_tag(&self.story_opts.language).to_string()), self.prefs.speed)
        } else if let (Some(d), true) = (self.direct.clone(), self.saved_audio.is_none()) {
            d.sink(self.prefs.voice.clone().filter(|v| !v.is_empty()).as_deref(), Some(hostvoice::lang_tag(&self.story_opts.language)), self.prefs.speed)
        } else {
            let play_cmd = port.voice_settings().play_cmd;
            port.player_sink(&play_cmd, &path_env())
        };
        self.reader.start(self.chunks.clone(), from, self.gen.is_some(), &self.voice_key(), self.synth(port), sink);
        self.live = true;
        self.manual_unit = None;
        self.note = None;
    }

    /// A click in the text at (`xr`, `yr`) relative to the text area: read from the sentence under it.
    pub fn read_from_point(&mut self, xr: f32, yr: f32, port: &dyn TalesPort) {
        let Some((line, ch)) = self.pane.locate(xr, yr) else { return };
        let Some(&(unit, start)) = self.line_map.get(line) else { return };
        self.read_from(unit, start + ch, port);
    }

    /// Start reading at the sentence that holds character `pos` of unit `unit`. A piece holds several sentences, so the piece
    /// is cut in two at that sentence (not with saved audio, whose sound is made per piece).
    pub fn read_from(&mut self, unit: usize, pos: usize, port: &dyn TalesPort) {
        if !self.can_read() {
            self.note = Some((st::status_text(&self.tts), false));
            return;
        }
        if self.chunks.is_empty() {
            self.chunks = st::chunks_for(&self.story.units(), 0, st::MAX_CHUNK_CHARS);
            self.chunked_units = self.story.units().len();
        }
        let Some(mut idx) = self.chunks.iter().position(|c| c.unit == unit && c.start <= pos && pos < c.end).or_else(|| self.chunks.iter().position(|c| c.unit == unit && c.start >= pos)).or_else(|| self.chunks.iter().rposition(|c| c.unit == unit)) else { return };
        if self.saved_audio.is_none() {
            let units = self.story.units();
            if let Some(text) = units.get(unit) {
                let chars: Vec<char> = text.chars().collect();
                let c = self.chunks[idx].clone();
                let sentences = st::sentence_ranges(text);
                if let Some(k) = sentences.iter().position(|(a, b)| *a <= pos && pos < *b).or_else(|| sentences.iter().position(|(a, _)| *a >= pos)) {
                    let (sa, _) = sentences[k];
                    if sa > c.start && sa < c.end && k > 0 {
                        let before_end = sentences[k - 1].1.min(sa);
                        let cut = |a: usize, b: usize| Chunk { unit, start: a, end: b, text: chars[a..b.min(chars.len())].iter().collect() };
                        let (left, right) = (cut(c.start, before_end), cut(sa, c.end.min(chars.len())));
                        if !left.text.trim().is_empty() && !right.text.trim().is_empty() {
                            self.chunks.splice(idx..=idx, [left, right]);
                            idx += 1;
                        }
                    }
                }
            }
        }
        self.start_reading(idx, port);
    }

    pub fn play_pause(&mut self, port: &dyn TalesPort) {
        match self.reader.status() {
            Status::Playing | Status::Buffering => self.reader.pause(),
            Status::Paused => self.reader.resume(),
            _ => {
                if self.live && self.reader.snapshot().chunks > 0 && !matches!(self.reader.status(), Status::Done | Status::Failed(_)) {
                    self.reader.seek(0);
                } else {
                    self.start_reading(0, port);
                }
            }
        }
    }

    pub fn seek(&mut self, chunk: usize, port: &dyn TalesPort) {
        if self.chunks.is_empty() {
            return;
        }
        let chunk = chunk.min(self.chunks.len() - 1);
        if self.live {
            self.reader.seek(chunk);
            self.manual_unit = None;
        } else {
            self.start_reading(chunk, port);
        }
    }

    pub fn step_unit(&mut self, d: i32, port: &dyn TalesPort) {
        if self.chunks.is_empty() {
            return;
        }
        let cur = self.reader.snapshot().chunk.min(self.chunks.len() - 1);
        let unit = self.chunks[cur].unit;
        let first_of = |u: usize| self.chunks.iter().position(|c| c.unit == u);
        let target = if d > 0 {
            self.chunks.iter().position(|c| c.unit > unit)
        } else if first_of(unit) != Some(cur) {
            first_of(unit)
        } else {
            self.chunks[..cur].last().and_then(|c| first_of(c.unit))
        };
        if let Some(t) = target {
            self.seek(t, port);
        } else if d < 0 {
            self.seek(0, port);
        }
    }

    pub fn stop_reading(&mut self) {
        self.reader.stop();
    }

    pub fn cycle_voice(&mut self, port: &dyn TalesPort) {
        if self.voices.len() < 2 {
            return;
        }
        let cur = self.prefs.voice.clone().unwrap_or_else(|| self.voices[0].clone());
        let i = self.voices.iter().position(|v| *v == cur).unwrap_or(0);
        self.prefs.voice = Some(self.voices[(i + 1) % self.voices.len()].clone());
        self.after_voice_change(port);
    }

    pub fn cycle_speed(&mut self, up: bool, port: &dyn TalesPort) {
        let i = SPEEDS.iter().position(|v| (*v - self.prefs.speed).abs() < 0.01).unwrap_or(2);
        let n = SPEEDS.len();
        self.prefs.speed = SPEEDS[if up { (i + 1) % n } else { (i + n - 1) % n }];
        self.after_voice_change(port);
    }

    pub fn after_voice_change(&mut self, port: &dyn TalesPort) {
        // the saved sound has one voice: a different choice means the engine speaks again
        self.saved_audio = None;
        self.save_prefs();
        if self.live && matches!(self.reader.status(), Status::Playing | Status::Buffering | Status::Paused) {
            let cur = self.reader.snapshot().chunk;
            self.start_reading(cur, port);
        } else {
            self.live = false;
        }
    }

    pub fn save_audio(&mut self, port: &dyn TalesPort) {
        if self.gen.is_some() {
            self.note = Some(("Wait until the story is finished.".into(), false));
            return;
        }
        if self.no_audio_file && self.saved_audio.is_none() {
            self.note = Some((if self.browser_voice { SAVE_AUDIO_BROWSER } else { SAVE_AUDIO_PLAYONLY }.into(), false));
            return;
        }
        if !matches!(self.tts, TtsStatus::Ready { .. }) {
            self.note = Some((st::status_text(&self.tts), false));
            return;
        }
        if self.saved.is_none() {
            self.save_story(port, "");
        }
        let Some(e) = self.saved.clone() else { return };
        if self.audio_job.is_some() {
            return;
        }
        if self.chunks.is_empty() {
            self.chunks = st::chunks_for(&self.story.units(), 0, st::MAX_CHUNK_CHARS);
        }
        let (chunks, have) = {
            let (c, w) = self.reader.sounds();
            if self.live && self.direct.is_none() && c.len() == self.chunks.len() && c.iter().zip(&self.chunks).all(|(a, b)| a.text == b.text) { (c, w) } else { (self.chunks.clone(), vec![None; self.chunks.len()]) }
        };
        let (tx, rx) = channel();
        let (cancel, done) = (Arc::new(AtomicBool::new(false)), Arc::new(AtomicUsize::new(0)));
        let (c2, d2) = (cancel.clone(), done.clone());
        let synth = if self.saved_audio.is_none() { self.file_synth(port) } else { self.synth(port) };
        let store = self.store.clone();
        let id = e.id.clone();
        let total = chunks.len();
        port.audit("story.audio", &format!("{total} parts"));
        std::thread::spawn(move || {
            let r = st::render_audio(&chunks, &have, synth.as_ref(), &c2, &d2).and_then(|(wav, starts)| store.save_audio_indexed(&id, &wav, &chunks, &starts).map(|p| p.display().to_string()));
            let _ = tx.send(r);
        });
        self.audio_job = Some(AudioJob { rx, cancel, done, total });
        self.note = None;
    }

    // ------------------------------------------------------------------------------------------------ the library

    pub fn reload_library(&mut self) {
        self.lib = if self.search.value.trim().is_empty() { self.store.list() } else { self.store.search(&self.search.value) };
        self.lib_sel = self.lib_sel.min(self.lib.len().saturating_sub(1));
    }

    pub fn open_saved(&mut self, id: &str, listen: bool, port: &dyn TalesPort) {
        let doc = match self.store.load(id) {
            Ok(d) => d,
            Err(e) => {
                self.note = Some((e, false));
                return;
            }
        };
        self.stop_writing();
        self.gen = None;
        self.reader.stop();
        self.reset_story();
        self.story = doc.story.clone();
        self.raw = doc.story.markdown();
        self.story_opts = self.options_of(&doc.entry);
        self.saved = Some(doc.entry.clone());
        // the conversation that was kept with it, so it can go on (or one made up from the idea for stories saved without)
        self.ver_count = self.store.versions(id).max(1);
        match self.store.load_chat(id) {
            Some(c) if !c.messages.is_empty() => {
                self.convo = c.messages;
                self.ver = c.version.clamp_safe(1, self.ver_count);
            }
            _ => {
                self.ver = self.ver_count;
                self.convo = vec![Message::user(doc.entry.topic.clone()), Message::assistant(format!("Opened from Listen again (version {}). Tell me what to change.", self.ver))];
            }
        }
        self.step_seq = self.convo.len() + 1;
        self.tool_ok = self.convo.iter().filter(|m| m.role == iyakki_chat_model::Role::Tool).filter_map(|m| m.tool_call_id.clone().map(|id| (id, !m.content.starts_with("Failed:")))).collect();
        // the pictures that were kept, in their places
        self.sheet = self.store.load_character(id);
        self.setting = self.store.load_setting(id);
        for p in self.store.load_pictures(id) {
            let mode = PictureMode::from_id(&p.mode).unwrap_or(PictureMode::Drawings);
            let style = PictureStyle::from_id(&p.style).unwrap_or_default();
            let key = hash(&format!("{id}{}{}", p.title, p.bytes.len()));
            let alts: Vec<Alt> = p.variants.iter().map(|v| Alt { look: illus::Look::from_id(&v.style).unwrap_or_default(), seed: v.seed, provider: v.provider.clone(), model: v.model.clone(), size: v.size.clone(), ms: v.ms, ext: v.ext.clone(), key: hash(&format!("{id}{}{}{}", p.title, v.style, v.bytes.len())), bytes: Arc::new(v.bytes.clone()), prompt: v.prompt.clone() }).collect();
            self.pics.push(Pic { scene: st::Scene { place_before: p.unit, title: p.title, text: p.scene_text }, state: PicState::Ready, bytes: Some(Arc::new(p.bytes)), ext: p.ext, prompt: p.prompt, mode, style, key, call: None, how: String::new(), log: String::new(), feedback: String::new(), size: p.size, look: illus::Look::from_id(&p.look).unwrap_or_default(), seed: p.seed, provider: p.provider.clone(), model: p.model.clone(), ms: p.ms, alts, links: Vec::new() });
        }
        self.story_opts.style = self.pics.first().map_or_else(|| st::auto_style(self.story_opts.audience, self.story_opts.genre), |p| p.style);
        self.chunks = st::chunks_for(&self.story.units(), 0, st::MAX_CHUNK_CHARS);
        self.chunked_units = self.story.units().len();
        self.saved_audio = if doc.entry.has_audio {
            self.store.load_audio_pieces(id).ok().filter(|p| p.len() == self.chunks.len() && p.iter().zip(&self.chunks).all(|(a, b)| a.0.text == b.text)).map(|p| Arc::new(st::PrebuiltSynth::new(&p)))
        } else {
            None
        };
        self.page = Page::Story;
        self.focus = Focus::None;
        self.pane_open = false;
        self.refresh_tts(port);
        if listen {
            self.start_reading(0, port);
        }
    }

    pub fn toggle_fav(&mut self, id: &str) {
        let now = self.store.load(id).map(|d| d.entry.favourite).unwrap_or(false);
        match self.store.set_favourite(id, !now) {
            Ok(()) => {
                if let Some(s) = self.saved.as_mut().filter(|s| s.id == id) {
                    s.favourite = !now;
                }
                self.reload_library();
                self.note = Some((if now { "Removed from favourites.".into() } else { "Added to favourites.".into() }, true));
            }
            Err(e) => self.note = Some((e, false)),
        }
    }

    pub fn delete_saved(&mut self, id: &str, port: &dyn TalesPort) {
        match self.store.delete(id) {
            Ok(()) => {
                port.audit("story.delete", id);
                if self.saved.as_ref().is_some_and(|s| s.id == id) {
                    self.saved = None;
                    self.saved_audio = None;
                }
                self.reload_library();
                self.note = Some(("Story deleted.".into(), true));
            }
            Err(e) => self.note = Some((e, false)),
        }
    }

    /// Ask what to export (the page with its pictures, the text, the audio) and hand the choice to the shell's save dialog.
    pub fn ask_export(&mut self, id: Option<String>) {
        if id.is_none() && self.story.blocks.is_empty() {
            self.note = Some(("There is no story to export yet.".into(), false));
            return;
        }
        let mut items = vec!["Page with pictures (.html)".to_string(), "Text (.md)".to_string(), "Audio (.wav)".to_string()];
        let has_alts = match &id {
            Some(i) => self.store.load_pictures(i).iter().any(|p| !p.variants.is_empty()),
            None => self.pics.iter().any(|p| !p.alts.is_empty()),
        };
        if has_alts {
            items.push("Page with every picture variant (.html)".to_string());
        }
        self.modal = Some(Modal::Export(Picker::new("Export to the computer", items), id));
    }

    /// What the export of `id` (or of the story on the page) needs: the story, its options and its pictures.
    pub fn export_source(&self, id: Option<&str>) -> Result<(Story, StoryOptions, Vec<StoredPic>, Option<String>), String> {
        match id {
            Some(id) => {
                let d = self.store.load(id)?;
                let o = StoryOptions {
                    topic: d.entry.topic.clone(),
                    audience: Audience::from_id(&d.entry.audience).unwrap_or(Audience::Kids),
                    genre: Genre::from_id(&d.entry.genre).unwrap_or(Genre::Bedtime),
                    language: if d.entry.language.is_empty() { "English".into() } else { d.entry.language.clone() },
                    ..StoryOptions::default()
                };
                Ok((d.story, o, self.store.load_pictures(id), Some(id.to_string())))
            }
            None => {
                let shift = usize::from(self.story.title.is_empty());
                let pics: Vec<StoredPic> = self
                    .pics
                    .iter()
                    .filter(|p| p.state == PicState::Ready)
                    .filter_map(|p| Some(StoredPic { variants: Self::stored_variants(p), look: p.look.id().into(), seed: p.seed, provider: p.provider.clone(), model: p.model.clone(), ms: p.ms, size: p.size.clone(), source: p.how.clone(), scene_text: p.scene.text.clone(), unit: p.scene.place_before + shift * 0, title: p.scene.title.clone(), prompt: p.prompt.clone(), style: p.style.id().into(), mode: p.mode.id().into(), ext: p.ext.clone(), bytes: p.bytes.as_ref()?.as_ref().clone() }))
                    .collect();
                Ok((self.story.clone(), self.story_opts.clone(), pics, self.saved_id()))
            }
        }
    }

    pub fn do_export(&mut self, what: usize, id: Option<String>) {
        let (story, opts, pics, saved) = match self.export_source(id.as_deref()) {
            Ok(x) => x,
            Err(e) => {
                self.note = Some((e, false));
                return;
            }
        };
        let name = saved.clone().unwrap_or_else(|| {
            let s = st::slug(&story.title);
            if s.is_empty() { "story".into() } else { s }
        });
        match what {
            0 => self.host_save = Some((format!("{name}.html"), st::export_html(&story, &opts, &pics).into_bytes())),
            3 => self.host_save = Some((format!("{name}-all-pictures.html"), st::export_html_with(&story, &opts, &pics, true).into_bytes())),
            1 => {
                // a saved story goes out as it is stored (with its picture lines); one not saved yet, as it is on the screen
                let md = saved.as_deref().and_then(|i| self.store.raw_markdown(i).ok()).unwrap_or_else(|| story.markdown());
                self.host_save = Some((format!("{name}.md"), md.into_bytes()));
            }
            _ => match saved.as_deref().and_then(|i| self.store.audio_path(i)) {
                Some(p) => match std::fs::read(&p) {
                    Ok(b) => self.host_save = Some((format!("{name}.{}", p.extension().map_or("wav".into(), |e| e.to_string_lossy().into_owned())), b)),
                    Err(e) => self.note = Some((format!("could not read the audio: {e}"), false)),
                },
                None => self.note = Some(("This story has no saved audio yet. Use Save audio first.".into(), false)),
            },
        }
    }

    pub fn copy_story(&mut self) {
        let t = self.pane.selected_text().unwrap_or_else(|| self.story.plain());
        if !t.is_empty() {
            iyakki_uikit::ui::clipboard::copy(&t);
        }
    }

}
