//! The QR code tool's text and pictures live in `iyakki-devtools-engine`; this file adds what needs the shell's picture writer and clock.

pub use iyakki_devtools_engine::qrcode::*;

/// The picture as a PNG about 600 pixels wide, a whole number of pixels to a module (96 dpi, the shell's PNG writer).
pub fn png(q: &iyakki_qr::qr::Qr) -> Result<Vec<u8>, String> {
    png_with(q, |w, h, data| crate::camera::pixels::png_with_dpi(w, h, data, 96.0))
}

/// `qr-20261105-143012.png`: the name of a saved picture, from the time.
pub fn file_name(ext: &str) -> String {
    file_name_for(&crate::camera::stamp(), ext)
}
