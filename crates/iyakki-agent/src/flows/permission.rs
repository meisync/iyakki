//! Permission requests. A flow step that stopped for want of a permission only the person can give (the camera, the camera when
//! nobody is there) does not leave them to hunt for the setting: the host makes a REQUEST that reaches them as a popup on the open
//! screen and as an entry in the INBOX (Approvals page and a note), with the exact text of the permission and four answers:
//!
//! * **Allow this time**: a grant stored on THIS run only (`Run::grants`); the stopped step runs again. The flow is not changed.
//! * **Always allow for this flow**: switches the person's own setting on (`defaults.camera.allowed`, or `unattended` for the separate
//!   permission for runs nobody started) in the flow's draft, and publishes it as a new version when nothing else is waiting in the
//!   draft; otherwise it stays in the draft until the person publishes.
//! * **Not now**: the popup goes, the INBOX entry stays.
//! * **Block**: `defaults.camera.blocked`: the flow stops asking (undo: the flow settings).
//!
//! Only the signed-in person's own input answers (the shell's popup and the INBOX call [`FlowEnv::answer_permission`]); no tool, the
//! assistant, the fixing agent, an imported flow or the web API has a way to. Developer-mode input is refused (see
//! `browserconsent::ConsentStore::set_synthetic`). Every answer is audited without secrets.

use iyakki_flow::host::PermissionNeed;
use iyakki_flow::{Flow, StartRequest};

use crate::browserconsent::{Answer, ConsentRequest};

use super::host::AgentHost;
use super::FlowEnv;

/// The popup is shown for this long after the request was made; the INBOX entry stays until it is answered.
pub const POPUP_SECS: i64 = 600;

/// The fields a request carries to the answer: run, step, permission, flow.
pub fn payload(n: &PermissionNeed) -> String {
    format!("{}|{}|{}|{}", n.run_id, n.node_id, n.permission, n.flow_id)
}

fn parts(payload: &str) -> Option<(String, String, String, String)> {
    let mut it = payload.splitn(4, '|');
    Some((it.next()?.into(), it.next()?.into(), it.next()?.into(), it.next()?.into()))
}

impl AgentHost {
    /// The text the person reads: what the permission is, which flow and step, where the picture would go.
    pub(super) fn permission_text(&self, n: &PermissionNeed) -> String {
        let place = match self.svc.vision_model(None) {
            Ok(p) if p.place == crate::visionmodel::Place::Cloud => format!("The picture would be shown to {} on {}, a CLOUD service ({}): it would be sent over the internet. A model on your own computer or network keeps it there.", p.model, p.provider, p.host),
            Ok(p) => format!("The picture would be shown to {} on {}, {} ({}).", p.model, p.provider, p.place.describe(&p.host), match p.how {
                crate::visionmodel::How::Setting => "your Vision model setting",
                crate::visionmodel::How::Chat => "the chat model",
                _ => "picked automatically",
            }),
            Err(e) => format!("No vision model is available yet, so the picture could not be looked at: {}", e.message()),
        };
        let what = if n.permission == "camera_unattended" {
            "Camera also when nobody is there: this flow may take pictures when a schedule or an event starts it, with nobody to see the camera-in-use sign. This is a separate permission from Camera allowed.\n".to_string()
        } else {
            format!("{}\n", iyakki_flow::kinds::camera::ALLOW_TEXT)
        };
        format!("{what}Flow: {} · step: {}\n{place}\nAllow this time: only this run. Always allow: switches it on in the flow's settings (and publishes that one change when nothing else is waiting in the draft). Block: stop asking for this flow.", n.flow_name, n.node_title)
    }

    pub fn permission_needed_impl(&self, n: &PermissionNeed) {
        if n.blocked || self.perm_blocked_now(&n.flow_id) {
            self.audit_to("flow.permission.not_asked", &format!("{} {} (the flow says Block)", n.run_id, n.permission));
            return;
        }
        let key = payload(n);
        let title = format!("{} / {} needs your permission", n.flow_name, n.node_title);
        let made = self.svc.browser.consent.create_permission(&self.user, &n.flow_id, &title, &self.permission_text(n), &key, crate::clock::now_unix());
        match made {
            Ok(Some(_)) => {
                self.audit_to("flow.permission.asked", &format!("{} {} step {}", n.run_id, n.permission, n.node_id));
                // one note per run and step; a repeat while it is unread adds nothing
                let body = format!("The step \"{}\" stopped: {}. Answer in the INBOX (Approvals), or in the popup on the screen.", n.node_title, n.message.chars().take(200).collect::<String>());
                let _ = self.svc.notify_once(&self.user, crate::inbox::Kind::Agent, &title, &body, Some(crate::inbox::Link::new("flows", &n.run_id)));
            }
            Ok(None) => {}
            Err(e) => self.audit_to("flow.permission.not_asked", &format!("{}: {e}", n.run_id)),
        }
    }

    /// The flow's current draft says Block (the setting can be changed after the run began).
    fn perm_blocked_now(&self, flow_id: &str) -> bool {
        iyakki_flow::store::FlowStore::new(&self.flows_dir, std::sync::Arc::new(|| 0)).get_draft(flow_id).is_some_and(|f| f.defaults.camera.blocked)
    }
}

impl FlowEnv {
    /// Apply the person's answer to a permission request. Called by the shell after its own dialog or the INBOX, never by a tool.
    pub fn answer_permission(&self, req: &ConsentRequest, answer: Answer) -> Result<String, String> {
        let (run, node, perm, flow) = parts(&req.payload).ok_or("that request is not a flow permission")?;
        let who = self.who();
        let audit = |what: &str| iyakki_flow::host::Host::audit(self.host.as_ref(), what, &format!("{run} {perm} step {node} by {who}"));
        match answer {
            Answer::NotNow => {
                audit("flow.permission.not_now");
                Ok("Not now. The request stays in the INBOX.".into())
            }
            Answer::AllowOnce => {
                audit("flow.permission.allow_once");
                self.run_with(&run, &flow, &perm)
            }
            Answer::AlwaysAllow => {
                audit("flow.permission.always_allow");
                let saved = self.set_camera(&flow, |c| {
                    c.blocked = false;
                    if perm == "camera_unattended" {
                        c.allowed = true;
                        c.unattended = true;
                    } else {
                        c.allowed = true;
                    }
                })?;
                let ran = self.run_with(&run, &flow, &perm)?;
                Ok(format!("{saved} {ran}"))
            }
            Answer::Block => {
                audit("flow.permission.block");
                let saved = self.set_camera(&flow, |c| c.blocked = true)?;
                Ok(format!("Blocked: this flow will not ask again. {saved} Undo it in the flow's settings (Camera permission requests)."))
            }
            _ => Err("that is not an answer to a permission request".into()),
        }
    }

    /// Give the run the permission and run its step again; a run that is already over is run again from the start with it.
    fn run_with(&self, run: &str, flow: &str, perm: &str) -> Result<String, String> {
        match self.engine.grant_run(run, perm, &self.who()) {
            Ok(()) => Ok("This run goes on.".into()),
            Err(_) => {
                let mut req = StartRequest::new(flow);
                req.grants = vec![perm.to_string()];
                req.trigger = iyakki_flow::TriggerInfo { kind: "manual".into(), by: self.who(), detail: "after a permission request".into(), ..Default::default() };
                let o = self.engine.start_run(req)?;
                Ok(format!("The run had ended, so the flow runs again as {} with the permission for that run.", o.run_id))
            }
        }
    }

    /// Change the camera settings of the flow's draft; publish them when nothing else is waiting in the draft.
    fn set_camera(&self, flow_id: &str, f: impl FnOnce(&mut iyakki_flow::CameraDefaults)) -> Result<String, String> {
        let mut draft = self.store.get_draft(flow_id).ok_or_else(|| format!("there is no flow `{flow_id}`"))?;
        let clean = self.store.published(flow_id).is_some_and(|p| same_but_camera(&draft, &p));
        f(&mut draft.defaults.camera);
        self.store.save_draft(&draft, None)?;
        if !clean {
            return Ok("Saved in the flow's draft; publish it to use it in scheduled runs.".into());
        }
        let by = self.who();
        match self.store.publish(flow_id, &iyakki_flow::store::PublishReq { by: &by, label: "", changelog: "Camera permission changed by the person (permission request)", approved_by: Some(&by) }) {
            Ok(v) => Ok(format!("Saved and published as version {v} (nothing else changed).")),
            Err(e) => Ok(format!("Saved in the flow's draft; it could not be published: {e}")),
        }
    }
}

fn same_but_camera(a: &Flow, b: &Flow) -> bool {
    let norm = |f: &Flow| {
        let mut f = f.clone();
        f.meta = Default::default();
        f.defaults.camera = Default::default();
        f
    };
    norm(a) == norm(b)
}
