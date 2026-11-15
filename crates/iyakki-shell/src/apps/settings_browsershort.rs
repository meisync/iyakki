//! Settings -> Browser -> Short words: the person's own address-bar short words (`yt cats` opens a site's search for cats). Keys: `w`
//! adds or changes one (a form: the word and the address with `%s`), `W` removes one; the buttons sit beside the search engine row.
//! The same words can be typed in the browser's address bar (`short add`, `short remove`, `short list`) and are changed by the chat
//! assistant (`browser_short_words_*`, which asks). They are kept in the person's browser file; every change is audited by word only.

use super::*;

impl SettingsApp {
    /// What the search row says about the short words.
    pub(super) fn short_help(c: &super::super::browser_config::BrowserConfig) -> String {
        if c.short_words.is_empty() {
            "Short words of your own: none yet (w adds one, W removes one).".to_string()
        } else {
            format!("Your short words: {} (w adds or changes one, W removes one).", c.short_words.keys().take(8).cloned().collect::<Vec<_>>().join(", "))
        }
    }

    /// The width of the two buttons, or 0 when the window is too narrow to hold them (the keys still work and the text names them).
    pub(super) fn short_buttons_width(&self, g: &mut Gfx, a: R, s: f32) -> f32 {
        let w = g.text_width(15.0 * s, "Add short word (w)", Face::Bold) + g.text_width(15.0 * s, "Remove (W)", Face::Bold) + 2.0 * 15.0 * s * 1.7 + 12.0 * s;
        if a.w >= 900.0 * s { w } else { 0.0 }
    }

    pub(super) fn render_short_buttons(&mut self, g: &mut Gfx, right: f32, y: f32, s: f32, width: f32) {
        if width <= 0.0 {
            return;
        }
        let rw = g.text_width(15.0 * s, "Remove (W)", Face::Bold) + 15.0 * s * 1.7;
        g.button(right - rw, y, 15.0 * s, "Remove (W)", ACCENT, Event::Key(Key::Char('W')));
        let aw = g.text_width(15.0 * s, "Add short word (w)", Face::Bold) + 15.0 * s * 1.7;
        g.button(right - rw - 6.0 * s - aw, y, 15.0 * s, "Add short word (w)", ACCENT, Event::Key(Key::Char('w')));
    }

    /// `w` and `W`: the forms. True when the key was used.
    pub(super) fn handle_browser_short(&mut self, k: &Key, _cx: &AppCx) -> bool {
        match k {
            Key::Char('w') => {
                self.open_form(FormKind::BrowserShort(true), Form::new("Add a short word").text("Word (1 to 12 lowercase letters or digits)", "", "for example yt").text("Address with %s where the search words go", "", "https://www.youtube.com/results?search_query=%s"));
                true
            }
            Key::Char('W') => {
                self.open_form(FormKind::BrowserShort(false), Form::new("Remove a short word").text("Word", "", "one of your own words"));
                true
            }
            _ => false,
        }
    }

    pub(super) fn short_submit(&mut self, cx: &AppCx, add: bool, form: &Form) -> Result<String, String> {
        let mut c = super::super::browser_config::load(cx);
        let word = form.value(0).trim().to_ascii_lowercase();
        let what = if add {
            let template = form.value(1).trim().to_string();
            super::super::browser_config::valid_short_word(&word, &template)?;
            if c.short_words.len() >= iyakki_agent::browsershort::MAX_WORDS && !c.short_words.contains_key(&word) {
                return Err(format!("At most {} short words.", iyakki_agent::browsershort::MAX_WORDS));
            }
            c.short_words.insert(word.clone(), template);
            format!("Short word {word} saved.")
        } else {
            if c.short_words.remove(&word).is_none() {
                return Err(format!("You have no short word {word} (the built-in ones cannot be removed)."));
            }
            format!("Short word {word} removed.")
        };
        super::super::browser_config::save(cx, &c);
        cx.audit.record(&cx.session.user, "settings.browser", &what);
        Ok(what)
    }
}
