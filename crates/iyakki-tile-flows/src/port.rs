//! What the FLOWS tile asks of the shell (alpha.8 N1): the account's flows environment is [`iyakki_flows_model::port::FlowsPort`] (data only);
//! everything else the tile used to reach through the shell's context is [`FlowsShell`] (whether the assistant's tools are on, one file the
//! person names for import) and [`ChatPane`] (the assistant beside the flow). The tile holds no `Services`, no chat app and no file path.

use iyakki_uikit::gfx::Gfx;
use iyakki_uikit::ui::{Event, R};

/// What a screen of the tile answers to an event: stay, or go back to the launcher.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AppFlow {
    Stay,
    Back,
}

/// What the tile says of files dropped on the window.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DropAction {
    /// The tile handled them itself.
    Taken,
    /// Nothing sensible to do here: say so.
    Ignore(String),
}

/// The shell's side of the tile for one call.
pub trait FlowsShell {
    /// The shell's own context, for the chat pane's implementation to read back (nothing else may use it; the tile never looks inside).
    fn shell(&self) -> &dyn std::any::Any;
    /// The assistant's tools are enabled for this account.
    fn agent_enabled(&self) -> bool;
    /// Read the flow file the person named, inside their folders only: the path is resolved against their home, judged by their file rules and
    /// refused above `max` bytes. `Err` is the sentence to show.
    fn read_import(&self, path: &str, max: u64) -> Result<String, String>;
}

/// The assistant's chat, shown beside the flow. Its click ids live in the slot the shell gives it ([`crate::flows::CHAT_SLOT`]); the tile asks the
/// pane to decode the events of its slot and to draw itself in it.
pub trait ChatPane {
    /// An event for the pane (already decoded from its slot); `Back` when the pane wants to leave (Esc), which hands the keyboard back to the flow.
    fn handle(&mut self, ev: &Event, cx: &dyn FlowsShell) -> AppFlow;
    /// The event behind an id of the pane's slot, or `None` when the event is not the pane's.
    fn decode_slot(&self, ev: &Event) -> Option<Event>;
    /// Draw the pane into `area`, moving the click regions it registers into its slot.
    fn render(&mut self, g: &mut Gfx, area: R, s: f32, cx: &dyn FlowsShell, tick: u64);
    /// The pane itself, for the shell's own tests to read the thread (the tile never calls it).
    fn as_any(&self) -> &dyn std::any::Any;
    /// A run of the assistant is going on.
    fn is_running(&self) -> bool;
    /// The assistant waits for the person to approve something.
    fn pending_approval(&self) -> bool;
    /// A line said in the thread by the screen itself (not by the person, not sent to the model).
    fn local_note(&mut self, cx: &dyn FlowsShell, text: &str);
    /// Switch the assistant's tools on for the thread.
    fn set_agent_mode(&mut self, on: bool);
    /// Auto approval or autopilot is on.
    fn auto_active(&self) -> bool;
    /// End auto approval unless the person pinned it.
    fn end_auto_unless_pinned(&mut self);
    /// What is typed in the composer now.
    fn input_text(&self) -> String;
    /// Send a question now.
    fn ask(&mut self, cx: &dyn FlowsShell, text: &str);
    /// Put a text in the composer (it is sent when the person presses Enter).
    fn prefill(&mut self, text: &str);
}

/// The program's own picture files: the pictures a browser step saved (the run's Pictures tab, the live view of an open browser session) and the
/// pages the QA report is made of. Only files inside `<data>/artifacts` are read; any other path is refused (`None`).
pub trait PictureFiles {
    /// Read and decode a picture (premultiplied, as the renderer wants it); `None` when the path is outside the folder, over `max` bytes, missing or
    /// not a picture this build can show.
    fn pixmap(&self, data: &std::path::Path, path: &str, max: usize) -> Option<std::sync::Arc<tiny_skia::Pixmap>>;
    /// The bytes of a picture file of the folder (for the report's web page); `None` outside it.
    fn bytes(&self, data: &std::path::Path, path: &str) -> Option<Vec<u8>>;
}
