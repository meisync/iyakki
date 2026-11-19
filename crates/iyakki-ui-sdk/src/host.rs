//! The host: the one piece that drives an app on any target. It owns the app and its context data, turns raw pointers into gestures and
//! taps into actions (through the hit regions of the last frame), resizes and moves the pane, and publishes the dock status.
//! The desktop shell, the browser preview and the tests all use this same driver, so an app behaves the same everywhere.

use iyakki_uikit::look::Look;
use crate::typeface::TypeStyle;

use crate::app::{Flow, UiApp};
use crate::canvas::Canvas;
use crate::cx::UiCx;
use crate::desktop::{Arrangement, DockPort, Pane, ViewInfo, WallpaperPort};
use crate::event::{Gesture, Phase, Pointer, UiEvent};
use crate::gesture::Recognizer;
use crate::list::{DisplayList, ListCanvas};

pub struct Host<A: UiApp> {
    pub app: A,
    pub look: Look,
    pub arrangement: Arrangement,
    pub pane: Pane,
    pub view: ViewInfo,
    pub touch: bool,
    pub text_scale: f32,
    /// The person's type choices (family, line height, letter spacing, tabular figures); the shell sets them from the core's settings.
    pub type_style: TypeStyle,
    pub dock: Box<dyn DockPort>,
    pub wallpaper: Box<dyn WallpaperPort>,
    recognizer: Recognizer,
    last: Option<DisplayList>,
    /// Where the current drag began and the action of the hit region under it (for drag and drop).
    drag: Option<((f32, f32), Option<u32>)>,
    /// The clock of the last heartbeat (`tick`), in milliseconds: what `UiCx::now_ms` tells the app.
    now_ms: u64,
}

impl<A: UiApp> Host<A> {
    /// The pane starts as the whole primary screen in the view "main".
    pub fn new(app: A, look: Look, arrangement: Arrangement, dock: Box<dyn DockPort>, wallpaper: Box<dyn WallpaperPort>) -> Host<A> {
        let s = arrangement.primary().clone();
        let (w, h) = s.logical_size();
        Host {
            app,
            look,
            pane: Pane { screen: s.id, view: "main".into(), w, h },
            view: ViewInfo { name: "main".into(), index: 0, count: 1 },
            arrangement,
            touch: false,
            text_scale: 1.0,
            type_style: TypeStyle::default(),
            dock,
            wallpaper,
            recognizer: Recognizer::new(),
            last: None,
            drag: None,
            now_ms: 0,
        }
    }

    /// Give the app one event.
    pub fn event(&mut self, ev: &UiEvent) -> Flow {
        let Host { app, look, arrangement, pane, view, touch, text_scale, type_style, dock, wallpaper, now_ms, .. } = self;
        let flow = {
            let mut cx = UiCx { look, arrangement, pane, view, touch: *touch, text_scale: *text_scale, type_style: *type_style, now_ms: *now_ms, dock: &mut **dock, wallpaper: &mut **wallpaper };
            app.event(ev, &mut cx)
        };
        if let Some(st) = app.dock_status() {
            dock.set_status(app.id(), st);
        }
        flow
    }

    /// The action of the hit region of the last frame at a place: what a tap there would be told (the tile adapter uses it to tell a tap
    /// the host already resolved from the tile ABI's `Hit` event for the same tap).
    pub fn action_at(&self, x: f32, y: f32) -> Option<u32> {
        self.last.as_ref().and_then(|l| l.hit_test(x, y)).map(|h| h.action)
    }

    /// Whether the app wants the on-screen keyboard now (a field is in focus). The shell, the tile adapter and the browser read this after
    /// each event and show or hide the keyboard.
    pub fn wants_keyboard(&self) -> bool {
        self.app.wants_keyboard()
    }

    /// A raw pointer: it goes to the app, and the gestures it makes follow; a tap on a hit region of the last frame is its `Action`.
    pub fn pointer(&mut self, p: Pointer) -> Flow {
        let mut flow = self.event(&UiEvent::Pointer(p));
        for g in self.recognizer.feed(&p) {
            flow = self.gesture(g, flow);
        }
        flow
    }

    fn gesture(&mut self, g: Gesture, so_far: Flow) -> Flow {
        let mut flow = so_far;
        let action = match g {
            Gesture::Tap { x, y } => self.last.as_ref().and_then(|l| l.hit_test(x, y)).map(|h| h.action),
            _ => None,
        };
        if self.event(&UiEvent::Gesture(g)) == Flow::Back {
            flow = Flow::Back;
        }
        // drag and drop: a drag that starts on a hit region and ends over a different one is a Drop of the first onto the second
        match g {
            Gesture::DragStart { x, y } => self.drag = Some(((x, y), self.action_at(x, y))),
            Gesture::DragEnd { dx, dy } => {
                if let Some(((sx, sy), Some(from))) = self.drag.take() {
                    if let Some(to) = self.action_at(sx + dx, sy + dy).filter(|t| *t != from) {
                        if self.event(&UiEvent::Drop { from, to }) == Flow::Back {
                            flow = Flow::Back;
                        }
                    }
                }
            }
            _ => {}
        }
        if let Some(a) = action {
            if self.event(&UiEvent::Action(a)) == Flow::Back {
                flow = Flow::Back;
            }
        }
        flow
    }

    /// The clock moved on (long presses) and the heartbeat.
    pub fn tick(&mut self, now_ms: u64) -> Flow {
        self.now_ms = now_ms;
        let mut flow = Flow::Stay;
        for g in self.recognizer.tick(now_ms) {
            flow = self.gesture(g, flow);
        }
        if self.event(&UiEvent::Tick) == Flow::Back {
            flow = Flow::Back;
        }
        flow
    }

    /// Mouse press, release and move are `Pointer`s too (id 0).
    pub fn mouse(&mut self, x: f32, y: f32, phase: Phase, t_ms: u64) -> Flow {
        self.pointer(Pointer { id: 0, x, y, phase, t_ms })
    }

    /// The pane was resized (a window drag, a rotation).
    pub fn resize(&mut self, w: f32, h: f32) {
        if (self.pane.w, self.pane.h) != (w, h) {
            self.pane.w = w;
            self.pane.h = h;
            self.event(&UiEvent::Resized);
        }
    }

    /// The pane moved to another screen and view (size from the screen unless the caller resizes it after).
    pub fn move_to(&mut self, screen: u32, view: ViewInfo) {
        let s = self.arrangement.resolve(screen).clone();
        let (w, h) = s.logical_size();
        self.pane = Pane { screen: s.id, view: view.name.clone(), w, h };
        self.view = view;
        self.event(&UiEvent::Resized);
    }

    /// The shell's pane manager says where this app is drawn now: the pane (screen, view, size in logical units) and the view's place in the
    /// person's views. `Resized` is sent only when something changed (so a still pane costs the app nothing). True when it did.
    pub fn place(&mut self, pane: Pane, view: ViewInfo) -> bool {
        if self.pane == pane && self.view == view {
            return false;
        }
        self.pane = pane;
        self.view = view;
        self.event(&UiEvent::Resized);
        true
    }

    /// A screen was unplugged: a pane that was on it moves back to the primary screen, so nothing is lost.
    pub fn unplug_screen(&mut self, id: u32) -> bool {
        if !self.arrangement.unplug(id) {
            return false;
        }
        if self.pane.screen == id {
            let v = self.view.clone();
            let p = self.arrangement.primary().id;
            self.move_to(p, v);
        }
        true
    }

    /// Draw the frame on a canvas (the pane takes the canvas's size).
    pub fn draw(&mut self, canvas: &mut dyn Canvas) {
        let (w, h) = canvas.size();
        self.pane.w = w;
        self.pane.h = h;
        let Host { app, look, arrangement, pane, view, touch, text_scale, type_style, dock, wallpaper, now_ms, .. } = self;
        let cx = UiCx { look, arrangement, pane, view, touch: *touch, text_scale: *text_scale, type_style: *type_style, now_ms: *now_ms, dock: &mut **dock, wallpaper: &mut **wallpaper };
        canvas.set_type(cx.type_style.opts());
        app.view(canvas, &cx);
        canvas.set_type(Default::default());
    }

    /// Draw the frame as a display list (what a browser or client gets) and keep it for hit testing.
    /// Switch the skin (the Look) live: the app keeps its state and the next frame is drawn with the new one. The person's choice per
    /// account is kept by the core (`look_registry`), not here.
    pub fn set_look(&mut self, look: Look) {
        self.look = look;
    }

    /// The person's type choices changed (Settings): the next frame uses them.
    pub fn set_type_style(&mut self, t: TypeStyle) {
        self.type_style = t;
    }

    pub fn record(&mut self) -> DisplayList {
        let mut c = ListCanvas::new(self.pane.w, self.pane.h);
        self.draw(&mut c);
        let list = c.finish();
        self.last = Some(list.clone());
        list
    }

    /// Remember a frame drawn some other way (the raster path records the same hits) for tap resolution.
    pub fn set_last(&mut self, list: DisplayList) {
        self.last = Some(list);
    }
}
