use super::*;

/// A descriptor of a display of four cells with routing keys and pan buttons, with no report ids.
fn desc4() -> Vec<u8> {
    vec![
        0x05, 0x41, // usage page braille
        0x09, 0x01, // usage braille display
        0xa1, 0x01, // collection application
        0x09, 0x02, // braille row
        0xa1, 0x02, //   collection logical
        0x09, 0x03, //     usage 8-dot cell
        0x75, 0x08, 0x95, 0x04, // size 8 count 4
        0x91, 0x02, //     output variable
        0xc0, //          end
        0x0a, 0x00, 0x01, // usage router key (0x100)
        0x75, 0x01, 0x95, 0x04, 0x81, 0x02, // 4 one-bit routers, input variable
        0x0a, 0x1a, 0x02, // usage pan left (0x21a)
        0x0a, 0x1b, 0x02, // usage pan right (0x21b)
        0x75, 0x01, 0x95, 0x02, 0x81, 0x02, // 2 one-bit buttons
        0x75, 0x01, 0x95, 0x02, 0x81, 0x03, // 2 padding bits
        0xc0,
    ]
}

#[test]
fn the_descriptor_gives_the_cells_the_routing_keys_and_the_pan_buttons() {
    let l = Layout::parse(&desc4()).unwrap();
    assert_eq!(l.cells(), 4);
    // routers 0..3 in bits 0..3 of the first byte, pan left and right in bits 4 and 5
    assert_eq!(l.keys(&[0b0000_0100]), [Key::Router(2)]);
    assert_eq!(l.keys(&[0b0001_0001]), [Key::Router(0), Key::Button(0x21a)]);
    assert_eq!(l.keys(&[0b0010_0000]), [Key::Button(0x21b)]);
    assert!(l.keys(&[0]).is_empty());
}

#[test]
fn cells_become_the_dots_of_the_output_report_and_anything_else_is_blank() {
    let l = Layout::parse(&desc4()).unwrap();
    assert_eq!(l.show("\u{2801}\u{283f}a"), [0x01, 0x3f, 0, 0]);
    assert_eq!(l.show(""), [0, 0, 0, 0]);
    assert_eq!(l.show("\u{28ff}\u{28ff}\u{28ff}\u{28ff}\u{28ff}"), [0xff; 4], "cells past the display are dropped");
}

#[test]
fn a_device_with_report_ids_puts_the_id_first_and_checks_it_on_input() {
    // like desc4 but the cells are in output report 2 and the keys in input report 3
    let d = vec![
        0x05, 0x41, 0x09, 0x01, 0xa1, 0x01, 0x85, 0x02, 0x09, 0x03, 0x75, 0x08, 0x95, 0x02, 0x91, 0x02, 0x85, 0x03, 0x0a, 0x00, 0x01, 0x75, 0x01, 0x95, 0x02, 0x81, 0x02, 0xc0,
    ];
    let l = Layout::parse(&d).unwrap();
    assert_eq!(l.show("\u{2803}\u{2804}"), [2, 3, 4]);
    assert_eq!(l.keys(&[3, 0b10]), [Key::Router(1)]);
    assert!(l.keys(&[4, 0b10]).is_empty(), "another report is not ours");
}

#[test]
fn a_keyboard_or_a_mouse_is_not_a_braille_display_and_damage_is_not_a_panic() {
    assert!(Layout::parse(&[0x05, 0x01, 0x09, 0x06, 0xa1, 0x01, 0x05, 0x07, 0x19, 0xe0, 0x29, 0xe7, 0x75, 0x01, 0x95, 0x08, 0x81, 0x02, 0xc0]).is_none());
    assert!(Layout::parse(&[]).is_none());
    assert!(Layout::parse(&[0x05]).is_none());
    assert!(Layout::parse(&[0xfe, 0xff]).is_none());
}

#[test]
fn keys_have_meanings_for_the_reader() {
    assert_eq!(action(Key::Router(5)), Some(Action::Route(5)));
    assert_eq!(action(Key::Button(0x21a)), Some(Action::PanLeft));
    assert_eq!(action(Key::Button(0x21b)), Some(Action::PanRight));
    assert_eq!(action(Key::Button(0x211)), Some(Action::FocusPrev));
    assert_eq!(action(Key::Button(0x212)), Some(Action::FocusNext));
    assert_eq!(action(Key::Button(0x201)), None);
}

#[test]
fn displays_are_found_by_their_descriptor_and_the_node_takes_the_cells() {
    let root = std::fs::canonicalize(std::env::temp_dir()).unwrap().join(format!("iyakki-brl-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let (sys, dev) = (root.join("sys"), root.join("dev"));
    for (n, d) in [("hidraw0", vec![0x05u8, 0x01, 0x09, 0x02, 0xa1, 0x01, 0xc0]), ("hidraw1", desc4())] {
        std::fs::create_dir_all(sys.join("class/hidraw").join(n).join("device")).unwrap();
        std::fs::write(sys.join("class/hidraw").join(n).join("device/report_descriptor"), d).unwrap();
    }
    std::fs::create_dir_all(&dev).unwrap();
    std::fs::write(dev.join("hidraw1"), b"").unwrap();
    let roots = Roots { sys, proc_: root.join("proc") };
    let found = find(&roots, &dev);
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].node, dev.join("hidraw1"));
    let mut hid = Hid::open(&found[0]).unwrap();
    assert_eq!(hid.cells(), 4);
    hid.show("\u{2801}\u{2803}").unwrap();
    assert_eq!(std::fs::read(dev.join("hidraw1")).unwrap(), [1, 3, 0, 0]);
    assert!(hid.keys().is_empty());
    let _ = std::fs::remove_dir_all(&root);
}
