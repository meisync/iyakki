use super::*;
use crate::mixer::MixError;
use crate::webmix::Opener;
use iyakki_shared::linuxapps_devices::{grant, DevicePolicy};

fn dir(tag: &str) -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!("iyakki-linsound-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// A tap whose mixer refuses (no card is touched).
fn refusing(p: &Path, _label: &'static str) -> Result<Tap, String> {
    let opener: Opener = Arc::new(|| Err(MixError::Disabled));
    Tap::start(p, opener)
}

fn off(_: &Path, _: &'static str) -> Result<Tap, String> {
    Err("the sound mixer is off".into())
}

#[test]
fn the_host_side_is_a_folder_with_a_pipe_and_a_conf_that_writes_into_it_and_a_second_start_reuses_the_tap() {
    use std::os::unix::fs::FileTypeExt;
    let d = dir("a");
    let r = MixerRoute::with_starter(refusing);
    r.prepare("linuxsb-a", &d.join("sound")).unwrap();
    let fifo = d.join("sound").join(ROUTE_FIFO);
    assert!(std::fs::symlink_metadata(&fifo).unwrap().file_type().is_fifo());
    let conf = std::fs::read_to_string(d.join("sound").join(ROUTE_CONF)).unwrap();
    assert!(conf.contains("type file") && conf.contains(&format!("file \"{ROUTE_DST}/{ROUTE_FIFO}\"")) && conf.contains("pcm.!default"), "{conf}");
    assert!(!conf.contains(&d.display().to_string()), "the sandbox sees its own path, not the host's");
    // again: the same tap is kept (the registry holds one per sandbox)
    r.prepare("linuxsb-a", &d.join("sound")).unwrap();
    assert_eq!(TAPS.lock().unwrap().keys().filter(|k| *k == "linuxsb-a").count(), 1);
    TAPS.lock().unwrap().remove("linuxsb-a");
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn a_mixer_that_is_off_or_a_bad_name_leaves_the_sandbox_on_the_card_nodes() {
    let d = dir("b");
    let off = MixerRoute::with_starter(off);
    assert!(off.prepare("linuxsb-b", &d.join("sound")).unwrap_err().contains("mixer is off"));
    assert!(!TAPS.lock().unwrap().contains_key("linuxsb-b"));
    for bad in ["", "a b", "../x", "a/b"] {
        assert!(MixerRoute::with_starter(refusing).prepare(bad, &d.join("sound2")).is_err(), "{bad:?}");
    }
    // through grant: no route made, so the playback nodes are still there
    let dev = d.join("dev");
    std::fs::create_dir_all(dev.join("snd")).unwrap();
    std::fs::write(dev.join("snd/pcmC0D0p"), b"").unwrap();
    let g = grant(&DevicePolicy { sound: true, gpu: false }, &dev, false, "linuxsb-b", &d.join("sound3"), Some(&off));
    assert!(!g.mixed && g.devices == ["/dev/snd/pcmC0D0p"]);
    // and with a working route the sandbox gets the folder and no card
    let ok = MixerRoute::with_starter(refusing);
    let g = grant(&DevicePolicy { sound: true, gpu: false }, &dev, false, "linuxsb-b2", &d.join("sound4"), Some(&ok));
    assert!(g.mixed && g.devices.is_empty() && g.bind.is_some(), "{g:?}");
    TAPS.lock().unwrap().remove("linuxsb-b2");
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn the_mixers_label_for_a_sandbox_names_it() {
    assert_eq!(label_of("dev"), "Linux app dev");
    assert!(std::ptr::eq(label_of("dev"), label_of("dev")), "one copy per name");
}
