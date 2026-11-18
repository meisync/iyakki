//! What the MEDIA tile asks of the rest of the system (the third tile moved onto the app standard, docs/development/APP-DEVELOPMENT-PROCEDURE.md section 12).
//!
//! The tile (`media.rs`) holds no `Services` and no `AppCx`. It gets its own things from the port once, when it opens ([`MediaParts`]: the person's
//! playlists and folders, the library index, the download queue) and asks the port for what depends on the account: who the person is, what the policy lets
//! them read and write, the context a download needs, the profile folder of the video engine, and the audit line. The real port ([`CxPort`]) is the only
//! place that names the services.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use iyakki_shared::media::MediaStore;
use iyakki_shared::mediaindex::{ItemKind};
use iyakki_shared::mediaindex::{MediaIndex};
use iyakki_agent_api::DownloadQueueApi;
use iyakki_shared::mediaqueue::Who;

use super::AppCx;

/// The things the tile keeps for one person.
pub struct MediaParts {
    pub store: MediaStore,
    pub index: Arc<Mutex<MediaIndex>>,
    pub dlq: Arc<dyn DownloadQueueApi>,
}

pub trait MediaPort {
    /// The signed-in person's name.
    fn user(&self) -> String;
    /// The account's policy (a copy, for a scan that runs on its own thread).
    fn policy(&self) -> iyakki_sec::policy::Policy;
    fn may_read(&self, path: &str) -> bool;
    fn may_write(&self, path: &str) -> bool;
    /// The playlists and folders, the index and the download queue of this person.
    fn parts(&self) -> MediaParts;
    /// What a download needs to start (the services, the policy, the person, the folders it may go to).
    fn download_context(&self, folders: Vec<String>) -> Who;
    /// The folder an automatic download of this kind would go to.
    fn planned_folder(&self, folders: &[String], kind: Option<ItemKind>) -> Option<PathBuf>;
    /// The profile folder of the embedded web engine for this person (a video plays in it).
    fn web_profile(&self) -> Result<PathBuf, String>;
    fn audit(&self, action: &str, subject: &str);
}

/// The real port, over the shell's context.
pub struct CxPort<'a>(pub &'a AppCx);

impl MediaPort for CxPort<'_> {
    fn user(&self) -> String {
        self.0.session.user.clone()
    }
    fn policy(&self) -> iyakki_sec::policy::Policy {
        self.0.session.policy.clone()
    }
    fn may_read(&self, path: &str) -> bool {
        self.0.session.policy.allows_read(&self.0.session.user, path)
    }
    fn may_write(&self, path: &str) -> bool {
        self.0.session.policy.allows_write(&self.0.session.user, path)
    }
    fn parts(&self) -> MediaParts {
        let svc = &self.0.services;
        crate::agent().player_register_dir(svc.clis().bin_dir());
        MediaParts { store: MediaStore::new(self.0.data_dir(), &self.0.session.user), index: svc.media_index(&self.0.session.user), dlq: svc.b5().media_queue(&self.0.session.user).expect("the core serves the download queue") }
    }
    fn download_context(&self, folders: Vec<String>) -> Who {
        Who { policy: self.0.session.policy.clone(), user: self.0.session.user.clone(), folders }
    }
    fn planned_folder(&self, folders: &[String], kind: Option<ItemKind>) -> Option<PathBuf> {
        crate::agent().host_probe().media_planned_folder(folders, &self.0.session.policy, &self.0.session.user, kind)
    }
    fn web_profile(&self) -> Result<PathBuf, String> {
        crate::agent().webengine_prepare_profile(self.0.data_dir(), &self.0.session.user)
    }
    fn audit(&self, action: &str, subject: &str) {
        self.0.audit.record(&self.0.session.user, action, subject);
    }
}
