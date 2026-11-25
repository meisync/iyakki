//! What the LAB tile asks of the rest of the system (the app standard, docs/development/APP-DEVELOPMENT-PROCEDURE.md section 10).
//!
//! The tile (the crate `iyakki-tile-lab`) names no `Services`, no session and no audit log: it asks a [`LabPort`] for the rig it
//! works with (the picture and vision backends the core serves, the frame source, the camera and the screen), the model catalogue, the
//! vision check, the files it may read and write, the Lemonade lists and the audit line. The real port ([`CxPort`]) is the only place that
//! reads the session and calls the services.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use iyakki_shared::lab::{Catalogue, Sees};
use iyakki_tile_lab::pic::PictureCodec;

use super::lab::Rig;
use super::AppCx;

pub use iyakki_tile_lab::port::LabPort;

/// The shell's picture decoders for the tile (PNG, JPEG, GIF, WebP and SVG).
pub struct ShellCodec;

impl PictureCodec for ShellCodec {
    fn is_picture(&self, bytes: &[u8]) -> bool {
        iyakki_web::image::sniff(bytes) != iyakki_web::image::Kind::Unknown
    }
    fn decode(&self, bytes: &[u8]) -> Result<tiny_skia::Pixmap, String> {
        super::browser_images::to_pixmap(bytes)
    }
}

/// The real port, over the shell's context.
pub struct CxPort<'a>(pub &'a AppCx);

impl LabPort for CxPort<'_> {
    fn rig(&self) -> Rig {
        let cx = self.0;
        let (picture, vision) = cx.services.b5().lab_backends().expect("the core serves the lab backends");
        let (svc, user) = (cx.services.clone(), cx.session.user.clone());
        let (svc2, user2) = (cx.services.clone(), cx.session.user.clone());
        let svc3 = cx.services.clone();
        let allowed = cx.session.policy.apps.contains(&iyakki_sec::policy::App::Camera);
        Rig {
            codec: Arc::new(ShellCodec),
            picture,
            vision,
            frames: cx.services.b5().lab_frames_for(&cx.session.user),
            camera: Arc::new(move || {
                let cam = svc.camera().backend().ok_or("The camera service is not ready yet. Try again in a moment.")?;
                // the red "camera in use" sign shows while this guard lives
                let _busy = iyakki_agent_api::CameraBusy::begin(svc.camera(), &user, "taking a picture in LAB");
                cam.photo_bytes()
            }),
            has_camera: Arc::new(move || allowed && svc3.camera().backend().is_some_and(|b| !b.cameras().is_empty())),
            screen: Arc::new(move || {
                let f = svc2.screen().request(&user2, std::time::Duration::from_secs(10))?;
                Ok((f.w, f.h, f.rgba))
            }),
            home: PathBuf::from(format!("/home/{}", cx.session.user)),
            roots_home: None,
        }
    }
    fn catalogue(&self) -> Catalogue {
        self.0.services.b5().lab_catalogue()
    }
    fn check_vision(&self, cat: &Catalogue, provider: &str, model: &str) -> Result<Sees, String> {
        self.0.services.b5().lab_check_vision(cat, provider, model)
    }
    fn read_path(&self, home: Option<PathBuf>, input: &str) -> Result<PathBuf, String> {
        let cx = self.0;
        let roots = match home {
            Some(h) => cx.core.file_scope().roots_with_home(&cx.session.policy, &cx.session.user, cx.data_dir(), h),
            None => cx.core.file_scope().roots(&cx.session.policy, &cx.session.user, cx.data_dir()),
        };
        let real = roots.resolve(&roots.home, input.trim())?;
        cx.core.file_scope().check_read(&cx.session.policy, &cx.session.user, &real)?;
        Ok(real)
    }
    fn save_file(&self, dir: &Path, name: &str, bytes: &[u8]) -> Result<PathBuf, String> {
        super::chatpic::save_file(&self.0.session.policy, &self.0.session.user, dir, name, bytes)
    }
    fn lemonade_ids(&self) -> Vec<String> {
        self.0.settings().lock().unwrap_or_else(|e| e.into_inner()).get().providers.iter().filter(|p| p.enabled && p.kind == iyakki_shared::providers::Kind::Lemonade).map(|p| p.id.clone()).collect()
    }
    fn refresh_lemonade(&self) -> Arc<dyn Fn(&str) + Send + Sync> {
        let svc = self.0.services.clone();
        Arc::new(move |id: &str| {
            let _ = svc.refresh_lemonade(id);
        })
    }
    fn audit(&self, action: &str, subject: &str) {
        self.0.audit.record(&self.0.session.user, action, subject);
    }
}
