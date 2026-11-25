//! Settings: the browser page code moved out of `settings.rs` unchanged (plan: docs/development/SETTINGS-EXTRACTION-PLAN.md, step S0).

use super::*;
use super::browser_port::BrowserSettingsPort;

impl SettingsApp {
    pub(super) fn handle_browser(&mut self, k: &Key, p: &dyn BrowserSettingsPort) {
        if self.handle_browser_headers(k, p) {
            return;
        }
        if self.browser_scroll_key(k) {
            return;
        }
        if self.handle_browser_short(k, p) || self.handle_browser_graphics(k, p) || self.handle_browser_speed(k, p) || self.handle_browser_block(k, p) || self.handle_browser_protect(k, p) || self.handle_browser_cert(k, p) || self.handle_browser_bad(k, p) || self.handle_browser_proxy(k, p) || self.handle_browser_media(k, p) || self.handle_browser_choices(k, p) || self.handle_browser_devices(k, p) || self.handle_browser_sites(k, p) {
            return;
        }
        let mut c = super::super::browser_config::load(p);
        let what = match k {
            Key::Char('a') => {
                c.js_all = !c.js_all;
                if c.js_all {
                    iyakki_js::warm_up();
                }
                format!("Scripts on every site: {}.", if c.js_all { "on" } else { "off" })
            }
            Key::Char('i') => {
                c.images = !c.images;
                format!("Pictures: {}.", if c.images { "on" } else { "off" })
            }
            Key::Char('s') => {
                c.styles = !c.styles;
                format!("Style sheets: {}.", if c.styles { "on" } else { "off" })
            }
            Key::Char('c') => {
                c.cookies = !c.cookies;
                format!("Cookies: {}.", if c.cookies { "on" } else { "off" })
            }
            Key::Char('m') => {
                c.smooth = !c.smooth;
                format!("Smooth pages: {}.", if c.smooth { "on" } else { "off" })
            }
            Key::Char('e') => {
                c.search = c.search.next();
                format!("Search engine: {}.", c.search.name())
            }
            Key::Char('d') => {
                let on = !p.devtools().enabled(p.acting_user());
                p.devtools().set_enabled(p.acting_user(), on);
                let what = format!("Developer tools for the chat agent: {}.", if on { "on" } else { "off" });
                p.audit("settings.browser", &what);
                self.ok(what);
                return;
            }
            Key::Char('f') => {
                c.js_sites.clear();
                "Forgot the sites allowed to run scripts.".to_string()
            }
            _ => return,
        };
        super::super::browser_config::save(p, &c);
        p.audit("settings.browser", &what);
        self.ok(what);
    }

    pub(super) fn render_browser(&mut self, g: &mut Gfx, a: R, s: f32, p: &dyn BrowserSettingsPort) {
        if self.bh.open {
            return self.render_browser_headers(g, a, s, p);
        }
        // the page is longer than a small window: it scrolls (wheel, finger, arrows, Page keys) and everything in it is cut to the
        // window, so nothing under the buttons is hit; the body is laid out with all the room it wants
        self.bh.max = self.bh.max.max(0.0);
        self.bh.scroll = self.bh.scroll.clamp_safe(0.0, self.bh.max);
        let top = a.y - self.bh.scroll;
        let tall = R::new(a.x, top, a.w, 20000.0 * s);
        self.bh.page_h = a.h;
        let end = g.clipped(a, |g| self.render_browser_body(g, tall, s, p));
        self.bh.max = (end - top + 12.0 * s - a.h).max(0.0);
    }

    /// The whole Browser page from the top of the rectangle down; returns where it ends.
    pub(super) fn render_browser_body(&mut self, g: &mut Gfx, a: R, s: f32, p: &dyn BrowserSettingsPort) -> f32 {
        let c = super::super::browser_config::load(p);
        let (gap, rh) = (10.0 * s, 70.0 * s);
        let two = a.w > 820.0 * s;
        let cw = if two { (a.w - gap) / 2.0 } else { a.w };
        let mut y = a.y + controls::section_title(g, a.x, a.y, s, "Pages");
        let rows: [(&str, &str, bool, char); 4] = [
            ("Run scripts on every site", "Pages can change themselves, load more content and answer clicks. Scripts run in a sandbox with time and memory limits. When this is off, scripts run only on the sites you allow (press j on a page).", c.js_all, 'a'),
            ("Show pictures", "PNG, JPEG and GIF pictures are fetched after the text.", c.images, 'i'),
            ("Apply style sheets", "Colours, sizes and layout from the page's own style sheets.", c.styles, 's'),
            ("Accept cookies", "Let sites remember you for this session (sign-ins, preferences). Off: none are sent or kept.", c.cookies, 'c'),
        ];
        for (i, (title, help, on, key)) in rows.into_iter().enumerate() {
            let x = if two { a.x + (i % 2) as f32 * (cw + gap) } else { a.x };
            controls::switch_row(g, R::new(x, y, cw, rh), s, title, help, on, true, Event::Key(Key::Char(key)), ACCENT);
            if !two || i % 2 == 1 {
                y += rh + gap;
            }
        }
        if two {
            // an even number of switches ends a row
        }
        y += 6.0 * s;
        // how the engine draws: near the top too, because it is the answer to a computer that stops when the browser starts
        y += self.render_browser_graphics(g, a, y, s, p);
        // only when a camera or microphone is marked "don't use": near the top, because the tab does not scroll
        y += self.render_browser_devices(g, a, y, s, p);
        y += controls::section_title(g, a.x, y, s, "Search");
        let ebw0 = g.text_width(15.0 * s, "Change (e)", Face::Bold) + 15.0 * s * 1.7;
        // the person's short words (`yt cats`): Add (w) and Remove (W) sit beside the search engine's button when the window is wide enough
        let short_w = self.short_buttons_width(g, a, s);
        controls::info_row(g, R::new(a.x, y, a.w - ebw0 - short_w - 10.0 * s, 44.0 * s), s, c.search.name(), &format!("words typed in the address bar are searched here (Google, Bing and DuckDuckGo ask for checks this browser cannot pass). {}", Self::short_help(&c)));
        let ebw = g.text_width(15.0 * s, "Change (e)", Face::Bold) + 15.0 * s * 1.7;
        g.button(a.x + a.w - ebw, y + 6.0 * s, 15.0 * s, "Change (e)", ACCENT, Event::Key(Key::Char('e')));
        self.render_short_buttons(g, a.x + a.w - ebw - 8.0 * s, y + 6.0 * s, s, short_w);
        y += 54.0 * s;
        y += controls::section_title(g, a.x, y, s, "The assistant in your browser");
        {
            let p = p.acting_policy();
            let (title, help) = match p.browser_agent {
                iyakki_sec::policy::BrowserAgent::Off => ("Off for your account".to_string(), "The assistant cannot use your browser. An administrator can change this in Users (press w).".to_string()),
                iyakki_sec::policy::BrowserAgent::ReadOnly => ("Read-only".to_string(), "The assistant can open pages in your browser, read them, take pictures and scroll, after you allow each site once. It cannot click or type. An administrator can allow more in Users (press w).".to_string()),
                iyakki_sec::policy::BrowserAgent::Interactive => ("Read, click and type, with your approval".to_string(), format!("The assistant asks you before it acts on a site: approve all actions on that site for the session, one by one, or deny. Payments, deletions and sign-in codes are always yours. At most {} actions and {} minutes per request.", p.browser_max_actions, p.browser_max_minutes)),
            };
            controls::info_row(g, R::new(a.x, y, a.w, 56.0 * s), s, &title, &help);
            y += 66.0 * s;
        }
        y += self.render_browser_block(g, a, y, s, p);
        y += self.render_browser_speed(g, a, y, s, p);
        y += self.render_browser_devtools(g, a, y, s, p);
        // Smooth pages: where there is room, a switch; on a small window only where the key is (the page does not scroll)
        if y + (rh + gap) <= a.y + a.h - 4.0 * s {
            controls::switch_row(g, R::new(a.x, y, a.w, rh), s, "Smooth pages (faster, softer)", "YouTube, Google and other big pages are drawn at half the width and height, a quarter of the pixels, so scrolling and video keep up on this computer. Text and pictures look softer. Off: sharp, but slower here.", c.smooth, true, Event::Key(Key::Char('m')), ACCENT);
            y += rh + gap;
        } else if y + 22.0 * s < a.y + a.h {
            g.text(a.x + 4.0 * s, y, 13.0 * s, DIM, &format!("Smooth pages (faster, softer): {}. Press m to change.", if c.smooth { "on" } else { "off" }), Face::Sans);
            y += 22.0 * s;
        }
        y += self.render_browser_protect(g, a, y, s, p);
        y += self.render_browser_cert(g, a, y, s, p);
        y += self.render_browser_bad(g, a, y, s, p);
        y += self.render_browser_proxy(g, a, y, s, p);
        y += self.render_browser_media(g, a, y, s, p);
        y += self.render_browser_choices(g, a, y, s, p);
        if y + 60.0 * s > a.y + a.h {
            return y;
        }
        y += controls::section_title(g, a.x, y, s, "Sites allowed to run scripts");
        if c.js_sites.is_empty() {
            g.text(a.x + 4.0 * s, y, 14.0 * s, DIM, if c.js_all { "None needed: scripts run everywhere." } else { "None yet. Press j on a page in the browser to allow one." }, Face::Sans);
            y += 26.0 * s;
        }
        for h in c.js_sites.iter().take(8) {
            if y + 38.0 * s > a.y + a.h {
                break;
            }
            controls::info_row(g, R::new(a.x, y, a.w, 38.0 * s), s, h, "scripts allowed");
            y += 44.0 * s;
        }
        // at the end of the page, so the switches and lists above keep their places: the sites allowed for camera, microphone and screen,
        // and the audio and video add-on
        y += self.render_browser_sites(g, a, y, s, p);
        y += self.render_browser_addon(g, a, y, s, p);
        y
    }
}
