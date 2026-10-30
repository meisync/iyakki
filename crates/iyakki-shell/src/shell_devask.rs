//! The shell's side of the developer-mode approval (`devask.rs`): reading the waiting requests on the heartbeat, redrawing while one
//! waits (the pulse) and the card's buttons. Only an administrator sees it, the same people who may answer in Settings.

use super::*;
use iyakki_agent::devmode::DevMode;

impl Shell {
    pub(super) fn devask_tick(&mut self) {
        if self.tick % crate::devask::POLL_EVERY == 4 {
            let list = match &self.session {
                Some(s) if s.policy.manage_settings => {
                    let d = DevMode::open(self.env.services.dir());
                    if d.is_on() { d.shell_pending() } else { Vec::new() }
                }
                _ => Vec::new(),
            };
            if self.devask.update(list) {
                self.stale = true;
            }
        }
        // the pulse is a moving picture: a few redraws a second, and none when nothing waits
        if self.devask.pending.is_some() && self.tick % crate::devask::FRAME_EVERY == 0 {
            self.stale = true;
        }
    }

    pub(super) fn devask_event(&mut self, id: usize) {
        use crate::devask as da;
        let Some(s) = self.session.clone() else { return };
        self.stale = true;
        match id {
            da::OPEN => self.devask.open = !self.devask.open,
            da::CLOSE => self.devask.open = false,
            _ => {
                let Some(req) = self.devask.pending.clone() else { return };
                let d = DevMode::open(self.env.services.dir());
                let p = &s.policy;
                let r = match id {
                    da::ONCE => d.shell_approve(p, &s.user, &req.computer, 0).map(|g| format!("{} may {} for one session.", g.computer, da::doing(&g.kind))),
                    da::AUTO if !da::auto_allowed(&req) => Err("This request is always asked each time: use Approve.".to_string()),
                    da::AUTO => d.shell_approve(p, &s.user, &req.computer, 10).and_then(|g| {
                        d.shell_auto_on(p, &s.user, &req.computer, iyakki_agent::devmode::grants::AUTO_DEFAULT_HOURS).map(|_| format!("{} may run commands for 10 minutes; its next requests are approved by themselves for {} hours (never as root).", g.computer, iyakki_agent::devmode::grants::AUTO_DEFAULT_HOURS))
                    }),
                    _ => d.shell_deny(p, &s.user, &req.computer).map(|_| format!("{} was refused.", req.computer)),
                };
                match r {
                    Ok(m) => {
                        self.env.audit.record(&s.user, "devmode.shell", &m);
                        self.say_info(m);
                        self.devask.open = false;
                    }
                    // a refusal keeps the card up so the person can choose again
                    Err(e) => self.say(e),
                }
                // the next waiting request (if any) shows at once
                let list = d.shell_pending();
                self.devask.update(list);
            }
        }
    }
}
