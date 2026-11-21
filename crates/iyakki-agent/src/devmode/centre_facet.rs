//! The update centre behind `iyakki_agent_api::UpdateCentre` (F-SPLIT-03): the UI sees plain packs, versions and previews; the manifest
//! and the boot-partition work stay here.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use iyakki_agent_api::{UpdateCentre, UpdateFound, UpdatePreview, UpdateTrust, UpdateVersions};

use super::centre::{self, Centre, Found, Trust};

fn trust_of(t: &Trust) -> UpdateTrust {
    match t {
        Trust::Signed => UpdateTrust::Signed,
        Trust::Unsigned => UpdateTrust::Unsigned,
        Trust::Bad(e) => UpdateTrust::Bad(e.clone()),
    }
}

fn found_of(f: &Found) -> UpdateFound {
    UpdateFound { dir: f.dir.clone(), version: f.manifest.version.clone(), variant: f.manifest.variant.clone(), trust: trust_of(&f.trust) }
}

impl Centre {
    /// This centre as the UI's handle (`iyakki_agent_api::UpdateCentre`).
    pub fn into_facet(self) -> Arc<dyn UpdateCentre> {
        Arc::new(RealCentre(self, None))
    }

    /// The same with the transport a download goes over (tests give a fake channel; the device uses the plain download transport).
    pub fn into_facet_with_streamer(self, st: Arc<dyn iyakki_proxy::Streamer>) -> Arc<dyn UpdateCentre> {
        Arc::new(RealCentre(self, Some(st)))
    }
}

struct RealCentre(Centre, Option<Arc<dyn iyakki_proxy::Streamer>>);

impl RealCentre {
    /// The agent's own record of a pack the UI holds: read again from its folder (the signature is checked again).
    fn pack(&self, p: &UpdateFound) -> Result<Found, String> {
        centre::read_pack(&p.dir, self.0.key())
    }
}

impl UpdateCentre for RealCentre {
    fn data(&self) -> PathBuf {
        self.0.data().to_path_buf()
    }
    fn versions(&self) -> UpdateVersions {
        let v = self.0.versions();
        UpdateVersions { running: v.running, on_disk: v.on_disk, variant: v.variant, previous: v.previous, interrupted: v.interrupted }
    }
    fn find_packs(&self, root: &Path) -> Vec<UpdateFound> {
        centre::find_packs(root, self.0.key()).iter().map(found_of).collect()
    }
    fn open_file(&self, path: &Path) -> Result<UpdateFound, String> {
        let v = self.0.versions();
        let staged = crate::sysupdate::open_for_centre(self.0.data(), path, self.0.key(), &v.running, &v.variant)?;
        Ok(UpdateFound { dir: staged.dir, version: staged.release.version, variant: staged.release.variant, trust: UpdateTrust::Signed })
    }
    fn fetch_file(&self, url: &str) -> Result<UpdateFound, String> {
        let v = self.0.versions();
        let st = self.1.clone().unwrap_or_else(iyakki_proxy::https::download_streamer);
        // the person's apps may hear that the download ended, with the fixed name only (F-SDK-207; the page has no person to tell, so a note)
        let staged = crate::sysupdate::fetch_for_centre(self.0.data(), &*st, url, self.0.key(), &v.running, &v.variant).inspect_err(|_| crate::appevent_more::note_download_made(self.0.data(), "system update", false))?;
        crate::appevent_more::note_download_made(self.0.data(), "system update", true);
        Ok(UpdateFound { dir: staged.dir, version: staged.release.version, variant: staged.release.variant, trust: UpdateTrust::Signed })
    }
    fn preview(&self, p: &UpdateFound) -> Result<UpdatePreview, String> {
        let pv = self.0.preview(&self.pack(p)?)?;
        Ok(UpdatePreview { lines: pv.lines(), from: pv.from, to: pv.to, variant: pv.variant, confirm: pv.confirm })
    }
    fn install(&self, p: &UpdateFound, confirmed: bool, who: &str) -> Result<String, String> {
        self.0.install(&self.pack(p)?, confirmed, who)
    }
    fn rollback(&self, who: &str) -> Result<String, String> {
        self.0.rollback(who)
    }
    fn after_boot(&self, running: &str, boot_id: &str) -> Option<String> {
        centre::after_boot(self.0.data(), running, boot_id)
    }
}
