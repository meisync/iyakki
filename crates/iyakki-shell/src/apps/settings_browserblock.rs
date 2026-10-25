//! Settings -> Browser, "Ads and trackers": the switch, the lists the person has downloaded (each with its licence, hash and age),
//! and the buttons that fetch, update, add and remove lists. Nothing is downloaded until a button is pressed. The page does not
//! scroll: when the section does not fit under the rows above it, one line names the keys instead of drawing buttons off the
//! screen. The switch (key b) is the first row of the section.

use super::*;
use super::super::browser_block as bb;

const KEYS_HINT: &str = "Ads and trackers: b switches blocking, l downloads EasyList and EasyPrivacy, u adds a list address, o a list file, n updates, r removes the lists.";

impl SettingsApp {
    /// The section; returns the height it used.
    pub(super) fn render_browser_block(&mut self, g: &mut Gfx, a: R, y0: f32, s: f32, cx: &AppCx) -> f32 {
        let store = bb::store(cx);
        let lists = store.list();
        let c = bb::load(cx);
        let rows = lists.len().min(3);
        let row_h = 64.0 * s;
        let need = 28.0 + 80.0 + (rows.max(1) as f32) * 70.0 + 56.0 + 24.0;
        if y0 + need * s > a.y + a.h - 4.0 * s {
            if y0 + 22.0 * s < a.y + a.h {
                let hint = g.fit(13.0 * s, KEYS_HINT, Face::Sans, a.w - 8.0 * s);
                g.text(a.x + 4.0 * s, y0, 13.0 * s, DIM, &hint, Face::Sans);
            }
            return 22.0 * s;
        }
        let mut y = y0 + controls::section_title(g, a.x, y0, s, "Ads and trackers");
        let (blocked, checked) = iyakki_web::blocker_state::Filter::global().totals();
        let b = iyakki_web::blocker_state::Filter::global().blocker();
        let st = b.stats();
        let state = if !c.enabled { "Blocking is off".to_string() } else if lists.is_empty() { "On, but no list yet: nothing is blocked".to_string() } else { format!("On: {} rules, {} blocked so far", st.blocking + st.exceptions + st.cosmetic, blocked) };
        let help = format!("{state}. Requests your lists name are not sent, in the reader and in the full web engine. Lists are other people's work with their own licences, downloaded only when you press a button. A page is never blocked silently; on a page, a allows or stops allowing one site. {checked} requests checked since start.");
        controls::switch_row(g, R::new(a.x, y, a.w, 70.0 * s), s, "Block ads and trackers", &help, c.enabled, true, Event::Key(Key::Char('b')), ACCENT);
        y += 80.0 * s;
        if lists.is_empty() {
            let l1 = g.fit(13.0 * s, "None downloaded yet. Press l to fetch EasyList and EasyPrivacy from easylist.to.", Face::Sans, a.w - 8.0 * s);
            g.text(a.x + 4.0 * s, y + 4.0 * s, 13.0 * s, MUTED, &l1, Face::Sans);
            let licence = iyakki_web::blocker_lists::KNOWN.first().map(|k| k.licence).unwrap_or("");
            let l2 = g.fit(13.0 * s, &format!("Both are free under {licence}; nothing is shipped with the system."), Face::Sans, a.w - 8.0 * s);
            g.text(a.x + 4.0 * s, y + 24.0 * s, 13.0 * s, DIM, &l2, Face::Sans);
            y += 70.0 * s;
        }
        let now = bb::now_secs();
        for m in lists.iter().take(3) {
            let r = R::new(a.x, y, a.w, row_h);
            crate::fx::glass(g, r.x, r.y, r.w, r.h, 14.0 * s, crate::fx::CYAN);
            let name = g.fit(15.0 * s, &format!("{}{}", m.name, if m.enabled { "" } else { " (off)" }), Face::Bold, r.w - 30.0 * s);
            g.text(r.x + 16.0 * s, r.y + 6.0 * s, 15.0 * s, TEXT, &name, Face::Bold);
            let l2 = format!("{} rules · {} · sha256 {}…", m.rules, bb::ago(m.updated, now), &m.hash[..m.hash.len().min(12)]);
            let l2 = g.fit(13.0 * s, &l2, Face::Sans, r.w - 30.0 * s);
            g.text(r.x + 16.0 * s, r.y + 26.0 * s, 13.0 * s, MUTED, &l2, Face::Sans);
            let src = iyakki_sec::url_host(&m.url).unwrap_or_else(|| "a file you chose".into());
            let l3 = g.fit(13.0 * s, &format!("Licence: {} · from {src}", m.licence), Face::Sans, r.w - 30.0 * s);
            g.text(r.x + 16.0 * s, r.y + 44.0 * s, 13.0 * s, DIM, &l3, Face::Sans);
            y += 70.0 * s;
        }
        // buttons, wrapped to the width
        let labels = [("Download lists (l)", 'l'), ("Update (n)", 'n'), ("Add address (u)", 'u'), ("Add file (o)", 'o'), ("Remove lists (r)", 'r')];
        let (mut x, size) = (a.x, 14.0 * s);
        let mut by = y;
        for (label, key) in labels {
            let w = g.text_width(size, label, Face::Bold) + size * 1.7;
            if x + w > a.x + a.w && x > a.x {
                x = a.x;
                by += size * 2.1 + 6.0 * s;
            }
            let bw = g.button(x, by, size, label, ACCENT, Event::Key(Key::Char(key)));
            x += bw + 8.0 * s;
        }
        y = by + size * 2.1 + 6.0 * s;
        let job = bb::job_status();
        if !job.is_empty() {
            let t = g.fit(13.0 * s, &job, Face::Sans, a.w - 8.0 * s);
            g.text(a.x + 4.0 * s, y, 13.0 * s, MUTED, &t, Face::Sans);
            y += 20.0 * s;
        }
        y - y0
    }

    /// Keys of the section; true when `k` was one of them.
    pub(super) fn handle_browser_block(&mut self, k: &Key, cx: &AppCx) -> bool {
        match k {
            Key::Char('b') => {
                let mut c = bb::load(cx);
                c.enabled = !c.enabled;
                bb::save(cx, &c);
                bb::apply(cx);
                let what = format!("Block ads and trackers: {}.", if c.enabled { "on" } else { "off" });
                cx.audit.record(&cx.session.user, "settings.browser", &what);
                self.ok(what);
                true
            }
            Key::Char('l') => {
                let _ = self.start_block_job(cx, bb::Job::DownloadStandard, "Downloading EasyList and EasyPrivacy from easylist.to…");
                true
            }
            Key::Char('n') => {
                let _ = self.start_block_job(cx, bb::Job::UpdateAll, "Updating the lists…");
                true
            }
            Key::Char('u') => {
                let f = Form::new("Add a filter list").text("Address (https://…)", "", "https://example.org/list.txt");
                self.open_form(FormKind::BlockListUrl, f);
                true
            }
            Key::Char('o') => {
                let f = Form::new("Add a filter list file").path("File path", "", "/path/to/list.txt", crate::filepicker::PathKind::File, &["txt", "list"]);
                self.open_form(FormKind::BlockListFile, f);
                true
            }
            Key::Char('r') => {
                let store = bb::store(cx);
                let n = store.list().len();
                for m in store.list() {
                    store.remove(&m.id);
                }
                bb::forget_loaded();
                bb::apply(cx);
                cx.audit.record(&cx.session.user, "settings.browser", "Removed the filter lists");
                self.ok(if n == 0 { "There were no lists.".to_string() } else { format!("Removed {n} list{}.", if n == 1 { "" } else { "s" }) });
                true
            }
            _ => false,
        }
    }

    fn start_block_job(&mut self, cx: &AppCx, job: bb::Job, started: &str) -> bool {
        let what = format!("Filter lists: {started}");
        cx.audit.record(&cx.session.user, "settings.browser", &what);
        if bb::start_job(bb::lists_dir(cx), bb::source(cx), job) {
            self.ok(started.to_string());
            true
        } else {
            self.fail("A list job is already running.");
            false
        }
    }

    /// A form of this section was sent: the message, or the reason it was refused.
    pub(super) fn submit_block_form(&mut self, kind: &FormKind, form: &Form, cx: &AppCx) -> Result<String, String> {
        match kind {
            FormKind::BlockListUrl => {
                let u = form.value(0);
                let u = u.trim().to_string();
                if !(u.starts_with("https://") || u.starts_with("http://")) {
                    return Err("The address must start with https://".into());
                }
                if self.start_block_job(cx, bb::Job::AddUrl(u), "Downloading the list…") {
                    Ok("Downloading the list…".into())
                } else {
                    Err("A list job is already running.".into())
                }
            }
            FormKind::BlockListFile => {
                let p = form.value(0);
                let p = p.trim();
                if p.is_empty() {
                    return Err("Choose a file.".into());
                }
                let path = std::path::PathBuf::from(p);
                if !path.is_file() {
                    return Err("That file does not exist.".into());
                }
                if self.start_block_job(cx, bb::Job::AddFile(path), "Reading the list file…") {
                    Ok("Reading the list file…".into())
                } else {
                    Err("A list job is already running.".into())
                }
            }
            _ => Err("not a list form".into()),
        }
    }
}
