//! The AI IDE's WORKSPACE pane: the file tree of the project folder. Folders expand and collapse, build output and
//! dependency folders stay hidden (unless asked for), files that are open are marked, and everything the tree does
//! (create, rename, delete) goes through `iyakki_agent::workspace`, which never leaves the workspace root.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use iyakki_agent::workspace::{self, Entry};

use crate::fx::{self, with_alpha, CYAN};
use crate::gfx::{rgb, Face, Gfx};
use crate::ui::{Event, Key, Nav, DIM, R, TEXT};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Row {
    /// Path below the root, with `/`.
    pub rel: String,
    pub name: String,
    pub depth: usize,
    pub is_dir: bool,
    pub open: bool,
    pub escapes: bool,
    pub heavy: bool,
}

/// What a key in the tree asks the IDE to do.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TreeAct {
    None,
    Open(String),
    NewFile(String),
    NewDir(String),
    Rename(String),
    Delete(String),
    /// Move the focus to the name filter.
    Filter,
}

pub struct Tree {
    pub root: PathBuf,
    pub expanded: BTreeSet<String>,
    pub rows: Vec<Row>,
    pub sel: usize,
    pub scroll: usize,
    pub show_heavy: bool,
    /// Rows found by the name filter instead of the tree.
    pub found: Option<Vec<String>>,
    seen: Vec<(Option<std::time::SystemTime>, usize)>,
    visible_rows: usize,
    row_rects: Vec<(R, usize)>,
}

fn join(dir: &str, name: &str) -> String {
    if dir.is_empty() { name.to_string() } else { format!("{dir}/{name}") }
}

impl Tree {
    pub fn new(root: &Path) -> Tree {
        let mut t = Tree { root: root.into(), expanded: BTreeSet::new(), rows: vec![], sel: 0, scroll: 0, show_heavy: false, found: None, seen: vec![], visible_rows: 20, row_rects: vec![] };
        t.refresh();
        t
    }

    /// Modification times of the root and every expanded folder: a folder's time changes when something is
    /// created, renamed or deleted in it, which is all the tree shows.
    fn stamps(&self) -> Vec<(Option<std::time::SystemTime>, usize)> {
        // the entry count catches changes inside one coarse timestamp tick
        let mtime = |p: PathBuf| (std::fs::metadata(&p).and_then(|m| m.modified()).ok(), std::fs::read_dir(&p).map_or(0, |d| d.count()));
        std::iter::once(mtime(self.root.clone())).chain(self.expanded.iter().map(|e| mtime(self.root.join(e)))).collect()
    }

    /// Called about once a second: rebuild the rows when the disk changed behind the tree's back (the agent or the
    /// terminal wrote a file). Expanded folders, the selection and open files are kept. True when rows changed.
    pub fn poll(&mut self) -> bool {
        if self.stamps() == self.seen {
            return false;
        }
        let before = self.rows.clone();
        self.refresh();
        self.rows != before
    }

    /// Rebuild the rows from the disk (expanded folders that vanished are forgotten).
    pub fn refresh(&mut self) {
        let keep = self.rows.get(self.sel).map(|r| r.rel.clone());
        let mut rows = Vec::new();
        let mut gone = Vec::new();
        self.walk("", 0, &mut rows, &mut gone);
        for g in gone {
            self.expanded.remove(&g);
        }
        // folders that vanished behind the tree's back are forgotten
        let root = self.root.clone();
        self.expanded.retain(|e| root.join(e).is_dir());
        self.rows = rows;
        if let Some(k) = keep {
            if let Some(i) = self.rows.iter().position(|r| r.rel == k) {
                self.sel = i;
            }
        }
        self.sel = self.sel.min(self.rows.len().saturating_sub(1));
        self.seen = self.stamps();
    }

    fn walk(&self, dir: &str, depth: usize, out: &mut Vec<Row>, gone: &mut Vec<String>) {
        let Ok(entries) = workspace::list_dir(&self.root, dir, self.show_heavy) else {
            gone.push(dir.to_string());
            return;
        };
        for Entry { name, is_dir, escapes, heavy } in entries {
            let rel = join(dir, &name);
            let open = is_dir && self.expanded.contains(&rel);
            out.push(Row { rel: rel.clone(), name, depth, is_dir, open, escapes, heavy });
            if open && depth < 24 {
                self.walk(&rel, depth + 1, out, gone);
            }
        }
    }

    pub fn selected(&self) -> Option<&Row> {
        self.rows.get(self.sel)
    }

    /// The folder new files go into: the selected folder, or the selected file's folder, or the root.
    pub fn target_dir(&self) -> String {
        match self.selected() {
            Some(r) if r.is_dir => r.rel.clone(),
            Some(r) => r.rel.rsplit_once('/').map_or(String::new(), |x| x.0.to_string()),
            None => String::new(),
        }
    }

    pub fn toggle(&mut self, i: usize) {
        let Some(r) = self.rows.get(i).cloned() else { return };
        if !r.is_dir {
            return;
        }
        if !self.expanded.remove(&r.rel) {
            self.expanded.insert(r.rel.clone());
        }
        self.refresh();
    }

    pub fn expand_to(&mut self, rel: &str) {
        let mut acc = String::new();
        let parts: Vec<&str> = rel.split('/').collect();
        for p in &parts[..parts.len().saturating_sub(1)] {
            acc = join(&acc, p);
            self.expanded.insert(acc.clone());
        }
        self.refresh();
        if let Some(i) = self.rows.iter().position(|r| r.rel == rel) {
            self.sel = i;
            self.ensure_visible();
        }
    }

    pub fn set_filter(&mut self, q: &str) {
        self.found = if q.trim().is_empty() { None } else { Some(workspace::find_files(&self.root, q.trim(), 200)) };
        self.sel = 0;
        self.scroll = 0;
    }

    fn len(&self) -> usize {
        self.found.as_ref().map_or(self.rows.len(), |f| f.len())
    }

    fn ensure_visible(&mut self) {
        if self.sel < self.scroll {
            self.scroll = self.sel;
        } else if self.sel >= self.scroll + self.visible_rows {
            self.scroll = self.sel + 1 - self.visible_rows;
        }
    }

    pub fn scroll_by(&mut self, d: i32) {
        let max = self.len().saturating_sub(self.visible_rows);
        self.scroll = (self.scroll as i32 - d * 3).clamp(0, max as i32) as usize;
    }

    /// Select row `i` (a click).
    pub fn select(&mut self, i: usize) {
        if i < self.len() {
            self.sel = i;
        }
    }

    pub fn key(&mut self, k: &Key) -> TreeAct {
        let n = self.len();
        match k {
            Key::Up => self.sel = self.sel.saturating_sub(1),
            Key::Down => self.sel = (self.sel + 1).min(n.saturating_sub(1)),
            Key::PageUp => self.sel = self.sel.saturating_sub(self.visible_rows.max(1)),
            Key::PageDown => self.sel = (self.sel + self.visible_rows.max(1)).min(n.saturating_sub(1)),
            Key::Home => self.sel = 0,
            Key::End => self.sel = n.saturating_sub(1),
            Key::Right => {
                if let Some(r) = self.selected().cloned().filter(|r| r.is_dir && !r.open && self.found.is_none()) {
                    self.toggle(self.sel);
                    let _ = r;
                } else if self.selected().is_some_and(|r| r.is_dir && r.open) {
                    self.sel = (self.sel + 1).min(n.saturating_sub(1));
                }
            }
            Key::Left => {
                if let Some(r) = self.selected().cloned() {
                    if r.is_dir && r.open {
                        self.toggle(self.sel);
                    } else if let Some((parent, _)) = r.rel.rsplit_once('/') {
                        if let Some(i) = self.rows.iter().position(|x| x.rel == parent) {
                            self.sel = i;
                        }
                    }
                }
            }
            Key::Enter | Key::Char(' ') => {
                if let Some(rel) = self.found.as_ref().and_then(|f| f.get(self.sel)).cloned() {
                    return TreeAct::Open(rel);
                }
                if let Some(r) = self.selected().cloned() {
                    if r.is_dir {
                        self.toggle(self.sel);
                    } else if !r.escapes {
                        return TreeAct::Open(r.rel);
                    }
                }
            }
            Key::Char('n') => return TreeAct::NewFile(self.target_dir()),
            Key::Char('N') => return TreeAct::NewDir(self.target_dir()),
            Key::F(2) | Key::Char('r') => {
                if let Some(r) = self.selected() {
                    return TreeAct::Rename(r.rel.clone());
                }
            }
            Key::Delete | Key::Char('d') => {
                if let Some(r) = self.selected() {
                    return TreeAct::Delete(r.rel.clone());
                }
            }
            Key::Char('/') => return TreeAct::Filter,
            Key::Char('h') => {
                self.show_heavy = !self.show_heavy;
                self.refresh();
            }
            Key::Esc if self.found.is_some() => {
                self.found = None;
                self.sel = 0;
            }
            Key::Nav { nav: Nav::Up, .. } => self.sel = self.sel.saturating_sub(1),
            Key::Nav { nav: Nav::Down, .. } => self.sel = (self.sel + 1).min(n.saturating_sub(1)),
            _ => {}
        }
        self.ensure_visible();
        TreeAct::None
    }

    /// The path (relative) of the row under a pointer position of the last drawing.
    pub fn row_at(&self, x: i32, y: i32) -> Option<usize> {
        self.row_rects.iter().find(|(r, _)| r.contains(x, y)).map(|(_, i)| *i)
    }

    /// Draw the pane. `mark` tells, for a path below the root, whether it is open in the editor and whether it has unsaved changes.
    pub fn draw(&mut self, g: &mut Gfx, r: R, s: f32, focused: bool, accent: [u8; 4], mark: &dyn Fn(&str) -> Option<bool>) {
        let m = super::ide_chrome::cur();
        let pad = m.pad * s;
        let title = if self.found.is_some() { "SEARCH RESULTS" } else { "WORKSPACE" };
        let head = super::ide_chrome::pane_head(g, r, s, title);
        // the small actions at the top right: new file, new folder, refresh. They give way (are not drawn) when the pane is
        // too narrow to hold them next to the title, so nothing overlaps
        let tsz = m.font_title * s;
        let title_w = g.text_width(tsz, title, Face::Bold);
        let bsz = (head.h - 5.0 * s).max(18.0);
        let mut bx = r.x + r.w - pad - bsz + 2.0 * s;
        let room_left = r.x + pad + 2.0 * s + title_w + 10.0 * s;
        for (label, glyph, key) in [("refresh", "⟳", Key::Char('R')), ("new folder", "▣", Key::Char('N')), ("new file", "+", Key::Char('n'))] {
            if bx < room_left {
                break;
            }
            let b = R::new(bx, r.y + (head.h - bsz) / 2.0, bsz, bsz);
            let hot = g.hovered(b);
            if hot {
                g.rrect(b.x, b.y, b.w, b.h, 4.0 * s, rgb(34, 44, 78));
            }
            let gsz = m.font_ui * s;
            let gw = g.text_width(gsz, glyph, Face::Bold);
            g.text(b.x + (b.w - gw) / 2.0, b.y + (b.h - g.line_height(gsz, Face::Bold)) / 2.0, gsz, if hot { accent } else { super::ide_chrome::muted() }, glyph, Face::Bold);
            g.hit(b, Event::Sub(TREE_SUB, key), label);
            bx -= bsz + 2.0 * s;
        }
        let list = R::new(r.x, r.y + head.h + 2.0 * s, r.w, (r.h - head.h - 4.0 * s).max(0.0));
        let rh = m.row_h * s;
        let fsz = m.font_ui * s;
        self.visible_rows = ((list.h / rh) as usize).max(1);
        self.ensure_visible();
        self.row_rects.clear();
        let total = self.len();
        if total == 0 {
            let msg = if self.found.is_some() { "No file matches." } else { "This folder is empty. Press n to add a file." };
            let t = g.fit(fsz, msg, Face::Sans, list.w - 2.0 * pad);
            g.text(list.x + pad + 4.0 * s, list.y + 6.0 * s, fsz, super::ide_chrome::dim(), &t, Face::Sans);
            return;
        }
        let end = (self.scroll + self.visible_rows).min(total);
        let indent = 12.0 * s;
        for i in self.scroll..end {
            let y = list.y + (i - self.scroll) as f32 * rh;
            let rect = R::new(list.x + 3.0 * s, y, list.w - 6.0 * s - 6.0 * s, rh);
            let (rel, name, depth, is_dir, open, escapes, heavy) = match &self.found {
                Some(f) => {
                    let rel = f[i].clone();
                    let name = rel.rsplit('/').next().unwrap_or(&rel).to_string();
                    (rel, name, 0usize, false, false, false, false)
                }
                None => {
                    let row = &self.rows[i];
                    (row.rel.clone(), row.name.clone(), row.depth, row.is_dir, row.open, row.escapes, row.heavy)
                }
            };
            let selected = i == self.sel;
            let hot = g.hovered(rect);
            if selected {
                g.rrect(rect.x, rect.y, rect.w, rect.h, 3.0 * s, if focused { fx::over(rgb(22, 30, 58), with_alpha(accent, 56)) } else { rgb(24, 31, 56) });
                if focused {
                    g.rect(rect.x, rect.y + 2.0 * s, 2.0 * s.max(1.0), rect.h - 4.0 * s, accent);
                }
            } else if hot {
                g.rrect(rect.x, rect.y, rect.w, rect.h, 3.0 * s, rgb(22, 29, 54));
            }
            // indent guides: one hairline per level, under the carets of the parents
            for d in 0..depth {
                let gx = rect.x + 6.0 * s + d as f32 * indent + 5.0 * s;
                g.rect(gx, rect.y, 1.0, rect.h, [255, 255, 255, 14]);
            }
            let x = rect.x + 6.0 * s + depth as f32 * indent;
            let cy = rect.y + rect.h / 2.0;
            let lh = g.line_height(fsz, Face::Sans);
            if is_dir {
                // a caret and a folder
                let caret = if open { "▾" } else { "▸" };
                g.text(x, cy - lh / 2.0, fsz * 0.85, super::ide_chrome::muted(), caret, Face::Bold);
                folder_icon(g, x + 17.0 * s, cy, s * 0.9, if heavy { DIM } else { fx::AMBER });
            } else {
                file_icon(g, x + 17.0 * s, cy, s * 0.9, &name);
            }
            let tx = x + 30.0 * s;
            let color = if escapes || heavy { super::ide_chrome::dim() } else if selected { TEXT } else { rgb(205, 212, 232) };
            let marker = mark(&rel);
            let reserve = if marker.is_some() { 14.0 * s } else { 0.0 };
            let label = if escapes { format!("{name}  (link leaves the workspace)") } else if self.found.is_some() { rel.clone() } else { name.clone() };
            let face = if is_dir { Face::Bold } else { Face::Sans };
            let t = g.fit(fsz, &label, face, rect.x + rect.w - tx - reserve - 4.0 * s);
            g.text(tx, cy - lh / 2.0, fsz, color, &t, face);
            if let Some(dirty) = marker {
                g.circle(rect.x + rect.w - 8.0 * s, cy, 3.0 * s, if dirty { fx::AMBER } else { CYAN });
            }
            g.hit(rect, Event::Row(i), &rel);
            self.row_rects.push((rect, i));
        }
        fx::scrollbar(g, R::new(r.x + r.w - 5.0 * s, list.y, 5.0 * s, list.h), s, self.scroll, self.visible_rows, total, accent);
    }
}

/// The `Event::Sub` code the tree's header buttons use (the IDE routes it to the tree).
pub const TREE_SUB: u8 = 21;

pub fn folder_icon(g: &mut Gfx, cx: f32, cy: f32, s: f32, c: [u8; 4]) {
    let (w, h) = (14.0 * s, 10.0 * s);
    g.rrect(cx - w / 2.0, cy - h / 2.0 - 1.0 * s, w * 0.45, 3.0 * s, 1.5 * s, with_alpha(c, 200));
    g.rrect(cx - w / 2.0, cy - h / 2.0 + 1.5 * s, w, h - 1.5 * s, 2.0 * s, with_alpha(c, 220));
}

/// A small coloured tab for a file, by its kind.
pub fn file_color(name: &str) -> ([u8; 4], &'static str) {
    let ext = name.rsplit_once('.').map(|x| x.1.to_lowercase()).unwrap_or_default();
    match (name, ext.as_str()) {
        (_, "rs") => (rgb(255, 150, 90), "rs"),
        (_, "py") => (rgb(110, 190, 255), "py"),
        (_, "js" | "mjs" | "jsx") => (rgb(255, 220, 90), "js"),
        (_, "ts" | "tsx") => (rgb(90, 160, 255), "ts"),
        (_, "go") => (rgb(90, 220, 230), "go"),
        (_, "c" | "h") => (rgb(150, 170, 255), "c"),
        (_, "cpp" | "cc" | "hpp") => (rgb(150, 150, 255), "c+"),
        (_, "md" | "txt") => (rgb(190, 200, 225), "md"),
        (_, "toml" | "yaml" | "yml" | "json" | "ini" | "conf") => (rgb(255, 120, 190), "{}"),
        (_, "sh" | "bash") => (rgb(130, 230, 140), "sh"),
        (_, "html" | "css") => (rgb(255, 120, 120), "<>"),
        (_, "png" | "jpg" | "jpeg" | "svg" | "gif" | "webp") => (rgb(200, 140, 255), "im"),
        (_, "lock") => (rgb(130, 140, 170), "lk"),
        ("Dockerfile", _) | ("Makefile", _) => (rgb(120, 200, 255), "mk"),
        _ => (rgb(150, 160, 190), ""),
    }
}

pub fn file_icon(g: &mut Gfx, cx: f32, cy: f32, s: f32, name: &str) {
    let (c, tag) = file_color(name);
    let (w, h) = (16.0 * s, 13.0 * s);
    g.rrect(cx - w / 2.0, cy - h / 2.0, w, h, 3.0 * s, with_alpha(c, 70));
    g.rrect_outline(cx - w / 2.0, cy - h / 2.0, w, h, 3.0 * s, 1.0 * s, with_alpha(c, 200));
    if !tag.is_empty() {
        let sz = 8.0 * s;
        let tw = g.text_width(sz, tag, Face::Bold);
        g.text(cx - tw / 2.0, cy - g.line_height(sz, Face::Bold) / 2.0, sz, c, tag, Face::Bold);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn world(tag: &str) -> PathBuf {
        let d = std::fs::canonicalize(std::env::temp_dir()).unwrap().join(format!("iyakki-tree-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        for dir in ["src/deep", "target/debug", "node_modules/x", ".git"] {
            std::fs::create_dir_all(d.join(dir)).unwrap();
        }
        std::fs::write(d.join("README.md"), "r").unwrap();
        std::fs::write(d.join("src/main.rs"), "m").unwrap();
        std::fs::write(d.join("src/deep/inner.rs"), "i").unwrap();
        d
    }

    #[test]
    fn poll_notices_files_written_behind_its_back_and_keeps_expansion() {
        let d = world("poll");
        let mut t = Tree::new(&d);
        t.expanded.insert("src".into());
        t.refresh();
        assert!(!t.poll(), "nothing changed");
        std::fs::write(d.join("Dockerfile"), "FROM node").unwrap();
        std::fs::write(d.join("src/app.js"), "x").unwrap();
        assert!(t.poll());
        let names: Vec<&str> = t.rows.iter().map(|r| r.rel.as_str()).collect();
        assert!(names.contains(&"Dockerfile") && names.contains(&"src/app.js"), "{names:?}");
        assert!(t.expanded.contains("src"), "expanded state stays");
        assert!(!t.poll(), "and it settles");
        std::fs::remove_file(d.join("Dockerfile")).unwrap();
        assert!(t.poll());
        assert!(t.rows.iter().all(|r| r.rel != "Dockerfile"));
    }

    #[test]
    fn expands_collapses_navigates_and_hides_heavy_folders() {
        let d = world("nav");
        let mut t = Tree::new(&d);
        assert_eq!(t.rows.iter().map(|r| r.name.as_str()).collect::<Vec<_>>(), vec!["src", "README.md"], "target, node_modules and .git are hidden");
        assert_eq!(t.key(&Key::Enter), TreeAct::None, "Enter on a folder expands it");
        assert_eq!(t.rows.len(), 4);
        assert!(t.rows[0].open && t.rows[1].depth == 1 && t.rows[1].name == "deep");
        t.key(&Key::Down);
        t.key(&Key::Right);
        assert!(t.rows.iter().any(|r| r.rel == "src/deep/inner.rs"));
        t.key(&Key::Down);
        assert_eq!(t.key(&Key::Enter), TreeAct::Open("src/deep/inner.rs".into()));
        t.key(&Key::Left);
        assert_eq!(t.selected().unwrap().rel, "src/deep", "Left goes to the parent");
        t.key(&Key::Left);
        assert!(!t.rows.iter().any(|r| r.rel == "src/deep/inner.rs"), "and collapses");
        t.key(&Key::Char('h'));
        assert!(t.rows.iter().any(|r| r.name == "target" && r.heavy));
        t.key(&Key::Char('h'));
        assert_eq!(t.key(&Key::Char('n')), TreeAct::NewFile("src/deep".into()), "new files go to the selected folder");
        t.sel = t.rows.iter().position(|r| r.rel == "README.md").unwrap();
        assert_eq!(t.key(&Key::Char('n')), TreeAct::NewFile(String::new()));
        assert_eq!(t.key(&Key::F(2)), TreeAct::Rename("README.md".into()));
        assert_eq!(t.key(&Key::Delete), TreeAct::Delete("README.md".into()));
        t.expand_to("src/deep/inner.rs");
        assert_eq!(t.selected().unwrap().rel, "src/deep/inner.rs");
        // a folder deleted behind its back is forgotten
        std::fs::remove_dir_all(d.join("src/deep")).unwrap();
        t.refresh();
        assert!(!t.expanded.contains("src/deep"));
    }

    #[test]
    fn symlinks_out_of_the_workspace_are_shown_but_not_opened_and_the_filter_finds_files() {
        let d = world("links");
        let outside = d.with_file_name(format!("{}-outside", d.file_name().unwrap().to_string_lossy()));
        std::fs::create_dir_all(&outside).unwrap();
        std::fs::write(outside.join("secret"), "s").unwrap();
        std::os::unix::fs::symlink(&outside, d.join("leak")).unwrap();
        std::os::unix::fs::symlink(outside.join("secret"), d.join("leakfile")).unwrap();
        let mut t = Tree::new(&d);
        let leak = t.rows.iter().position(|r| r.name == "leak").unwrap();
        assert!(t.rows[leak].escapes && !t.rows[leak].is_dir);
        t.sel = t.rows.iter().position(|r| r.name == "leakfile").unwrap();
        assert_eq!(t.key(&Key::Enter), TreeAct::None, "a link out is never opened");
        t.set_filter("inner");
        assert_eq!(t.found.as_ref().unwrap(), &vec!["src/deep/inner.rs".to_string()]);
        assert_eq!(t.key(&Key::Enter), TreeAct::Open("src/deep/inner.rs".into()));
        t.key(&Key::Esc);
        assert!(t.found.is_none());
    }

    #[test]
    fn draws_rows_with_open_markers_and_registers_clicks() {
        let d = world("draw");
        let mut t = Tree::new(&d);
        t.toggle(0);
        let mut g = Gfx::new(400, 400);
        g.clear(rgb(8, 11, 26));
        t.draw(&mut g, R::new(0.0, 0.0, 260.0, 400.0), 1.0, true, CYAN, &|rel| if rel == "src/main.rs" { Some(true) } else { None });
        assert!(g.hits.iter().filter(|h| matches!(h.ev, Event::Row(_))).count() >= 4);
        assert!(g.hits.iter().any(|h| h.label == "new file"));
        assert_eq!(t.row_at(30, 40), Some(0));
    }
}
