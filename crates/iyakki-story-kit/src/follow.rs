//! Following an external coding agent's run that was started without tools: the loop over a plain feed of messages, so the agent's own
//! run handle stays behind `iyakki_shared::story::follow_agent_run` and a tile can fake the feed (alpha.11 AP1, D-P6-03).

use std::sync::mpsc::RecvTimeoutError;
use std::time::Duration;

/// What a run says, reduced to what the story needs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RunMsg {
    Delta(String),
    Thinking(String),
    /// The model's text with the ids of the tool calls it asked for (all are denied).
    Model { text: String, calls: Vec<String> },
    /// An approval that is asked for (denied).
    Approval(String),
    Final(String),
    Failed(String),
    Stopped,
    Other,
}

/// A run: messages to wait for, a way to end it and a way to deny a request.
pub trait RunFeed {
    fn recv(&self, wait: Duration) -> Result<RunMsg, RecvTimeoutError>;
    fn cancel(&self);
    fn deny(&self, id: &str);
}

/// Follow a run until it ends. `on_text` gets the story so far each time it grows and `stopped` says whether to end the run (checked
/// often). A tool request or approval that still appears is denied. Ok is the whole story (what was written so far, when stopped); Err
/// says why the agent could not write.
pub fn follow_run(h: &dyn RunFeed, stopped: &dyn Fn() -> bool, on_text: &mut dyn FnMut(&str), on_thinking: &mut dyn FnMut(&str)) -> Result<String, String> {
    let mut last = String::new();
    loop {
        match h.recv(Duration::from_millis(100)) {
            Ok(RunMsg::Delta(text)) => {
                last = text;
                on_text(&last);
                if stopped() {
                    h.cancel();
                    return Ok(last);
                }
            }
            Ok(RunMsg::Thinking(text)) => on_thinking(&text),
            Ok(RunMsg::Model { text, calls }) => {
                if !text.is_empty() {
                    last = text;
                }
                for c in calls {
                    h.deny(&c);
                }
            }
            Ok(RunMsg::Approval(id)) => h.deny(&id),
            Ok(RunMsg::Final(text)) => {
                if !text.trim().is_empty() {
                    on_text(&text);
                    return Ok(text);
                }
                return if last.trim().is_empty() { Err("it sent no story".into()) } else { Ok(last) };
            }
            Ok(RunMsg::Failed(error)) => return Err(error),
            Ok(RunMsg::Stopped) => return if last.trim().is_empty() { Err("it was stopped".into()) } else { Ok(last) },
            Ok(RunMsg::Other) => {}
            Err(RecvTimeoutError::Timeout) => {
                if stopped() {
                    h.cancel();
                    return Ok(last);
                }
            }
            Err(RecvTimeoutError::Disconnected) => return if last.trim().is_empty() { Err("it ended without a story".into()) } else { Ok(last) },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::{Cell, RefCell};
    use std::collections::VecDeque;

    struct Fake {
        script: RefCell<VecDeque<Result<RunMsg, RecvTimeoutError>>>,
        cancelled: Cell<bool>,
        denied: RefCell<Vec<String>>,
    }
    impl Fake {
        fn new(v: Vec<Result<RunMsg, RecvTimeoutError>>) -> Fake {
            Fake { script: RefCell::new(v.into()), cancelled: Cell::new(false), denied: RefCell::new(vec![]) }
        }
    }
    impl RunFeed for Fake {
        fn recv(&self, _w: Duration) -> Result<RunMsg, RecvTimeoutError> {
            self.script.borrow_mut().pop_front().unwrap_or(Err(RecvTimeoutError::Disconnected))
        }
        fn cancel(&self) {
            self.cancelled.set(true);
        }
        fn deny(&self, id: &str) {
            self.denied.borrow_mut().push(id.into());
        }
    }
    fn run(f: &Fake, stop: bool) -> (Result<String, String>, Vec<String>) {
        let mut seen = vec![];
        let r = follow_run(f, &|| stop, &mut |t| seen.push(t.to_string()), &mut |_| {});
        (r, seen)
    }

    #[test]
    fn the_final_text_wins_and_tool_requests_are_denied() {
        let f = Fake::new(vec![Ok(RunMsg::Delta("Once".into())), Ok(RunMsg::Model { text: String::new(), calls: vec!["c1".into()] }), Ok(RunMsg::Approval("a1".into())), Ok(RunMsg::Final("Once upon".into()))]);
        let (r, seen) = run(&f, false);
        assert_eq!(r, Ok("Once upon".to_string()));
        assert_eq!(seen, vec!["Once".to_string(), "Once upon".to_string()]);
        assert_eq!(*f.denied.borrow(), vec!["c1".to_string(), "a1".to_string()]);
    }

    #[test]
    fn stopping_cancels_the_run_and_keeps_what_was_written() {
        let f = Fake::new(vec![Ok(RunMsg::Delta("part".into()))]);
        assert_eq!(run(&f, true).0, Ok("part".to_string()));
        assert!(f.cancelled.get());
        let f = Fake::new(vec![Err(RecvTimeoutError::Timeout)]);
        assert_eq!(run(&f, true).0, Ok(String::new()));
    }

    #[test]
    fn a_failed_stopped_or_vanished_run_says_why() {
        assert_eq!(run(&Fake::new(vec![Ok(RunMsg::Failed("no".into()))]), false).0, Err("no".to_string()));
        assert_eq!(run(&Fake::new(vec![Ok(RunMsg::Stopped)]), false).0, Err("it was stopped".to_string()));
        assert_eq!(run(&Fake::new(vec![Ok(RunMsg::Final("  ".into()))]), false).0, Err("it sent no story".to_string()));
        assert_eq!(run(&Fake::new(vec![]), false).0, Err("it ended without a story".to_string()));
        assert_eq!(run(&Fake::new(vec![Ok(RunMsg::Delta("x".into()))]), false).0, Ok("x".to_string()));
    }
}
