//! The Raspberry Pi 5 with two HDMI sound cards (`vc4-hdmi-0`, `vc4-hdmi-1`), read from the real board on 2026-11-24 (KI-1038): the monitor
//! was on HDMI-A-2 (card 1). Each card has one PCM named `MAI PCM i2s-hifi-0`, a plain `HDMI Jack` control and an `ELD` control.
use super::*;

const CARDS: &str = " 0 [vc4hdmi0       ]: vc4-hdmi - vc4-hdmi-0\n                      vc4-hdmi-0\n 1 [vc4hdmi1       ]: vc4-hdmi - vc4-hdmi-1\n                      vc4-hdmi-1\n";

struct Tree(PathBuf);
impl Drop for Tree {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn tree() -> Tree {
    let d = std::env::temp_dir().join(format!("iyakki-pi5-sound-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    for card in 0..2 {
        let p = d.join(format!("proc/asound/card{card}/pcm0p"));
        std::fs::create_dir_all(&p).unwrap();
        std::fs::write(p.join("info"), format!("card: {card}\ndevice: 0\nsubdevice: 0\nstream: PLAYBACK\nid: MAI PCM i2s-hifi-0\nname: MAI PCM i2s-hifi-0\nsubname: subdevice #0\nclass: 0\n")).unwrap();
    }
    std::fs::write(d.join("proc/asound/cards"), CARDS).unwrap();
    Tree(d)
}

/// Card 0: nothing attached (jack off, ELD all zero). Card 1: a monitor (jack on, an ELD that names it).
fn mixer(card: u32) -> Option<String> {
    let (jack, eld) = if card == 0 {
        ("off", vec![0u8; 128])
    } else {
        // the first bytes of the real ELD of a Dell monitor, as `amixer` printed them on the Pi
        let mut b = vec![0u8; 128];
        let head = [0x10u8, 0x00, 0x08, 0x00, 0x6c, 0x10, 0x00, 0x01, 0, 0, 0, 0, 0, 0, 0, 0, 0x10, 0xac, 0x1d, 0xd1, 0x44, 0x45, 0x4c, 0x4c, 0x20, 0x53, 0x33, 0x32, 0x32, 0x32, 0x48, 0x4e, 0x09, 0x07, 0x07];
        b[..head.len()].copy_from_slice(&head);
        ("on", b)
    };
    let hex: Vec<String> = eld.iter().map(|x| format!("0x{x:02x}")).collect();
    Some(format!("numid=1,iface=CARD,name='HDMI Jack'\n  ; type=BOOLEAN,access=r-------,values=1\n  : values={jack}\nnumid=5,iface=PCM,name='ELD'\n  ; type=BYTES,access=r--v----,values=128\n  : values={}\n", hex.join(",")))
}

#[test]
fn the_pis_vc4_hdmi_cards_are_hdmi_outputs_and_the_plain_hdmi_jack_says_which_has_a_monitor() {
    let t = tree();
    let outs = scan(&t.0.join("proc"), &|c| mixer(c));
    assert_eq!(outs.len(), 2, "{outs:?}");
    assert!(outs.iter().all(|o| o.kind == Kind::Hdmi), "both cards are HDMI outputs: {outs:?}");
    let (a, b) = (&outs[0], &outs[1]);
    assert_eq!((a.card, a.connected), (0, Some(false)), "the unplugged port must read as not connected: {a:?}");
    assert_eq!((b.card, b.connected), (1, Some(true)), "the port with the monitor must read as connected: {b:?}");
}

#[test]
fn a_pi_card_without_a_jack_control_is_still_an_hdmi_output_of_unknown_state() {
    let t = tree();
    let outs = scan(&t.0.join("proc"), &|_| Some(String::new()));
    assert!(outs.iter().all(|o| o.kind == Kind::Hdmi && o.connected.is_none()), "{outs:?}");
}
