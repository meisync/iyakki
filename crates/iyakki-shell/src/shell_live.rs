//! Painting only the live picture of the open app (the browser's full web engine) between full redraws.
//!
//! The shell draws its whole scene in software on every change, which at 2560x1600 costs 12 to 95 ms, while a page that is
//! scrolled or plays a video changes only the page rectangle. When the open app says that only its live picture changed
//! (`App::redraw_kind` is `Live`), nothing else on screen changed (no event made the scene stale) and nothing is drawn over the
//! picture, the shell copies just the changed rows of the picture into the scene it kept and the screen write handles those rows
//! only. Anything unsure falls back to the whole scene, and the backend still draws the whole scene at least every 100 ms, so a
//! missed overlay costs one short flicker at most, never a lasting wrong picture.

use super::*;

impl Shell {
    /// Nothing is drawn over the app's live picture: no menu, card, toast, dialog, popover or sign, no several panes, and the
    /// pointer is below the header (the clock's tooltip hangs from it).
    pub(super) fn live_clear(&self) -> bool {
        let s = scale_for(self.size.0, self.size.1);
        self.session.is_some()
            && !self.ws_active()
            && self.ctx.is_none()
            && self.toast.is_none()
            && self.fnk.osd.is_none()
            && !self.fnk.power_card
            && !self.help
            && matches!(self.shot, Shot::Off)
            && self.clip.is_none()
            && self.ring.is_none()
            && self.hostdlg.is_none()
            && !self.drag_over
            && self.login.as_ref().is_none_or(|m| m.hidden())
            && !self.devpop.open
            && !self.brand.open
            && !self.installs.open
            && !self.startup.open
            && !self.battery.open
            && self.loss.card_until.is_none()
            && iyakki_storage::loss::snapshot().state.healthy()
            && self.env.services.camera.in_use().is_none()
            && self.mouse.1 as f32 > (HEADER_H + 80.0) * s
    }

    /// Between full redraws: paint what changed in the open app's live picture. Returns the screen rectangle `(x, y, w, h)`
    /// written (the backend writes those rows to the screen), or `None` when the whole scene has to be drawn instead (also when
    /// nothing is pending). Call after the events and the heartbeat were handled and no event made the scene stale.
    pub fn live_update(&mut self, g: &mut Gfx) -> Option<(i32, i32, i32, i32)> {
        if !std::mem::take(&mut self.live) {
            return None;
        }
        if !self.live_clear() {
            return None;
        }
        let Screen::App(app, _) = &mut self.screen else { return None };
        let done = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| app.live_draw(g).map(|bound| (bound, app.live_rects(), app.live_full_every_ms())))).ok().flatten();
        let (bound, rects, every) = done?;
        self.live_rects = rects;
        // an app that repaints only small rectangles says how rarely the whole scene needs drawing as a safety net: half a
        // second after its last live frame the usual 100 ms applies again
        self.live_every_ms = every.max(100);
        self.live_until = self.tick + 15;
        Some(bound)
    }

    /// The rectangles `(x, y, w, h)` painted by the last successful [`Shell::live_update`] when the app named them (empty: only the bounding rectangle it returned): the screen write copies only these.
    pub fn take_live_rects(&mut self) -> Vec<(i32, i32, i32, i32)> {
        std::mem::take(&mut self.live_rects)
    }

    /// The front end calls [`Shell::live_update`] between whole redraws (the framebuffer display does; the remote display, the window and
    /// the multi-display driver do not): off, a new live picture makes the whole scene stale as it always did.
    pub fn set_live_paint(&mut self, on: bool) {
        self.live_paint = on;
        if !on {
            self.live = false;
        }
    }

    /// True when a live picture update is waiting (the backend then tries [`Shell::live_update`] before a whole redraw).
    pub fn live_pending(&self) -> bool {
        self.live
    }
}
