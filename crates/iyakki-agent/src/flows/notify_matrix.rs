//! The `notify` node's channels on this computer: Matrix, through the person's own account (`crate::msg`). The INBOX note is made by
//! `Host::notify` first; this adds the message. Rules: only the person's saved rooms, the hourly cap of the account, a picture only if it
//! is a file of THIS run's camera folder, nothing leaves when the run asks for a dry run, and a refusal (an encrypted room, a cap, no
//! account) comes back as one plain sentence plus one INBOX warning, never as a failed run.

use std::path::Path;
use std::sync::Arc;

use iyakki_core_api::OutAttachment;
use iyakki_flow::host::{ChannelNotice, Notice};

use super::host::AgentHost;
use crate::inbox::{Kind, Link};

fn mime_of(p: &Path) -> Option<&'static str> {
    match p.extension().and_then(|e| e.to_str()).map(str::to_ascii_lowercase).as_deref() {
        Some("jpg" | "jpeg") => Some("image/jpeg"),
        Some("png") => Some("image/png"),
        _ => None,
    }
}

impl AgentHost {
    /// The picture of this run that `attach` names, or why it cannot be sent.
    fn run_picture(&self, run_id: &str, attach: &str) -> Result<OutAttachment, String> {
        let folder = self.cam_folder(run_id);
        let want = std::fs::canonicalize(attach).map_err(|_| "the picture to send is not there".to_string())?;
        let root = std::fs::canonicalize(&folder).map_err(|_| "this run has taken no picture".to_string())?;
        if !want.starts_with(&root) || !want.is_file() {
            return Err("only a picture taken by this run's camera step can be sent".into());
        }
        let mime = mime_of(&want).ok_or_else(|| "only a JPEG or PNG picture can be sent".to_string())?;
        let name = want.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "picture".into());
        Ok(OutAttachment { path: want, name, mime: mime.into() })
    }

    pub(super) fn send_through_channels(&self, n: &Notice, c: &ChannelNotice) -> Result<Vec<String>, String> {
        if let Some(other) = c.channels.iter().find(|x| x.as_str() != "matrix") {
            return Err(format!("the channel `{other}` is not available"));
        }
        let sink = self.audit_sink.lock().unwrap_or_else(|e| e.into_inner()).clone();
        let (svc, user) = (self.svc.clone(), self.user.clone());
        let audit: crate::gitkeys::AuditFn = Arc::new(move |a, d| match &sink {
            Some(f) => f(&user, a, d),
            None => svc.browser.audit(&user, a, d),
        });
        let ms = crate::msg::MessageService::for_services(&self.svc, &self.user, audit);
        let result = (|| {
            // the flow runs as the person: their account's network policy decides which server it may reach (a cluster address is the cluster's)
            let acct = ms.accounts().pick("")?;
            if !acct.homeserver.starts_with("cluster") && !self.policy.allows_host(&crate::gitconn::host_of(&acct.homeserver)) {
                return Err(format!("your account may not reach {} (the network policy of this account)", crate::gitconn::host_of(&acct.homeserver)));
            }
            let attach = if c.attach.is_empty() { None } else { Some(self.run_picture(&n.run_id, &c.attach)?) };
            let text = format!("{}\n{}", n.title.trim(), n.body.trim());
            let r = ms.send("", &c.room, text.trim(), attach, &c.dedupe_key, c.dry_run)?;
            Ok::<_, String>(vec![r.note])
        })();
        if let Err(e) = &result {
            // once per reason: a flow that fires all night leaves one note, not hundreds
            let _ = self.svc.notify_once(&self.user, Kind::Warning, "Message not sent", &e.chars().take(300).collect::<String>(), Some(Link::new("flows", &n.flow_id)));
        }
        result
    }
}
