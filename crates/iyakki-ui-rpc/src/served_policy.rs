//! The calls that used to carry the person's POLICY over the socket (D-SPLIT-03-POLICY, alpha.7 S3): the radio directory search, the study PDF
//! export and the download queue. A UI that names the policy names its own rights, so on the wire there is only the person's NAME: the core reads
//! that account's policy from its own accounts (a name it does not know is refused), judges every folder the UI names by that person's file rules
//! and uses its own data folder. The rules are the ones of the store (`served_store.rs`): all of them fail closed.
#![allow(non_snake_case)] // the generator names the hand-written entry points c_<Trait>_<method>

use std::path::Path;
use std::sync::Arc;

use iyakki_shared::mediaqueue::{Request, Who};
use serde_json::{json, Value};

use crate::served::NOT_SERVED;
use crate::{de, ser, Registry};

impl Registry<'_> {
    /// The folders of `folders` this person may write to, by the machine's file rules (an automatic download never goes anywhere else).
    pub(crate) fn writable_of(&self, user: &str, policy: &iyakki_sec::policy::Policy, folders: Vec<String>) -> Vec<String> {
        let Some(scope) = self.side().file_scope else { return Vec::new() };
        folders.into_iter().filter(|f| scope.check_write(policy, user, Path::new(f)).is_ok()).collect()
    }

    /// The caller as the core's accounts know them, with the policy of the account (never the one the UI sent).
    pub(crate) fn who_of(&self, user: &str, folders: Vec<String>) -> Result<Who, String> {
        let policy = self.policy_of(user)?;
        let folders = self.writable_of(user, &policy, folders);
        Ok(Who { policy, user: user.to_string(), folders })
    }
}

// ---------------------------------------------------------------------------------------------------------------- the UI's end

pub(crate) fn c_ServicesFacetB5_radio_search(s: &crate::RemoteServicesFacetB5, _policy: &iyakki_sec::policy::Policy, user: &str, words: &str, limit: usize) -> (Vec<(String, String)>, Option<String>) {
    s.rpc.call::<(Vec<(String, String)>, Option<String>)>("ServicesFacetB5", s.obj, "radio_search", json!([user, words, limit])).unwrap_or_else(|e| (Vec::new(), Some(format!("{NOT_SERVED}: {e}"))))
}

pub(crate) fn c_ServicesFacetB5_study_pdf_export(s: &crate::RemoteServicesFacetB5, _policy: &iyakki_sec::policy::Policy, user: &str, _data_dir: &Path, item: &iyakki_shared::studypdf::Item, a4: bool, path: Option<&str>) -> Result<String, String> {
    s.rpc.call::<Result<String, String>>("ServicesFacetB5", s.obj, "study_pdf_export", json!([user, item, a4, path])).map_err(|e| format!("{NOT_SERVED}: {e}"))?
}

pub(crate) fn c_DownloadQueueApi_set_context(s: &crate::RemoteDownloadQueueApi, who: Who) {
    let _ = s.rpc.call::<()>("DownloadQueueApi", s.obj, "set_context", json!([who.user, who.folders]));
}

pub(crate) fn c_DownloadQueueApi_enqueue(s: &crate::RemoteDownloadQueueApi, who: Who, req: Request) -> Result<u64, String> {
    s.rpc.call::<Result<u64, String>>("DownloadQueueApi", s.obj, "enqueue", json!([who.user, who.folders, req])).map_err(|e| format!("{NOT_SERVED}: {e}"))?
}

// ---------------------------------------------------------------------------------------------------------------- the core's end

pub(crate) fn s_ServicesFacetB5_radio_search(t: &Arc<dyn iyakki_agent_api::ServicesFacetB5>, reg: &Registry<'_>, a: Value) -> Result<Value, String> {
    let (user, words, limit): (String, String, usize) = de(a)?;
    match reg.policy_of(&user) {
        Ok(policy) => ser(&t.radio_search(&policy, &user, &words, limit)),
        Err(e) => ser(&(Vec::<(String, String)>::new(), Some(e))),
    }
}

pub(crate) fn s_ServicesFacetB5_study_pdf_export(t: &Arc<dyn iyakki_agent_api::ServicesFacetB5>, reg: &Registry<'_>, a: Value) -> Result<Value, String> {
    let (user, item, a4, path): (String, iyakki_shared::studypdf::Item, bool, Option<String>) = de(a)?;
    let policy = reg.policy_of(&user)?;
    let data = reg.side().data_dir.ok_or_else(|| Registry::missing("data folder"))?;
    ser(&t.study_pdf_export(&policy, &user, &data, &item, a4, path.as_deref()))
}

pub(crate) fn s_DownloadQueueApi_set_context(t: &Arc<dyn iyakki_agent_api::DownloadQueueApi>, reg: &Registry<'_>, a: Value) -> Result<Value, String> {
    let (user, folders): (String, Vec<String>) = de(a)?;
    t.set_context(reg.who_of(&user, folders)?);
    ser(&())
}

pub(crate) fn s_DownloadQueueApi_enqueue(t: &Arc<dyn iyakki_agent_api::DownloadQueueApi>, reg: &Registry<'_>, a: Value) -> Result<Value, String> {
    let (user, folders, req): (String, Vec<String>, Request) = de(a)?;
    ser(&t.enqueue(reg.who_of(&user, folders)?, req))
}
