//! The rendered view of a Markdown file in the AI IDE's editor: a `.md` file opens as a preview (headings, lists, code, links drawn by
//! `mdtext`), and the toolbar's Edit button (or the key `e`, Enter or Ctrl+E) switches that file to the text editor; Preview (Ctrl+E) goes
//! back. The choice is kept per file for as long as the editor lives. The preview is of the text in the editor, so unsaved changes show.

use crate::gfx::{rgb, Gfx};
use crate::mdtext::{self, Layout};
use crate::ui::{Key, Nav, R, TEXT};
use std::collections::HashSet;
use std::path::{Path, PathBuf};

pub fn is_markdown(p: &Path) -> bool {
    p.extension().is_some_and(|e| matches!(e.to_string_lossy().to_ascii_lowercase().as_str(), "md" | "markdown" | "mdown"))
}

#[derive(Default)]
pub struct MdPreview {
    /// Markdown files the person switched to the text editor.
    edit: HashSet<PathBuf>,
    /// Pixels scrolled down, per the file on screen (the position is forgotten when another file is shown).
    scroll: f32,
    shown: Option<PathBuf>,
    /// The page height and the visible height of the last frame (for the limits of the scroll).
    page: f32,
    view: f32,
    cache: Option<(u64, u32, Layout)>,
}

fn hash(text: &str) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    text.hash(&mut h);
    h.finish()
}

impl MdPreview {
    pub fn editing(&self, p: &Path) -> bool {
        self.edit.contains(p)
    }

    pub fn set_edit(&mut self, p: &Path, on: bool) {
        if on {
            self.edit.insert(p.to_path_buf());
        } else {
            self.edit.remove(p);
        }
    }

    pub fn scroll_by(&mut self, notches: i32) {
        // a wheel notch is about three lines
        self.scroll = (self.scroll - notches as f32 * 54.0).clamp(0.0, (self.page - self.view).max(0.0));
    }

    /// Keys that move the page. Returns whether the key was one of them.
    pub fn scroll_key(&mut self, k: &Key) -> bool {
        let page = (self.view * 0.9).max(40.0);
        let to = match k {
            Key::Up | Key::Nav { nav: Nav::Up, .. } => self.scroll - 36.0,
            Key::Down | Key::Nav { nav: Nav::Down, .. } => self.scroll + 36.0,
            Key::PageUp | Key::Nav { nav: Nav::PageUp, .. } => self.scroll - page,
            Key::PageDown | Key::Nav { nav: Nav::PageDown, .. } => self.scroll + page,
            Key::Home | Key::Nav { nav: Nav::Home, .. } => 0.0,
            Key::End | Key::Nav { nav: Nav::End, .. } => self.page,
            Key::Char(' ') => self.scroll + page,
            _ => return false,
        };
        self.scroll = to.clamp(0.0, (self.page - self.view).max(0.0));
        true
    }

    /// Draw the file's text, rendered, inside `body`.
    pub fn draw(&mut self, g: &mut Gfx, body: R, s: f32, path: Option<&Path>, text: &str, accent: [u8; 4]) {
        if self.shown.as_deref() != path {
            self.shown = path.map(Path::to_path_buf);
            self.scroll = 0.0;
        }
        g.rrect(body.x, body.y, body.w, body.h, 10.0 * s, rgb(12, 15, 28));
        g.rrect_outline(body.x, body.y, body.w, body.h, 10.0 * s, 1.0, [accent[0], accent[1], accent[2], 46]);
        let pad = 18.0 * s;
        let width = (body.w - 2.0 * pad - 10.0 * s).clamp(80.0, 920.0 * s);
        let key = (hash(text), width as u32);
        if self.cache.as_ref().is_none_or(|c| (c.0, c.1) != key) {
            self.cache = Some((key.0, key.1, mdtext::layout_with(g, text, 15.5 * s, width, 1.32)));
        }
        let Some((_, _, l)) = self.cache.as_ref() else { return };
        self.page = l.height + 2.0 * pad;
        self.view = body.h;
        self.scroll = self.scroll.clamp(0.0, (self.page - self.view).max(0.0));
        let clip = R::new(body.x + 1.0, body.y + 1.0, body.w - 2.0, body.h - 2.0);
        let (x, y) = (body.x + pad, body.y + pad - self.scroll);
        g.clipped(clip, |g| mdtext::draw(g, l, x, y, TEXT, (clip.y, clip.y + clip.h), width));
        if self.page > self.view {
            crate::fx::scrollbar(g, R::new(body.x + body.w - 8.0 * s, body.y + 6.0 * s, 4.0 * s, body.h - 12.0 * s), s, self.scroll as usize, self.view as usize, self.page as usize, accent);
        }
        if l.lines.is_empty() {
            g.text(x, body.y + pad, 14.0 * s, crate::ui::MUTED, "This file is empty. Press Edit to write in it.", crate::gfx::Face::Sans);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn markdown_files_are_told_by_their_extension() {
        for p in ["README.md", "notes/a.MD", "x.markdown", "y.mdown"] {
            assert!(is_markdown(Path::new(p)), "{p}");
        }
        for p in ["a.rs", "md", "a.mdx", "Makefile"] {
            assert!(!is_markdown(Path::new(p)), "{p}");
        }
    }

    #[test]
    fn the_page_scrolls_within_its_limits_by_wheel_and_keys() {
        let mut m = MdPreview { page: 1000.0, view: 400.0, ..Default::default() };
        m.scroll_by(-3);
        assert!(m.scroll > 0.0, "a wheel notch down moves the page up");
        m.scroll_by(1000);
        assert_eq!(m.scroll, 0.0, "not above the top");
        assert!(m.scroll_key(&Key::End));
        assert_eq!(m.scroll, 600.0, "the end shows the last of the page");
        assert!(m.scroll_key(&Key::PageUp) && m.scroll < 600.0);
        assert!(m.scroll_key(&Key::Home) && m.scroll == 0.0);
        assert!(!m.scroll_key(&Key::Char('x')), "other keys are not scrolling");
    }

    #[test]
    fn the_choice_to_edit_is_kept_per_file() {
        let mut m = MdPreview::default();
        let (a, b) = (Path::new("/w/a.md"), Path::new("/w/b.md"));
        assert!(!m.editing(a));
        m.set_edit(a, true);
        assert!(m.editing(a) && !m.editing(b));
        m.set_edit(a, false);
        assert!(!m.editing(a));
    }
}
