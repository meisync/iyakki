//! Settings -> Browser, "Speed and memory": how big the page cache may grow (Auto, Small, Large; the machine's free memory still
//! limits it), what it holds now, what it saved, and a button that empties it. The numbers are this person's own cache. The sizes
//! themselves are worked out in `browser_speed.rs` and applied by the browser.

use super::*;

impl SettingsApp {
    /// The section; returns the height it used.
    pub(super) fn render_browser_speed(&mut self, g: &mut Gfx, a: R, y0: f32, s: f32, cx: &AppCx) -> f32 {
        let c = super::super::browser_config::load(cx);
        // the page does not scroll: when the whole section does not fit under the rows above it, say where the keys are instead of
        // drawing buttons outside the window
        let (lines_n, need) = (3.0, 28.0 + 66.0 + 3.0 * 50.0 + 8.0);
        let _ = lines_n;
        if y0 + need * s > a.y + a.h - 4.0 * s {
            if y0 + 22.0 * s < a.y + a.h {
                g.text(a.x + 4.0 * s, y0, 13.0 * s, DIM, "Speed and memory: k changes the cache size, x clears the cache.", Face::Sans);
            }
            return 22.0 * s;
        }
        let mut y = y0 + controls::section_title(g, a.x, y0, s, "Speed and memory");
        let btn_w = 150.0 * s;
        let info_w = (a.w - btn_w - 10.0 * s).max(120.0 * s);
        let m = super::super::browser_speed::machine(cx.services.dir());
        let b = super::super::browser_speed::budget_for(c.cache, &m);
        let help = format!("The browser keeps pages, pictures, styles and scripts so the next visit is faster: up to {} in memory and {} on disk right now. It shrinks by itself when the computer is short of memory.", iyakki_web::net::human(b.mem), iyakki_web::net::human(b.disk));
        controls::info_row(g, R::new(a.x, y, info_w, 58.0 * s), s, &format!("Cache size: {}", c.cache.label()), &help);
        g.button(a.x + a.w - btn_w, y + 8.0 * s, 15.0 * s, "Change (k)", ACCENT, Event::Key(Key::Char('k')));
        y += 66.0 * s;
        let stats = cx.web.cache_stats(&cx.session.user);
        let lines = super::super::browser_speed::numbers(stats, iyakki_web::net::snapshot());
        for (title, text) in lines {
            controls::info_row(g, R::new(a.x, y, info_w, 44.0 * s), s, &title, &text);
            y += 50.0 * s;
        }
        g.button(a.x + a.w - btn_w, y - 44.0 * s, 15.0 * s, "Clear cache (x)", ACCENT, Event::Key(Key::Char('x')));
        y += 8.0 * s;
        y - y0
    }

    /// `k` changes the cache size, `x` empties the person's cache. True when the key was one of these.
    pub(super) fn handle_browser_speed(&mut self, k: &Key, cx: &AppCx) -> bool {
        match k {
            Key::Char('k') => {
                let mut c = super::super::browser_config::load(cx);
                c.cache = match c.cache {
                    iyakki_web::net::CacheSize::Auto => iyakki_web::net::CacheSize::Large,
                    iyakki_web::net::CacheSize::Large => iyakki_web::net::CacheSize::Small,
                    iyakki_web::net::CacheSize::Small => iyakki_web::net::CacheSize::Auto,
                };
                super::super::browser_config::save(cx, &c);
                super::super::browser_speed::refresh_now();
                let what = format!("Cache size: {}.", c.cache.label());
                cx.audit.record(&cx.session.user, "settings.browser", &what);
                self.ok(what);
                true
            }
            Key::Char('x') => {
                cx.web.clear_cache(&cx.session.user);
                cx.audit.record(&cx.session.user, "settings.browser", "Cleared the page cache");
                self.ok("The page cache is empty.".to_string());
                true
            }
            _ => false,
        }
    }
}
