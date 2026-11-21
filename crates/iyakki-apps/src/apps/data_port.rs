//! What the DATA tile asks of the rest of the system (the fifth tile moved onto the app standard, docs/development/APP-DEVELOPMENT-PROCEDURE.md section 12).
//!
//! The data view already worked from an environment object (`DataEnv`: the data service, what the account may read and write, the folders, the file
//! of recent files, the host folders); what was left was the one place that built it from the shell context. That is the port: [`CxPort`] builds
//! the environment for the signed-in person, and a test builds its own.

use std::path::Path;
use std::sync::Arc;

use super::data::DataEnv;
use super::AppCx;

pub trait DataPort {
    /// The environment of this person's view: the data service and the account's file scope.
    fn env(&self) -> DataEnv;
    /// The signed-in person's name (the assistant is told which table this person has open).
    fn user(&self) -> String;
}

/// The real port, over the shell's context.
pub struct CxPort<'a>(pub &'a AppCx);

impl DataPort for CxPort<'_> {
    fn env(&self) -> DataEnv {
        let cx = self.0;
        let (p, u, d) = (cx.session.policy.clone(), cx.session.user.clone(), cx.data_dir().to_path_buf());
        let (c1, c2, c3) = (cx.core.clone(), cx.core.clone(), cx.core.clone());
        let (p2, u2, d2, p3, u3, d3) = (p.clone(), u.clone(), d.clone(), p.clone(), u.clone(), d.clone());
        DataEnv {
            svc: iyakki_data::service(),
            // the account's scopes plus any shared host folder, as they are at this moment
            can_read: Arc::new(move |path| c1.file_scope().roots(&p, &u, &d).read_ok(&p, &u, Path::new(path))),
            can_write: Arc::new(move |path| c2.file_scope().roots(&p2, &u2, &d2).write_ok(&p2, &u2, Path::new(path))),
            roots: Arc::new(move || c3.file_scope().roots(&p3, &u3, &d3)),
            recent: Some(cx.data_dir().join(format!("data-recent-{}.json", cx.session.user))),
            host: Some(super::hostfiles::env_for_cx(cx)),
        }
    }
    fn user(&self) -> String {
        self.0.session.user.clone()
    }
}
