//! The picture loop of one connection, shared by the browser page (`server`) and the client session (`clientsession`, F-CORE-30): one turn takes
//! the screen's new pixels, lets the controller decide whether another picture may go out, encodes the difference and asks the screen for the next
//! picture once the line is free. Both servers used to carry their own copy of these lines; a fix to the pacing now lands in both.

use crate::adapt::Controller;
use crate::codec::{Caps, Opts, Streamer};
use crate::screen::Screen;

pub struct PicturePump {
    pub ctl: Controller,
    pub st: Streamer,
    pub caps: Caps,
    armed: bool,
}

impl PicturePump {
    pub fn new(now_ms: u64, deflate: bool) -> PicturePump {
        PicturePump { ctl: Controller::new(now_ms), st: Streamer::new(), caps: Caps { deflate }, armed: false }
    }

    /// One turn. `Some(message)` is one encoded picture (the tile codec's bytes) for the caller to frame and send.
    pub fn turn(&mut self, screen: &mut dyn Screen, now: u64) -> Option<Vec<u8>> {
        self.ctl.tick(now);
        if let Some(p) = screen.take_pixels() {
            self.armed = false;
            self.st.set_source(p.w as usize, p.h as usize, p.rgba);
        }
        let mut sent = None;
        if self.ctl.can_send(now) && self.st.wants_encode(now) {
            let o = Opts { level: self.ctl.level(), caps: self.caps, budget: self.ctl.budget(), now_ms: now };
            if let Some(e) = self.st.encode(&o) {
                self.ctl.on_send(e.seq, e.msg.len(), now);
                sent = Some(e.msg);
            }
        }
        if !self.armed && self.ctl.can_send(now) {
            self.armed = true;
            screen.arm();
        }
        sent
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::input::Input;
    use crate::screen::{Frame, Pixels};

    struct Fake {
        next: Option<Pixels>,
        armed: u32,
    }
    impl Screen for Fake {
        fn user(&self) -> String {
            "amy".into()
        }
        fn input(&mut self, _: Input) {}
        fn viewing(&mut self, _: bool) {}
        fn step(&mut self) -> Option<Frame> {
            None
        }
        fn take_copy(&mut self) -> Option<String> {
            None
        }
        fn arm(&mut self) {
            self.armed += 1;
        }
        fn take_pixels(&mut self) -> Option<Pixels> {
            self.next.take()
        }
        fn finished(&self) -> bool {
            false
        }
    }

    #[test]
    fn a_new_picture_is_encoded_once_the_line_is_free_and_the_screen_is_asked_for_the_next_one_only_once() {
        let mut screen = Fake { next: Some(Pixels { w: 64, h: 32, rgba: vec![200; 64 * 32 * 4] }), armed: 0 };
        let mut pump = PicturePump::new(0, false);
        let first = pump.turn(&mut screen, 10);
        assert!(first.is_some_and(|m| !m.is_empty()), "the first picture goes out");
        // the line is busy until the page acknowledges: nothing more goes out and the screen is not asked yet
        assert!(pump.turn(&mut screen, 20).is_none());
        assert_eq!(screen.armed, 0);
        pump.ctl.on_ack(1, 30);
        // (injected clock: later turns, never a sleep) the line frees up and the screen is asked for the next picture, once
        for t in (40..2000).step_by(10) {
            assert!(pump.turn(&mut screen, t).is_none(), "the screen has not changed");
        }
        assert_eq!(screen.armed, 1, "the screen is asked once");
        // a new picture from the screen is encoded
        screen.next = Some(Pixels { w: 64, h: 32, rgba: vec![30; 64 * 32 * 4] });
        assert!(pump.turn(&mut screen, 2100).is_some());
    }
}
