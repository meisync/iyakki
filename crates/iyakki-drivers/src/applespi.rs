//! The Apple SPI keyboard and trackpad (MacBook8,1 to MacBookPro14,3 without the T2 chip).
//!
//! These laptops have no USB or I2C keyboard: it sits on an SPI bus behind the Intel LPSS (low power subsystem) controller of the
//! PCH. Three drivers have to be loaded in a chain: `intel_lpss_pci` (the PCI function, which creates the SPI controller as a child
//! platform device `pxa2xx-spi.N`), `spi_pxa2xx_platform` (binds that child, alias `platform:pxa2xx-spi`; it pulls in
//! `spi_pxa2xx_core`) and `applespi` (the keyboard, alias `acpi*:APP000D:*`, which registers the keyboard, its backlight
//! `spi::kbd_backlight` and the trackpad).
//!
//! The coldplug finds `intel_lpss_pci` and `applespi` by their aliases, but the child device does not exist during the first scan; the
//! repeated scan (`iyakki-core` coldplug module) finds it. This list is the belt and braces: on these models the three modules are
//! also loaded by name, in this order, so a kernel or ACPI quirk that hides the child from the scan cannot leave the keyboard dead.
//! Measured on a MacBookPro14,1: without it the controller `pxa2xx-spi.3` (under PCI 0000:00:1e.3) was never bound.

/// DMI product names with the Apple SPI keyboard.
pub const MODELS: &[&str] = &[
    "MacBook8,1",
    "MacBook9,1",
    "MacBook10,1",
    "MacBookPro12,1",
    "MacBookPro13,1",
    "MacBookPro13,2",
    "MacBookPro13,3",
    "MacBookPro14,1",
    "MacBookPro14,2",
    "MacBookPro14,3",
];

/// Modules to load by name, in order.
pub const MODULES: &[&str] = &["intel_lpss_pci", "spi_pxa2xx_platform", "applespi"];

/// The model (DMI product name, whitespace around it ignored) has the Apple SPI keyboard.
pub fn model_applies(product: &str) -> bool {
    MODELS.contains(&product.trim())
}

/// The modules this machine needs by name: all of [`MODULES`] on a listed model, none elsewhere.
pub fn modules_for(product: &str) -> &'static [&'static str] {
    if model_applies(product) {
        MODULES
    } else {
        &[]
    }
}

/// Load them. `modprobe` is the loader (`-b` honours blacklists; a missing module is not an error here: the image may not carry it).
/// Returns how many calls were made.
pub fn load(product: &str, mut modprobe: impl FnMut(&str)) -> usize {
    let m = modules_for(product);
    m.iter().for_each(|x| modprobe(x));
    m.len()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_spi_keyboard_models_are_listed_and_others_are_not() {
        for m in ["MacBook8,1", "MacBook9,1", "MacBook10,1", "MacBookPro12,1", "MacBookPro13,1", "MacBookPro13,2", "MacBookPro13,3", "MacBookPro14,1", "MacBookPro14,2", "MacBookPro14,3"] {
            assert!(model_applies(m), "{m}");
        }
        assert!(model_applies("MacBookPro14,1\n"));
        // T2 Macs use apple-bce, older MacBooks have USB keyboards, other makers are not Macs
        for m in ["MacBookPro15,1", "MacBookPro11,1", "MacBookAir8,1", "MacBook7,1", "iMac18,3", "", "ThinkPad X1"] {
            assert!(!model_applies(m), "{m}");
        }
    }

    #[test]
    fn the_modules_load_in_dependency_order_and_only_on_a_listed_model() {
        let mut calls = Vec::new();
        assert_eq!(load("MacBookPro14,1", |m| calls.push(m.to_string())), 3);
        assert_eq!(calls, ["intel_lpss_pci", "spi_pxa2xx_platform", "applespi"]);
        let mut none = Vec::new();
        assert_eq!(load("MacBookPro15,2", |m| none.push(m.to_string())), 0);
        assert!(none.is_empty());
    }
}
