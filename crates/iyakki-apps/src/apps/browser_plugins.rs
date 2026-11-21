//! The Browser's plug-ins, per person (F-BROW-90): the file next to their other browser settings and `apply`, which hands the plug-ins
//! that are on to the full engine's extension snapshot (`iyakki_shared::browserplugins`). The assistant's tools are in `chat_settings`.

use std::path::PathBuf;
use std::sync::Mutex;

use super::AppCx;

pub fn path_for(dir: &std::path::Path, user: &str) -> PathBuf {
    let who: String = user.chars().filter(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.')).collect();
    dir.join(format!("browser-plugins-{who}.json"))
}

/// What `apply` last handed over: the file's text.
static LOADED: Mutex<Option<(PathBuf, String)>> = Mutex::new(None);

/// Make this person's plug-ins that are on the ones the engine gets. Cheap when the file is the same text as last time.
pub fn apply(cx: &AppCx) {
    apply_at(&path_for(cx.data_dir(), &cx.session.user));
}

pub(super) fn apply_at(path: &std::path::Path) {
    let sig = std::fs::read_to_string(path).unwrap_or_default();
    {
        let mut l = LOADED.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        if l.as_ref().is_some_and(|(p, s)| p == path && *s == sig) {
            return;
        }
        *l = Some((path.to_path_buf(), sig));
    }
    iyakki_shared::browserplugins::activate(iyakki_shared::browserplugins::PluginStore::open(path).list());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_path_is_per_person_and_apply_hands_over_only_what_is_on_and_follows_the_file() {
        use iyakki_shared::browserplugins::{active, Kind, Plugin, PluginStore};
        let d = std::env::temp_dir().join(format!("iyakki-bp-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        assert_ne!(path_for(&d, "alice"), path_for(&d, "bob"));
        assert_eq!(path_for(&d, "../x"), d.join("browser-plugins-..x.json"));
        let path = path_for(&d, "alice");
        apply_at(&path);
        assert!(active().is_empty());
        let s = PluginStore::open(&path);
        let p = s.add(Plugin { id: String::new(), name: "N".into(), kind: Kind::Style, sites: vec!["example.com".into()], body: "a{}".into(), at_end: false, enabled: false }).unwrap();
        apply_at(&path);
        assert!(active().is_empty(), "off until switched on");
        s.set_enabled(&p.id, true).unwrap();
        apply_at(&path);
        assert_eq!(active().len(), 1);
        s.remove(&p.id).unwrap();
        apply_at(&path);
        assert!(active().is_empty());
    }
}
