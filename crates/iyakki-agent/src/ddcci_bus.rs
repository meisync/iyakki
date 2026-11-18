//! DDC/CI on the machine's own screens (F-KBD-08): the display connectors that have an I2C channel, each opened through the core's
//! one device module (`iyakki_core_local::i2c`), read first, and written to only for the monitor the person chose
//! ([`crate::ddcci::Chosen`]). The opening of a bus is an argument so the tests use a fake monitor; the real one is [`open_real`].

use std::path::Path;

use crate::ddcci::{probe, Brightness, Chosen, DdcError, I2cBus};
use iyakki_core_local::i2c::{bus_path, ddc_buses, DdcBus, I2cDev};

/// The real bus behind the DDC/CI messages.
pub struct DevBus(I2cDev);

impl I2cBus for DevBus {
    fn write(&mut self, addr: u16, data: &[u8]) -> Result<(), String> {
        self.0.write(addr, data).map_err(|e| e.to_string())
    }
    fn read(&mut self, addr: u16, len: usize) -> Result<Vec<u8>, String> {
        self.0.read(addr, len).map_err(|e| e.to_string())
    }
    fn pause(&mut self, ms: u64) {
        std::thread::sleep(std::time::Duration::from_millis(ms));
    }
}

/// Opens bus `n` under `dev_root` (`/dev` on the machine). The error says what is missing: without the `i2c-dev` driver there is no file.
pub fn open_real(dev_root: &Path, n: u32) -> Result<Box<dyn I2cBus>, String> {
    let p = bus_path(dev_root, n);
    match I2cDev::open(&p) {
        Ok(d) => Ok(Box::new(DevBus(d))),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Err("this system has no I2C device files (the kernel lacks the i2c-dev driver)".into()),
        Err(e) => Err(format!("the I2C bus cannot be opened: {e}")),
    }
}

/// One screen as the person sees it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Screen {
    pub connector: String,
    /// The id the person chooses by (maker, product, serial from the EDID), or `None` when the monitor did not answer.
    pub id: Option<String>,
    pub label: String,
    /// The brightness in percent, or why it cannot be read (many monitors do not answer DDC/CI).
    pub brightness: Result<u8, String>,
}

/// Looks at every screen with a DDC channel. Reads only.
pub fn list(buses: &[DdcBus], open: &mut dyn FnMut(u32) -> Result<Box<dyn I2cBus>, String>) -> Vec<Screen> {
    buses
        .iter()
        .map(|b| match open(b.bus).map_err(DdcError::Bus).and_then(|mut bus| probe(bus.as_mut())) {
            Ok((edid, br)) => Screen { connector: b.connector.clone(), id: Some(edid.id()), label: edid.label(), brightness: Ok(br.percent()) },
            Err(e) => Screen { connector: b.connector.clone(), id: None, label: b.connector.clone(), brightness: Err(e.to_string()) },
        })
        .collect()
}

/// The screens of this machine (`sys` is `/sys`, `dev_root` is `/dev`).
pub fn list_real(sys: &Path, dev_root: &Path) -> Vec<Screen> {
    list(&ddc_buses(sys), &mut |n| open_real(dev_root, n))
}

/// Sets the brightness of the screen whose id is `id`, which must be the one chosen. Every other screen is left alone, and the value is
/// read back.
pub fn set(buses: &[DdcBus], open: &mut dyn FnMut(u32) -> Result<Box<dyn I2cBus>, String>, chosen: &Chosen, id: &str, percent: u8) -> Result<Brightness, DdcError> {
    if chosen.0.as_deref() != Some(id) {
        return Err(DdcError::NotSelected);
    }
    for b in buses {
        let Ok(mut bus) = open(b.bus) else { continue };
        let Ok((edid, _)) = probe(bus.as_mut()) else { continue };
        if edid.id() == id {
            return chosen.set_brightness(bus.as_mut(), percent);
        }
    }
    Err(DdcError::Bus("no screen with that id answers on this machine".into()))
}

#[cfg(test)]
#[path = "ddcci_bus_tests.rs"]
mod tests;
