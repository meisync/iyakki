//! The glue of a cloud node at boot (docs/CLOUD-IMAGES.md): when to run the first-run adopt mode, and the guarded self-install.
//! The decisions are in `iyakki_agent::cloudadopt` and `iyakki_agent::cloudseed`; the listener is `iyakki_remote::adopt_server`.

/// Is adopt mode wanted now? Only on a cloud node (`iyakki.cloud=1`, `iyakki.mode=cloud`, a cloud variant, or Cloud mode's own
/// automatic decision for a machine with no screen, keyboard or sound), only while no account exists. `iyakki.adopt=0` or
/// `IYAKKI_ADOPT=0` switches it off; `IYAKKI_ADOPT=1` switches it on (a container).
#[cfg(feature = "web")]
pub fn adopt_wanted(cmdline: &str, env_adopt: Option<&str>, users: &std::path::Path, cloud_auto: &dyn Fn() -> bool) -> bool {
    if iyakki_remote::adopt_server::admin_exists(users) {
        return false;
    }
    if cmdline.split_whitespace().any(|t| t == "iyakki.adopt=0") || env_adopt == Some("0") {
        return false;
    }
    env_adopt == Some("1") || cloud_auto()
}

#[cfg(all(test, feature = "web"))]
mod tests {
    use super::*;

    #[test]
    fn adopt_runs_on_cloud_nodes_without_accounts_only() {
        let d = std::env::temp_dir().join(format!("iyakki-cn-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        let f = d.join("users.json");
        assert!(adopt_wanted("quiet", None, &f, &|| true), "a cloud node with no account");
        assert!(!adopt_wanted("quiet", None, &f, &|| false), "a desktop has its own first-run screen");
        assert!(!adopt_wanted("iyakki.adopt=0", None, &f, &|| true));
        assert!(!adopt_wanted("quiet", Some("0"), &f, &|| true));
        assert!(adopt_wanted("quiet", Some("1"), &f, &|| false), "a container asks for it");
        std::fs::write(&f, "{\"users\":[{\"name\":\"a\",\"hash\":\"h\",\"role\":\"admin\"}]}").unwrap();
        assert!(!adopt_wanted("quiet", Some("1"), &f, &|| true), "never after the first administrator");
        let _ = std::fs::remove_dir_all(&d);
    }
}
