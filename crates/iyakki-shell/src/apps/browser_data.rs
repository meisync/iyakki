//! What the browser remembers for one person: bookmarks (with folders), the pages visited (how often and when), the zoom
//! chosen for each site and the list of downloads. Kept in one small file next to their other settings. Only addresses and
//! titles are stored, and the person can clear the history.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use super::AppCx;

pub const HISTORY_MAX: usize = 2000;
pub const BOOKMARKS_MAX: usize = 1000;
pub const DOWNLOADS_MAX: usize = 200;
pub const ZOOM_MIN: f32 = 0.5;
pub const ZOOM_MAX: f32 = 3.0;
const ZOOM_STEPS: [f32; 11] = [0.5, 0.67, 0.8, 0.9, 1.0, 1.1, 1.25, 1.5, 1.75, 2.0, 3.0];

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Bookmark {
    pub title: String,
    pub url: String,
    /// "" is the top level.
    #[serde(default)]
    pub folder: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Visit {
    pub url: String,
    pub title: String,
    pub visits: u32,
    /// Unix seconds of the last visit.
    pub last: i64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Download {
    pub url: String,
    /// The file as saved (name only).
    pub name: String,
    pub bytes: u64,
    pub at: i64,
    /// "saved", or why it did not work.
    pub status: String,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Data {
    #[serde(default)]
    pub bookmarks: Vec<Bookmark>,
    #[serde(default)]
    pub history: Vec<Visit>,
    #[serde(default)]
    pub zoom: BTreeMap<String, f32>,
    /// Sites shown in night colours (N).
    #[serde(default)]
    pub dark: std::collections::BTreeSet<String>,
    #[serde(default)]
    pub downloads: Vec<Download>,
}

/// Something the address bar offers while typing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Suggestion {
    pub title: String,
    pub url: String,
    pub bookmark: bool,
}

fn path(cx: &AppCx) -> std::path::PathBuf {
    cx.services.dir().join(format!("browser-data-{}.json", super::browser_config::who(cx)))
}

pub fn load(cx: &AppCx) -> Data {
    std::fs::read_to_string(path(cx)).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
}

pub fn save(cx: &AppCx, d: &Data) {
    if let Ok(t) = serde_json::to_string(d) {
        let p = path(cx);
        let tmp = p.with_extension("tmp");
        if std::fs::write(&tmp, t).is_ok() {
            let _ = std::fs::rename(&tmp, &p);
        }
    }
}

/// The part of an address that decides which site it is.
pub fn host_of(url: &str) -> &str {
    let rest = url.split_once("://").map_or(url, |(_, r)| r);
    rest.split(['/', '?', '#']).next().unwrap_or("").rsplit('@').next().unwrap_or("")
}

fn words(s: &str) -> Vec<String> {
    s.split_whitespace().map(str::to_lowercase).collect()
}

/// Every word of the query appears in the title or the address.
fn matches(q: &[String], title: &str, url: &str) -> bool {
    let (t, u) = (title.to_lowercase(), url.to_lowercase());
    q.iter().all(|w| t.contains(w) || u.contains(w))
}

impl Data {
    /// Record a visit to `url` (a page that loaded).
    pub fn visit(&mut self, url: &str, title: &str, now: i64) {
        if url.is_empty() {
            return;
        }
        match self.history.iter_mut().find(|v| v.url == url) {
            Some(v) => {
                v.visits = v.visits.saturating_add(1);
                v.last = now;
                if !title.is_empty() {
                    v.title = title.to_string();
                }
            }
            None => self.history.push(Visit { url: url.to_string(), title: title.to_string(), visits: 1, last: now }),
        }
        if self.history.len() > HISTORY_MAX {
            // forget the least recently visited
            self.history.sort_by_key(|v| std::cmp::Reverse(v.last));
            self.history.truncate(HISTORY_MAX);
        }
    }

    /// The title of a page already in the history became known (the full engine reports it after the address).
    pub fn retitle(&mut self, url: &str, title: &str) {
        if title.is_empty() {
            return;
        }
        if let Some(v) = self.history.iter_mut().find(|v| v.url == url) {
            v.title = title.to_string();
        }
    }

    pub fn clear_history(&mut self) {
        self.history.clear();
    }

    pub fn forget(&mut self, url: &str) {
        self.history.retain(|v| v.url != url);
    }

    pub fn is_bookmarked(&self, url: &str) -> bool {
        self.bookmarks.iter().any(|b| b.url == url)
    }

    /// Add or remove the bookmark for `url`; true when it is bookmarked afterwards.
    pub fn toggle_bookmark(&mut self, url: &str, title: &str) -> bool {
        if let Some(i) = self.bookmarks.iter().position(|b| b.url == url) {
            self.bookmarks.remove(i);
            return false;
        }
        if self.bookmarks.len() >= BOOKMARKS_MAX {
            return false;
        }
        let title = if title.trim().is_empty() { host_of(url).to_string() } else { title.trim().to_string() };
        self.bookmarks.push(Bookmark { title, url: url.to_string(), folder: String::new() });
        true
    }

    pub fn set_folder(&mut self, url: &str, folder: &str) {
        let folder: String = folder.trim().chars().filter(|c| !c.is_control()).take(60).collect();
        if let Some(b) = self.bookmarks.iter_mut().find(|b| b.url == url) {
            b.folder = folder;
        }
    }

    /// The bookmarks grouped by folder: the top level first, then folders by name.
    pub fn bookmark_folders(&self) -> Vec<(String, Vec<&Bookmark>)> {
        let mut m: BTreeMap<&str, Vec<&Bookmark>> = BTreeMap::new();
        for b in &self.bookmarks {
            m.entry(b.folder.as_str()).or_default().push(b);
        }
        m.into_iter().map(|(k, v)| (k.to_string(), v)).collect()
    }

    /// The pages visited most often.
    pub fn top_sites(&self, n: usize) -> Vec<&Visit> {
        let mut v: Vec<&Visit> = self.history.iter().collect();
        v.sort_by(|a, b| b.visits.cmp(&a.visits).then(b.last.cmp(&a.last)));
        v.truncate(n);
        v
    }

    pub fn recent(&self, n: usize) -> Vec<&Visit> {
        let mut v: Vec<&Visit> = self.history.iter().collect();
        v.sort_by_key(|v| std::cmp::Reverse(v.last));
        v.truncate(n);
        v
    }

    pub fn search_history(&self, query: &str, n: usize) -> Vec<&Visit> {
        let q = words(query);
        let mut v: Vec<&Visit> = self.history.iter().filter(|v| matches(&q, &v.title, &v.url)).collect();
        v.sort_by_key(|v| std::cmp::Reverse(v.last));
        v.truncate(n);
        v
    }

    /// What to offer under the address bar for what has been typed: bookmarks first, then visited pages, the ones whose
    /// address starts with the text before the others, most visited first.
    pub fn suggestions(&self, typed: &str, n: usize) -> Vec<Suggestion> {
        let q = words(typed);
        if q.is_empty() || typed.trim().len() < 2 {
            return Vec::new();
        }
        let first = q[0].as_str();
        let starts = |url: &str| {
            let h = host_of(url).to_lowercase();
            h.trim_start_matches("www.").starts_with(first.trim_start_matches("www.")) || url.to_lowercase().starts_with(first)
        };
        let mut out: Vec<Suggestion> = Vec::new();
        let push = |out: &mut Vec<Suggestion>, title: &str, url: &str, bookmark: bool| {
            if !out.iter().any(|s| s.url == url) && out.len() < n {
                out.push(Suggestion { title: title.to_string(), url: url.to_string(), bookmark });
            }
        };
        let mut b: Vec<&Bookmark> = self.bookmarks.iter().filter(|b| matches(&q, &b.title, &b.url)).collect();
        b.sort_by_key(|b| !starts(&b.url));
        for b in b {
            push(&mut out, &b.title, &b.url, true);
        }
        let mut h: Vec<&Visit> = self.history.iter().filter(|v| matches(&q, &v.title, &v.url)).collect();
        h.sort_by(|a, b| starts(&b.url).cmp(&starts(&a.url)).then(b.visits.cmp(&a.visits)).then(b.last.cmp(&a.last)));
        for v in h {
            push(&mut out, &v.title, &v.url, false);
        }
        out
    }

    pub fn is_dark(&self, host: &str) -> bool {
        self.dark.contains(host)
    }

    /// Turn night colours on or off for a site; returns the new state.
    pub fn toggle_dark(&mut self, host: &str) -> bool {
        if !self.dark.remove(host) && self.dark.len() < 1000 {
            self.dark.insert(host.to_string());
            return true;
        }
        false
    }

    pub fn zoom_for(&self, host: &str) -> f32 {
        self.zoom.get(host).copied().filter(|z| (ZOOM_MIN..=ZOOM_MAX).contains(z)).unwrap_or(1.0)
    }

    pub fn set_zoom(&mut self, host: &str, z: f32) {
        if host.is_empty() {
            return;
        }
        if (z - 1.0).abs() < 0.01 {
            self.zoom.remove(host);
        } else if self.zoom.len() < 1000 {
            self.zoom.insert(host.to_string(), z.clamp(ZOOM_MIN, ZOOM_MAX));
        }
    }

    pub fn add_download(&mut self, d: Download) {
        self.downloads.insert(0, d);
        self.downloads.truncate(DOWNLOADS_MAX);
    }

    /// The bookmarks as the HTML file every browser exports and imports (Netscape bookmark format).
    pub fn export_html(&self) -> String {
        let esc = |s: &str| s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;");
        let mut h = String::from("<!DOCTYPE NETSCAPE-Bookmark-file-1>\n<META HTTP-EQUIV=\"Content-Type\" CONTENT=\"text/html; charset=UTF-8\">\n<TITLE>Bookmarks</TITLE>\n<H1>Bookmarks</H1>\n<DL><p>\n");
        for (folder, items) in self.bookmark_folders() {
            if !folder.is_empty() {
                h.push_str(&format!("    <DT><H3>{}</H3>\n    <DL><p>\n", esc(&folder)));
            }
            for b in items {
                h.push_str(&format!("        <DT><A HREF=\"{}\">{}</A>\n", esc(&b.url), esc(&b.title)));
            }
            if !folder.is_empty() {
                h.push_str("    </DL><p>\n");
            }
        }
        h.push_str("</DL><p>\n");
        h
    }

    /// Read bookmarks from an exported file; returns how many were new. Only http and https addresses are taken.
    pub fn import_html(&mut self, html: &str) -> usize {
        let unesc = |s: &str| s.replace("&quot;", "\"").replace("&lt;", "<").replace("&gt;", ">").replace("&amp;", "&");
        let mut folder = String::new();
        let mut depth_folders: Vec<String> = Vec::new();
        let mut added = 0;
        let lower = html.to_lowercase();
        let mut i = 0;
        while let Some(rel) = lower[i..].find('<') {
            let at = i + rel;
            let rest = &lower[at..];
            if rest.starts_with("<h3") {
                if let (Some(gt), Some(end)) = (rest.find('>'), rest.find("</h3>")) {
                    folder = unesc(html[at + gt + 1..at + end].trim());
                    depth_folders.push(folder.clone());
                    i = at + end + 5;
                    continue;
                }
            } else if rest.starts_with("</dl") {
                if depth_folders.pop().is_some() {
                    folder = depth_folders.last().cloned().unwrap_or_default();
                }
            } else if rest.starts_with("<a ") || rest.starts_with("<a\t") {
                if let (Some(gt), Some(end)) = (rest.find('>'), rest.find("</a>")) {
                    let tag = &html[at..at + gt];
                    let href = tag.to_lowercase().find("href=\"").map(|p| &tag[p + 6..]).and_then(|t| t.split('"').next()).map(unesc);
                    let title = unesc(html[at + gt + 1..at + end].trim());
                    if let Some(url) = href.filter(|u| u.starts_with("http://") || u.starts_with("https://")) {
                        if !self.is_bookmarked(&url) && self.bookmarks.len() < BOOKMARKS_MAX {
                            self.bookmarks.push(Bookmark { title: if title.is_empty() { host_of(&url).to_string() } else { title }, url, folder: folder.clone() });
                            added += 1;
                        }
                    }
                    i = at + end + 4;
                    continue;
                }
            }
            i = at + 1;
        }
        added
    }
}

/// The next zoom level above or below `z` (the page zoom steps of every browser).
pub fn zoom_step(z: f32, up: bool) -> f32 {
    if up {
        ZOOM_STEPS.iter().copied().find(|s| *s > z + 0.001).unwrap_or(ZOOM_MAX)
    } else {
        ZOOM_STEPS.iter().rev().copied().find(|s| *s < z - 0.001).unwrap_or(ZOOM_MIN)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn visits_count_and_the_most_visited_and_most_recent_come_out_right() {
        let mut d = Data::default();
        d.visit("https://a.example/", "A", 100);
        d.visit("https://b.example/", "B", 200);
        d.visit("https://a.example/", "A page", 300);
        d.visit("", "nothing", 400);
        assert_eq!(d.history.len(), 2);
        assert_eq!(d.top_sites(5)[0].url, "https://a.example/");
        assert_eq!((d.top_sites(5)[0].visits, d.top_sites(5)[0].title.as_str()), (2, "A page"));
        assert_eq!(d.recent(1)[0].url, "https://a.example/");
        assert_eq!(d.search_history("page", 5).len(), 1);
        assert_eq!(d.search_history("example b", 5).len(), 1, "every word must appear, in the title or the address");
        d.forget("https://a.example/");
        assert_eq!(d.history.len(), 1);
        d.clear_history();
        assert!(d.history.is_empty());
    }

    #[test]
    fn the_history_is_capped_and_forgets_the_oldest() {
        let mut d = Data::default();
        for i in 0..HISTORY_MAX + 50 {
            d.visit(&format!("https://x.example/{i}"), "", i as i64);
        }
        assert_eq!(d.history.len(), HISTORY_MAX);
        assert!(!d.history.iter().any(|v| v.url.ends_with("/0")) && d.history.iter().any(|v| v.url.ends_with(&format!("/{}", HISTORY_MAX + 49))));
    }

    #[test]
    fn bookmarks_toggle_group_into_folders_and_round_trip_through_html() {
        let mut d = Data::default();
        assert!(d.toggle_bookmark("https://rust-lang.org/", "Rust & more"));
        assert!(d.toggle_bookmark("https://docs.rs/", ""));
        d.set_folder("https://rust-lang.org/", "Dev <tools>");
        assert!(d.is_bookmarked("https://docs.rs/"));
        assert_eq!(d.bookmarks[1].title, "docs.rs", "no title falls back to the site");
        let folders = d.bookmark_folders();
        assert_eq!(folders.iter().map(|(f, v)| (f.as_str(), v.len())).collect::<Vec<_>>(), vec![("", 1), ("Dev <tools>", 1)]);
        let html = d.export_html();
        assert!(html.contains("Rust &amp; more") && html.contains("Dev &lt;tools&gt;"));
        let mut e = Data::default();
        assert_eq!(e.import_html(&html), 2);
        let find = |url: &str| e.bookmarks.iter().find(|b| b.url == url).cloned().expect("imported");
        assert_eq!((find("https://rust-lang.org/").title, find("https://rust-lang.org/").folder), ("Rust & more".to_string(), "Dev <tools>".to_string()));
        assert_eq!((find("https://docs.rs/").title, find("https://docs.rs/").folder), ("docs.rs".to_string(), String::new()));
        assert_eq!(e.import_html(&html), 0, "the same file again adds nothing");
        assert!(!d.toggle_bookmark("https://docs.rs/", ""), "toggling again removes it");
        let mut f = Data::default();
        assert_eq!(f.import_html("<a href=\"javascript:alert(1)\">x</a><a href=\"file:///etc/passwd\">y</a><a href=\"https://ok.example/\">ok</a>"), 1, "only web addresses are imported");
    }

    #[test]
    fn suggestions_put_bookmarks_first_and_addresses_that_start_with_the_text_before_the_rest() {
        let mut d = Data::default();
        d.visit("https://www.rust-lang.org/", "Rust Programming Language", 10);
        d.visit("https://docs.rs/", "Docs.rs: Rust packages", 20);
        d.visit("https://docs.rs/", "Docs.rs: Rust packages", 21);
        d.visit("https://example.org/rust-story", "A story", 30);
        d.toggle_bookmark("https://blog.example/rusty", "Rusty blog");
        let s = d.suggestions("rust", 10);
        assert_eq!(s[0], Suggestion { title: "Rusty blog".into(), url: "https://blog.example/rusty".into(), bookmark: true });
        assert_eq!(s[1].url, "https://www.rust-lang.org/", "the address that starts with the text comes before the others");
        assert_eq!(s.len(), 4);
        assert!(d.suggestions("r", 10).is_empty(), "one letter is too little to guess from");
        assert_eq!(d.suggestions("rust", 2).len(), 2, "capped");
        assert_eq!(d.suggestions("docs rust", 10).len(), 1, "all words must match");
    }

    #[test]
    fn zoom_is_remembered_per_site_and_steps_like_other_browsers() {
        let mut d = Data::default();
        assert_eq!(d.zoom_for("a.example"), 1.0);
        d.set_zoom("a.example", 1.5);
        assert_eq!(d.zoom_for("a.example"), 1.5);
        assert_eq!(d.zoom_for("b.example"), 1.0);
        d.set_zoom("a.example", 1.0);
        assert!(d.zoom.is_empty(), "back to normal is not kept");
        assert_eq!(zoom_step(1.0, true), 1.1);
        assert_eq!(zoom_step(1.0, false), 0.9);
        assert_eq!(zoom_step(3.0, true), 3.0);
        assert_eq!(zoom_step(0.5, false), 0.5);
        let mut z = 1.0;
        for _ in 0..20 {
            z = zoom_step(z, true);
        }
        assert_eq!(z, ZOOM_MAX);
        assert_eq!(host_of("https://user@Example.com:8080/a?b#c"), "Example.com:8080");
    }

    #[test]
    fn downloads_are_listed_newest_first_and_capped() {
        let mut d = Data::default();
        for i in 0..DOWNLOADS_MAX + 5 {
            d.add_download(Download { url: format!("https://x/{i}"), name: format!("f{i}"), bytes: 1, at: i as i64, status: "saved".into() });
        }
        assert_eq!(d.downloads.len(), DOWNLOADS_MAX);
        assert_eq!(d.downloads[0].name, format!("f{}", DOWNLOADS_MAX + 4));
    }
}
