//! The shell's side of the developer-mode approval (`devask.rs`): reading the waiting requests on the heartbeat, redrawing while one
//! waits (the pulse) and the card's buttons. Only an administrator sees it, the same people who may answer in Settings.

use crate::stale_stats::mark_stale;
use super::*;

impl Shell {
    pub(super) fn devask_tick(&mut self) {
        if self.tick % crate::devask::POLL_EVERY == 4 {
            let list = match &self.session {
                Some(s) if s.policy.manage_settings => {
                    iyakki_apps::agent().devmode().shell_pending(self.env.data_dir())
                }
                _ => Vec::new(),
            };
            if self.devask.update(list) {
                mark_stale!(self);
            }
        }
        // the pulse is a moving picture: a few redraws a second, and none when nothing waits
        if self.devask.pending.is_some() && self.tick % crate::devask::FRAME_EVERY == 0 {
            mark_stale!(self);
        }
    }

    pub(super) fn devask_event(&mut self, id: usize) {
        use crate::devask as da;
        let Some(s) = self.session.clone() else { return };
        mark_stale!(self);
        match id {
            da::OPEN => self.devask.open = !self.devask.open,
            da::CLOSE => self.devask.open = false,
            _ => {
                let Some(req) = self.devask.pending.clone() else { return };
                let (ag, data) = (iyakki_apps::agent(), self.env.data_dir().to_path_buf());
                let (d, p) = (ag.devmode(), &s.policy);
                let r = match id {
                    da::ONCE => d.shell_approve(&data, p, &s.user, &req.computer, 0).map(|(computer, kind)| format!("{} may {} for one session.", computer, da::doing(&kind))),
                    da::AUTO if !da::auto_allowed(&req) => Err("This request is always asked each time: use Approve.".to_string()),
                    da::AUTO => d.shell_approve(&data, p, &s.user, &req.computer, 10).and_then(|(computer, _)| {
                        d.shell_auto_on(&data, p, &s.user, &req.computer, iyakki_shared::devmode::grants::AUTO_DEFAULT_HOURS).map(|_| format!("{} may run commands for 10 minutes; its next requests are approved by themselves for {} hours (never as root).", computer, iyakki_shared::devmode::grants::AUTO_DEFAULT_HOURS))
                    }),
                    _ => d.shell_deny(&data, p, &s.user, &req.computer).map(|_| format!("{} was refused.", req.computer)),
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
                let list = d.shell_pending(&data);
                self.devask.update(list);
            }
        }
    }
}
