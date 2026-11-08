//! Cloud mode: a machine-wide mode for machines with no local screen, speakers or camera (servers, cloud VMs, headless boxes,
//! remote-desktop only). It hides what needs local hardware and keeps what makes sense remotely.
//!
//! The setting (`Auto` by default, `On`, `Off`) lives in the machine settings (administrators). `Auto` turns the mode on when the
//! machine has no display, no keyboard or pointer, no sound card, or the image is a cloud variant (`VARIANT=` contains `cloud`
//! in `/etc/iyakki-release`, or the boot parameter `iyakki.mode=cloud`). The boot parameter `iyakki.mode=cloud|desktop` beats the
//! setting. With the mode on, a thing is hidden when the hardware it needs is missing (`Auto`, boot parameter excepted) or always
//! (`On`, `iyakki.mode=cloud`). Everything is a pure function of [`Facts`], so tests feed it made-up machines.
//!
//! The tables below are the one place that says which app, Settings page, IYAKKI-menu entry and Devices row needs which hardware.
//! [`app_class`] is an exhaustive `match`: a new app does not compile until somebody decides.

use std::path::Path;

use iyakki_sec::policy::App;
use serde::{Deserialize, Serialize};

/// The administrator's choice.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Setting {
    #[default]
    Auto,
    On,
    Off,
}

impl Setting {
    pub fn label(self) -> &'static str {
        match self {
            Setting::Auto => "Automatic",
            Setting::On => "On",
            Setting::Off => "Off",
        }
    }
    /// Automatic -> On -> Off -> Automatic.
    pub fn next(self) -> Setting {
        match self {
            Setting::Auto => Setting::On,
            Setting::On => Setting::Off,
            Setting::Off => Setting::Auto,
        }
    }
}

/// The boot parameter `iyakki.mode=`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BootMode {
    Cloud,
    Desktop,
}

impl BootMode {
    pub fn parse(cmdline: &str) -> Option<BootMode> {
        // `iyakki.cloud=1` (the cloud images' kernel line) is the same as `iyakki.mode=cloud`; `iyakki.mode=` beats it when both are given
        if !cmdline.split_whitespace().any(|t| t.starts_with("iyakki.mode=")) {
            return cmdline.split_whitespace().any(|t| t == "iyakki.cloud=1").then_some(BootMode::Cloud);
        }
        let v = cmdline.split_whitespace().find_map(|t| t.strip_prefix("iyakki.mode="))?;
        match v {
            "cloud" => Some(BootMode::Cloud),
            "desktop" => Some(BootMode::Desktop),
            _ => None,
        }
    }
}

/// A piece of local hardware a feature needs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Need {
    Audio,
    Camera,
    Display,
    /// A keyboard or pointer (not just a power button).
    Input,
    Bluetooth,
    Wifi,
    Battery,
    /// Apple hardware (a Mac, or the Mac image).
    Apple,
    /// A disk, SSD, SD card or USB drive of this machine (the DISKS tile).
    Disks,
}

/// Where an app stands in cloud mode.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Class {
    /// Works over a remote desktop with no local hardware.
    Remote,
    /// Needs this hardware; hidden in cloud mode when it is missing (always, when forced).
    Hardware(Need),
}

/// The class of every app. Exhaustive on purpose: a new app has to be placed here.
pub fn app_class(app: App) -> Class {
    use Class::{Hardware, Remote};
    match app {
        App::Media => Hardware(Need::Audio),
        App::Radio => Hardware(Need::Audio),
        App::Tales => Hardware(Need::Audio), // reads aloud
        App::Camera => Hardware(Need::Camera),
        // reads aloud (needs sound output); dictation needs a microphone, which the tile says when there is none
        App::Voice => Hardware(Need::Audio),
        // mounting and partitioning the drives of this computer: pointless over a remote desktop, so hidden in cloud mode
        App::Disks => Hardware(Need::Disks),
        App::Chat | App::Rag | App::Mcp | App::Cli | App::Users | App::Settings | App::Runs | App::Edit | App::Browser | App::Mesh | App::Scan | App::Skills | App::Usage | App::Calendar | App::System | App::SrePilot | App::Inbox | App::Ide | App::Data | App::Store | App::Dash | App::Flows | App::K8s | App::LinuxWin | App::Lab | App::Git | App::Doctor | App::Checkup | App::Learn | App::DevTools | App::Auth => Remote,
    }
}

/// What a browser can supply to a session in place of local hardware (`browsermedia.rs`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BrowserSource {
    SoundOut,
    Mic,
    Camera,
}

/// Which browser sources can stand in for the hardware an app needs. Derived from [`app_class`], so a new app is placed there first.
pub fn browser_supply(app: App) -> &'static [BrowserSource] {
    match app_class(app) {
        Class::Hardware(Need::Audio) if app == App::Voice => &[BrowserSource::SoundOut, BrowserSource::Mic],
        Class::Hardware(Need::Audio) => &[BrowserSource::SoundOut],
        Class::Hardware(Need::Camera) => &[BrowserSource::Camera],
        Class::Hardware(_) | Class::Remote => &[],
    }
}

/// The Settings pages that need hardware (by the name Settings knows them under). Network stays: wired settings are remote-safe.
pub const SETTINGS_PAGES: [(&str, Need); 4] = [("sound", Need::Audio), ("display", Need::Display), ("keyboard", Need::Input), ("bluetooth", Need::Bluetooth)];

/// The entries of the IYAKKI menu that need hardware (by page name).
pub const MENU_ENTRIES: [(&str, Need); 2] = [("media", Need::Audio), ("mac", Need::Apple)];

/// The rows of the Devices popover that describe hardware (by title).
pub const DEVICE_ROWS: [(&str, Need); 6] = [("Sound", Need::Audio), ("Display", Need::Display), ("Camera", Need::Camera), ("Bluetooth", Need::Bluetooth), ("Keyboard and pointer", Need::Input), ("Battery", Need::Battery)];

/// What the machine has, found by looking at `/sys` (or a made-up copy of it).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Facts {
    pub display: bool,
    pub input: bool,
    pub audio: bool,
    pub camera: bool,
    pub bluetooth: bool,
    pub wifi: bool,
    pub battery: bool,
    pub apple: bool,
    /// A physical drive (not a RAM disk, loop device or device-mapper node) is listed in `/sys/block`.
    pub disks: bool,
    /// `VARIANT=` of the release file.
    pub variant: String,
    pub boot: Option<BootMode>,
}

impl Facts {
    /// A machine with everything (what tests and desktops look like).
    pub fn full() -> Facts {
        Facts { display: true, input: true, audio: true, camera: true, bluetooth: true, wifi: true, battery: true, apple: true, disks: true, variant: String::new(), boot: None }
    }

    /// A machine with nothing local at all.
    pub fn bare() -> Facts {
        Facts { display: false, input: false, audio: false, camera: false, bluetooth: false, wifi: false, battery: false, apple: false, disks: false, variant: String::new(), boot: None }
    }

    pub fn has(&self, n: Need) -> bool {
        match n {
            Need::Audio => self.audio,
            Need::Camera => self.camera,
            Need::Display => self.display,
            Need::Input => self.input,
            Need::Bluetooth => self.bluetooth,
            Need::Wifi => self.wifi,
            Need::Battery => self.battery,
            Need::Apple => self.apple || self.variant.to_lowercase().contains("mac"),
            Need::Disks => self.disks,
        }
    }

    pub fn variant_is_cloud(&self) -> bool {
        self.variant.to_lowercase().contains("cloud")
    }

    /// Look at the machine under `root` (`/` for the real one): `sys/class/...`, with the release file's variant and the kernel
    /// command line given.
    pub fn probe(root: &Path, cmdline: &str, variant: &str) -> Facts {
        let class = |name: &str| -> Vec<String> {
            std::fs::read_dir(root.join("sys/class").join(name)).map(|rd| rd.flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect()).unwrap_or_default()
        };
        let read = |p: &str| std::fs::read_to_string(root.join(p)).unwrap_or_default().trim().to_string();
        let display = class("drm").iter().any(|n| n.starts_with("card")) || class("graphics").iter().any(|n| n.starts_with("fb"));
        // power, sleep and lid buttons are inputs too, but nobody types on them
        let input = class("input").iter().filter(|n| n.starts_with("input")).any(|n| {
            let name = read(&format!("sys/class/input/{n}/name")).to_lowercase();
            !(name.contains("button") || name.contains("lid switch") || name.contains("speaker") || name.contains("hdmi") || name.is_empty())
        });
        let audio = class("sound").iter().any(|n| n.starts_with("card"));
        let camera = class("video4linux").iter().any(|n| n.starts_with("video"));
        let bluetooth = class("bluetooth").iter().any(|n| n.starts_with("hci"));
        let wifi = class("net").iter().any(|n| root.join("sys/class/net").join(n).join("wireless").exists()) || !class("ieee80211").is_empty();
        let battery = class("power_supply").iter().any(|n| read(&format!("sys/class/power_supply/{n}/type")).eq_ignore_ascii_case("battery"));
        let apple = read("sys/class/dmi/id/sys_vendor").contains("Apple");
        let disks = std::fs::read_dir(root.join("sys/block")).map(|rd| rd.flatten().any(|e| !["loop", "ram", "zram", "dm-", "md", "sr", "fd", "nbd"].iter().any(|p| e.file_name().to_string_lossy().starts_with(p)))).unwrap_or(false);
        Facts { display, input, audio, camera, bluetooth, wifi, battery, apple, disks, variant: variant.to_string(), boot: BootMode::parse(cmdline) }
    }
}

/// The decision, and everything that follows from it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CloudMode {
    pub on: bool,
    /// Everything hardware is hidden whether the hardware exists or not (the setting `On`, or `iyakki.mode=cloud`).
    pub forced: bool,
    /// Why the mode is on or off, in a sentence for About and Settings.
    pub why: String,
    pub hidden_apps: Vec<App>,
    pub hidden_settings_pages: Vec<&'static str>,
    pub hidden_menu_entries: Vec<&'static str>,
    pub hidden_device_rows: Vec<&'static str>,
    /// The Fn, volume, brightness and media keys (and their on-screen display) do nothing.
    pub fn_keys_off: bool,
    /// The sound output is not probed and no soft volume is set up.
    pub audio_services: bool,
    /// The shell does not look for cameras every second.
    pub camera_watch: bool,
}

impl CloudMode {
    /// Nothing hidden: the desktop.
    pub fn desktop() -> CloudMode {
        CloudMode { on: false, forced: false, why: "Desktop mode.".into(), hidden_apps: Vec::new(), hidden_settings_pages: Vec::new(), hidden_menu_entries: Vec::new(), hidden_device_rows: Vec::new(), fn_keys_off: false, audio_services: true, camera_watch: true }
    }

    pub fn hides_app(&self, app: App) -> bool {
        self.hidden_apps.contains(&app)
    }

    pub fn hides_page(&self, page: &str) -> bool {
        self.hidden_settings_pages.contains(&page)
    }

    pub fn hides_menu(&self, entry: &str) -> bool {
        self.hidden_menu_entries.contains(&entry)
    }

    /// The same mode with the apps a connected browser supplies the hardware for shown again (the browser's sound, microphone or
    /// camera is on for this session). Nothing changes when the mode is off or no source is on.
    pub fn with_browser(&self, src: &crate::browsermedia::Sources) -> CloudMode {
        let on = |b: BrowserSource| match b {
            BrowserSource::SoundOut => src.audio_out,
            BrowserSource::Mic => src.mic,
            BrowserSource::Camera => src.camera,
        };
        let mut c = self.clone();
        c.hidden_apps.retain(|a| !browser_supply(*a).iter().any(|b| on(*b)));
        c.camera_watch |= src.camera;
        c
    }

    /// The About / Settings line: the mode and why.
    pub fn about_line(&self) -> String {
        format!("{} {}", if self.on { "Cloud mode: on." } else { "Cloud mode: off." }, self.why)
    }
}

/// Decide the mode for a machine with `facts` under the administrator's `setting`.
pub fn cloud_mode(setting: Setting, facts: &Facts) -> CloudMode {
    let missing: Vec<&str> = [(!facts.display, "no screen"), (!facts.input, "no keyboard or pointer"), (!facts.audio, "no sound card")].iter().filter(|(m, _)| *m).map(|(_, t)| *t).collect();
    let (on, forced, why) = match (facts.boot, setting) {
        (Some(BootMode::Cloud), _) => (true, true, "The boot parameter iyakki.mode=cloud says so.".to_string()),
        (Some(BootMode::Desktop), _) => (false, false, "The boot parameter iyakki.mode=desktop says so.".to_string()),
        (None, Setting::On) => (true, true, "The setting is On (Settings, Cloud mode).".to_string()),
        (None, Setting::Off) => (false, false, "The setting is Off (Settings, Cloud mode).".to_string()),
        (None, Setting::Auto) if facts.variant_is_cloud() => (true, false, format!("Automatic: this image is the cloud variant ({}).", facts.variant)),
        (None, Setting::Auto) if !missing.is_empty() => (true, false, format!("Automatic: this machine has {}.", missing.join(", "))),
        (None, Setting::Auto) => (false, false, "Automatic: this machine has a screen, input and sound.".to_string()),
    };
    if !on {
        let mut d = CloudMode::desktop();
        d.why = why;
        return d;
    }
    let hide = |n: Need| forced || !facts.has(n);
    let hidden_apps: Vec<App> = App::ALL.iter().copied().filter(|a| matches!(app_class(*a), Class::Hardware(n) if hide(n))).collect();
    let pick = |t: &[(&'static str, Need)]| -> Vec<&'static str> { t.iter().filter(|(_, n)| hide(*n)).map(|(s, _)| *s).collect() };
    CloudMode {
        on,
        forced,
        why,
        camera_watch: !hidden_apps.contains(&App::Camera),
        hidden_apps,
        hidden_settings_pages: pick(&SETTINGS_PAGES),
        hidden_menu_entries: pick(&MENU_ENTRIES),
        hidden_device_rows: pick(&DEVICE_ROWS),
        fn_keys_off: hide(Need::Input),
        audio_services: !hide(Need::Audio),
    }
}

/// The kernel command line of this start (`/proc/cmdline`), empty when it cannot be read. The one reader of that file in the agent crate:
/// other code asks here (the shared kernel-access rule, tests/kernel_single_reader.rs).
pub fn kernel_cmdline() -> String {
    std::fs::read_to_string("/proc/cmdline").unwrap_or_default()
}

/// The real machine: `/sys`, `/proc/cmdline` and `/etc/iyakki-release`.
pub fn detect(setting: Setting) -> CloudMode {
    let cmdline = kernel_cmdline();
    let variant = crate::relinfo::ReleaseInfo::load(Path::new(crate::relinfo::RELEASE_FILE)).map(|r| r.variant).unwrap_or_default();
    cloud_mode(setting, &Facts::probe(Path::new("/"), &cmdline, &variant))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(tag: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("iyakki-cloud-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }
    fn touch(root: &Path, p: &str, text: &str) {
        let f = root.join(p);
        std::fs::create_dir_all(f.parent().unwrap()).unwrap();
        std::fs::write(f, text).unwrap();
    }

    #[test]
    fn every_app_has_a_class_and_the_audio_and_camera_ones_are_hardware() {
        for a in App::ALL {
            let _ = app_class(a); // exhaustive: adding an App without a class does not compile
        }
        let hw: Vec<App> = App::ALL.iter().copied().filter(|a| matches!(app_class(*a), Class::Hardware(_))).collect();
        assert_eq!(hw.len(), 6, "MEDIA, RADIO, TALES, VOICE, CAMERA and DISKS are the apps that need local hardware");
        for a in [App::Chat, App::Rag, App::Mcp, App::Cli, App::Skills, App::Calendar, App::Ide, App::Edit, App::Data, App::Dash, App::Flows, App::Runs, App::SrePilot, App::System, App::Store, App::Users, App::Usage, App::Scan, App::Mesh, App::Browser, App::Settings, App::K8s] {
            assert_eq!(app_class(a), Class::Remote, "{a:?} is remote-safe");
        }
    }

    #[test]
    fn browser_sources_bring_back_the_tiles_they_stand_in_for() {
        use crate::browsermedia::Sources;
        // every app has a class, and the browser can supply exactly the audio and camera ones (not DISKS)
        for a in App::ALL {
            let sup = browser_supply(a);
            match app_class(a) {
                Class::Remote => assert!(sup.is_empty(), "{a:?}"),
                Class::Hardware(Need::Audio) => assert!(sup.contains(&BrowserSource::SoundOut), "{a:?}"),
                Class::Hardware(Need::Camera) => assert_eq!(sup, &[BrowserSource::Camera]),
                Class::Hardware(_) => assert!(sup.is_empty(), "{a:?}"),
            }
        }
        let bare = cloud_mode(Setting::On, &Facts::full());
        assert!(bare.hides_app(App::Media) && bare.hides_app(App::Camera) && bare.hides_app(App::Voice));
        let none = Sources::default();
        assert_eq!(bare.with_browser(&none), bare, "nothing on, nothing changes");
        let sound = bare.with_browser(&Sources { audio_out: true, ..none });
        for a in [App::Media, App::Radio, App::Tales, App::Voice] {
            assert!(!sound.hides_app(a), "{a:?} works with the browser's speakers");
        }
        assert!(sound.hides_app(App::Camera) && sound.hides_app(App::Disks));
        let cam = bare.with_browser(&Sources { camera: true, ..none });
        assert!(!cam.hides_app(App::Camera) && cam.hides_app(App::Media) && cam.camera_watch);
        let mic = bare.with_browser(&Sources { mic: true, ..none });
        assert!(!mic.hides_app(App::Voice) && mic.hides_app(App::Media), "the microphone brings back VOICE (dictation) only");
        let off = cloud_mode(Setting::Off, &Facts::bare());
        assert_eq!(off.with_browser(&Sources { camera: true, mic: true, audio_out: true, clip_read: false }), off);
    }

    #[test]
    fn auto_follows_the_hardware() {
        let c = cloud_mode(Setting::Auto, &Facts::full());
        assert!(!c.on && c.hidden_apps.is_empty() && c.audio_services && c.camera_watch && !c.fn_keys_off, "a full machine is a desktop");
        for f in [Facts { display: false, ..Facts::full() }, Facts { input: false, ..Facts::full() }, Facts { audio: false, ..Facts::full() }] {
            assert!(cloud_mode(Setting::Auto, &f).on, "{f:?}");
        }
        let nosound = cloud_mode(Setting::Auto, &Facts { audio: false, ..Facts::full() });
        assert_eq!(nosound.hidden_apps, vec![App::Radio, App::Media, App::Tales, App::Voice]);
        assert!(nosound.hides_page("sound") && !nosound.hides_page("display") && !nosound.hides_app(App::Camera), "a camera that exists stays");
        assert!(nosound.hides_menu("media") && !nosound.hides_menu("mac"));
        assert!(!nosound.audio_services && nosound.camera_watch);
        assert!(nosound.why.contains("no sound card"));
    }

    #[test]
    fn bare_machine_hides_all_hardware() {
        let c = cloud_mode(Setting::Auto, &Facts::bare());
        assert!(c.on && !c.forced);
        for a in [App::Media, App::Radio, App::Tales, App::Voice, App::Camera] {
            assert!(c.hides_app(a));
        }
        assert_eq!(c.hidden_settings_pages, vec!["sound", "display", "keyboard", "bluetooth"]);
        assert_eq!(c.hidden_device_rows.len(), DEVICE_ROWS.len());
        assert!(c.fn_keys_off && !c.audio_services && !c.camera_watch);
        assert!(c.why.contains("no screen") && c.why.contains("no keyboard") && c.why.contains("no sound"));
    }

    #[test]
    fn the_cloud_variant_turns_it_on_but_keeps_hardware_that_exists() {
        let f = Facts { variant: "cloud-x86_64".into(), ..Facts::full() };
        let c = cloud_mode(Setting::Auto, &f);
        assert!(c.on && !c.forced && c.hidden_apps.is_empty(), "auto: hardware that exists stays visible");
        assert!(c.why.contains("cloud variant"));
        let c = cloud_mode(Setting::On, &Facts::full());
        assert!(c.on && c.forced && c.hides_app(App::Media) && c.hides_app(App::Camera) && c.hides_page("keyboard") && c.fn_keys_off, "On forces hiding");
        assert!(!cloud_mode(Setting::Off, &Facts::bare()).on, "Off beats missing hardware");
        assert!(cloud_mode(Setting::Off, &Facts::bare()).why.contains("Off"));
    }

    #[test]
    fn the_boot_parameter_beats_the_setting() {
        assert_eq!(BootMode::parse("quiet iyakki.mode=cloud x=1"), Some(BootMode::Cloud));
        assert_eq!(BootMode::parse("iyakki.mode=desktop"), Some(BootMode::Desktop));
        assert_eq!(BootMode::parse("iyakki.mode=other"), None);
        assert_eq!(BootMode::parse("quiet"), None);
        assert_eq!(BootMode::parse("quiet iyakki.cloud=1"), Some(BootMode::Cloud));
        assert_eq!(BootMode::parse("iyakki.cloud=1 iyakki.mode=desktop"), Some(BootMode::Desktop));
        assert_eq!(BootMode::parse("iyakki.cloud=0"), None);
        let cloud = Facts { boot: Some(BootMode::Cloud), ..Facts::full() };
        let c = cloud_mode(Setting::Off, &cloud);
        assert!(c.on && c.forced && c.hides_app(App::Media) && c.why.contains("iyakki.mode=cloud"));
        let desk = Facts { boot: Some(BootMode::Desktop), ..Facts::bare() };
        let c = cloud_mode(Setting::On, &desk);
        assert!(!c.on && c.hidden_apps.is_empty() && c.why.contains("iyakki.mode=desktop"));
    }

    #[test]
    fn mac_entry_follows_apple_hardware_or_the_mac_variant() {
        let c = cloud_mode(Setting::Auto, &Facts { audio: false, apple: false, ..Facts::full() });
        assert!(c.hides_menu("mac"));
        let c = cloud_mode(Setting::Auto, &Facts { audio: false, apple: false, variant: "macweb".into(), ..Facts::full() });
        assert!(!c.hides_menu("mac"));
        let c = cloud_mode(Setting::On, &Facts::full());
        assert!(c.hides_menu("mac") && c.hides_menu("media"), "forced hides them even on a Mac");
    }

    #[test]
    fn the_probe_reads_a_made_up_sys() {
        let r = tmp("probe-empty");
        let f = Facts::probe(&r, "quiet", "libre");
        assert_eq!((f.display, f.input, f.audio, f.camera, f.bluetooth, f.wifi, f.battery, f.apple), (false, false, false, false, false, false, false, false));
        assert!(cloud_mode(Setting::Auto, &f).on);
        let r = tmp("probe-full");
        touch(&r, "sys/class/drm/card0/uevent", "");
        touch(&r, "sys/class/input/input0/name", "Power Button");
        touch(&r, "sys/class/sound/card0/id", "x");
        let f = Facts::probe(&r, "iyakki.mode=cloud", "");
        assert!(f.display && !f.input && f.audio, "a power button is not a keyboard");
        assert_eq!(f.boot, Some(BootMode::Cloud));
        touch(&r, "sys/class/input/input1/name", "AT Translated Set 2 keyboard");
        touch(&r, "sys/class/video4linux/video0/name", "cam");
        touch(&r, "sys/class/bluetooth/hci0/x", "");
        touch(&r, "sys/class/net/wlan0/wireless/x", "");
        touch(&r, "sys/class/power_supply/BAT0/type", "Battery\n");
        touch(&r, "sys/class/dmi/id/sys_vendor", "Apple Inc.\n");
        let f = Facts::probe(&r, "", "");
        assert_eq!(f, Facts { display: true, input: true, audio: true, camera: true, bluetooth: true, wifi: true, battery: true, apple: true, disks: false, variant: String::new(), boot: None });
        assert!(!cloud_mode(Setting::Auto, &f).on);
        let _ = std::fs::remove_dir_all(&r);
    }

    #[test]
    fn setting_round_trips_and_defaults_to_auto() {
        assert_eq!(Setting::default(), Setting::Auto);
        assert_eq!(serde_json::to_string(&Setting::On).unwrap(), "\"on\"");
        assert_eq!(serde_json::from_str::<Setting>("\"off\"").unwrap(), Setting::Off);
        assert_eq!(Setting::Auto.next().next().next(), Setting::Auto);
    }
}
