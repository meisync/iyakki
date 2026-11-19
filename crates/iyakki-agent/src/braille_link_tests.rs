use super::*;
use iyakki_core_api::BrailleLink;

fn desc4() -> Vec<u8> {
    vec![
        0x05, 0x41, 0x09, 0x01, 0xa1, 0x01, 0x09, 0x02, 0xa1, 0x02, 0x09, 0x03, 0x75, 0x08, 0x95, 0x04, 0x91, 0x02, 0xc0, 0x0a, 0x00, 0x01, 0x75, 0x01, 0x95, 0x04, 0x81, 0x02, 0x0a, 0x1a, 0x02, 0x0a,
        0x1b, 0x02, 0x75, 0x01, 0x95, 0x02, 0x81, 0x02, 0x75, 0x01, 0x95, 0x02, 0x81, 0x03, 0xc0,
    ]
}

struct Fake {
    shown: Vec<String>,
    queue: Vec<braillehid::Key>,
}

impl Display for Fake {
    fn cells(&self) -> usize {
        4
    }
    fn show(&mut self, c: &str) -> Result<(), String> {
        self.shown.push(c.to_string());
        Ok(())
    }
    fn keys(&mut self) -> Vec<braillehid::Key> {
        std::mem::take(&mut self.queue)
    }
}

#[test]
fn keys_of_the_display_become_the_cores_keys_and_keys_without_meaning_are_dropped() {
    let mut c = Cells { display: Fake { shown: vec![], queue: vec![braillehid::Key::Router(2), braillehid::Key::Button(0x21b), braillehid::Key::Button(0x211), braillehid::Key::Button(0x999)] } };
    assert_eq!(c.width(), 4);
    c.put("\u{2801}").unwrap();
    assert_eq!(c.display.shown, ["\u{2801}"]);
    assert_eq!(c.keys(), [BrailleKey::Route(2), BrailleKey::PanRight, BrailleKey::FocusPrev]);
    assert!(c.keys().is_empty());
}

#[test]
fn the_link_finds_a_display_on_a_made_up_machine_and_says_when_there_is_none() {
    let root = std::fs::canonicalize(std::env::temp_dir()).unwrap().join(format!("iyakki-brl-link-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let (sys, dev) = (root.join("sys"), root.join("dev"));
    std::fs::create_dir_all(&dev).unwrap();
    let link = HidLink { roots: Roots { sys: sys.clone(), proc_: root.join("proc") }, dev: dev.clone() };
    assert!(link.open().is_none());
    assert!(link.status().starts_with("No braille display"));
    std::fs::create_dir_all(sys.join("class/hidraw/hidraw1/device")).unwrap();
    std::fs::write(sys.join("class/hidraw/hidraw1/device/report_descriptor"), desc4()).unwrap();
    std::fs::write(dev.join("hidraw1"), b"").unwrap();
    assert!(link.status().contains("4 cells"));
    let mut cells = link.open().expect("the display opens");
    assert_eq!(cells.width(), 4);
    cells.put("\u{2801}\u{2803}").unwrap();
    assert_eq!(std::fs::read(dev.join("hidraw1")).unwrap(), [1, 3, 0, 0]);
    let _ = std::fs::remove_dir_all(&root);
}
