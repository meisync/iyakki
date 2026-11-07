// SPDX-License-Identifier: MIT OR Apache-2.0
//! Drawing on the firmware's graphics output: an off-screen buffer, filled rectangles, text in the built-in font, one blit.

use crate::font;
use crate::sys::locate;
use alloc::vec::Vec;
use uefi_sys::*;

/// Packed colour.
#[derive(Clone, Copy)]
pub struct Rgb(pub u8, pub u8, pub u8);

/// The screen and its off-screen copy.
pub struct Screen {
    gop: *mut GraphicsOutput,
    /// Width in pixels.
    pub w: u32,
    /// Height in pixels.
    pub h: u32,
    buf: Vec<BltPixel>,
}

impl Screen {
    /// The graphics output in its current mode (no mode change: the screen keeps the mode the firmware chose).
    pub fn open() -> Option<Screen> {
        let gop: *mut GraphicsOutput = locate(&GRAPHICS_OUTPUT_PROTOCOL_GUID)?;
        let (w, h) = unsafe {
            let m = (*gop).mode;
            if m.is_null() || (*m).info.is_null() {
                return None;
            }
            ((*(*m).info).horizontal_resolution, (*(*m).info).vertical_resolution)
        };
        if w < 320 || h < 200 || w > 8192 || h > 8192 {
            return None;
        }
        let px = BltPixel { blue: 0, green: 0, red: 0, reserved: 0 };
        Some(Screen { gop, w, h, buf: alloc::vec![px; (w * h) as usize] })
    }

    /// Fill a rectangle (clipped).
    pub fn rect(&mut self, x: u32, y: u32, w: u32, h: u32, c: Rgb) {
        let px = BltPixel { blue: c.2, green: c.1, red: c.0, reserved: 0 };
        for yy in y..(y + h).min(self.h) {
            let row = (yy * self.w) as usize;
            for xx in x..(x + w).min(self.w) {
                self.buf[row + xx as usize] = px;
            }
        }
    }

    /// Copy a decoded picture to (x, y) (clipped).
    pub fn image(&mut self, x: u32, y: u32, img: &boot_core::splash::Image) {
        for iy in 0..img.h {
            let yy = y + iy;
            if yy >= self.h {
                break;
            }
            for ix in 0..img.w {
                let xx = x + ix;
                if xx >= self.w {
                    break;
                }
                let p = img.px[(iy * img.w + ix) as usize];
                self.buf[(yy * self.w + xx) as usize] = BltPixel { blue: p as u8, green: (p >> 8) as u8, red: (p >> 16) as u8, reserved: 0 };
            }
        }
    }

    /// Draw text; each font pixel becomes `scale` x `scale` pixels. Returns the width drawn.
    pub fn text(&mut self, x: u32, y: u32, s: &str, scale: u32, c: Rgb) -> u32 {
        let mut cx = x;
        for ch in s.chars() {
            let g = font::glyph(ch);
            for (ry, bits) in g.iter().enumerate() {
                for rx in 0..8u32 {
                    if bits >> rx & 1 == 1 {
                        self.rect(cx + rx * scale, y + ry as u32 * scale, scale, scale, c);
                    }
                }
            }
            cx += 8 * scale;
        }
        cx - x
    }

    /// Copy the off-screen buffer to the screen.
    pub fn present(&mut self) {
        unsafe {
            ((*self.gop).blt)(self.gop, self.buf.as_mut_ptr(), BltOperation::BufferToVideo, 0, 0, 0, 0, self.w as usize, self.h as usize, 0);
        }
    }
}
