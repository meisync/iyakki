//! The SDK's `CameraPort` over the core's camera facet (`cx.core.cameras()`), for one person and one app: what an SDK app gets so that it can
//! embed live camera pictures (`iyakki_ui_sdk::cameraview::CameraView`, F-SDK-163). The port only maps types; the core decides everything
//! (who sees which camera, the consent card, the indicator, the limits, the snapshot's folder, the audit lines), so a refusal arrives here
//! as a sentence and is handed to the app. One port holds at most one view; `open` replaces an earlier one (and closes it first).

use std::sync::Arc;

use iyakki_core_api::cameras as api;
use iyakki_core_api::Core;
use iyakki_ui_sdk::cameras as sdk;
use iyakki_ui_sdk::canvas::{Image, ImageFormat};

pub struct CoreCameraPort {
    core: Arc<dyn Core>,
    user: String,
    app: String,
    view: Option<String>,
}

impl CoreCameraPort {
    pub fn new(core: Arc<dyn Core>, user: &str, app: &str) -> CoreCameraPort {
        CoreCameraPort { core, user: user.into(), app: app.into(), view: None }
    }

    fn id(&self) -> sdk::CamResult<&str> {
        self.view.as_deref().ok_or_else(|| "No camera view is open.".to_string())
    }
}

fn status(s: api::CamStatus) -> sdk::CamStatus {
    match s {
        api::CamStatus::Connecting => sdk::CamStatus::Connecting,
        api::CamStatus::Live => sdk::CamStatus::Live,
        api::CamStatus::Lost => sdk::CamStatus::Lost,
        api::CamStatus::NeedsAddon => sdk::CamStatus::NeedsAddon,
        api::CamStatus::Busy => sdk::CamStatus::Busy,
        api::CamStatus::NeedsPassword => sdk::CamStatus::NeedsPassword,
        api::CamStatus::Dark => sdk::CamStatus::Dark,
    }
}

fn info(c: api::CameraInfo) -> sdk::CamInfo {
    sdk::CamInfo { id: c.id, name: c.name, status: status(c.status), detail: c.detail, ptz: c.ptz, recording: c.recording, motion_watch: c.motion_watch, size: c.size }
}

fn action(a: api::CamAction) -> sdk::CamAction {
    match a {
        api::CamAction::View => sdk::CamAction::View,
        api::CamAction::Snapshot => sdk::CamAction::Snapshot,
        api::CamAction::Ptz => sdk::CamAction::Ptz,
    }
}

fn mark(m: api::MotionMark) -> sdk::Mark {
    sdk::Mark { id: m.id, at_ms: m.at_ms, zone: m.zone, score: m.score, bbox: m.bbox, active: m.active }
}

fn event(e: api::CamEvent) -> sdk::CamEvent {
    use api::CamEvent as E;
    match e {
        E::Approval { call, action: a, camera, summary, detail } => sdk::CamEvent::Approval { call, action: action(a), camera, summary, detail },
        E::Decision { call, action: a, approved, by } => sdk::CamEvent::Decision { call, action: action(a), approved, by },
        E::Motion { camera, mark: m } => sdk::CamEvent::Motion { camera, mark: mark(m) },
        E::Snapshot { camera, file, folder } => sdk::CamEvent::Snapshot { camera, file, folder },
        E::Failed { camera, action: a, error } => sdk::CamEvent::Failed { camera, action: action(a), error },
        E::Status { camera, status: s, detail } => sdk::CamEvent::Status { camera, status: status(s), detail },
    }
}

/// A core that has no camera views says so in one sentence.
fn refused(e: String) -> String {
    if e.contains("no camera views") { sdk::UNAVAILABLE.to_string() } else { e }
}

impl sdk::CameraPort for CoreCameraPort {
    fn list(&mut self) -> sdk::CamResult<Vec<sdk::CamInfo>> {
        let l: Vec<sdk::CamInfo> = self.core.cameras().list(&self.user, &self.app).into_iter().map(info).collect();
        // the core's default has no cameras and no sentence: an empty list from a core that has none is the same to the person
        Ok(l)
    }

    fn open(&mut self, cameras: &[String], large: bool, label: &str) -> sdk::CamResult<Vec<sdk::CamInfo>> {
        if let Some(old) = self.view.take() {
            self.core.cameras().close(&self.user, &old);
        }
        let spec = api::ViewSpec { cameras: cameras.to_vec(), size: if large { api::FrameSize::Large } else { api::FrameSize::Small }, audio: false, label: label.into() };
        let v = self.core.cameras().open(&self.user, &self.app, &spec).map_err(refused)?;
        self.view = Some(v.id);
        Ok(v.cameras.into_iter().map(info).collect())
    }

    fn decide(&mut self, call: &str, approve: bool) -> sdk::CamResult<()> {
        let id = self.id()?.to_string();
        self.core.cameras().decide(&self.user, &id, call, approve).map_err(refused)
    }

    fn state(&mut self) -> sdk::CamResult<sdk::ViewState> {
        let id = self.id()?.to_string();
        let s = self.core.cameras().state(&self.user, &id).map_err(refused)?;
        let phase = match s.phase {
            api::ViewPhase::AwaitingConsent { .. } => sdk::Phase::AwaitingConsent,
            api::ViewPhase::Open => sdk::Phase::Open,
            api::ViewPhase::Denied { reason } => sdk::Phase::Denied(reason),
            api::ViewPhase::Closed => sdk::Phase::Closed,
        };
        Ok(sdk::ViewState { phase, indicator: s.indicator, cameras: s.cameras.into_iter().map(info).collect() })
    }

    fn set_visible(&mut self, visible: bool) -> sdk::CamResult<()> {
        let id = self.id()?.to_string();
        self.core.cameras().set_visible(&self.user, &id, visible).map_err(refused)
    }

    fn frame(&mut self, camera: &str, after: u64, format: ImageFormat) -> sdk::CamResult<Option<Image>> {
        let id = self.id()?.to_string();
        let fmt = if format == ImageFormat::Jpeg { api::FrameFormat::Jpeg } else { api::FrameFormat::Rgba };
        Ok(self.core.cameras().frame(&self.user, &id, camera, after, fmt).map_err(refused)?.map(|f| Image { format: if f.format == api::FrameFormat::Jpeg { ImageFormat::Jpeg } else { ImageFormat::Rgba }, width: f.width, height: f.height, seq: f.seq, data: f.data }))
    }

    fn overlay(&mut self, camera: &str) -> sdk::CamResult<sdk::Overlay> {
        let id = self.id()?.to_string();
        let o = self.core.cameras().overlay(&self.user, &id, camera).map_err(refused)?;
        Ok(sdk::Overlay { zones: o.zones.into_iter().map(|z| sdk::Shape { name: z.name, include: z.include, mask: z.mask, points: z.points }).collect(), marks: o.marks.into_iter().map(mark).collect() })
    }

    fn snapshot(&mut self, camera: &str) -> sdk::CamResult<String> {
        let id = self.id()?.to_string();
        self.core.cameras().snapshot(&self.user, &id, camera).map_err(refused)
    }

    fn ptz(&mut self, camera: &str, cmd: sdk::Ptz) -> sdk::CamResult<Option<String>> {
        let id = self.id()?.to_string();
        let c = match cmd {
            sdk::Ptz::Move { pan, tilt, zoom } => api::PtzCommand::Move { pan, tilt, zoom },
            sdk::Ptz::Stop => api::PtzCommand::Stop,
        };
        self.core.cameras().ptz(&self.user, &id, camera, c).map_err(refused)
    }

    fn events(&mut self) -> Vec<sdk::CamEvent> {
        match &self.view {
            Some(id) => self.core.cameras().events(&self.user, id).into_iter().map(event).collect(),
            None => Vec::new(),
        }
    }

    fn close(&mut self) {
        if let Some(id) = self.view.take() {
            self.core.cameras().close(&self.user, &id);
        }
    }
}

impl Drop for CoreCameraPort {
    fn drop(&mut self) {
        sdk::CameraPort::close(self);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use iyakki_core_fake::{FakeCam, FakeCore};
    use iyakki_ui_sdk::cameras::{CamAction, CamEvent, CameraPort, Phase};

    fn core() -> Arc<FakeCore> {
        let c = Arc::new(FakeCore::new());
        c.cameras_fake().add(FakeCam::new("front", "Front door", "amy").allowing(&["bob"]).with_ptz());
        c.cameras_fake().add(FakeCam::new("yard", "Yard", "amy"));
        c.cameras_fake().push_solid("front", 4, 2, [10, 20, 30]);
        c
    }

    #[test]
    fn the_port_maps_the_core_view_from_the_list_to_the_card_the_frames_and_the_close() {
        let c = core();
        let mut p = CoreCameraPort::new(c.clone(), "amy", "sample");
        assert_eq!(p.list().unwrap().len(), 2);
        let mut bob = CoreCameraPort::new(c.clone(), "bob", "sample");
        assert_eq!(bob.list().unwrap().iter().map(|i| i.name.as_str()).collect::<Vec<_>>(), vec!["Front door"]);
        assert!(bob.open(&["yard".into()], false, "").unwrap_err().contains("not on its list"), "the core's words reach the app");
        let cams = p.open(&["front".into()], true, "Panel").unwrap();
        assert_eq!(cams[0].name, "Front door");
        assert!(cams[0].ptz);
        let ev = p.events();
        let CamEvent::Approval { call, action, summary, .. } = &ev[0] else { panic!("{ev:?}") };
        assert_eq!(*action, CamAction::View);
        assert!(summary.contains("sample (Panel) wants to show 1 camera: Front door"));
        assert_eq!(p.state().unwrap().phase, Phase::AwaitingConsent);
        assert!(p.frame("front", 0, ImageFormat::Rgba).unwrap().is_none());
        assert_eq!(c.cameras_fake().pulled("front"), 0);
        p.decide(call, true).unwrap();
        assert!(p.state().unwrap().indicator);
        let f = p.frame("front", 0, ImageFormat::Rgba).unwrap().unwrap();
        assert_eq!((f.width, f.height, f.data.len()), (4, 2, 32));
        assert_eq!(p.frame("front", 0, ImageFormat::Jpeg).unwrap().unwrap().format, ImageFormat::Jpeg);
        p.set_visible(false).unwrap();
        assert!(!p.state().unwrap().indicator);
        let call = p.snapshot("front").unwrap();
        p.decide(&call, true).unwrap();
        assert_eq!(c.cameras_fake().snapshots(), vec![("amy".to_string(), "front".to_string())]);
        assert!(p.events().iter().any(|e| matches!(e, CamEvent::Snapshot { folder, .. } if folder == "Pictures")));
        p.close();
        assert!(p.state().is_err());
        assert!(c.cameras_fake().audit().contains(&"cameras.close sample".to_string()));
        // opening again replaces an earlier view, and a dropped port closes its view
        p.open(&["front".into()], false, "").unwrap();
        p.open(&["front".into()], false, "").unwrap();
        drop(p);
        assert!(c.cameras_fake().indicator_names().is_empty());
    }

    #[test]
    fn a_core_with_no_camera_views_is_one_sentence() {
        // a core that serves everything but cameras: the facet's default answers
        struct Bare(FakeCore);
        impl iyakki_core_api::Core for Bare {
            fn mixer(&self) -> &dyn iyakki_core_api::Mixer {
                self.0.mixer()
            }
            fn app_status(&self) -> &dyn iyakki_core_api::AppStatuses {
                self.0.app_status()
            }
        }
        let mut p = CoreCameraPort::new(Arc::new(Bare(FakeCore::new())), "amy", "sample");
        assert!(p.list().unwrap().is_empty());
        assert_eq!(p.open(&["x".into()], false, "").unwrap_err(), sdk::UNAVAILABLE);
    }
}
