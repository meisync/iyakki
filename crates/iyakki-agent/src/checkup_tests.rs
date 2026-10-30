//! CHECK-UP's decisions on made-up machines, fake recorders and files in a temporary folder. No microphone, speaker or camera is touched.

use super::*;
use std::sync::{Arc, Mutex};

fn inv() -> Inventory {
    Inventory {
        keyboards: 1,
        pointers: 1,
        touchscreens: 0,
        sound_cards: 1,
        sound_chosen_present: true,
        mics: vec![Mic { id: "plughw:1,0".into(), label: "USB Audio (USB)".into(), usb: true }],
        product: "Mac mini".into(),
        screens: vec![ScreenOut { name: "HDMI-A-1".into(), connected: true, current: Some((1920, 1080)) }],
        ethernet: vec![Adapter { name: "eth0".into(), up: true }],
        wifi: vec![Adapter { name: "wlan0".into(), up: false }],
        route: true,
        bluetooth: 1,
        ..Inventory::default()
    }
}

fn tmp(tag: &str) -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!("iyakki-checkup-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn put(root: &Path, rel: &str, text: &str) {
    let p = root.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, text).unwrap();
}

#[test]
fn every_step_has_a_name_and_the_capture_steps_are_the_two_that_need_a_click() {
    assert_eq!(StepId::ALL.len(), 12);
    let ids: std::collections::HashSet<_> = StepId::ALL.iter().map(|s| s.id()).collect();
    assert_eq!(ids.len(), 12);
    assert!(StepId::ALL.iter().all(|s| !s.title().is_empty()));
    assert_eq!(StepId::ALL.iter().filter(|s| s.is_capture()).map(|s| s.id()).collect::<Vec<_>>(), ["microphone", "camera"]);
}

#[test]
fn the_screen_step_passes_on_yes_and_fails_with_the_display_page_on_no() {
    assert_eq!(display(Said::Yes).outcome, Outcome::Pass);
    let f = display(Said::No);
    assert_eq!(f.outcome, Outcome::Fail);
    assert!(f.fix.is_some_and(|x| x.action == Action::Settings("tab:display")) && !f.advice.is_empty());
    assert_eq!(display(Said::Skip).outcome, Outcome::Skipped);
}

#[test]
fn touch_is_skipped_when_no_touch_screen_is_listed_and_otherwise_follows_the_person() {
    assert_eq!(touch(&inv(), Said::No).outcome, Outcome::Skipped, "nothing to test");
    let t = Inventory { touchscreens: 1, ..inv() };
    assert_eq!(touch(&t, Said::Yes).outcome, Outcome::Pass);
    assert_eq!(touch(&t, Said::No).outcome, Outcome::Fail);
}

#[test]
fn the_keyboard_needs_three_different_keys_and_says_when_none_is_listed() {
    assert_eq!(keyboard(&inv(), 3, Said::Yes).outcome, Outcome::Pass);
    let few = keyboard(&inv(), 1, Said::No);
    assert_eq!((few.outcome, few.note.as_str()), (Outcome::Fail, "Only 1 key arrived."));
    let none = keyboard(&Inventory { keyboards: 0, ..inv() }, 0, Said::No);
    assert!(none.note.contains("No keyboard is listed"), "{}", none.note);
    assert_eq!(keyboard(&inv(), 0, Said::Skip).outcome, Outcome::Skipped);
}

#[test]
fn the_pointer_passes_on_a_click_on_the_target() {
    assert_eq!(pointer(&inv(), true, Said::Yes).outcome, Outcome::Pass);
    let f = pointer(&Inventory { pointers: 0, ..inv() }, false, Said::No);
    assert_eq!(f.outcome, Outcome::Fail);
    assert!(f.note.contains("No mouse or touch pad is listed"));
}

#[test]
fn inputs_are_counted_from_the_kernels_device_list() {
    let text = "I: Bus=0011 Vendor=0001 Product=0001 Version=ab41\nN: Name=\"AT Translated Set 2 keyboard\"\nH: Handlers=sysrq kbd event3 leds\nB: PROP=0\nB: EV=120013\nB: KEY=402000000 3803078f800d001 feffffdfffefffff fffffffffffffffe\nB: MSC=10\nB: LED=7\n\nI: Bus=0003 Vendor=046d Product=c077 Version=0111\nN: Name=\"Logitech USB Optical Mouse\"\nH: Handlers=mouse0 event5\nB: PROP=0\nB: EV=17\nB: KEY=70000 0 0 0 0\nB: REL=903\nB: MSC=10\n";
    assert_eq!(count_inputs(text), (1, 1, 0));
    assert_eq!(count_inputs(""), (0, 0, 0));
}

#[test]
fn the_tone_is_a_short_valid_wav() {
    let w = tone_wav();
    assert_eq!(&w[..4], b"RIFF");
    let parsed = crate::voice::parse_wav(&w).unwrap();
    assert_eq!(parsed.rate, crate::voice::SAMPLE_RATE);
    let secs = parsed.samples.len() as f32 / parsed.rate as f32;
    assert!((0.5..1.0).contains(&secs), "{secs}");
    assert!(crate::voice::peak(&parsed.samples) > 1000, "loud enough to hear, soft enough to be safe");
    assert!(crate::voice::peak(&parsed.samples) < 16000);
}

#[test]
fn the_speaker_step_reads_the_card_the_output_the_mixer_and_the_persons_ears() {
    let ok = Ok(());
    assert_eq!(speaker(&inv(), &ok, Said::Yes).outcome, Outcome::Pass);
    let deaf = speaker(&inv(), &ok, Said::No);
    assert_eq!(deaf.outcome, Outcome::Fail);
    assert!(deaf.advice.contains("volume") && deaf.fix.is_some());
    let none = speaker(&Inventory { sound_cards: 0, ..inv() }, &ok, Said::No);
    assert_eq!((none.outcome, none.doctor), (Outcome::Fail, Some("sound-none")));
    let gone = speaker(&Inventory { sound_chosen_present: false, ..inv() }, &ok, Said::No);
    assert_eq!(gone.doctor, Some("sound-missing"));
    let busy = speaker(&inv(), &Err("the speaker is busy (Radio)".into()), Said::Yes);
    assert_eq!(busy.outcome, Outcome::Fail);
    assert!(busy.note.contains("busy"), "{}", busy.note);
    assert_eq!(speaker(&inv(), &ok, Said::Skip).outcome, Outcome::Skipped);
}

// ------------------------------------------------------------------------------------------------ the microphone

struct Cap {
    pcm: Vec<i16>,
}
impl Capture for Cap {
    fn level(&self) -> f32 {
        level_of(&self.pcm)
    }
    fn elapsed(&self) -> Duration {
        MIC_SECS
    }
    fn is_full(&self) -> bool {
        false
    }
    fn is_alive(&mut self) -> bool {
        true
    }
    fn stop(self: Box<Self>) -> Result<Vec<i16>, String> {
        Ok(self.pcm)
    }
}

struct Recs {
    mics: Vec<Mic>,
    product: String,
    pcm: Vec<i16>,
    fail: Option<String>,
    started: Mutex<Vec<String>>,
}
impl Recorders for Recs {
    fn mics(&self) -> Vec<Mic> {
        self.mics.clone()
    }
    fn product(&self) -> String {
        self.product.clone()
    }
    fn start(&self, cmd: &str, _max: usize) -> Result<Box<dyn Capture>, String> {
        self.started.lock().unwrap().push(cmd.to_string());
        match &self.fail {
            Some(e) => Err(e.clone()),
            None => Ok(Box::new(Cap { pcm: self.pcm.clone() })),
        }
    }
}

fn recs(mics: Vec<Mic>, pcm: Vec<i16>) -> Recs {
    Recs { mics, product: "Mac mini".into(), pcm, fail: None, started: Mutex::new(Vec::new()) }
}

fn usb(id: &str) -> Mic {
    Mic { id: id.into(), label: format!("USB {id}"), usb: true }
}

#[test]
fn the_same_microphone_as_the_voice_tile_is_used() {
    let r = recs(vec![usb("plughw:1,0"), usb("plughw:2,0")], crate::voice::tone(300.0, 500));
    // the person's choice from VoiceSettings.mic
    assert!(start_mic(&r, "plughw:2,0", "arecord -q -f S16_LE -r 16000 -c 1 -t raw", 5, false).is_ok());
    let cmd = r.started.lock().unwrap()[0].clone();
    assert!(cmd.contains("-D plughw:2,0"), "{cmd}");
    // a choice that is gone falls back to the first listed, as Voice does
    assert!(start_mic(&r, "plughw:9,9", "arecord -q -f S16_LE -r 16000 -c 1 -t raw", 5, false).is_ok());
    assert!(r.started.lock().unwrap()[1].contains("-D plughw:1,0"));
    // automatic
    assert!(start_mic(&r, "", "arecord -q -f S16_LE -r 16000 -c 1 -t raw", 5, false).is_ok());
    assert!(r.started.lock().unwrap()[2].contains("-D plughw:1,0"));
}

#[test]
fn no_listed_microphone_says_so_and_a_t2_mac_says_the_capture_switch_is_off() {
    let r = recs(vec![], vec![]);
    let Err(MicStart::NoMic(plain)) = start_mic(&r, "", "arecord", 5, false) else { panic!("expected NoMic") };
    assert!(plain.contains("No microphone found"), "{plain}");
    let Err(MicStart::NoMic(t2)) = start_mic(&r, "", "arecord", 5, true) else { panic!("expected NoMic") };
    assert!(t2.contains("switched off") && t2.contains("capture"), "{t2}");
    assert!(r.started.lock().unwrap().is_empty(), "nothing was opened");
    let s = microphone(&Inventory { t2_mic_hidden: true, ..inv() }, &MicHeard::NoMic(t2), "");
    assert_eq!(s.outcome, Outcome::Skipped);
    assert!(s.advice.contains("capture switch") && s.advice.contains("USB microphone"), "{}", s.advice);
    let plain_s = microphone(&inv(), &MicHeard::NoMic(plain), "");
    assert_eq!(plain_s.outcome, Outcome::Skipped);
    assert!(plain_s.advice.contains("Plug in a USB microphone"));
}

#[test]
fn a_recording_that_cannot_start_is_a_fail_with_the_sound_page() {
    let mut r = recs(vec![usb("plughw:1,0")], vec![]);
    r.fail = Some("device busy".into());
    let Err(MicStart::Failed(e)) = start_mic(&r, "", "arecord", 5, false) else { panic!("expected Failed") };
    let s = microphone(&inv(), &MicHeard::Failed(e), "USB");
    assert_eq!(s.outcome, Outcome::Fail);
    assert!(s.note.contains("device busy") && s.fix.is_some());
}

#[test]
fn a_loud_recording_passes_and_silence_fails_and_nothing_is_kept_in_the_verdict() {
    let loud = crate::voice::tone(300.0, 1200);
    let ok = microphone(&inv(), &MicHeard::Samples(loud.clone()), "USB Audio (USB)");
    assert_eq!(ok.outcome, Outcome::Pass);
    assert!(ok.note.contains("USB Audio (USB)") && ok.note.contains("nothing was kept"));
    let quiet = microphone(&inv(), &MicHeard::Samples(vec![3; 32000]), "");
    assert_eq!(quiet.outcome, Outcome::Fail);
    assert!(quiet.note.contains("Only silence"));
    assert_eq!(microphone(&inv(), &MicHeard::Samples(vec![]), "").outcome, Outcome::Fail);
    assert!(level_of(&loud) > 0.3 && level_of(&[]) == 0.0 && level_of(&[i16::MAX]) <= 1.0);
}

// ------------------------------------------------------------------------------------------------ the camera

#[test]
fn the_camera_step_tells_no_camera_a_busy_camera_a_flat_picture_and_a_good_one() {
    assert_eq!(camera(&CamHeard::NoCamera("No camera found.".into())).outcome, Outcome::Skipped);
    let busy = camera(&CamHeard::Failed { name: "USB Camera".into(), why: "device or resource busy".into() });
    assert_eq!((busy.outcome, busy.doctor), (Outcome::Fail, Some("engine-camera")));
    let flat = camera(&CamHeard::Picture { name: "USB Camera".into(), width: 640, height: 480, spread: 0 });
    assert_eq!(flat.outcome, Outcome::Fail);
    assert!(flat.advice.contains("lens cover"));
    let good = camera(&CamHeard::Picture { name: "USB Camera".into(), width: 640, height: 480, spread: 90 });
    assert_eq!(good.outcome, Outcome::Pass);
    assert!(good.note.contains("nothing was kept"));
}

#[test]
fn a_flat_picture_has_no_spread_and_a_busy_one_has_some() {
    assert_eq!(picture_spread(&[40, 40, 40, 255].repeat(1000)), 0);
    let mut px = [0u8, 0, 0, 255].repeat(500);
    px.extend([255u8, 255, 255, 255].repeat(500));
    assert!(picture_spread(&px) > 200);
}

// ------------------------------------------------------------------------------------------------ the machine's own steps

#[test]
fn the_monitor_step_lists_the_outputs() {
    let mut i = inv();
    i.screens = vec![ScreenOut { name: "eDP-1".into(), connected: true, current: Some((2560, 1600)) }];
    let alone = monitor(&i);
    assert_eq!(alone.outcome, Outcome::Skipped);
    assert!(alone.note.contains("eDP-1 2560 x 1600"), "{}", alone.note);
    i.screens.push(ScreenOut { name: "HDMI-A-1".into(), connected: true, current: Some((1920, 1080)) });
    i.screens.push(ScreenOut { name: "DP-2".into(), connected: false, current: None });
    let two = monitor(&i);
    assert_eq!(two.outcome, Outcome::Pass);
    assert!(two.note.contains("HDMI-A-1 1920 x 1080") && !two.note.contains("DP-2"), "{}", two.note);
    i.screens[1].current = None;
    let blank = monitor(&i);
    assert_eq!(blank.outcome, Outcome::Fail);
    assert!(blank.note.contains("HDMI-A-1") && blank.fix.is_some());
    assert_eq!(monitor(&Inventory { screens: vec![], ..inv() }).outcome, Outcome::Skipped);
}

#[test]
fn the_network_step_needs_an_adapter_a_link_and_a_route() {
    assert_eq!(network(&inv()).outcome, Outcome::Pass);
    let none = network(&Inventory { ethernet: vec![], wifi: vec![], ..inv() });
    assert_eq!(none.outcome, Outcome::Fail);
    assert!(none.note.contains("No network adapter"));
    let down = network(&Inventory { ethernet: vec![Adapter { name: "eth0".into(), up: false }], ..inv() });
    assert_eq!(down.outcome, Outcome::Fail);
    assert!(down.note.contains("none is connected"));
    let noroute = network(&Inventory { route: false, ..inv() });
    assert_eq!(noroute.outcome, Outcome::Fail);
    assert!(noroute.note.contains("no way out"));
}

#[test]
fn wifi_and_bluetooth_say_missing_switched_off_or_fine() {
    assert_eq!(wifi(&inv()).outcome, Outcome::Pass);
    assert_eq!(wifi(&Inventory { wifi: vec![], ..inv() }).outcome, Outcome::Skipped);
    let off = wifi(&Inventory { wifi_blocked: true, ..inv() });
    assert_eq!(off.outcome, Outcome::Fail);
    assert!(off.advice.contains("airplane"));
    assert_eq!(bluetooth(&inv()).outcome, Outcome::Pass);
    assert_eq!(bluetooth(&Inventory { bluetooth: 0, ..inv() }).outcome, Outcome::Skipped);
    assert_eq!(bluetooth(&Inventory { bluetooth_blocked: true, ..inv() }).outcome, Outcome::Fail);
}

#[test]
fn the_power_step_knows_a_desktop_a_laptop_without_a_battery_and_a_worn_battery() {
    assert_eq!(power(&inv()).outcome, Outcome::Skipped);
    let lap = Inventory { power: PowerFacts { laptop: true, ..PowerFacts::default() }, ..inv() };
    assert_eq!(power(&lap).outcome, Outcome::Fail);
    let nodrv = Inventory { power: PowerFacts { no_driver: true, ..PowerFacts::default() }, ..inv() };
    assert!(power(&nodrv).note.contains("no driver"));
    let good = Inventory { power: PowerFacts { battery: true, laptop: true, percent: Some(80), status: "Discharging".into(), ac_online: Some(false), health_pct: Some(91), ..PowerFacts::default() }, ..inv() };
    let g = power(&good);
    assert_eq!(g.outcome, Outcome::Pass);
    assert!(g.note.contains("80 %") && g.note.contains("running on the battery") && g.note.contains("91 %"), "{}", g.note);
    let worn = Inventory { power: PowerFacts { health_pct: Some(40), ..good.power.clone() }, ..inv() };
    assert_eq!(power(&worn).outcome, Outcome::Fail);
}

// ------------------------------------------------------------------------------------------------ the readers

#[test]
fn network_adapters_are_read_from_sysfs_and_virtual_ones_are_left_out() {
    let sys = tmp("net");
    for n in ["lo", "docker0", "veth12", "eth0", "wlan0"] {
        put(&sys, &format!("class/net/{n}/operstate"), if n == "eth0" { "up\n" } else { "down\n" });
    }
    put(&sys, "class/net/eth0/device/vendor", "0x10ec");
    put(&sys, "class/net/wlan0/device/vendor", "0x14e4");
    put(&sys, "class/net/wlan0/wireless/link", "0");
    let (eth, wifi) = read_adapters(&sys);
    assert_eq!(eth, [Adapter { name: "eth0".into(), up: true }]);
    assert_eq!(wifi, [Adapter { name: "wlan0".into(), up: false }]);
    assert_eq!(read_adapters(&sys.join("nothing")), (vec![], vec![]));
}

#[test]
fn bluetooth_and_the_radio_switches_are_read_from_sysfs() {
    let sys = tmp("bt");
    put(&sys, "class/bluetooth/hci0/address", "x");
    put(&sys, "class/bluetooth/hci0:12/address", "x");
    put(&sys, "class/rfkill/rfkill0/type", "bluetooth\n");
    put(&sys, "class/rfkill/rfkill0/soft", "1\n");
    put(&sys, "class/rfkill/rfkill0/hard", "0\n");
    put(&sys, "class/rfkill/rfkill1/type", "wlan\n");
    put(&sys, "class/rfkill/rfkill1/soft", "0\n");
    put(&sys, "class/rfkill/rfkill1/hard", "0\n");
    assert_eq!(read_radios(&sys), (1, true, false), "one adapter (the connection entry is not one), Bluetooth off, Wi-Fi on");
    assert_eq!(read_radios(&sys.join("none")), (0, false, false));
}

// ------------------------------------------------------------------------------------------------ the report

#[test]
fn the_report_holds_verdicts_and_advice_and_never_sound_or_pictures() {
    let mut rs: Vec<StepResult> = StepId::ALL.iter().map(|i| StepResult::pending(*i)).collect();
    rs[0] = display(Said::Yes);
    rs[4] = speaker(&inv(), &Ok(()), Said::No);
    rs[5] = microphone(&inv(), &MicHeard::Samples(vec![1; 100]), "USB");
    let lines = report_lines(&rs);
    assert_eq!(lines[0], "1 passed, 2 failed, 0 skipped, 9 not done yet");
    assert!(lines.iter().any(|l| l == "Screen: Pass - You could see the test picture, all four colour bars and the frame."));
    assert!(lines.iter().any(|l| l.starts_with("Speaker: Fail")) && lines.iter().any(|l| l.starts_with("  advice: Check that the volume")));
    assert!(lines.iter().any(|l| l == "Camera: Not done yet"));
    assert!(lines.iter().filter(|l| !l.starts_with("  ")).count() >= 13, "summary plus one line per step");
}

#[test]
fn the_last_report_is_kept_for_share_diagnostics() {
    set_last_report(vec!["a".into(), "b".into()]);
    assert_eq!(last_report(), ["a", "b"]);
    set_last_report(vec![]);
    assert!(last_report().is_empty());
}

#[test]
fn the_real_readers_run_on_this_computer_without_panicking() {
    let i = Inventory::read(vec![]);
    let _ = (i.keyboards, i.route, i.power.laptop);
    let _ = Arc::new(i);
}
