//! HOST FILES (window mode only): the dialog itself lives in the `iyakki-tile-hostfiles` crate; the rules in `iyakki-hostfiles` (also reachable as
//! `iyakki_shared::hostfiles`). This file is the shell's side: the [`HostEnv`] of the signed-in person.

use std::sync::Arc;

pub use iyakki_tile_hostfiles::*;

use super::AppCx;

/// Imports go to `<home>/Uploads`.
pub fn env_for_cx(cx: &AppCx) -> HostEnv {
    let home = cx.core.file_scope().home_root(&cx.session.user, cx.data_dir());
    let audit = cx.audit.clone();
    let user = cx.session.user.clone();
    HostEnv {
        data_dir: cx.data_dir().to_path_buf(),
        user: cx.session.user.clone(),
        dest: home.join("Uploads"),
        dest_label: "your Uploads folder".into(),
        audit: Arc::new(move |a, d| audit.record(&user, a, d)),
    }
}
