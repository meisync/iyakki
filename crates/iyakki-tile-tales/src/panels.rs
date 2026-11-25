//! TALES drawing: the tabs, banner, composer, options and menus.

use super::*;

impl StoryApp {
    pub fn tabs(&mut self, g: &mut Gfx, x: f32, y: f32, w: f32, s: f32) -> f32 {
        let items = vec![("Story".to_string(), Event::Tab(T_STORY_PAGE)), ("Listen again".to_string(), Event::Tab(T_LIB_PAGE))];
        let sel = match self.page {
            Page::Story => 0,
            Page::Library => 1,
        };
        let tw = fx::tab_strip(g, x, y, s, w, &items, sel, ACCENT);
        let th = 14.0 * s * 2.1;
        // narrow: the composer is a toggle (it sits below the story when shown)
        let right = x + w;
        if !self.wide && self.page == Page::Story {
            let sz = 14.0 * s;
            let label = if self.pane_open { "Hide the idea box" } else { "Idea & options" };
            let ow = g.text_width(sz, label, Face::Bold) + sz * 1.9;
            if right - ow > x + tw + 8.0 * s {
                chip(g, right - ow, y + (th - 28.0 * s) / 2.0, s, label, self.pane_open, Event::Tab(T_OPTIONS));
            }
            return th;
        }
        // what is going on, on the right
        let (label, col) = if self.gen.is_some() {
            (format!("writing{}", ".".repeat(1 + (self.ticks / 10 % 3) as usize)), MAGENTA)
        } else if matches!(self.reader.status(), Status::Playing) {
            ("reading aloud".to_string(), CYAN)
        } else if matches!(self.reader.status(), Status::Paused) {
            ("paused".to_string(), MUTED)
        } else {
            (String::new(), MUTED)
        };
        if !label.is_empty() {
            let lw = g.text_width(13.0 * s, &label, Face::Bold);
            if tw + lw + 24.0 * s < w {
                g.circle(x + w - lw - 14.0 * s, y + 15.0 * s, 4.0 * s, col);
                g.text(x + w - lw, y + 15.0 * s - g.line_height(13.0 * s, Face::Bold) / 2.0, 13.0 * s, col, &label, Face::Bold);
            }
        }
        th
    }

    pub fn banner(&mut self, g: &mut Gfx, x: f32, y: f32, w: f32, s: f32) -> f32 {
        let shown = self.note.clone().or_else(|| self.wait_text.clone().map(|t| (t, true)));
        if let Some((m, ok)) = &shown {
            let m = if *ok { m.clone() } else { (self.reword)(m) };
            let lines = g.wrap(13.0 * s, &m, Face::Sans, w - 24.0 * s);
            let lh = g.line_height(13.0 * s, Face::Sans);
            let h = lines.len().min(3) as f32 * lh + 12.0 * s;
            g.rrect(x, y, w, h, 10.0 * s, if *ok { fx::over(WELL, with_alpha(OK, 30)) } else { fx::over(WELL, with_alpha(ERR, 34)) });
            for (i, l) in lines.iter().take(3).enumerate() {
                g.text(x + 12.0 * s, y + 6.0 * s + i as f32 * lh, 13.0 * s, if *ok { OK } else { ERR }, l, Face::Sans);
            }
            return h + 8.0 * s;
        }
        0.0
    }

    // ---------------------------------------------------------------------------------------------- the form pane

    /// The idea box: draws it (when `draw`) and registers its press area. Returns its height (with the notice under it).
    pub fn idea_box(&mut self, g: &mut Gfx, x: f32, y: f32, w: f32, s: f32, draw: bool, tick: u64) -> f32 {
        let focused = self.focus == Focus::Idea && self.modal.is_none();
        let ph = "Describe your story, e.g. a 5-minute funny story in Tamil about a fox who is afraid of the dark";
        let h = self.idea.render(g, x, y, w, s, 15.0 * s, 9.0 * s, 40.0 * s, focused, tick, ph, ACCENT, IDEA_AREA, "idea", false, draw);
        self.idea_geo = self.idea.geo;
        h
    }

    /// The one line under the player about reading aloud, and the colour to draw it in; `None` when a voice is ready.
    /// No voice is ready and the person can fix it: the chip "Turn on reading aloud" is shown instead of a message.
    pub fn chip_wanted(&self) -> bool {
        self.saved_audio.is_none() && matches!(self.tts, TtsStatus::NoEngine | TtsStatus::HostUnavailable(_) | TtsStatus::BrowserUnavailable(_))
    }

    /// The chip: look for the speech engines of this computer again; when one is found it is used (nothing to choose). When none
    /// is, the desktop window offers the browser voice helper, and every other screen goes to Settings → Voice.
    pub fn turn_on_reading(&mut self, port: &dyn TalesPort) {
        if self.turning_on {
            return;
        }
        port.audit("story.turn_on_reading", "");
        if !port.host_tts().is_enabled() {
            self.open_voice_settings("Reading aloud is set up in Settings → Voice.");
            return;
        }
        self.turning_on = true;
        self.note = Some(("Looking for a speech engine on this computer…".into(), true));
        port.host_tts().recheck();
    }

    pub fn open_voice_settings(&mut self, why: &str) {
        self.note = Some((why.to_string(), true));
        self.request = Some(("settings".into(), "voice".into()));
    }

    pub fn poll_turn_on(&mut self, port: &dyn TalesPort) {
        if !self.turning_on || port.host_tts().scanning() {
            return;
        }
        self.turning_on = false;
        self.refresh_tts(port);
        if matches!(self.tts, TtsStatus::Ready { .. }) {
            self.note = Some((format!("{} Press play.", st::status_text(&self.tts)), true));
            return;
        }
        let host = HostVoice::current();
        if host.front() == hostvoice::Front::Window && !host.helper_active() {
            match port.host_tts().helper_open(host, None) {
                Ok(_) => self.note = Some(("No speech engine was found on this computer. The voice helper is opening in your browser: allow sound there, then press play. (Settings → Voice has more choices.)".into(), true)),
                Err(e) => self.open_voice_settings(&format!("No speech engine was found, and the voice helper could not start: {e}")),
            }
            return;
        }
        self.open_voice_settings("No speech engine was found on this computer. Settings → Voice shows what is missing and how to get it.");
    }

    pub fn read_hint(&self) -> Option<(String, [u8; 4])> {
        match &self.tts {
            TtsStatus::Ready { .. } => None,
            _ if self.saved_audio.is_some() => None,
            TtsStatus::NoEngine => Some(("Reading aloud: open IYAKKI in a browser for browser voice, or set up Settings → Voice".into(), DIM)),
            other => Some((st::status_text(other), fx::AMBER)),
        }
    }

    // ---------------------------------------------------------------------------------------------- the composer

    /// The composer, in the manner of the AI IDE's chat: the writer line, one row of controls (who it is for, kind and tone as
    /// drop-downs, Model, Agent, Options), the idea box that grows with Send beside it, and a row of chips (what Pictures will do
    /// and the suggestions found in the text). Draws it (when `draw`) with its top left at (`x`, `y0`) and registers the clicks.
    /// Returns its height.
    #[allow(clippy::too_many_arguments)]
    pub fn composer_body(&mut self, g: &mut Gfx, x: f32, y0: f32, w: f32, s: f32, port: &dyn TalesPort, tick: u64, draw: bool, bounds: R) -> f32 {
        enum Ctl {
            Label(String),
            Aud(usize),
            Kind(String),
            Tone(String),
            Model(String),
            Opts,
        }
        let m = iyakki_uikit::ide_chrome::cur();
        let side = 12.0 * s;
        let (x0, w) = (x + side, w - 2.0 * side);
        let th = iyakki_uikit::ide_chrome::tool(s) + 2.0 * s;
        let fsz = m.font_small * s + 1.0 * s;
        let mut y = y0 + 6.0 * s;
        // who writes (and Stop while it is writing)
        if draw {
            let line = self.writer_line(port);
            let stop_w = if self.gen.is_some() { g.text_width(fsz, "Stop", Face::Bold) + 16.0 * s } else { 0.0 };
            let t = g.fit(13.0 * s, &line, Face::Sans, w - stop_w - 6.0 * s);
            g.text(x0, y + 1.0 * s, 13.0 * s, iyakki_uikit::ide_chrome::dim(), &t, Face::Sans);
            if self.gen.is_some() {
                iyakki_uikit::ide_chrome::text_btn(g, x0 + w - stop_w, y - 3.0 * s, s, "Stop", ERR, true, Event::Tab(T_STOP_WRITING), "Stop", Some("Stop writing: what is written so far is kept (Send a correction to steer it)"), bounds);
            }
        }
        y += 18.0 * s;
        // the controls, left to right, wrapping when the row is full
        let shown = self.shown_options();
        let chip = self.chip_text(port);
        let ctls: Vec<Ctl> = {
            let mut v = vec![Ctl::Label("Who is it for".into())];
            v.extend((0..Audience::ALL.len()).map(Ctl::Aud));
            v.push(Ctl::Kind(format!("Kind: {}", shown.genre.label())));
            v.push(Ctl::Tone(format!("Tone: {}", shown.tone.label())));
            v.push(Ctl::Model(chip.clone()));
            v.push(Ctl::Opts);
            v
        };
        let btn_w = |g: &mut Gfx, l: &str| g.text_width(fsz, l, Face::Bold) + 16.0 * s;
        let (mut cxp, mut cy) = (x0, y);
        for c in &ctls {
            let cw = match c {
                Ctl::Label(l) => g.text_width(fsz, l, Face::Sans) + 10.0 * s,
                Ctl::Aud(i) => btn_w(g, Audience::ALL[*i].label()),
                Ctl::Kind(l) | Ctl::Tone(l) => select_width(g, s, l),
                Ctl::Model(l) => btn_w(g, &format!("{l} ▾")),
                Ctl::Opts => btn_w(g, "Options ▾"),
            };
            if cxp > x0 && cxp + cw > x0 + w {
                cxp = x0;
                cy += th + 2.0 * s;
            }
            if draw {
                let ty = cy + 1.0 * s;
                match c {
                    Ctl::Label(l) => {
                        g.text(cxp, ty + (th - 2.0 * s - g.line_height(fsz, Face::Sans)) / 2.0, fsz, iyakki_uikit::ide_chrome::dim(), l, Face::Sans);
                    }
                    Ctl::Aud(i) => {
                        let a = Audience::ALL[*i];
                        let on = self.opts.audience == a;
                        let r = R::new(cxp, ty, cw, iyakki_uikit::ide_chrome::tool(s));
                        if on {
                            g.rrect(r.x, r.y, r.w, r.h, 4.0 * s, with_alpha(ACCENT, 46));
                            g.rrect_outline(r.x, r.y, r.w, r.h, 4.0 * s, 1.2 * s, ACCENT);
                        }
                        iyakki_uikit::ide_chrome::text_btn(g, cxp, ty, s, a.label(), if on { ACCENT } else { MUTED }, true, Event::Tab(T_AUDIENCE + i), a.label(), Some("Who the story is for (remembered)"), bounds);
                    }
                    Ctl::Kind(l) => self.anchors[0] = select_box(g, cxp, ty, cw, s, l, self.menu == Some(Menu::Kind), ACCENT, Event::Tab(T_KIND_MENU), "Kind of story"),
                    Ctl::Tone(l) => self.anchors[1] = select_box(g, cxp, ty, cw, s, l, self.menu == Some(Menu::Tone), ACCENT, Event::Tab(T_TONE_MENU), "Tone"),
                    Ctl::Model(l) => {
                        self.anchors[4] = R::new(cxp, ty, cw, iyakki_uikit::ide_chrome::tool(s));
                        let label = format!("{l} ▾");
                        let on = self.agent_id(port).is_some();
                        iyakki_uikit::ide_chrome::text_btn(g, cxp, ty, s, &label, if on { rgb(255, 196, 94) } else { ACCENT }, true, Event::Tab(T_MODEL), &label, Some("Choose who writes the story: same as chat, a model of a provider, or an agent such as Claude Code with one of its models (Ctrl+P); an agent gets no tools"), bounds);
                    }
                    Ctl::Opts => {
                        self.anchors[2] = R::new(cxp, ty, cw, iyakki_uikit::ide_chrome::tool(s));
                        iyakki_uikit::ide_chrome::text_btn(g, cxp, ty, s, "Options ▾", if self.menu == Some(Menu::Options) { ACCENT } else { MUTED }, true, Event::Tab(T_OPTIONS_MENU), "Options ▾", Some("Length, language, characters, pictures, reading and saving (Ctrl+O)"), bounds);
                    }
                }
            }
            cxp += cw + 2.0 * s;
        }
        y = cy + th + 4.0 * s;
        // the idea box and Send
        let send_w = 64.0 * s;
        let ih = self.idea_box(g, x0, y, w - send_w - 8.0 * s, s, draw, tick);
        if draw {
            let bs = 13.0 * s;
            let by = y + (self.idea.box_h - bs * 2.1) / 2.0;
            if self.gen.is_some() {
                g.button_off(x0 + w - send_w + 8.0 * s, by, bs, "Send");
            } else {
                g.button(x0 + w - send_w + 8.0 * s, by, bs, "Send", ACCENT, Event::Tab(T_CREATE));
            }
        }
        y += ih + 6.0 * s;
        // the chips: what the pictures will be, and what was understood from the text
        let mut items: Vec<(String, bool, Event)> = vec![(self.pictures_status(port), false, Event::Tab(T_OPTIONS_MENU))];
        for c in self.hint_chips() {
            let label = if c.applied { format!("{}: {} ×", c.name, c.value) } else { format!("\"{}\" ignored, {} is {} ×", c.said, c.name, c.chosen) };
            items.push((label, c.applied, Event::Tab(T_HINT + c.k)));
        }
        y += flow(g, x0, y, w, s, &items, draw) + 8.0 * s;
        y - y0
    }

    /// The height of the composer in a panel `w` wide and `avail` high: the idea box shows fewer lines (three at least when there
    /// is room, two or one when it is short) when room is short.
    pub fn composer_height(&mut self, g: &mut Gfx, w: f32, avail: f32, s: f32, port: &dyn TalesPort) -> f32 {
        for rows in (1..=iyakki_uikit::multiline::MAX_ROWS).rev() {
            self.idea.max_rows = rows;
            self.idea.min_rows = rows.min(iyakki_uikit::multiline::MIN_ROWS);
            let h = self.composer_body(g, 0.0, 0.0, w, s, port, 0, false, R::new(0.0, 0.0, w, avail));
            if h <= avail * 0.62 || rows == 1 {
                return h;
            }
        }
        0.0
    }

    /// The panel on the right, like the AI IDE's chat: a header, the conversation (what was understood, the steps of writing
    /// and drawing, the person's corrections) with the same renderer as the chat, and the composer at the bottom.
    pub fn panel(&mut self, g: &mut Gfx, r: R, s: f32, port: &dyn TalesPort, tick: u64, area: R) {
        let m = iyakki_uikit::ide_chrome::cur();
        g.rect(r.x, r.y, r.w, r.h, iyakki_uikit::ui::BG_BOTTOM);
        g.rect(r.x, r.y, 1.0, r.h, [255, 255, 255, 22]);
        let head_h = (m.head_h * s).max(30.0 * s);
        let title = if self.story.blocks.is_empty() { "Create a story".to_string() } else { self.display_title() };
        let tsz = m.font_ui * s;
        let hide_w = if self.wide { 0.0 } else { g.text_width(m.font_small * s + s, "Hide", Face::Bold) + 22.0 * s };
        let t = g.fit(tsz, &title, Face::Bold, (r.w - 24.0 * s - hide_w).max(40.0 * s));
        g.text(r.x + 12.0 * s, r.y + (head_h - g.line_height(tsz, Face::Bold)) / 2.0, tsz, TEXT, &t, Face::Bold);
        if !self.wide {
            iyakki_uikit::ide_chrome::text_btn(g, r.x + r.w - hide_w - 4.0 * s, r.y + (head_h - iyakki_uikit::ide_chrome::tool(s)) / 2.0, s, "Hide", MUTED, true, Event::Tab(T_OPTIONS), "Hide the idea box", Some("Back to the story"), area);
        }
        g.rect(r.x + 1.0, r.y + head_h, r.w - 1.0, 1.0, [255, 255, 255, 14]);
        let comp_h = self.composer_height(g, r.w, r.h - head_h, s, port);
        let comp_y = r.y + r.h - comp_h;
        g.rect(r.x + 1.0, comp_y - 2.0 * s, r.w - 1.0, comp_h + 2.0 * s, iyakki_uikit::ui::BG_BOTTOM);
        g.rect(r.x + 1.0, comp_y - 2.0 * s, r.w - 1.0, 1.0, [255, 255, 255, 14]);
        // the conversation, in the chat's own renderer
        let log = R::new(r.x + 12.0 * s, r.y + head_h + 6.0 * s, r.w - 24.0 * s, (comp_y - 6.0 * s - (r.y + head_h + 6.0 * s)).max(30.0 * s));
        if g.hovered(r) {
            self.panel_hover = true;
        }
        let busy = self.gen.is_some() || self.pics.iter().any(|p| matches!(p.state, PicState::Working | PicState::Queued));
        let think_secs = self.think_since.map_or(0, |t| t.elapsed().as_secs());
        chat_view::set_mono_boost(1.0);
        let total = {
            let bctx = chat_view::Ctx { tool_ok: &self.tool_ok, approval: None, busy, open: &Default::default(), live: "", thinking: &self.thinking, think_secs };
            self.view.prepare(g, &self.convo, &bctx, log.w, s, &mut NoPics)
        };
        self.max_off = (total - log.h).max(0.0);
        self.scroll_px = self.scroll_px.clamp_safe(0.0, self.max_off);
        if total <= 0.0 {
            let lines = ["Describe a story and press Send.", "Say it in words if you like: a 5-minute funny story in Tamil about a fox who is afraid of the dark.", "Then tell me what to change: shorter, a different name, funnier, another language, or redraw picture 2 as a comic."];
            let mut y = log.y + 8.0 * s;
            // the help lines stay above the composer: a short window shows the first lines only (never one running into the controls)
            let limit = (log.y + log.h).min(comp_y - 2.0 * s);
            'help: for (i, l) in lines.iter().enumerate() {
                for w in g.wrap(if i == 0 { 15.5 * s } else { 13.5 * s }, l, Face::Sans, log.w) {
                    let step = if i == 0 { 22.0 * s } else { 19.0 * s };
                    if y + step > limit {
                        break 'help;
                    }
                    g.text(log.x, y, if i == 0 { 15.5 * s } else { 13.5 * s }, if i == 0 { TEXT } else { MUTED }, &w, Face::Sans);
                    y += step;
                }
                y += 8.0 * s;
            }
        }
        let (scroll_px, view) = (self.scroll_px, &self.view);
        g.clipped(log, |g| view.draw(g, log, scroll_px, tick, s, ACCENT, &|k| Event::Key(k)));
        self.composer_body(g, r.x, comp_y, r.w, s, port, tick, true, area);
    }

    /// The Options popover: the little that is not always on show.
    #[allow(clippy::too_many_arguments)]
    pub fn options_content(&mut self, g: &mut Gfx, x: f32, y0: f32, w: f32, s: f32, port: &dyn TalesPort, draw: bool) -> f32 {
        let gap = 12.0 * s;
        let mut y = y0;
        y += section(g, x, y, w, s, "Or pick an idea", draw);
        let items: Vec<(String, bool, Event)> = SUGGESTIONS.iter().enumerate().map(|(i, t)| (t.to_string(), self.idea.text == *t, Event::Tab(T_SUGGEST + i))).collect();
        y += flow(g, x, y, w, s, &items, draw) + gap;
        y += section(g, x, y, w, s, "How long", draw);
        let items: Vec<_> = Length::ALL.iter().enumerate().map(|(i, a)| (a.label().to_string(), self.opts.length == *a, Event::Tab(T_LENGTH + i))).collect();
        y += flow(g, x, y, w, s, &items, draw) + gap;
        // the one manual control for the language (the idea text can say it too)
        y += section(g, x, y, w, s, "Language", draw);
        let label = format!("Language: {}", self.shown_options().language);
        let lw = select_width(g, s, "Language: Portuguese").max(select_width(g, s, &label));
        if draw {
            self.anchors[5] = select_box(g, x, y, lw, s, &label, self.menu == Some(Menu::Language), ACCENT, Event::Tab(T_LANG_MENU), "Language");
        }
        y += iyakki_uikit::ide_chrome::tool(s) + gap;
        y += section(g, x, y, w, s, "Pictures", draw);
        let images = !self.image_models.is_empty();
        let mut chips: Vec<(String, bool, Event)> = Vec::new();
        let mut real_lines: Vec<(String, [u8; 4])> = Vec::new();
        if self.opts.pictures != PictureMode::Off && images {
            // the one visible choice for real-looking pictures, with the model and server it will use
            let pic = self.pic_now();
            let on = pic.is_realistic();
            chips.push((format!("{} Realistic pictures (photo style, people look real)", if on { "\u{2611}" } else { "\u{2610}" }), on, Event::Tab(T_REALISTIC)));
            let cands = port.picture_choices(&self.found_images(port));
            let info = illus::realism_info(&pic, self.prefs.picture_model.as_ref(), &cands, self.memory_now(port).filter(|m| m.total > 0).map(|m| m.available >> 20).as_ref().copied());
            real_lines.push((format!("{}{}", info.uses, if on { "" } else { " (when ticked)" }), MUTED));
            if let Some(w) = &info.warning {
                real_lines.push((w.clone(), fx::AMBER));
            }
            if let Some((p, m, size)) = &info.install {
                chips.push((format!("Install {m} on {p} ({size})"), false, Event::Tab(T_REAL_INSTALL)));
            }
        }
        chips.push(("Pictures off".to_string(), self.opts.pictures == PictureMode::Off, Event::Tab(T_PICMODE)));
        let mut pnote = None;
        if images && self.opts.pictures != PictureMode::Off {
            let (pl, pn) = self.picture_label(port);
            chips.push((format!("Picture model: {pl}"), self.prefs.picture_model.is_some(), Event::Tab(T_PICMODEL)));
            pnote = pn;
        }
        if self.opts.pictures != PictureMode::Off {
            chips.push((format!("Pictures: {}", self.opts.coverage.label()), self.opts.coverage != Coverage::Chapters, Event::Tab(T_COVERAGE)));
            if images {
                let pic = self.pic_now();
                let styles = pic.styles_list();
                for (i, l) in illus::Look::ALL.iter().enumerate() {
                    let on = styles.contains(l);
                    chips.push((format!("Style {}: {}", if on { "\u{2611}" } else { "\u{2610}" }, l.label()), on, Event::Tab(T_STYLE + i)));
                }
                chips.push((format!("Pictures per style: {}", pic.count()), pic.count() > 1, Event::Tab(T_PERSTYLE)));
                chips.push((format!("Quality: {}", pic.quality.label()), pic.quality != illus::Quality::Selected, Event::Tab(T_QUALITY)));
                let choices = port.picture_choices(&self.found_images(port));
                let mname = pic.model.as_ref().map_or("automatic".to_string(), |c| {
                    let state = choices.iter().find(|x| x.provider == c.provider && x.model == c.model).map_or(String::new(), |x| format!(" ({}{})", if x.downloaded { "installed" } else { "not downloaded" }, x.size_gb.map_or(String::new(), |g| format!(", {g:.1} GB"))));
                    format!("{}{state}", Self::model_name(c))
                });
                chips.push((format!("Model for this story: {mname}"), pic.model.is_some(), Event::Tab(T_STORYMODEL)));
                chips.push((format!("Time limit per picture: {} min", pic.limit_min()), pic.limit_min() != 15, Event::Tab(T_TIMEOUT)));
                chips.push((format!("Same look for all pictures: {}", if pic.keep_look { "on" } else { "off" }), pic.keep_look, Event::Tab(T_KEEPLOOK)));
            }
            chips.push(("Picture log (g)".to_string(), false, Event::Tab(T_PICLOG)));
            chips.push((format!("Drawing: {}", self.opts.drawing.label()), self.opts.drawing != st::DrawingMode::Auto, Event::Tab(T_DRAWMODE)));
            chips.push((format!("Allow short video clips: {}", if self.opts.video { "on" } else { "off" }), self.opts.video, Event::Tab(T_VIDEO)));
        }
        y += flow(g, x, y, w, s, &chips, draw) + 6.0 * s;
        for (t, col) in &real_lines {
            for l in g.wrap(12.0 * s, t, Face::Sans, w) {
                if draw {
                    g.text(x, y, 12.0 * s, *col, &l, Face::Sans);
                }
                y += g.line_height(12.0 * s, Face::Sans);
            }
        }
        // the picture models that are not installed, with their size and an Install button
        if images && self.opts.pictures != PictureMode::Off {
            let missing: Vec<(String, bool, Event)> = port.picture_choices(&self.found_images(port)).iter().enumerate().filter(|(_, c)| !c.downloaded).take(10).map(|(i, c)| (format!("Install {} on {} ({})", c.model, c.provider, c.line().split(", ").nth(1).unwrap_or("size unknown")), false, Event::Tab(T_CHOICE_INSTALL + i))).collect();
            if !missing.is_empty() {
                y += 4.0 * s + flow(g, x, y, w, s, &missing, draw) + 2.0 * s;
            }
        }
        let hint = match (self.opts.pictures, images) {
            (PictureMode::Off, _) => "No pictures: the story is text only.".to_string(),
            (_, true) => "Pictures are automatic: each scene is drawn as a real image by the picture model; if that fails the rest are drawn as SVG. The style follows the kind of story (say \"comic style\" or \"pixel art\" in your idea to change it).".to_string(),
            (_, false) => "Pictures are automatic: no picture model is set up (Settings → Models), so the writer draws each scene: a strong model or agent (Claude, GPT class) draws a rich SVG itself, a small model picks a scene for the built-in cartoon (Drawing ▸ Auto, Rich SVG or Built-in cartoon). Realistic or photo pictures need a picture model; the built-in drawings are cartoons. The style follows the kind of story (say \"comic style\" or \"pixel art\" in your idea to change it).".to_string(),
        };
        let mut hint = pnote.unwrap_or(hint);
        // one plain line about the picture model: why it is not used, or what it costs in time (an estimate, said so)
        if self.opts.pictures != PictureMode::Off {
            let r = self.readiness(port);
            match illus::reason(&r) {
                Some(why) => hint = format!("{why} {hint}"),
                None if images => {
                    let using = self.resolved_for(&self.pic_now(), port).map_or(String::new(), |r| format!("Using {}. ", r.summary()));
                    let list: Vec<String> = port.picture_choices(&self.found_images(port)).iter().map(|c| c.line()).collect();
                    let missing = self.resolved_for(&self.pic_now(), port).filter(|r| !r.downloaded).map_or(String::new(), |r| format!("{} ", illus::not_downloaded_text(&r.model, &r.provider, "see the list"))) ;
                    let job = self.job_line(port, 1).filter(|_| self.pic_now().styles_list().len() > 1 || self.pic_now().count() > 1).map_or(String::new(), |j| format!(" For one scene: {j}"));
                    hint = format!("{missing}{using}{} {hint}{job} {} Picture models: {}.", self.timing_line(port, 1), illus::advice(port.gpu_lemonade().as_deref()), list.join(" \u{b7} "));
                }
                None => {}
            }
        }
        let lh = g.line_height(12.0 * s, Face::Sans);
        for l in g.wrap(12.0 * s, &hint, Face::Sans, w) {
            if draw {
                g.text(x, y, 12.0 * s, MUTED, &l, Face::Sans);
            }
            y += lh;
        }
        y += gap;
        y += section(g, x, y, w, s, "Reading and saving", draw);
        let rw = [("Read while it is written".to_string(), self.prefs.read_while_writing, Event::Tab(T_READ_WHILE)), ("Save automatically".to_string(), self.prefs.auto_save, Event::Tab(T_AUTOSAVE))];
        y += flow(g, x, y, w, s, &rw, draw) + 4.0 * s;
        y - y0
    }

    /// The Options popover next to the panel, scrolling inside when it is taller than the room.
    pub fn options_popover(&mut self, g: &mut Gfx, area: R, panel: R, s: f32, port: &dyn TalesPort) {
        let w = (area.w - 16.0 * s).min(560.0 * s);
        let pad = 16.0 * s;
        let content_h = self.options_content(g, 0.0, 0.0, w - 2.0 * pad - 8.0 * s, s, port, false);
        let head = 36.0 * s;
        let h = (content_h + head + 12.0 * s).min(area.h - 16.0 * s);
        let x = (panel.x - w - 6.0 * s).max(area.x + 8.0 * s);
        let anchor = self.anchors[2];
        let y = (anchor.y + anchor.h - h).clamp_safe(area.y + 8.0 * s, (area.y + area.h - h - 8.0 * s).max(area.y + 8.0 * s));
        let r = R::new(x, y, w, h);
        g.hit(area, Event::Tab(T_MENU_CLOSE), "close the menu");
        g.rrect(r.x, r.y, r.w, r.h, 14.0 * s, rgb(16, 20, 36));
        g.rrect_outline(r.x, r.y, r.w, r.h, 14.0 * s, 1.5 * s, ACCENT);
        // swallow presses on the panel itself
        g.hit(r, Event::Tick, "options");
        fx::section_title(g, r.x + pad, r.y + 11.0 * s, r.w - 2.0 * pad - 70.0 * s, s, "Options", ACCENT);
        let sz = 13.0 * s;
        let cw = g.text_width(sz, "Close", Face::Bold) + sz * 1.7;
        g.button(r.x + r.w - cw - pad, r.y + 5.0 * s, sz, "Close", MUTED, Event::Tab(T_MENU_CLOSE));
        let vp = R::new(r.x + pad, r.y + head, r.w - 2.0 * pad - 8.0 * s, r.h - head - 8.0 * s);
        self.cmax = (content_h - vp.h).max(0.0);
        self.cscroll = self.cscroll.clamp_safe(0.0, self.cmax);
        let top = vp.y - self.cscroll;
        g.clipped(vp, |g| {
            self.options_content(g, vp.x, top, vp.w, s, port, true);
        });
        if self.cmax > 0.0 {
            fx::scrollbar(g, R::new(r.x + r.w - 10.0 * s, vp.y + 4.0 * s, 6.0 * s, vp.h - 8.0 * s), s, (self.cscroll / (28.0 * s)) as usize, (vp.h / (28.0 * s)) as usize, (content_h / (28.0 * s)) as usize, ACCENT);
        }
    }

    /// The menus that hang from a chip of the composer, and the reading-aloud panel.
    pub fn draw_menus(&mut self, g: &mut Gfx, area: R, panel: R, s: f32, port: &dyn TalesPort) {
        let Some(m) = self.menu else { return };
        match m {
            Menu::Kind => {
                let rows: Vec<(String, bool, Event)> = Genre::ALL.iter().enumerate().map(|(i, a)| (a.label().to_string(), i == self.menu_sel, Event::Tab(T_GENRE + i))).collect();
                menu_list(g, self.anchors[0], area, s, &rows, Event::Tab(T_MENU_CLOSE), ACCENT);
            }
            Menu::Tone => {
                let rows: Vec<(String, bool, Event)> = Tone::ALL.iter().enumerate().map(|(i, a)| (a.label().to_string(), i == self.menu_sel, Event::Tab(T_TONE + i))).collect();
                menu_list(g, self.anchors[1], area, s, &rows, Event::Tab(T_MENU_CLOSE), ACCENT);
            }
            Menu::Language => {
                self.options_popover(g, area, panel, s, port);
                let rows: Vec<(String, bool, Event)> = LANGUAGES.iter().enumerate().map(|(i, a)| (a.to_string(), i == self.menu_sel, Event::Tab(T_LANG + i))).collect();
                menu_list(g, self.anchors[5], area, s, &rows, Event::Tab(T_MENU_CLOSE), ACCENT);
            }
            Menu::Version => {
                let rows: Vec<(String, bool, Event)> = (1..=self.ver_count).map(|k| (format!("Version {k}{}", if k == self.ver { " (in use)" } else { "" }), k as usize == self.menu_sel + 1, Event::Tab(T_VERSION_PICK + k as usize))).collect();
                menu_list(g, self.anchors[6], area, s, &rows, Event::Tab(T_MENU_CLOSE), ACCENT);
            }
            Menu::Options => self.options_popover(g, area, panel, s, port),
            Menu::How => self.how_panel(g, area, s, port),
        }
    }

    /// The longer text about reading aloud, in a small panel.
    pub fn how_panel(&mut self, g: &mut Gfx, area: R, s: f32, port: &dyn TalesPort) {
        let w = 640.0 * s;
        let p = fx::modal(g, area, s, w, 400.0 * s, ACCENT, false);
        let top = fx::modal_title(g, p, s, "Reading aloud", ACCENT);
        let inner = R::new(p.x + 26.0 * s, top + 4.0 * s, p.w - 52.0 * s, p.y + p.h - top - 62.0 * s);
        let mut paras: Vec<(String, [u8; 4])> = vec![(st::status_text(&self.tts), if matches!(self.tts, TtsStatus::Ready { .. }) { OK } else { fx::AMBER })];
        if matches!(self.tts, TtsStatus::NoEngine) {
            paras.push((st::NO_ENGINE_HELP.to_string(), MUTED));
        }
        let mut y = inner.y - self.cscroll;
        let lh = g.line_height(13.0 * s, Face::Sans);
        let mut total = 0.0;
        let quick: Vec<(String, bool, Event)> = if matches!(self.tts, TtsStatus::NoEngine) && port.is_admin() { self.quick.iter().enumerate().take(6).map(|(i, q)| (q.label.clone(), false, Event::Tab(T_QUICK + i))).collect() } else { Vec::new() };
        g.clipped(inner, |g| {
            for (text, col) in &paras {
                for l in g.wrap(13.0 * s, text, Face::Sans, inner.w) {
                    g.text(inner.x, y, 13.0 * s, *col, &l, Face::Sans);
                    y += lh;
                    total += lh;
                }
                y += 8.0 * s;
                total += 8.0 * s;
            }
            if !quick.is_empty() {
                let h = flow(g, inner.x, y, inner.w, s, &quick, true);
                total += h;
            }
        });
        self.cmax = (total - inner.h).max(0.0);
        self.cscroll = self.cscroll.clamp_safe(0.0, self.cmax);
        let sz = 14.0 * s;
        let cw = g.text_width(sz, "Close", Face::Bold) + sz * 1.7;
        g.button(p.x + p.w - 26.0 * s - cw, p.y + p.h - 48.0 * s, sz, "Close", MUTED, Event::Tab(T_MENU_CLOSE));
    }

    // ---------------------------------------------------------------------------------------------- the story view

    pub fn ensure_layout(&mut self, g: &mut Gfx, size: f32, width: f32) {
        let key = ((size * 100.0) as i32, width as i32);
        if key != self.lay_key {
            self.lay.clear();
            self.lay_key = key;
        }
        // the units, in order, without copying their text (a long story would copy it every frame): the title (not drawn), the
        // headings and paragraphs, the sentence still being written
        let shift = usize::from(!self.story.title.is_empty());
        let partial = !self.story.partial.trim().is_empty();
        let count = shift + self.story.blocks.len() + usize::from(partial);
        self.lay.truncate(count);
        for i in 0..count {
            let (text, kind): (&str, UKind) = if i < shift {
                (self.story.title.as_str(), UKind::Hidden)
            } else if let Some(b) = self.story.blocks.get(i - shift) {
                (b.text(), if matches!(b, Block::Heading(_)) { UKind::Heading } else { UKind::Para })
            } else {
                (self.story.partial.as_str(), UKind::Partial)
            };
            if self.lay.get(i).is_some_and(|l| l.text == text && l.kind == kind) {
                continue;
            }
            let (sz, face) = if kind == UKind::Heading { (size * 1.18, Face::Bold) } else { (size, Face::Sans) };
            let lines = if kind == UKind::Hidden { Vec::new() } else { g.wrap(sz, text, face, width) };
            let chars: Vec<char> = text.chars().collect();
            let mut pos = 0usize;
            let mut with_start = Vec::with_capacity(lines.len());
            for l in lines {
                while pos < chars.len() && chars[pos].is_whitespace() {
                    pos += 1;
                }
                let n = l.chars().count();
                with_start.push((l, pos));
                pos += n;
            }
            let lay = ULay { text: text.to_string(), kind, lines: with_start };
            if i < self.lay.len() {
                self.lay[i] = lay;
            } else {
                self.lay.push(lay);
            }
        }
    }

    pub fn display_title(&self) -> String {
        if !self.story.title.is_empty() {
            self.story.title.clone()
        } else if let Some(e) = &self.saved {
            e.title.clone()
        } else if self.gen.is_none() && !self.story.blocks.is_empty() {
            self.story_opts.topic.chars().take(60).collect()
        } else {
            String::new()
        }
    }

    pub fn hero(&mut self, g: &mut Gfx, r: R, s: f32, snap_unit: Option<usize>) {
        let hh = r.h;
        let shown_title = self.display_title();
        cover(g, R::new(r.x + 4.0 * s, r.y + 4.0 * s, hh - 8.0 * s, hh - 8.0 * s), s, if shown_title.is_empty() { "Your story" } else { &shown_title });
        let rx = r.x + hh + 14.0 * s;
        let rw = r.w - hh - 14.0 * s;
        let title = if shown_title.is_empty() { "Writing your story…".to_string() } else { shown_title };
        let tsz = if hh >= 110.0 * s { 24.0 * s } else { 20.0 * s };
        let lines = g.wrap(tsz, &title, Face::Bold, rw);
        let lh = g.line_height(tsz, Face::Bold);
        let nl = if hh >= 110.0 * s { 2 } else { 1 };
        let tc = if snap_unit == Some(0) && !self.story.title.is_empty() { ACCENT } else { TEXT };
        for (i, l) in lines.iter().take(nl).enumerate() {
            let l = if i + 1 == nl && lines.len() > nl { g.fit(tsz, &format!("{l}…"), Face::Bold, rw) } else { l.clone() };
            g.text(rx, r.y + 4.0 * s + i as f32 * lh, tsz, tc, &l, Face::Bold);
        }
        let ty = r.y + 4.0 * s + lines.len().min(nl) as f32 * lh + 4.0 * s;
        let o = &self.story_opts;
        let state = if self.saved.is_some() { " · saved" } else if self.gen.is_none() { " · not saved" } else { "" };
        let by = if self.run_model.is_empty() { String::new() } else { format!(" · {}", self.run_model) };
        let version = if self.ver_count > 1 { format!(" · version {}", self.ver) } else { String::new() };
        let meta = format!("{} · {} · {} · {} · {} words{version}{by}{state}", o.audience.label(), o.genre.label(), o.length.id(), o.language, self.story.words());
        let meta = g.fit(13.0 * s, &meta, Face::Sans, rw);
        g.text(rx, ty, 13.0 * s, if self.saved.is_some() { fx::readable(OK, PANEL, 4.5) } else { MUTED }, &meta, Face::Sans);
        // the toolbar along the bottom of the hero, as many buttons as fit
        let by = r.y + hh - 14.0 * s * 2.1 - 4.0 * s;
        let mut bx = rx;
        let mut btns: Vec<(String, usize, [u8; 4], bool)> = Vec::new();
        if self.gen.is_some() {
            btns.push(("Stop writing".into(), T_STOP_WRITING, ERR, true));
        } else {
            if self.saved.is_some() {
                btns.push(("Saved ✓".into(), T_SAVE, OK, false));
            } else {
                btns.push(("Save story".into(), T_SAVE, ACCENT, true));
            }
            if self.held {
                btns.push(("Make the pictures".into(), T_PICS_GO, ACCENT, true));
                btns.push(("No pictures".into(), T_PICS_SKIP, MUTED, true));
            } else if self.pics.iter().any(|p| matches!(p.state, PicState::Working | PicState::Queued)) || self.alt_running.is_some() || !self.alt_jobs.is_empty() {
                let done = self.pics.iter().filter(|p| matches!(p.state, PicState::Ready | PicState::Failed(_))).count();
                btns.push((format!("Cancel pictures {done}/{}", self.pics.len()), T_PICS_CANCEL, ERR, true));
            }
            if let Some(j) = &self.audio_job {
                btns.push((format!("Cancel audio {}/{}", j.done.load(Ordering::SeqCst), j.total), T_CANCEL_AUDIO, ERR, true));
            } else {
                let off = self.no_audio_file && self.saved_audio.is_none();
                btns.push((if self.saved.as_ref().is_some_and(|e| e.has_audio) { "Save audio again".into() } else { "Save audio".into() }, T_SAVE_AUDIO, if off { DIM } else { MUTED }, true));
            }
            if self.ver_count > 1 && self.saved.is_some() {
                btns.push((format!("Version {} ▾", self.ver), T_VERSION_MENU, ACCENT, true));
            }
            if let Some(i) = &self.install {
                btns.push((if i.started { format!("Downloading {}…", i.model) } else { format!("Install {} ({})", i.model, i.size) }, T_PIC_INSTALL, ACCENT, !i.started));
            }
            if self.pics.iter().any(|p| !p.alts.is_empty()) {
                btns.push(("Keep only the chosen".into(), T_KEEP_ONLY, MUTED, true));
            }
            if !self.pics.is_empty() {
                btns.push(("Picture log (g)".into(), T_PICLOG, MUTED, true));
            }
            btns.push(("Export…".into(), T_EXPORT, MUTED, true));
            btns.push(("Save PDF".into(), T_PDF, MUTED, true));
            if let Some(e) = &self.saved {
                btns.push((if e.favourite { "Unfavourite".into() } else { "Favourite".into() }, T_FAV, MUTED, true));
            }
            btns.push(("Copy".into(), T_COPY, MUTED, true));
            btns.push(("New story".into(), T_NEW, MUTED, true));
        }
        for (label, code, col, active) in btns {
            let bsz = 13.0 * s;
            let bw = g.text_width(bsz, &label, Face::Bold) + bsz * 1.7;
            if bx + bw > rx + rw {
                continue;
            }
            if code == T_VERSION_MENU {
                self.anchors[6] = R::new(bx, by, bw, bsz * 2.1);
            }
            if active {
                g.button(bx, by, bsz, &label, col, Event::Tab(code));
            } else {
                // "Saved": shown lit, and still a button (it saves again what has changed since)
                let rr = R::new(bx, by, bw, bsz * 2.1);
                let hot = g.hovered(rr);
                let tc = fx::pill(g, rr, bsz / 14.0, col, hot, true);
                g.text(bx + bsz * 0.85, by + (bsz * 2.1 - g.line_height(bsz, Face::Bold)) / 2.0, bsz, tc, &label, Face::Bold);
                g.hit(rr, Event::Tab(code), &label);
            }
            bx += bw + 8.0 * s;
        }
    }

    /// The size of the slot of a picture inside a text column of width `w`.
    pub fn pic_slot(w: f32, s: f32) -> (f32, f32) {
        let pw = w.min(520.0 * s);
        (pw, pw * 0.6)
    }

    /// The poster frame of a WebM clip, scaled to the slot and kept with the pictures (`pic_scaled`, under a key of its own); `None` when the
    /// clip is not a VP8 one or its first picture cannot be decoded.
    pub fn poster_scaled(&mut self, key: u64, bytes: &[u8], slot: R, s: f32) -> Option<usize> {
        let ident = (key ^ 0x5649_4445_4F00_0000, slot.w.round() as i32, slot.h.round() as i32, (s * 1000.0) as i32);
        if let Some(i) = self.pic_scaled.iter().position(|e| (e.0, e.1, e.2, e.3) == ident) {
            return Some(i);
        }
        let bm = iyakki_web::webm::poster(bytes).ok()?;
        let mut full = tiny_skia::Pixmap::new(bm.width, bm.height)?;
        for (dst, px) in full.pixels_mut().iter_mut().zip(bm.rgba.chunks_exact(4)) {
            *dst = tiny_skia::ColorU8::from_rgba(px[0], px[1], px[2], px[3]).premultiply();
        }
        let (pw, ph) = (bm.width as f32, bm.height as f32);
        let k = ((slot.w - 8.0 * s) / pw).min((slot.h - 8.0 * s) / ph);
        let (iw, ih) = ((pw * k).round().max(1.0) as u32, (ph * k).round().max(1.0) as u32);
        let mut scaled = tiny_skia::Pixmap::new(iw, ih)?;
        let paint = tiny_skia::PixmapPaint { quality: tiny_skia::FilterQuality::Bilinear, ..Default::default() };
        scaled.draw_pixmap(0, 0, full.as_ref(), &paint, tiny_skia::Transform::from_scale(iw as f32 / pw, ih as f32 / ph), None);
        if self.pic_scaled.len() >= 10 {
            self.pic_scaled.remove(0);
        }
        self.pic_scaled.push((ident.0, ident.1, ident.2, ident.3, scaled));
        Some(self.pic_scaled.len() - 1)
    }

    /// Draw one picture (or its placeholder) in `slot`; the buttons to draw it again sit in its corner.
    pub fn draw_pic(&mut self, g: &mut Gfx, slot: R, s: f32, idx: usize, tick: u64) {
        let (state, title, bytes, key) = {
            let p = &self.pics[idx];
            (p.state.clone(), p.scene.title.clone(), p.bytes.clone(), p.key)
        };
        let video = matches!(self.pics[idx].ext.as_str(), "mp4" | "webm");
        match (&state, bytes) {
            // a video clip is kept with the story; this view has no video player, so the slot says so and where it is
            (PicState::Ready, Some(b)) if video => {
                cover(g, R::new(slot.x + 4.0 * s, slot.y + 4.0 * s, slot.w - 8.0 * s, slot.h - 8.0 * s), s, &title);
                // the first picture of a VP8 WebM clip is the poster frame; any other clip keeps the card (the sentence says why)
                let poster = if self.pics[idx].ext == "webm" { self.poster_scaled(key, &b, slot, s) } else { None };
                if let Some(i) = poster {
                    let scaled = &self.pic_scaled[i].4;
                    let (dx, dy) = ((slot.x + (slot.w - scaled.width() as f32) / 2.0).round() as i32, (slot.y + (slot.h - scaled.height() as f32) / 2.0).round() as i32);
                    g.pm_mut().draw_pixmap(dx, dy, scaled.as_ref(), &tiny_skia::PixmapPaint::default(), tiny_skia::Transform::identity(), None);
                    g.rrect_outline(slot.x, slot.y, slot.w, slot.h, 12.0 * s, 1.0, with_alpha(ACCENT, 90));
                }
                let label = format!("Video clip ({} KB, {}): saved in the story's pics folder", b.len() / 1024, self.pics[idx].ext);
                let t = g.fit(12.0 * s, &label, Face::Bold, slot.w - 24.0 * s);
                let tw = g.text_width(12.0 * s, &t, Face::Bold);
                g.rrect(slot.x + (slot.w - tw) / 2.0 - 8.0 * s, slot.y + slot.h / 2.0 - 11.0 * s, tw + 16.0 * s, 22.0 * s, 11.0 * s, [6, 8, 20, 200]);
                g.text(slot.x + (slot.w - tw) / 2.0, slot.y + slot.h / 2.0 - 11.0 * s + (22.0 * s - g.line_height(12.0 * s, Face::Bold)) / 2.0, 12.0 * s, CYAN, &t, Face::Bold);
            }
            (PicState::Ready, Some(b)) => {
                g.rrect(slot.x, slot.y, slot.w, slot.h, 12.0 * s, rgb(14, 18, 36));
                // the picture scaled to the size it is drawn at is kept (keyed by the picture, the slot and the scale), so a frame is a
                // plain copy: scaling a 1344x768 picture took milliseconds every frame
                let ident = (key, slot.w.round() as i32, slot.h.round() as i32, (s * 1000.0) as i32);
                let at = match self.pic_scaled.iter().position(|e| (e.0, e.1, e.2, e.3) == ident) {
                    Some(i) => Ok(i),
                    None => self.pic_cache.draw_bytes(g, key, &b, slot.w, s).and_then(|rd| {
                        let (pw, ph) = (rd.pm.width() as f32, rd.pm.height() as f32);
                        let k = (slot.w / pw).min(slot.h / ph);
                        let (iw, ih) = ((pw * k).round().max(1.0) as u32, (ph * k).round().max(1.0) as u32);
                        let mut scaled = tiny_skia::Pixmap::new(iw, ih).ok_or("the picture is too small to show")?;
                        let paint = tiny_skia::PixmapPaint { quality: tiny_skia::FilterQuality::Bilinear, ..Default::default() };
                        scaled.draw_pixmap(0, 0, rd.pm.as_ref().as_ref(), &paint, tiny_skia::Transform::from_scale(iw as f32 / pw, ih as f32 / ph), None);
                        if self.pic_scaled.len() >= 10 {
                            self.pic_scaled.remove(0);
                        }
                        self.pic_scaled.push((ident.0, ident.1, ident.2, ident.3, scaled));
                        Ok(self.pic_scaled.len() - 1)
                    }),
                };
                match at {
                    Ok(i) => {
                        let scaled = &self.pic_scaled[i].4;
                        let (dx, dy) = ((slot.x + (slot.w - scaled.width() as f32) / 2.0).round() as i32, (slot.y + (slot.h - scaled.height() as f32) / 2.0).round() as i32);
                        g.pm_mut().draw_pixmap(dx, dy, scaled.as_ref(), &tiny_skia::PixmapPaint::default(), tiny_skia::Transform::identity(), None);
                    }
                    Err(e) => {
                        let m = g.fit(12.0 * s, &format!("The picture cannot be shown: {e}"), Face::Sans, slot.w - 16.0 * s);
                        g.text(slot.x + 8.0 * s, slot.y + slot.h / 2.0, 12.0 * s, ERR, &m, Face::Sans);
                    }
                }
                g.rrect_outline(slot.x, slot.y, slot.w, slot.h, 12.0 * s, 1.0, with_alpha(ACCENT, 90));
            }
            _ => {
                cover(g, R::new(slot.x + 4.0 * s, slot.y + 4.0 * s, slot.w - 8.0 * s, slot.h - 8.0 * s), s, &title);
                let (label, col) = match &state {
                    PicState::Failed(e) => (format!("Could not draw this one: {e}"), ERR),
                    PicState::Working => (format!("Drawing{}", ".".repeat(1 + (tick / 10 % 3) as usize)), CYAN),
                    _ => ("Waiting to be drawn".to_string(), MUTED),
                };
                let t = g.fit(12.0 * s, &label, Face::Bold, slot.w - 24.0 * s);
                let tw = g.text_width(12.0 * s, &t, Face::Bold);
                g.rrect(slot.x + (slot.w - tw) / 2.0 - 8.0 * s, slot.y + 10.0 * s, tw + 16.0 * s, 22.0 * s, 11.0 * s, [6, 8, 20, 190]);
                g.text(slot.x + (slot.w - tw) / 2.0, slot.y + 10.0 * s + (22.0 * s - g.line_height(12.0 * s, Face::Bold)) / 2.0, 12.0 * s, col, &t, Face::Bold);
            }
        }
        // draw this one again
        if !matches!(state, PicState::Working) && self.gen.is_none() && idx < 16 {
            let (label, col) = if matches!(state, PicState::Failed(_)) { ("Try again", ACCENT) } else { ("Draw again", MUTED) };
            let bsz = 12.0 * s;
            let bw = g.text_width(bsz, label, Face::Bold) + bsz * 1.7;
            self.pic_buttons.push((R::new(slot.x + slot.w - bw - 8.0 * s, slot.y + slot.h - bsz * 2.1 - 8.0 * s, bw, bsz * 2.1), label, idx, col));
            let dw = g.text_width(bsz, "Delete", Face::Bold) + bsz * 1.7;
            self.pic_buttons.push((R::new(slot.x + slot.w - bw - dw - 14.0 * s, slot.y + slot.h - bsz * 2.1 - 8.0 * s, dw, bsz * 2.1), "Delete", idx + 100, ERR));
        }
        // links the writer's reply held: nothing was fetched; one button per link names the site, the person decides
        if matches!(state, PicState::Ready) && self.gen.is_none() && idx < 6 {
            let links = self.pics[idx].links.clone();
            for (k, u) in links.iter().take(3).enumerate() {
                let label = format!("Fetch the picture from {}", iyakki_scene::reply::host_of(u));
                self.gal_hits.push((R::new(slot.x + 8.0 * s, slot.y + 8.0 * s + k as f32 * 30.0 * s, 0.0, 0.0), T_LINK + idx * 3 + k, label));
            }
        }
        self.draw_gallery(g, slot, s, idx);
    }

}
