//! What the CAMERA tile asks of the rest of the system: who the person is, whether their account may save in a folder, which camera to use and the
//! audit log. The tile (`camera.rs`) names no `Services`, no session and no `AppCx` outside the `App` trait's entry points; the real port
//! ([`CxPort`]) is the only place that does (docs/development/APP-DEVELOPMENT-PROCEDURE.md section 10).

use std::sync::Arc;

use super::AppCx;
use crate::camera::{v4l2, Opener, Presence};

pub trait CameraPort {
    /// The signed-in person's account name (their home folder is `/home/<name>`).
    fn user(&self) -> String;
    /// May the account write to this folder?
    fn allows_write(&self, dir: &str) -> bool;
    /// The camera in use: the browser's in a browser session, the machine's otherwise.
    fn camera_source(&self) -> (Presence, Arc<dyn Opener>);
    /// A failure sentence as the core words it when the boot drive is gone (a good message comes back unchanged). Taken on every event and kept by the tile.
    fn failure_reworder(&self) -> Arc<dyn Fn(&str) -> String>;
    /// One audit line of the signed-in person.
    fn audit(&self, action: &str, subject: &str);
}

/// The real port, over the shell's context.
pub struct CxPort<'a>(pub &'a AppCx);

impl CameraPort for CxPort<'_> {
    fn user(&self) -> String {
        self.0.session.user.clone()
    }
    fn allows_write(&self, dir: &str) -> bool {
        self.0.session.policy.allows_write(&self.0.session.user, dir)
    }
    fn camera_source(&self) -> (Presence, Arc<dyn Opener>) {
        match iyakki_agent::browsersource::browser_camera_for(&self.0.session.user) {
            Some(m) => (Presence::browser(), Arc::new(crate::camera::browsercam::BrowserOpener::new(m))),
            None => (Presence::system(), Arc::new(v4l2::V4l2Opener)),
        }
    }
    fn failure_reworder(&self) -> Arc<dyn Fn(&str) -> String> {
        let os = self.0.os.clone();
        Arc::new(move |t| os.disks().reword_failure(t))
    }
    fn audit(&self, action: &str, subject: &str) {
        self.0.audit.record(&self.0.session.user, action, subject);
    }
}
