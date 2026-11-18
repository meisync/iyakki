//! `BarcodeView`: draws the modules of [`crate::barcode`] as bars with the number or text under them. A scanner needs dark bars on a light
//! ground whatever the skin, so the ink and the paper are the darker and the lighter of two colours of the Look (never a colour of its own),
//! with a quiet zone of ten modules each side. The view is a named image for the screen reader ("Barcode: <text>").

use crate::barcode;
use crate::canvas::{fit, Canvas, Hit, Role, TextStyle};
use crate::geom::{Color, Rect};
use crate::size::{Metrics, TypeRole};
use crate::theme::Theme;
use crate::tokens::Radius;
use crate::widgets;

/// Quiet zone, in modules, on each side.
const QUIET: usize = 10;

pub struct BarcodeView {
    base: u32,
    text: String,
    modules: Vec<bool>,
}

fn luma(c: Color) -> f32 {
    0.2126 * c[0] as f32 + 0.7152 * c[1] as f32 + 0.0722 * c[2] as f32
}

impl BarcodeView {
    /// A Code 128 barcode of `text`; `action` is the number of its (named) region.
    pub fn code128(action: u32, text: &str) -> Result<BarcodeView, String> {
        Ok(BarcodeView { base: action, text: text.to_string(), modules: barcode::encode_code128(text)? })
    }

    /// An EAN-13 barcode of 12 digits (check digit added) or 13 (verified).
    pub fn ean13(action: u32, digits: &str) -> Result<BarcodeView, String> {
        let m = barcode::encode_ean13(digits)?;
        let shown = if digits.trim().len() == 12 { format!("{}{}", digits.trim(), barcode::ean13_check_digit(digits.trim())?) } else { digits.trim().to_string() };
        Ok(BarcodeView { base: action, text: shown, modules: m })
    }

    /// A UPC-A barcode of 11 or 12 digits.
    pub fn upca(action: u32, digits: &str) -> Result<BarcodeView, String> {
        Ok(BarcodeView { base: action, text: digits.trim().to_string(), modules: barcode::encode_upca(digits)? })
    }

    pub fn modules(&self) -> &[bool] {
        &self.modules
    }

    pub fn text(&self) -> &str {
        &self.text
    }

    /// Draw in `area`; returns the rectangle used. The module width is the area's width over the modules and the quiet zones; bars are
    /// as tall as the area allows less the caption.
    pub fn draw(&self, c: &mut dyn Canvas, th: &Theme, m: &Metrics, area: Rect) -> Rect {
        let (ink, paper) = if luma(th.text) < luma(th.bg_top) { (th.text, th.bg_top) } else { (th.bg_top, th.text) };
        let size = m.size(TypeRole::Small);
        let pad = m.gutter();
        widgets::surface(c, th, m, area, paper, Radius::Md, false);
        let total = (self.modules.len() + 2 * QUIET) as f32;
        let mw = ((area.w - 2.0 * pad) / total).max(0.0);
        let x0 = area.x + pad + QUIET as f32 * mw;
        let bar_h = (area.h - 2.0 * pad - size - m.gutter()).max(0.0);
        let mut i = 0;
        while i < self.modules.len() {
            if self.modules[i] {
                let mut j = i;
                while j < self.modules.len() && self.modules[j] {
                    j += 1;
                }
                c.rect(Rect::new(x0 + i as f32 * mw, area.y + pad, (j - i) as f32 * mw, bar_h), ink);
                i = j;
            } else {
                i += 1;
            }
        }
        let caption = fit(c, size, &self.text, TextStyle::Mono, area.w - 2.0 * pad);
        let w = c.measure(size, &caption, TextStyle::Mono);
        c.text(area.x + ((area.w - w) / 2.0).max(0.0), area.y + pad + bar_h + m.gutter(), size, ink, &caption, TextStyle::Mono);
        c.hit(area, Hit { action: self.base, name: format!("Barcode: {}", self.text), role: Role::Image });
        area
    }
}
