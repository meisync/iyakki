//! Chat tools for the trust choice of encrypted camera streams (F-CAM-20, `camtls.rs`): `camera_trust_status` (free) lists the cameras
//! with a saved trust and the ones whose certificate waits for an answer, with the fingerprint to compare; `camera_trust_set` (asks every
//! time) approves the waiting certificate of one camera or forgets the saved trust. A fingerprint is never typed through a tool: only the one
//! the camera showed can be approved.

use serde_json::json;

use crate::camtls::TrustStore;
use crate::settingstools::{arg_str, no_args, Entry, Env};

const TAB: &str = "Cameras";
pub const TOOLS: [&str; 2] = ["camera_trust_status", "camera_trust_set"];

pub use iyakki_shared::settingstools_camtrust::PROMPT_RULE;

pub fn entries(env: &Env) -> Vec<Entry> {
    let mut v = Vec::new();
    {
        let e = env.clone();
        v.push(Entry::read(TAB, "camera_trust_status", "List the encrypted cameras with a trusted certificate and the ones whose certificate waits for the person's answer (camera id and fingerprint).", no_args(), move |_| {
            let st = TrustStore::load(&e.dir);
            if st.pinned.is_empty() && st.pending.is_empty() {
                return Ok("No encrypted camera has a saved trust and none waits for an answer.".into());
            }
            let mut out: Vec<String> = st.pinned.iter().map(|(id, fp)| format!("{id}: trusted ({fp})")).collect();
            out.extend(st.pending.iter().map(|(id, fp)| format!("{id}: WAITING for approval ({fp})")));
            Ok(out.join("\n"))
        }));
    }
    {
        let e = env.clone();
        let schema = json!({"type": "object", "properties": {"camera": {"type": "string", "description": "The camera id from camera_trust_status."}, "action": {"type": "string", "enum": ["approve", "forget"]}}, "required": ["camera", "action"]});
        v.push(Entry::change(TAB, "camera_trust_set", "Approve the certificate a camera showed (the one waiting in camera_trust_status) or forget the saved trust of a camera. The person approves a card first.", schema, |a| format!("{} the certificate of camera {}", if a["action"] == "forget" { "Forget" } else { "Trust" }, a["camera"].as_str().unwrap_or("")), move |a| {
            let (id, action) = (arg_str(a, "camera")?, arg_str(a, "action")?);
            let mut st = TrustStore::load(&e.dir);
            let msg = match action.as_str() {
                "approve" => format!("The certificate of {id} is trusted ({}).", st.approve(&id)?),
                "forget" if st.forget(&id) => format!("The saved trust of {id} is gone; the next connection asks again."),
                "forget" => return Err("that camera has no saved trust".into()),
                _ => return Err("action is approve or forget".into()),
            };
            st.save(&e.dir)?;
            e.audit("camera.trust", &format!("{id} {action}"));
            Ok(msg)
        }));
    }
    v
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::Tools;
    use crate::settingstools_tests::{assert_policy, rig, tools};

    #[test]
    fn the_tools_show_the_waiting_fingerprint_approve_only_that_one_and_forget() {
        let r = rig("camtrust", false);
        let t = tools(&r, entries(&r.env));
        assert_policy(&t, &["camera_trust_status"], &["camera_trust_set"]);
        assert!(t.call("camera_trust_status", &json!({})).unwrap().contains("No encrypted camera"));
        let mut st = TrustStore::default();
        st.pending.insert("cam1".into(), "ab".repeat(32));
        st.save(&r.dir).unwrap();
        assert!(t.call("camera_trust_status", &json!({})).unwrap().contains("cam1: WAITING"));
        assert!(t.call("camera_trust_set", &json!({"camera": "cam9", "action": "approve"})).is_err());
        assert!(t.call("camera_trust_set", &json!({"camera": "cam1", "action": "approve", "fingerprint": "00"})).unwrap().contains("trusted"));
        assert!(t.call("camera_trust_status", &json!({})).unwrap().contains("cam1: trusted"));
        assert!(t.call("camera_trust_set", &json!({"camera": "cam1", "action": "forget"})).unwrap().contains("gone"));
        assert!(t.call("camera_trust_set", &json!({"camera": "cam1", "action": "forget"})).is_err());
        assert!(r.audits.lock().unwrap().iter().any(|l| l.contains("camera.trust cam1 approve")));
        let _ = std::fs::remove_dir_all(&r.dir);
    }
}
