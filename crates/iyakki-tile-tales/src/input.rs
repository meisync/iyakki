//! TALES, Polling, clicks and keys.
use super::*;

impl StoryApp {
    pub fn apply_quick(&mut self, i: usize, port: &dyn TalesPort) {
        let Some(q) = self.quick.get(i).cloned() else { return };
        let r = port.set_speech_engine(q.engine.clone());
        match r {
            Ok(()) => {
                port.audit("story.engine", &q.label);
                self.note = Some((format!("{} is now the voice for Text to speech (Settings → Voice).", q.label.trim_start_matches("Use ")), true));
            }
            Err(e) => self.note = Some((format!("Could not change the voice settings: {e}"), false)),
        }
        self.refresh_tts(port);
    }

    // ------------------------------------------------------------------------------------------------ background results

    pub fn poll(&mut self, port: &dyn TalesPort) {
        self.poll_turn_on(port);
        self.poll_writing(port);
        self.poll_pictures(port);
        self.maybe_autostart(port);
        if let Some(j) = &self.audio_job {
            if let Ok(r) = j.rx.try_recv() {
                self.audio_job = None;
                match r {
                    Ok(p) => {
                        self.note = Some((format!("Audio saved: {p}"), true));
                        if let Some(e) = self.saved.clone() {
                            if let Ok(d) = self.store.load(&e.id) {
                                self.saved = Some(d.entry);
                            }
                        }
                    }
                    Err(e) if e == "cancelled" => self.note = Some(("Saving the audio was cancelled.".into(), false)),
                    Err(e) => self.note = Some((format!("The audio could not be saved: {e}"), false)),
                }
            }
        }
        if self.ticks % 45 == 0 {
            self.refresh_tts(port);
        }
        let target = if matches!(self.reader.status(), Status::Playing) { self.reader.level() } else { 0.0 };
        self.level = if target > self.level { target } else { self.level * 0.8 };
        let now = self.reader.status();
        if now != self.last_status {
            self.last_status = now;
            self.dirty = true;
        }
        if let Status::Failed(m) = self.reader.status() {
            if self.error.as_deref() != Some(m.as_str()) {
                self.error = Some(m);
            }
        }
    }

    pub fn animating(&self) -> bool {
        self.gen.is_some() || self.audio_job.is_some() || self.level > 0.01 || self.pics.iter().any(|p| matches!(p.state, PicState::Working | PicState::Queued)) || self.alt_running.is_some() || !self.alt_jobs.is_empty() || matches!(self.reader.status(), Status::Playing | Status::Buffering)
    }

    // ------------------------------------------------------------------------------------------------ clicks and keys

    pub fn click(&mut self, n: usize, port: &dyn TalesPort) {
        match n {
            T_OPTIONS => {
                if self.wide {
                    self.set_focus(Focus::Idea);
                } else {
                    self.pane_open = !self.pane_open;
                    self.focus = if self.pane_open { Focus::Idea } else { Focus::None };
                }
            }
            T_STORY_PAGE => self.go(Page::Story, port),
            T_LIB_PAGE => self.go(Page::Library, port),
            n if (T_AUDIENCE..T_AUDIENCE + Audience::ALL.len()).contains(&n) => {
                self.opts.audience = Audience::ALL[n - T_AUDIENCE];
                self.save_prefs();
            }
            n if (T_GENRE..T_GENRE + Genre::ALL.len()).contains(&n) => {
                self.opts.genre = Genre::ALL[n - T_GENRE];
                self.explicit[H_GENRE] = true;
                self.save_prefs();
                if self.menu == Some(Menu::Kind) {
                    self.close_menu();
                }
            }
            n if (T_LENGTH..T_LENGTH + Length::ALL.len()).contains(&n) => {
                self.opts.length = Length::ALL[n - T_LENGTH];
                self.explicit[H_LEN] = true;
                self.save_prefs();
            }
            n if (T_LANG..T_LANG + LANGUAGES.len()).contains(&n) => {
                self.opts.language = LANGUAGES[n - T_LANG].to_string();
                self.explicit[H_LANG] = true;
                self.save_prefs();
                if self.menu == Some(Menu::Language) {
                    self.close_menu();
                }
            }
            n if (T_TONE..T_TONE + Tone::ALL.len()).contains(&n) => {
                self.opts.tone = Tone::ALL[n - T_TONE];
                self.explicit[H_TONE] = true;
                self.save_prefs();
                if self.menu == Some(Menu::Tone) {
                    self.close_menu();
                }
            }
            T_KIND_MENU => self.toggle_menu(Menu::Kind, port),
            T_TONE_MENU => self.toggle_menu(Menu::Tone, port),
            T_OPTIONS_MENU => self.toggle_menu(Menu::Options, port),
            T_LANG_MENU => self.toggle_menu(Menu::Language, port),
            T_VERSION_MENU => self.toggle_menu(Menu::Version, port),
            n if (T_VERSION_PICK + 1..T_VERSION_PICK + 60).contains(&n) => {
                self.menu = None;
                self.switch_version((n - T_VERSION_PICK) as u32, port);
            }
            T_HOW => self.toggle_menu(Menu::How, port),
            T_TURN_ON => self.turn_on_reading(port),
            T_MENU_CLOSE => self.close_menu(),
            n if (T_HINT..T_HINT + 5).contains(&n) => {
                // the person removed a suggestion: it is not used (and not shown) until the words change
                let k = n - T_HINT;
                if let Some(c) = self.hint_chips().into_iter().find(|c| c.k == k) {
                    self.dismissed[k] = Some(c.said);
                }
                self.set_focus(Focus::Idea);
            }
            // the one switch for pictures: off, or automatic (images when a picture model exists, else SVG drawings)
            T_PICMODE => {
                self.opts.pictures = if self.opts.pictures == PictureMode::Off { PictureMode::Auto } else { PictureMode::Off };
                self.save_prefs();
            }
            // which scenes get a picture: every chapter, the cover only, or ask each time (chosen before Send, kept with the story)
            T_COVERAGE => {
                self.opts.coverage = self.opts.coverage.next();
                self.save_prefs();
            }
            n if (T_STYLE..T_STYLE + illus::Look::ALL.len()).contains(&n) => {
                let l = illus::Look::ALL[n - T_STYLE];
                self.set_pic(move |o| o.toggle_style(l));
            }
            T_PERSTYLE => self.set_pic(|o| o.next_count()),
            T_REALISTIC => {
                let on = !self.pic_now().is_realistic();
                self.set_pic(move |o| o.set_realistic(on));
            }
            T_REAL_INSTALL => {
                let pic = self.pic_now();
                let cands = port.picture_choices(&self.found_images(port));
                if let Some((p, m, size)) = illus::realism_info(&pic, self.prefs.picture_model.as_ref(), &cands, None).install {
                    self.install = Some(Install { provider: p, model: m, size, started: false });
                    self.start_install(port);
                }
            }
            n if (T_CHOICE_INSTALL..T_CHOICE_INSTALL + 10).contains(&n) => {
                let cands = port.picture_choices(&self.found_images(port));
                if let Some(c) = cands.get(n - T_CHOICE_INSTALL).filter(|c| !c.downloaded) {
                    let size = c.line().split(", ").nth(1).map_or("size unknown".to_string(), str::to_string);
                    self.install = Some(Install { provider: c.provider.clone(), model: c.model.clone(), size, started: false });
                    self.start_install(port);
                }
            }
            T_KEEP_ONLY => self.keep_only_chosen(),
            n if (T_ALT_PICK..T_ALT_PICK + 6 * 24).contains(&n) => self.choose_main((n - T_ALT_PICK) / 24, (n - T_ALT_PICK) % 24),
            n if (T_ALT_DEL..T_ALT_DEL + 6 * 24).contains(&n) => self.delete_alt((n - T_ALT_DEL) / 24, (n - T_ALT_DEL) % 24),
            n if (T_ALT_MORE..T_ALT_MORE + 6).contains(&n) => self.more_alt(n - T_ALT_MORE, port),
            n if (T_LINK..T_LINK + 18).contains(&n) => {
                let (i, k) = ((n - T_LINK) / 3, (n - T_LINK) % 3);
                if let Some(u) = self.pics.get(i).and_then(|p| p.links.get(k)).cloned() {
                    self.modal = Some(Modal::FetchLink(i, u));
                }
            }
            T_QUALITY => self.set_pic(|o| o.quality = o.quality.next()),
            T_KEEPLOOK => self.set_pic(|o| o.keep_look = !o.keep_look),
            T_STORYMODEL => self.cycle_story_model(port),
            T_PICLOG => self.open_pic_log(),
            T_PIC_INSTALL => self.start_install(port),
            T_TIMEOUT => self.set_pic(|o| o.timeout_min = match o.limit_min() { 5 => 15, 15 => 30, 30 => 60, _ => 5 }),
            T_LOG_VERBOSE => self.set_pic(|o| o.verbose = !o.verbose),
            T_LOG_CLOSE => self.modal = None,
            T_LOG_COPY => {
                let t = piclog::log_text(&self.pic_log_events());
                if t.is_empty() {
                    self.note = Some(("The picture log is empty.".into(), false));
                } else {
                    iyakki_uikit::ui::clipboard::copy(&t);
                    self.note = Some(("The picture log is copied.".into(), true));
                }
            }
            T_LOG_CLEAR => {
                if let Some(Modal::PicLog(v)) = self.modal.as_mut() {
                    if v.confirm_clear {
                        v.confirm_clear = false;
                        v.top.set(0);
                        if let Some(id) = self.saved_id() {
                            self.store.clear_picture_log(&id);
                        }
                        self.pic_pending.clear();
                        self.note = Some(("The picture log was cleared.".into(), true));
                    } else {
                        v.confirm_clear = true;
                    }
                }
            }
            T_PICS_GO => {
                self.held = false;
                let queued: Vec<usize> = (0..self.pics.len()).filter(|&i| self.pics[i].state == PicState::Queued).collect();
                self.push_draw_calls(&queued);
                self.pump_pictures(port);
            }
            T_PICS_SKIP => {
                self.held = false;
                self.alt_jobs.clear();
                self.pics.retain(|p| p.state == PicState::Ready);
                self.pic_gen += 1;
                self.persist_pictures();
                self.note = Some(("No pictures for this story.".into(), true));
            }
            T_PICS_CANCEL => self.cancel_pictures(),
            // how drawings are made: automatic (by the model), a rich SVG by the model, or the built-in cartoon
            T_DRAWMODE => {
                let all = st::DrawingMode::ALL;
                let at = all.iter().position(|m| *m == self.opts.drawing).unwrap_or(0);
                self.opts.drawing = all[(at + 1) % all.len()];
                self.save_prefs();
            }
            // short video clips as pictures: off unless the person allows them (they cost time and money)
            T_VIDEO => {
                self.opts.video = !self.opts.video;
                self.save_prefs();
            }
            T_READ_WHILE => {
                self.prefs.read_while_writing = !self.prefs.read_while_writing;
                self.save_prefs();
            }
            T_AUTOSAVE => {
                self.prefs.auto_save = !self.prefs.auto_save;
                self.save_prefs();
                if self.prefs.auto_save && self.saved.is_none() && self.gen.is_none() && !self.story.blocks.is_empty() {
                    self.save_story(port, "");
                }
            }
            T_CREATE => self.submit(port),
            T_MODEL => self.open_model_pick(false, port),
            T_PICMODEL => self.open_model_pick(true, port),
            n if (T_QUICK..T_QUICK + 6).contains(&n) => self.apply_quick(n - T_QUICK, port),
            n if (T_SUGGEST..T_SUGGEST + SUGGESTIONS.len()).contains(&n) => {
                self.idea.set(SUGGESTIONS[n - T_SUGGEST]);
                self.sync_fields();
                self.menu = None;
                self.set_focus(Focus::Idea);
            }
            T_PLAY => self.play_pause(port),
            T_STOP => self.stop_reading(),
            T_PREV => self.step_unit(-1, port),
            T_NEXT => self.step_unit(1, port),
            T_REPLAY => self.seek(0, port),
            T_VOICE => self.cycle_voice(port),
            T_SPEED => self.cycle_speed(true, port),
            T_SAVE_AUDIO => self.save_audio(port),
            T_FAV => {
                if let Some(id) = self.saved_id() {
                    self.toggle_fav(&id);
                } else {
                    self.note = Some(("Save the story first, then mark it as a favourite.".into(), false));
                }
            }
            T_COPY => self.copy_story(),
            T_SAVE => self.save_story(port, ""),
            T_EXPORT => self.ask_export(None),
            T_PDF => self.save_pdf(port, None),
            T_NEW => self.new_story(port),
            T_STOP_WRITING => self.stop_writing(),
            T_CANCEL_AUDIO => {
                if let Some(j) = &self.audio_job {
                    j.cancel.store(true, Ordering::SeqCst);
                }
            }
            T_LIB_OPEN | T_LIB_LISTEN => {
                if let Some(id) = self.lib.get(self.lib_sel).map(|e| e.id.clone()) {
                    self.open_saved(&id, n == T_LIB_LISTEN, port);
                }
            }
            T_LIB_FAV => {
                if let Some(id) = self.lib.get(self.lib_sel).map(|e| e.id.clone()) {
                    self.toggle_fav(&id);
                }
            }
            T_LIB_DELETE => {
                if let Some(e) = self.lib.get(self.lib_sel) {
                    self.modal = Some(Modal::ConfirmDelete(e.id.clone(), e.title.clone()));
                }
            }
            T_LIB_EXPORT => {
                let id = self.lib.get(self.lib_sel).map(|e| e.id.clone());
                if id.is_some() {
                    self.ask_export(id);
                }
            }
            n if (T_PIC_REDO..T_PIC_REDO + 16).contains(&n) => self.redo_picture(n - T_PIC_REDO, port),
            n if (T_PIC_DELETE..T_PIC_DELETE + 16).contains(&n) => self.delete_picture(n - T_PIC_DELETE),
            _ => {}
        }
    }

    pub fn next_focus(&mut self, back: bool) {
        let order: &[Focus] = match self.page {
            Page::Story => &[Focus::Idea, Focus::None],
            Page::Library => &[Focus::Search, Focus::None],
        };
        let i = order.iter().position(|f| *f == self.focus).unwrap_or(order.len() - 1);
        let n = order.len();
        self.set_focus(order[if back { (i + n - 1) % n } else { (i + 1) % n }]);
    }

    pub fn scroll_by(&mut self, delta: f32) {
        match self.page {
            Page::Story if self.menu == Some(Menu::Options) => self.cscroll = (self.cscroll + delta).clamp_safe(0.0, self.cmax),
            Page::Story => {
                self.tscroll = (self.tscroll + delta).clamp_safe(0.0, self.tmax);
                self.stick_bottom = self.tscroll >= self.tmax - 2.0;
                self.manual_unit = Some(self.reader.snapshot().unit);
            }
            Page::Library => {
                let d = if delta > 0.0 { 1 } else { -1 };
                self.lib_sel = (self.lib_sel as i32 + d).clamp_safe(0, self.lib.len().saturating_sub(1) as i32) as usize;
            }
        }
    }

    pub fn key(&mut self, k: &Key, port: &dyn TalesPort) -> Flow {
        if matches!(self.modal, Some(Modal::PicLog(_))) {
            let page = self.modal.as_ref().map_or(1, |m| if let Modal::PicLog(v) = m { v.rows.get().max(2) as i64 - 1 } else { 1 });
            match k {
                Key::Esc | Key::Char('q') | Key::Char('g') => self.modal = None,
                Key::Up => self.log_scroll(-1),
                Key::Down => self.log_scroll(1),
                Key::PageUp => self.log_scroll(-page),
                Key::PageDown => self.log_scroll(page),
                Key::Home => self.log_scroll(i64::MIN / 2),
                Key::End => self.log_scroll(i64::MAX / 2),
                Key::Char('c') | Key::Ctrl('c') => self.click(T_LOG_COPY, port),
                Key::Char('x') => self.click(T_LOG_CLEAR, port),
                Key::Char('v') => self.click(T_LOG_VERBOSE, port),
                _ => {}
            }
            if !matches!(k, Key::Char('x')) {
                if let Some(Modal::PicLog(v)) = self.modal.as_mut() {
                    v.confirm_clear = false;
                }
            }
            return Flow::Stay;
        }
        if let Some(Modal::FetchLink(idx, url)) = &self.modal {
            let (idx, url) = (*idx, url.clone());
            self.modal = None;
            if *k == Key::Char('y') {
                self.fetch_link(idx, &url, port);
            }
            return Flow::Stay;
        }
        if let Some(Modal::ConfirmDelete(id, _)) = &self.modal {
            let id = id.clone();
            if *k == Key::Char('y') {
                self.delete_saved(&id, port);
            }
            self.modal = None;
            return Flow::Stay;
        }
        if matches!(self.modal, Some(Modal::Model(_))) {
            self.model_event(&Event::Key(k.clone()), port);
            return Flow::Stay;
        }
        if self.page == Page::Story && self.modal.is_none() {
            if let Some(m) = self.menu {
                if k == &Key::Esc {
                    self.close_menu();
                    return Flow::Stay;
                }
                if matches!(m, Menu::Kind | Menu::Tone | Menu::Language | Menu::Version) {
                    let n = match m {
                        Menu::Kind => Genre::ALL.len(),
                        Menu::Tone => Tone::ALL.len(),
                        Menu::Language => LANGUAGES.len(),
                        Menu::Version => self.ver_count as usize,
                        _ => 0,
                    };
                    match k {
                        Key::Up => self.menu_sel = self.menu_sel.saturating_sub(1),
                        Key::Down => self.menu_sel = (self.menu_sel + 1).min(n.saturating_sub(1)),
                        Key::Home => self.menu_sel = 0,
                        Key::End => self.menu_sel = n.saturating_sub(1),
                        Key::Enter => {
                            let code = match m {
                                Menu::Kind => T_GENRE + self.menu_sel,
                                Menu::Tone => T_TONE + self.menu_sel,
                                Menu::Language => T_LANG + self.menu_sel,
                                Menu::Version => T_VERSION_PICK + self.menu_sel + 1,
                                _ => T_MENU_CLOSE,
                            };
                            self.click(code, port);
                        }
                        _ => {}
                    }
                    return Flow::Stay;
                }
                if m == Menu::How {
                    return Flow::Stay;
                }
            }
        }
        if matches!(k, Key::Ctrl('p') | Key::Ctrl('P')) && self.page == Page::Story {
            self.open_model_pick(false, port);
            return Flow::Stay;
        }
        if self.page == Page::Story && matches!(k, Key::Ctrl('g') | Key::Ctrl('G')) {
            // the agents are in the same list now: Ctrl+G opens it too
            self.open_model_pick(false, port);
            return Flow::Stay;
        }
        if self.page == Page::Story && matches!(k, Key::Ctrl('o') | Key::Ctrl('O')) {
            self.toggle_menu(Menu::Options, port);
            return Flow::Stay;
        }
        if matches!(self.modal, Some(Modal::Export(..))) {
            let ev = Event::Key(k.clone());
            if let Some(Modal::Export(p, id)) = self.modal.as_mut() {
                let r = p.handle_event(&ev);
                let id = id.clone();
                match r {
                    PickEvent::Chosen(label) => {
                        self.modal = None;
                        let what = if label.starts_with("Page with every") { 3 } else if label.starts_with("Page") { 0 } else if label.starts_with("Text") { 1 } else { 2 };
                        self.do_export(what, id);
                    }
                    PickEvent::Cancel => self.modal = None,
                    PickEvent::None => {}
                }
            }
            return Flow::Stay;
        }
        if self.focus != Focus::None {
            // Ctrl+N (or Ctrl+Enter) creates from anywhere in the form
            if matches!(k, Key::Ctrl('n') | Key::Ctrl('N') | Key::Ctrl('\n') | Key::Ctrl('\r')) && self.page == Page::Story {
                self.submit(port);
                return Flow::Stay;
            }
            match k {
                Key::Esc => self.focus = Focus::None,
                Key::Tab => self.next_focus(false),
                Key::BackTab => self.next_focus(true),
                // Enter sends, as in the chat; Ctrl+J makes a new line
                Key::Ctrl('j') | Key::Ctrl('J') if self.focus == Focus::Idea => {
                    self.idea.key(&Key::Enter);
                    self.sync_fields();
                }
                Key::Enter => match self.focus {
                    Focus::Search => {
                        if let Some(id) = self.lib.get(self.lib_sel).map(|e| e.id.clone()) {
                            self.open_saved(&id, false, port);
                        }
                    }
                    _ => self.submit(port),
                },
                Key::Up | Key::Down | Key::PageUp | Key::PageDown if self.page == Page::Library => {
                    let d = if matches!(k, Key::Up | Key::PageUp) { -1 } else { 1 };
                    self.lib_sel = (self.lib_sel as i32 + d).clamp_safe(0, self.lib.len().saturating_sub(1) as i32) as usize;
                }
                Key::PageUp => self.cscroll = (self.cscroll - 120.0).max(0.0),
                Key::PageDown => self.cscroll = (self.cscroll + 120.0).min(self.cmax),
                _ => {
                    let was = self.focus;
                    match self.focus {
                        Focus::Idea => {
                            self.idea.key(k);
                        }
                        Focus::Search => {
                            self.search.handle(k);
                        }
                        Focus::None => {}
                    }
                    self.sync_fields();
                    if was == Focus::Search {
                        self.reload_library();
                    }
                }
            }
            return Flow::Stay;
        }
        let ch = if let Key::Char(c) = k { Some(*c) } else { None };
        match k {
            Key::Esc => return Flow::Back,
            Key::Tab => {
                self.next_focus(false);
                return Flow::Stay;
            }
            Key::BackTab => {
                self.next_focus(true);
                return Flow::Stay;
            }
            Key::Ctrl('a') | Key::Ctrl('A') if self.page == Page::Story => {
                self.pane.select_all();
                return Flow::Stay;
            }
            Key::Ctrl('c') | Key::Ctrl('C') if self.page == Page::Story => {
                self.copy_story();
                return Flow::Stay;
            }
            Key::Ctrl('s') | Key::Ctrl('S') if self.page == Page::Story => {
                self.save_story(port, "");
                return Flow::Stay;
            }
            Key::Ctrl('n') | Key::Ctrl('N') if self.page == Page::Story => {
                self.submit(port);
                return Flow::Stay;
            }
            _ => {}
        }
        match ch {
            Some('1') => {
                self.open_form(port);
                return Flow::Stay;
            }
            Some('2') => return self.go_ret(Page::Story, port),
            Some('3') => return self.go_ret(Page::Library, port),
            _ => {}
        }
        match self.page {
            Page::Story => match (k, ch) {
                (_, Some(' ')) | (_, Some('p')) => self.play_pause(port),
                (_, Some('s')) => self.stop_reading(),
                (_, Some('r')) => self.seek(0, port),
                (Key::Left, _) | (_, Some('[')) => self.step_unit(-1, port),
                (Key::Right, _) | (_, Some(']')) => self.step_unit(1, port),
                (_, Some('f')) => self.click(T_FAV, port),
                (_, Some('c')) => self.copy_story(),
                (_, Some('e')) => self.ask_export(None),
                (_, Some('d')) => self.click(T_PDF, port),
                (_, Some('w')) => self.save_audio(port),
                (_, Some('v')) => self.cycle_voice(port),
                (_, Some('+')) | (_, Some('=')) => self.cycle_speed(true, port),
                (_, Some('-')) | (_, Some('_')) => self.cycle_speed(false, port),
                (_, Some('n')) => self.open_form(port),
                (_, Some('l')) => self.go(Page::Library, port),
                (_, Some('o')) => self.click(T_OPTIONS, port),
                (_, Some('g')) => self.click(T_PICLOG, port),
                (Key::Up, _) => self.scroll_by(-40.0),
                (Key::Down, _) => self.scroll_by(40.0),
                (Key::PageUp, _) => self.scroll_by(-240.0),
                (Key::PageDown, _) => self.scroll_by(240.0),
                (Key::Home, _) => self.scroll_by(-1e6),
                (Key::End, _) => self.scroll_by(1e6),
                _ => {}
            },
            Page::Library => match (k, ch) {
                (Key::Up, _) => self.lib_sel = self.lib_sel.saturating_sub(1),
                (Key::Down, _) => self.lib_sel = (self.lib_sel + 1).min(self.lib.len().saturating_sub(1)),
                (Key::Enter, _) => self.click(T_LIB_OPEN, port),
                (_, Some(' ')) | (_, Some('l')) => self.click(T_LIB_LISTEN, port),
                (_, Some('f')) => self.click(T_LIB_FAV, port),
                (_, Some('d')) => self.click(T_LIB_DELETE, port),
                (_, Some('e')) => self.click(T_LIB_EXPORT, port),
                (_, Some('n')) => self.open_form(port),
                (_, Some('/')) => self.set_focus(Focus::Search),
                _ => {}
            },
        }
        Flow::Stay
    }

    pub fn go_ret(&mut self, p: Page, port: &dyn TalesPort) -> Flow {
        self.go(p, port);
        Flow::Stay
    }
}
