//! Keeping the person's shared media index fresh (the part of `mediaindex` that needs the agent's `Services`).

use std::sync::Arc;

/// Bring the person's shared index up to date when it is empty or older than `max_age` seconds (0 = always), then give
/// every item a vector and save. Used by the assistant's tools; the MEDIA app does the same in its own scan.
pub fn refresh(svc: &Arc<crate::services::Services>, policy: &iyakki_sec::policy::Policy, user: &str, max_age: i64) -> iyakki_shared::mediaindex::SyncReport {
    let ix = svc.media_index(user);
    let now = crate::clock::now_unix();
    {
        let g = ix.lock().unwrap_or_else(|e| e.into_inner());
        if !g.is_empty() && max_age > 0 && now - g.synced_at() < max_age {
            return iyakki_shared::mediaindex::SyncReport::default();
        }
    }
    let folders = crate::media::MediaStore::new(svc.dir(), user).folders();
    let samples = crate::media::samples_dir();
    let folders = crate::media::scan_folders(&folders, samples.as_deref());
    let (lib, imgs) = iyakki_shared::mediaindex::scan_for_index(&folders, user, &|p| crate::media::may_read(policy, user, p));
    let rep = ix.lock().unwrap_or_else(|e| e.into_inner()).sync(&lib, &imgs, now);
    if let Some(d) = &samples {
        ix.lock().unwrap_or_else(|e| e.into_inner()).mark_samples(d);
    }
    iyakki_shared::mediaindex::embed_all(&ix, 256, 400);
    let _ = ix.lock().unwrap_or_else(|e| e.into_inner()).save();
    rep
}
