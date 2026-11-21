//! One app shown in two desktop views at once (F-VIEW-02): the live app is moved out of the shell's per-view slot into a shared holder that
//! both slots draw from. `SharedApp` is an `App` that forwards every call to the one instance behind a lock, so the views take turns:
//! one writer at a time, and what the person did in one view is there in the other. The shell wraps the open app with
//! [`SharedApp::pair`] (or hands out one more [`App::share_handle`] of an app that is already shared) and puts the second handle into the
//! other view. The instance lives until the last view drops it. A handle in a view that is not showing gets the shell's slow heartbeat like
//! any hidden app; the showing handle's own heartbeat keeps the instance moving.

use super::*;

/// One handle to a shared app instance.
pub struct SharedApp {
    inner: Arc<Mutex<Box<dyn App>>>,
}

impl SharedApp {
    /// Wrap `app` and return two handles to it.
    pub fn pair(app: Box<dyn App>) -> (Box<dyn App>, Box<dyn App>) {
        let inner = Arc::new(Mutex::new(app));
        (Box::new(SharedApp { inner: inner.clone() }), Box::new(SharedApp { inner }))
    }

    /// How many handles hold the instance (a test reads it).
    pub fn handles(&self) -> usize {
        Arc::strong_count(&self.inner)
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Box<dyn App>> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }
}

impl App for SharedApp {
    /// One more handle to the same instance.
    fn share_handle(&self) -> Option<Box<dyn App>> {
        Some(Box::new(SharedApp { inner: self.inner.clone() }))
    }
    fn title(&self) -> &'static str {
        self.lock().title()
    }
    fn page(&self) -> String {
        self.lock().page()
    }
    fn dialog_title(&self) -> Option<String> {
        self.lock().dialog_title()
    }
    fn icon(&self) -> Icon {
        self.lock().icon()
    }
    fn accent(&self) -> [u8; 4] {
        self.lock().accent()
    }
    fn hint(&self) -> String {
        self.lock().hint()
    }
    fn help(&self) -> Vec<String> {
        self.lock().help()
    }
    fn handle(&mut self, ev: &Event, cx: &AppCx) -> Flow {
        self.lock().handle(ev, cx)
    }
    fn on_lock(&mut self, cx: &AppCx) {
        self.lock().on_lock(cx)
    }
    fn list_frame(&mut self, w: f32, h: f32, cx: &AppCx) -> Option<String> {
        self.lock().list_frame(w, h, cx)
    }
    fn list_apply(&mut self, batch: &str, cx: &AppCx) -> Option<Result<(String, bool), String>> {
        self.lock().list_apply(batch, cx)
    }
    fn leave_guard(&mut self, cx: &AppCx) -> bool {
        self.lock().leave_guard(cx)
    }
    fn scroll_px(&mut self, px: i32, cx: &AppCx) -> bool {
        self.lock().scroll_px(px, cx)
    }
    fn claims_f12(&self) -> bool {
        self.lock().claims_f12()
    }
    fn wants_alt(&self) -> bool {
        self.lock().wants_alt()
    }
    fn scroll_raw(&self) -> bool {
        self.lock().scroll_raw()
    }
    fn media_key(&mut self, k: crate::fnkeys::FnKey, cx: &AppCx) -> bool {
        self.lock().media_key(k, cx)
    }
    fn take_request(&mut self) -> Option<(String, String)> {
        self.lock().take_request()
    }
    fn keeps_screen_on(&self) -> bool {
        self.lock().keeps_screen_on()
    }
    fn status(&self, cx: &AppCx) -> Option<iyakki_core_api::AppStatus> {
        self.lock().status(cx)
    }
    fn touchbar_state(&self) -> (bool, bool) {
        self.lock().touchbar_state()
    }
    fn take_jump(&mut self) -> Option<(String, String)> {
        self.lock().take_jump()
    }
    fn place(&mut self, w: f32, h: f32, view: &str, index: usize, count: usize) {
        self.lock().place(w, h, view, index, count)
    }
    fn wants_keyboard(&self) -> bool {
        self.lock().wants_keyboard()
    }
    fn open_item(&mut self, item: &str, cx: &AppCx) {
        self.lock().open_item(item, cx)
    }
    fn resume_item(&self) -> Option<String> {
        self.lock().resume_item()
    }
    fn wants_redraw(&mut self) -> bool {
        self.lock().wants_redraw()
    }
    fn redraw_kind(&mut self) -> browser_full::Redraw {
        self.lock().redraw_kind()
    }
    fn live_draw(&mut self, g: &mut Gfx) -> Option<(i32, i32, i32, i32)> {
        self.lock().live_draw(g)
    }
    fn live_rects(&self) -> Vec<(i32, i32, i32, i32)> {
        self.lock().live_rects()
    }
    fn live_full_every_ms(&self) -> u64 {
        self.lock().live_full_every_ms()
    }
    fn idle_redraw_ms(&self) -> u64 {
        self.lock().idle_redraw_ms()
    }
    fn wants_fast_poll(&self) -> bool {
        self.lock().wants_fast_poll()
    }
    fn poll_redraw_due(&mut self) -> bool {
        self.lock().poll_redraw_due()
    }
    fn pointer_hint(&self, x: i32, y: i32) -> Option<crate::ui::Pointer> {
        self.lock().pointer_hint(x, y)
    }
    fn on_drop(&mut self, paths: &[std::path::PathBuf], cx: &AppCx) -> DropAction {
        self.lock().on_drop(paths, cx)
    }
    fn take_host_save(&mut self) -> Option<(String, Vec<u8>)> {
        self.lock().take_host_save()
    }
    fn take_host_mount_menu(&mut self) -> Option<String> {
        self.lock().take_host_mount_menu()
    }
    fn dropped_done(&mut self, files: &[std::path::PathBuf], cx: &AppCx) {
        self.lock().dropped_done(files, cx)
    }
    fn context_items(&mut self, x: i32, y: i32) -> Vec<crate::ctxmenu::Item> {
        self.lock().context_items(x, y)
    }
    fn context_pick(&mut self, id: &str, cx: &AppCx) -> Flow {
        self.lock().context_pick(id, cx)
    }
    #[cfg(any(test, feature = "testing"))]
    fn probe(&self) -> String {
        self.lock().probe()
    }
    #[cfg(any(test, feature = "testing"))]
    fn probe_link(&self) -> Option<String> {
        self.lock().probe_link()
    }
    #[cfg(any(test, feature = "testing"))]
    fn as_any_tile(&mut self) -> Option<&mut tile::TileApp> {
        None
    }
    #[cfg(any(test, feature = "testing"))]
    fn wasm_tile_text(&mut self) -> String {
        self.lock().wasm_tile_text()
    }
    fn render(&mut self, g: &mut Gfx, area: R, s: f32, cx: &AppCx, tick: u64) {
        self.lock().render(g, area, s, cx, tick)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::apps::tests::cx_with_admin_store;
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct Counter {
        beats: Arc<AtomicUsize>,
        typed: String,
    }

    impl App for Counter {
        fn title(&self) -> &'static str {
            "COUNTER"
        }
        fn icon(&self) -> Icon {
            Icon::Cli
        }
        fn accent(&self) -> [u8; 4] {
            [1, 2, 3, 255]
        }
        fn hint(&self) -> String {
            format!("typed {}", self.typed)
        }
        fn handle(&mut self, ev: &Event, _cx: &AppCx) -> Flow {
            match ev {
                Event::Tick => {
                    self.beats.fetch_add(1, Ordering::SeqCst);
                    Flow::Stay
                }
                Event::Key(crate::ui::Key::Char(c)) => {
                    self.typed.push(*c);
                    Flow::Stay
                }
                _ => Flow::Back,
            }
        }
        fn page(&self) -> String {
            self.typed.clone()
        }
        fn wants_redraw(&mut self) -> bool {
            !self.typed.is_empty()
        }
        fn render(&mut self, _g: &mut Gfx, _area: R, _s: f32, _cx: &AppCx, _tick: u64) {}
    }

    fn counter() -> (Box<dyn App>, Arc<AtomicUsize>) {
        let beats = Arc::new(AtomicUsize::new(0));
        (Box::new(Counter { beats: beats.clone(), typed: String::new() }), beats)
    }

    #[test]
    fn two_handles_are_one_instance_what_one_view_typed_the_other_shows_and_each_view_beats_it() {
        let cx = cx_with_admin_store("sharedapp");
        let (app, beats) = counter();
        let (mut a, mut b) = SharedApp::pair(app);
        a.handle(&Event::Key(crate::ui::Key::Char('x')), &cx);
        b.handle(&Event::Key(crate::ui::Key::Char('y')), &cx);
        assert_eq!(a.page(), "xy");
        assert_eq!(b.page(), "xy");
        assert_eq!(b.hint(), "typed xy");
        assert_eq!((a.title(), b.title()), ("COUNTER", "COUNTER"));
        a.handle(&Event::Tick, &cx);
        b.handle(&Event::Tick, &cx);
        assert_eq!(beats.load(Ordering::SeqCst), 2);
        // a Back in one view is the instance's answer in that view; the instance goes on for the other
        assert!(matches!(a.handle(&Event::Row(0), &cx), Flow::Back));
        assert_eq!(b.page(), "xy");
        assert!(b.wants_redraw());
    }

    #[test]
    fn a_shared_app_hands_out_more_handles_and_the_instance_lives_until_the_last_one_goes() {
        let (app, beats) = counter();
        assert!(app.share_handle().is_none(), "an app that is not shared has no handle to give");
        let (a, b) = SharedApp::pair(app);
        let c = a.share_handle().expect("a shared app gives another handle");
        assert!(c.share_handle().is_some());
        drop(a);
        drop(b);
        // the instance is still there for the third view
        assert_eq!(c.title(), "COUNTER");
        assert_eq!(Arc::strong_count(&beats), 2, "the instance holds the counter");
        drop(c);
        assert_eq!(Arc::strong_count(&beats), 1, "the last handle released the instance");
    }

    #[test]
    fn a_panic_inside_the_app_does_not_lock_the_other_view_out() {
        struct Boom;
        impl App for Boom {
            fn title(&self) -> &'static str {
                "BOOM"
            }
            fn icon(&self) -> Icon {
                Icon::Cli
            }
            fn accent(&self) -> [u8; 4] {
                [0, 0, 0, 255]
            }
            fn hint(&self) -> String {
                String::new()
            }
            fn handle(&mut self, _: &Event, _: &AppCx) -> Flow {
                panic!("the app broke")
            }
            fn render(&mut self, _: &mut Gfx, _: R, _: f32, _: &AppCx, _: u64) {}
        }
        let cx = cx_with_admin_store("sharedappboom");
        let (mut a, b) = SharedApp::pair(Box::new(Boom));
        let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| a.handle(&Event::Tick, &cx)));
        assert!(r.is_err());
        // the lock is poisoned by that panic; the other handle still answers
        assert_eq!(b.title(), "BOOM");
    }
}
