//! The camera node: `cam_photo` takes one picture with the computer's camera and hands its file to the steps after it (usually an
//! `agent` step that shows it to a vision model).
//!
//! The camera is private, so the node runs only when the person has switched on "camera allowed" in the flow's own settings
//! (`defaults.camera`). The assistant cannot switch it on and a flow that comes from outside never arrives with it. A run nobody
//! started by hand (a schedule, an event) needs the second switch, "also unattended". The engine never touches the camera: it
//! asks [`Host::cam_photo`], and the host saves the picture in a private folder of the run and deletes it when the run ends.

use std::sync::Arc;

use serde_json::{json, Value};

use super::*;
use crate::host::CamPhotoRequest;

pub fn register(r: &mut Registry) {
    r.register(Arc::new(CamPhotoKind));
}

/// Shown next to the setting in the flow's settings and in the docs: what switching it on means.
pub const ALLOW_TEXT: &str = "Let this flow use the camera. Its camera step takes a picture of whatever the camera sees, saves it in a private folder of the run and deletes it when the run ends. The picture is sent only to the vision model used for this flow's picture steps, named below: it is on this computer, on another computer on your network, or a cloud service, and the text says which, with its host. If it is a cloud service the picture goes over the internet. Prefer a model on this computer. Only you can switch this on, never the assistant.";

/// A run the person started (by hand or from the chat) is attended; any other trigger is not.
pub fn run_is_attended(ctx: &Value) -> bool {
    matches!(ctx.pointer("/run/trigger").and_then(Value::as_str).unwrap_or("manual"), "manual" | "chat" | "")
}

/// Does a later `agent` step show this node's picture to a model?
fn shown_to_a_model(flow: &Flow, node_id: &str) -> bool {
    let needle = format!("nodes.{node_id}.output");
    flow.nodes.iter().any(|n| n.kind == "agent" && list_of_strings(n.config.get("images").unwrap_or(&Value::Null)).iter().any(|i| i.contains(&needle)))
}

struct CamPhotoKind;
impl NodeKind for CamPhotoKind {
    fn id(&self) -> &'static str {
        "cam_photo"
    }
    fn label(&self) -> &'static str {
        "Camera photo"
    }
    fn blurb(&self) -> &'static str {
        "Takes one picture with the camera (only when the flow's settings allow it); output path and taken_at."
    }
    fn category(&self) -> &'static str {
        "Run"
    }
    fn schema(&self) -> Vec<Field> {
        vec![
            Field::new("device", "Camera", FieldKind::Text, "empty is the camera the computer uses; a name from the CAMERA tile must be one that is connected"),
            Field::new("keep", "Keep the picture", FieldKind::Bool, "off (default): the picture is deleted when the run ends. on: it stays in the run's private folder, not in Pictures").default(json!(false)),
        ]
    }
    fn validate(&self, node: &Node, flow: &Flow) -> Vec<Issue> {
        let mut v = Vec::new();
        // only these two settings exist: anything else (a made-up `camera_allowed`, say) is refused, and none of it ever grants the camera,
        // which is `defaults.camera.allowed` in the flow's settings and only the person sets it
        if let Some(o) = node.config.as_object() {
            for k in o.keys().filter(|k| !matches!(k.as_str(), "device" | "keep")) {
                v.push(Issue::error(&node.id, format!("`{k}` is not a setting of the camera step (only device and keep). The permission to use the camera is set by you in the flow's settings, never on a step")));
            }
        }
        if node.config.get("keep").is_some_and(|k| !k.is_boolean() && !k.as_str().is_some_and(|t| t.contains("{{"))) {
            v.push(Issue::error(&node.id, "keep is on or off"));
        }
        if s(&node.config, "device").chars().any(char::is_control) || s(&node.config, "device").len() > 120 {
            v.push(Issue::error(&node.id, "the camera name is not usable"));
        }
        if !flow.defaults.camera.allowed {
            v.push(Issue::warn(&node.id, "the camera is not allowed for this flow yet: switch on \"Camera allowed\" in the flow settings (the Flow settings button, or S), only you can; until then this step fails"));
        }
        v
    }
    fn can_fail(&self) -> bool {
        true
    }
    fn run(&self, cx: &NodeCx, node: &Node, cfg: &Value) -> NodeResult {
        let cam = &cx.flow.defaults.camera;
        let granted = |p: &str| cx.ctx.pointer("/run/grants").and_then(Value::as_array).is_some_and(|g| g.iter().any(|x| x.as_str() == Some(p)));
        let allowed = cam.allowed || granted("camera");
        if !allowed {
            if cam.blocked {
                return Err(NodeError::consent("You chose to block camera use for this flow, so it is not asking. Open the flow's settings and set \"Camera permission requests\" back to ask, or switch on \"Camera allowed\"."));
            }
            return Err(NodeError::consent_for("camera", "This flow is not allowed to use the camera. Open the flow's settings and switch on \"Camera allowed\" (only you can do that), then run it again."));
        }
        let attended = run_is_attended(cx.ctx);
        let unattended = cam.unattended || granted("camera_unattended");
        if !attended && !unattended {
            if cam.blocked {
                return Err(NodeError::consent("You chose to block camera use for this flow, so it is not asking. Set \"Camera permission requests\" back to ask in the flow's settings."));
            }
            return Err(NodeError::consent_for("camera_unattended", "Nobody started this run, and the flow is allowed to use the camera only when you start it. Switch on \"Camera also when nobody is there\" in the flow's settings to let a schedule take pictures."));
        }
        let req = CamPhotoRequest {
            flow_id: cx.flow.id.clone(),
            run_id: cx.run_id.to_string(),
            node_id: cx.node_id.to_string(),
            device: s(cfg, "device").trim().to_string(),
            keep: b(cfg, "keep", false),
            allowed,
            unattended,
            attended,
            needs_vision: shown_to_a_model(cx.flow, &node.id),
        };
        cx.log(format!("camera: taking a picture ({})", if req.keep { "kept after the run" } else { "deleted when the run ends" }));
        let r = cx.host.cam_photo(&req, cx.cancel)?;
        for l in r.log.iter().take(10) {
            cx.log(l);
        }
        let artifacts = if req.keep { vec![r.path.clone()] } else { vec![] };
        Ok(NodeOutput { value: json!({"path": r.path, "taken_at": r.taken_at, "device": r.device}), port: "out".into(), wait: None, artifacts })
    }
    fn classify_error(&self, e: &NodeError, _n: &Node) -> ErrClass {
        default_class(e)
    }
    fn describe(&self, node: &Node) -> String {
        if b(&node.config, "keep", false) { "take a camera photo (kept)".into() } else { "take a camera photo".into() }
    }
}
