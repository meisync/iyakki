//! The screens of the machine over a fake bus (F-KBD-08): reading first, writing only to the chosen screen, and the plain reason when there
//! is no bus. The real device file is not opened here (it needs `i2c-dev` and a monitor).

use super::*;
use crate::ddcci::tests::{monitor, Monitor};

fn buses() -> Vec<DdcBus> {
    vec![DdcBus { connector: "card0-DP-1".into(), bus: 3 }, DdcBus { connector: "card0-HDMI-A-1".into(), bus: 7 }]
}

use std::cell::RefCell;
use std::rc::Rc;

struct Two(Vec<Rc<RefCell<Monitor>>>);

impl Two {
    fn new(v: Vec<Monitor>) -> Two {
        Two(v.into_iter().map(|m| Rc::new(RefCell::new(m))).collect())
    }
    fn opener(&self) -> impl FnMut(u32) -> Result<Box<dyn I2cBus>, String> + '_ {
        move |n| match n {
            3 => Ok(Box::new(Shared(self.0[0].clone()))),
            7 => Ok(Box::new(Shared(self.0[1].clone()))),
            _ => Err("no such bus".into()),
        }
    }
}

/// A bus onto one of the fake monitors the test also holds.
struct Shared(Rc<RefCell<Monitor>>);

impl I2cBus for Shared {
    fn write(&mut self, addr: u16, data: &[u8]) -> Result<(), String> {
        self.0.borrow_mut().write(addr, data)
    }
    fn read(&mut self, addr: u16, len: usize) -> Result<Vec<u8>, String> {
        self.0.borrow_mut().read(addr, len)
    }
}

#[test]
fn every_screen_is_listed_with_its_name_and_brightness_and_a_silent_one_with_the_reason() {
    let two = Two::new(vec![monitor(1), monitor(2)]);
    two.0[1].borrow_mut().unsupported = true;
    let list_ = list(&buses(), &mut two.opener());
    assert_eq!(list_.len(), 2);
    assert_eq!(list_[0].connector, "card0-DP-1");
    assert!(list_[0].id.is_some() && list_[0].label.contains("DELL"), "{:?}", list_[0]);
    assert_eq!(list_[0].brightness, Ok(40));
    assert!(list_[1].brightness.is_err(), "a monitor without DDC/CI says so: {:?}", list_[1]);
    let bad = list(&[DdcBus { connector: "x".into(), bus: 9 }], &mut two.opener());
    assert_eq!(bad[0].id, None);
    assert!(bad[0].brightness.as_ref().unwrap_err().contains("no such bus"));
}

#[test]
fn listing_never_writes_a_setting_to_any_monitor() {
    let two = Two::new(vec![monitor(1), monitor(2)]);
    let _ = list(&buses(), &mut two.opener());
    for m in &two.0 {
        let m = m.borrow();
        assert!(m.log.iter().all(|(a, d)| *a == crate::ddcci::EDID_ADDR || d.get(2) != Some(&0x03)), "{:?}", m.log);
        assert_eq!(m.value, 40);
    }
}

#[test]
fn only_the_chosen_screen_is_written_and_the_other_is_untouched() {
    let two = Two::new(vec![monitor(1), monitor(2)]);
    let ids = list(&buses(), &mut two.opener());
    let second = ids[1].id.clone().unwrap();
    let chosen = Chosen(Some(second.clone()));
    let got = set(&buses(), &mut two.opener(), &chosen, &second, 70).unwrap();
    assert_eq!(got.percent(), 70);
    assert_eq!(two.0[1].borrow().value, 70);
    assert_eq!(two.0[0].borrow().value, 40, "the first screen was not written");
    let first = ids[0].id.clone().unwrap();
    assert_eq!(set(&buses(), &mut two.opener(), &chosen, &first, 10).unwrap_err(), DdcError::NotSelected);
    assert_eq!(two.0[0].borrow().value, 40);
    assert_eq!(set(&buses(), &mut two.opener(), &Chosen(None), &second, 10).unwrap_err(), DdcError::NotSelected);
}

#[test]
fn a_chosen_screen_that_is_gone_is_a_plain_error_and_no_write_happens() {
    let two = Two::new(vec![monitor(1), monitor(2)]);
    let chosen = Chosen(Some("XXX-0000-9".into()));
    let e = set(&buses(), &mut two.opener(), &chosen, "XXX-0000-9", 50).unwrap_err();
    assert!(matches!(e, DdcError::Bus(_)), "{e:?}");
    assert!(two.0.iter().all(|m| m.borrow().value == 40));
}

#[test]
fn a_system_without_i2c_dev_says_so_in_plain_words() {
    let d = std::env::temp_dir().join(format!("iyakki-ddcbus-{}", std::process::id()));
    std::fs::create_dir_all(&d).unwrap();
    let e = open_real(&d, 4).err().unwrap();
    assert!(e.contains("i2c-dev"), "{e}");
    let _ = std::fs::remove_dir_all(&d);
}
