//! Cloud mode: the types and the pure decision live in the leaf crate `iyakki-cloudmode` (re-exported here); this file holds what
//! reads the machine: the kernel command line, the release file and the real `/sys`.

pub use iyakki_cloudmode::*;
use std::path::Path;

/// The kernel command line of this start (`/proc/cmdline`), empty when it cannot be read. The one reader of that file in the agent crate:
/// other code asks here (the shared kernel-access rule, tests/kernel_single_reader.rs).
pub fn kernel_cmdline() -> String {
    std::fs::read_to_string("/proc/cmdline").unwrap_or_default()
}

/// The real machine: `/sys`, `/proc/cmdline` and `/etc/iyakki-release`.
pub fn detect(setting: Setting) -> CloudMode {
    if window_mode() {
        return window_desktop();
    }
    let cmdline = kernel_cmdline();
    let variant = crate::relinfo::ReleaseInfo::load(Path::new(crate::relinfo::RELEASE_FILE)).map(|r| r.variant).unwrap_or_default();
    cloud_mode(setting, &Facts::probe(Path::new("/"), &cmdline, &variant))
}


impl crate::browsermedia::Sources {
    /// The three sources that matter to cloud mode.
    pub fn cloud(&self) -> iyakki_cloudmode::BrowserSources {
        iyakki_cloudmode::BrowserSources { audio_out: self.audio_out, mic: self.mic, camera: self.camera }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use iyakki_sec::policy::App;

    #[test]
    fn window_mode_is_always_desktop_mode_whatever_the_facts_and_the_setting() {
        // (the flag is process-wide: this is the only test that sets it, and it puts it back)
        let was = window_mode();
        set_window_mode(true);
        for setting in [Setting::Auto, Setting::On, Setting::Off] {
            let m = detect(setting);
            assert!(!m.on && !m.forced && m.hidden_apps.is_empty() && m.audio_services && m.camera_watch, "{setting:?}: {}", m.why);
            assert_eq!(m.why, WINDOW_WHY);
        }
        for a in [App::Radio, App::Media, App::Tales, App::Voice, App::Camera, App::Disks] {
            assert!(!detect(Setting::On).hides_app(a), "{a:?} stays listed in the window");
        }
        set_window_mode(false);
        // a real image is unchanged: facts and setting decide as before
        assert!(cloud_mode(Setting::Auto, &Facts::bare()).on && cloud_mode(Setting::On, &Facts::full()).on);
        assert!(!cloud_mode(Setting::Auto, &Facts::full()).on);
        set_window_mode(was);
    }
}
