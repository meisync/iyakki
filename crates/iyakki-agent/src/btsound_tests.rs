//! The Sound page's Bluetooth row (F-NET-28) over fakes: the same tools the assistant uses, driven through the page's functions.

use super::*;
use crate::btaudiotools::BtAudioTools;
use iyakki_core_local::devctl::a2dp::{AudioControl, AudioView};
use iyakki_core_local::devctl::a2dp_send::{SendControl, SendView};
use iyakki_core_local::devctl::bthid::{DeviceView, Devices, LinkState};
use std::sync::Mutex;

struct Devs(Vec<DeviceView>);
impl Devices for Devs {
    fn list(&self) -> Vec<DeviceView> {
        self.0.clone()
    }
    fn connect(&self, _: &str) -> Result<(), String> {
        Ok(())
    }
    fn disconnect(&self, _: &str) -> Result<(), String> {
        Ok(())
    }
    fn forget(&self, _: &str) -> Result<(), String> {
        Ok(())
    }
}

struct NoAudio;
impl AudioControl for NoAudio {
    fn view(&self) -> AudioView {
        AudioView::default()
    }
    fn connect(&self, _: &str) -> Result<(), String> {
        Ok(())
    }
    fn disconnect(&self) -> Result<(), String> {
        Ok(())
    }
    fn pairing_open(&self, _: bool) -> Result<(), String> {
        Ok(())
    }
}

#[derive(Default)]
struct FakeSend(Mutex<Option<String>>);
impl SendControl for FakeSend {
    fn view(&self) -> SendView {
        match self.0.lock().unwrap().clone() {
            Some(d) => SendView { active: true, device: d, codec: "SBC".into(), packets: 1, delay_ms: None, why: String::new() },
            None => SendView::default(),
        }
    }
    fn start(&self, _a: &str, name: &str) -> Result<(), String> {
        *self.0.lock().unwrap() = Some(name.into());
        Ok(())
    }
    fn stop(&self) -> Result<(), String> {
        self.0.lock().unwrap().take().map(|_| ()).ok_or_else(|| "no sound is being sent to a device".to_string())
    }
    fn write(&self, _: &[i16], _: u32, _: usize) -> Result<(), String> {
        Ok(())
    }
}

fn dev(a: &str, n: &str, kind: &str) -> DeviceView {
    DeviceView { address: a.into(), addr_type: 0, name: n.into(), kind: kind.into(), state: LinkState::Disconnected }
}

fn rig(devs: Vec<DeviceView>, tag: &str) -> (BtAudioTools, std::path::PathBuf) {
    let d = std::env::temp_dir().join(format!("iyakki-btsound-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    let t = BtAudioTools::new(Arc::new(NoTools), Arc::new(Devs(devs)), Arc::new(NoAudio)).with_send(Arc::new(FakeSend::default())).with_route(Arc::new(crate::btroute::BtRoute::new())).with_memory(d.join("bluetooth-sink.json"));
    (t, d)
}

#[test]
fn the_page_lists_only_headphones_and_speakers_and_says_where_the_sound_goes() {
    let (t, d) = rig(vec![dev("AA:BB:CC:DD:EE:05", "Buds", "headphones"), dev("AA:BB:CC:DD:EE:01", "Pixel 8", "phone"), dev("AA:BB:CC:DD:EE:06", "Boom", "speaker")], "list");
    assert_eq!(t.sinks(), vec![("AA:BB:CC:DD:EE:05".to_string(), "Buds".to_string()), ("AA:BB:CC:DD:EE:06".to_string(), "Boom".to_string())]);
    assert!(!active(&t));
    assert!(t.sound_line().contains("not sending"), "{}", t.sound_line());
    // two are paired and none named: the words of the chat tool, not a guess
    assert!(start_with(&t, "").unwrap_err().contains("more than one"));
    // naming one starts it, the page now says so, and stop gives it back
    let said = start_with(&t, "AA:BB:CC:DD:EE:05").unwrap();
    assert!(said.contains("Buds"), "{said}");
    assert!(active(&t) && t.sound_line().contains("Buds"), "{}", t.sound_line());
    assert!(stop_with(&t).unwrap().contains("speakers play again"));
    assert!(!active(&t));
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn with_nothing_paired_the_start_says_so_and_a_stop_with_nothing_running_says_that() {
    let (t, d) = rig(vec![dev("AA:BB:CC:DD:EE:01", "Pixel 8", "phone")], "none");
    assert!(t.sinks().is_empty());
    assert!(start_with(&t, "").unwrap_err().contains("no headphones or speaker are paired"));
    assert!(stop_with(&t).unwrap_err().contains("no sound is being sent"));
    let _ = std::fs::remove_dir_all(&d);
}
