// SPDX-License-Identifier: MIT OR Apache-2.0
//! The splash picture the distribution supplies (`splash PATH` in the config). Drawn once, before the menu, on the graphics output
//! the firmware already set up (no mode change); the menu uses the same background and a normal start leaves the picture on screen.
//!
//! While it is shown, log lines go to the serial port only (`sys::set_quiet`), because the firmware text console would print over it.

use crate::fs::Volume;
use crate::gfx::{Rgb, Screen};
use crate::say;
use boot_core::config::Config;
use boot_core::splash::{self, Image};
use core::cell::UnsafeCell;

struct Shown {
    bg: Rgb,
    img: Image,
    /// The picture cut to what differs from the background (for the menu, which draws the logo near the top), and its scaled copy.
    logo: Image,
    scaled: Option<Image>,
    x: u32,
    y: u32,
    /// Milliseconds of the minimum show time that have not passed yet.
    min_left_ms: u32,
}

struct Slot(UnsafeCell<Option<Shown>>);
// SAFETY: the loader is single threaded (boot services, one processor, no interrupts that touch this).
unsafe impl Sync for Slot {}
static SHOWN: Slot = Slot(UnsafeCell::new(None));

fn shown() -> Option<&'static mut Shown> {
    // SAFETY: see `Slot`; callers use the reference briefly and never keep two at once.
    unsafe { (*SHOWN.0.get()).as_mut() }
}

/// Load, decode and draw the splash if the config names one and a graphics output exists. Any problem is logged and the loader goes
/// on exactly as if no splash had been configured.
pub fn show(cfg: &Config, vol: &Volume) {
    let Some(path) = cfg.splash.as_deref() else { return };
    if crate::force_text() {
        say!("splash: text console forced; skipped\n");
        return;
    }
    let Some(mut screen) = Screen::open() else {
        say!("splash: no graphics output; skipped\n");
        return;
    };
    let Some(bytes) = vol.read(path, splash::MAX_FILE + 1) else {
        say!("splash: {path} cannot be read; skipped\n");
        return;
    };
    let img = match splash::decode_bmp(&bytes) {
        Ok(i) => i,
        Err(e) => {
            say!("splash: {path}: {e}; skipped\n");
            return;
        }
    };
    drop(bytes);
    let (iw, ih) = (img.w, img.h);
    let [r, g, b] = cfg.splash_bg.unwrap_or([0, 0, 0]);
    let logo = img.crop_to_content([r, g, b], 12);
    let (mw, mh) = Image::margin_box(screen.w, screen.h);
    let img = img.fit(mw, mh);
    let (x, y) = img.centred(screen.w, screen.h);
    let bg = Rgb(r, g, b);
    // one off-screen picture, one copy to the screen: no flicker
    screen.rect(0, 0, screen.w, screen.h, bg);
    screen.image(x, y, &img);
    screen.present();
    crate::sys::set_quiet(true);
    say!("splash: drawn {path} {iw}x{ih} as {}x{} at {x},{y} on {}x{}, background #{r:02x}{g:02x}{b:02x}\n", img.w, img.h, screen.w, screen.h);
    // SAFETY: see `Slot`.
    unsafe { *SHOWN.0.get() = Some(Shown { bg, img, logo, scaled: None, x, y, min_left_ms: cfg.splash_ms }) };
}

/// A splash is on the screen (and log lines are kept off it).
pub fn active() -> bool {
    shown().is_some()
}

/// Size of the logo as the menu draws it (the picture cut to its content), before scaling.
pub fn logo_size() -> Option<(u32, u32)> {
    shown().map(|s| (s.logo.w, s.logo.h))
}

/// Draw the logo scaled to `w` x `h` (what `boot_core::layout::under_logo` planned) with its top-left corner at (x, y).
pub fn draw_logo(screen: &mut Screen, x: u32, y: u32, w: u32, h: u32) {
    let Some(s) = shown() else { return };
    if s.scaled.as_ref().is_none_or(|i| (i.w, i.h) != (w, h)) {
        s.scaled = Some(s.logo.clone().fit(w, h));
    }
    if let Some(i) = &s.scaled {
        screen.image(x, y, i);
    }
}

/// Background colour for the menu while a splash is configured and shown.
pub fn background() -> Option<Rgb> {
    shown().map(|s| s.bg)
}

/// Count time already spent since the splash was drawn against its minimum show time.
pub fn spent(ms: u32) {
    if let Some(s) = shown() {
        s.min_left_ms = s.min_left_ms.saturating_sub(ms);
    }
}

/// Wait out the rest of the minimum show time (a normal start only; a menu the person looked at is not delayed).
pub fn wait_minimum() {
    if let Some(s) = shown() {
        let left = core::mem::take(&mut s.min_left_ms);
        if left > 0 {
            say!("splash: holding for {left} ms\n");
            crate::sys::stall_us(left as usize * 1000);
        }
    }
}

/// Draw the picture again (after a menu covered it).
pub fn repaint() {
    let Some(s) = shown() else { return };
    if let Some(mut screen) = Screen::open() {
        screen.rect(0, 0, screen.w, screen.h, s.bg);
        screen.image(s.x, s.y, &s.img);
        screen.present();
    }
}

/// Let the text console print again (before a message the person must read).
pub fn release() {
    crate::sys::set_quiet(false);
}

