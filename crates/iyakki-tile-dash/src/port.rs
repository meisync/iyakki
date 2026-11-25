//! What the DASH tile asks of the rest of the system (alpha.8 E3): the assistant's pane beside the board ([`ChatPane`]) and the person's files
//! ([`DashPort`]). The tile holds no `Services`, no chat app, no file path of its own and no account store: the shell builds the chat pane and
//! the engine's environment (`iyakki_dash::tools::Env`, which carries the vault names, the audit sink and the accounts of this machine for
//! sharing) and hands them to `DashApp::new`; every call the tile makes into the shell goes through these two traits.

use iyakki_dash::tools::Env;
use iyakki_uikit::ctxmenu::Item;
use iyakki_uikit::gfx::Gfx;
use iyakki_uikit::ui::{Event, R};

use crate::app::Flow;

/// The shell's side of the tile for one call.
pub trait DashPort {
    /// The shell's own context, for the chat pane's implementation to read back (nothing else may use it; the tile never looks inside).
    fn shell(&self) -> &dyn std::any::Any;
    /// Read the dashboard file the person named, inside their folders only: the path is resolved against their home, judged by their file
    /// rules and refused above `max` bytes. `Err` is the sentence to show.
    fn read_dashboard_file(&self, env: &Env, path: &str, max: u64) -> Result<String, String>;
    /// Write an export (a panel's CSV, a dashboard's bundle) as `name` in the person's `Dashboards/exports` folder, under their file rules;
    /// `true` when the person may also be offered to save it on the host computer. `Err` is the sentence to show.
    fn write_export(&self, env: &Env, name: &str, bytes: &[u8]) -> Result<bool, String>;
}

/// The assistant's chat, shown beside the dashboards. Its click ids live in the slot the shell gives it; the tile asks the pane to decode
/// the events of its slot and to draw itself in it.
pub trait ChatPane {
    /// An event for the pane (already decoded from its slot); `Flow::Back` when the pane wants to leave (Esc), which hands the keyboard back to the board.
    fn handle(&mut self, ev: &Event, cx: &dyn DashPort) -> Flow;
    /// The event behind an id of the pane's slot, or `None` when the event is not the pane's.
    fn decode_slot(&self, ev: &Event) -> Option<Event>;
    /// Draw the pane into `area`, moving the click regions it registers into its slot.
    fn render(&mut self, g: &mut Gfx, area: R, s: f32, cx: &dyn DashPort, tick: u64);
    fn wants_redraw(&mut self) -> bool;
    /// A run of the assistant is going on.
    fn is_running(&self) -> bool;
    /// The assistant waits for the person to approve something.
    fn pending_approval(&self) -> bool;
    /// Put a question in the composer (it is sent when the person presses Enter).
    fn prefill(&mut self, text: &str);
    /// What is typed in the composer now.
    fn input_text(&self) -> String;
    fn context_items(&mut self, x: i32, y: i32) -> Vec<Item>;
    fn context_pick(&mut self, id: &str, cx: &dyn DashPort) -> Flow;
    /// A link clicked in the chat that opens another tile: `(what, detail)`, once.
    fn take_request(&mut self) -> Option<(String, String)>;
}

/// A port that has no shell behind it and no files: for the tile's own tests and the test seams that open a screen without a shell.
pub struct NoDashPort;

impl DashPort for NoDashPort {
    fn shell(&self) -> &dyn std::any::Any {
        &()
    }
    fn read_dashboard_file(&self, _: &Env, _: &str, _: u64) -> Result<String, String> {
        Err("files are not available here".into())
    }
    fn write_export(&self, _: &Env, _: &str, _: &[u8]) -> Result<bool, String> {
        Err("files are not available here".into())
    }
}
