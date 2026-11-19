//! The app contract: three required calls. The same source builds native in the shell, as a WASM tile, and into the browser preview.

use crate::canvas::Canvas;
use crate::cx::UiCx;
use crate::desktop::DockStatus;
use crate::event::UiEvent;

/// What an app answers to an event.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Flow {
    Stay,
    /// Leave the app (back to the launcher).
    Back,
}

pub trait UiApp {
    /// The app id (the dock, the launcher and the policy know it by this).
    fn id(&self) -> &'static str;
    fn title(&self) -> String;
    /// A badge and a status line for this app's dock icon, asked after every event.
    fn dock_status(&self) -> Option<DockStatus> {
        None
    }
    /// True while the app has a text field in focus: the host shows the on-screen keyboard (a touch screen has no other way to type) and
    /// takes it away when this turns false. Asked after every event, like [`UiApp::dock_status`].
    fn wants_keyboard(&self) -> bool {
        false
    }
    fn event(&mut self, ev: &UiEvent, cx: &mut UiCx) -> Flow;
    /// Draw the whole frame. Lay it out from `canvas.size()` and `cx.metrics()`.
    fn view(&mut self, canvas: &mut dyn Canvas, cx: &UiCx);
}
