//! Local motion detection for network cameras (F-CAM-04), person-detection hook and alerts (F-CAM-05). Nothing leaves the computer.
//!
//! * [`motion`]: the in-house detector on decoded pictures (pure, deterministic).
//! * [`geom`]: zones, privacy masks, polygon checks (pure).
//! * [`events`]: the encrypted event store (same file format and vault key as recordings).
//! * [`alerts`]: rate limits, grouping and quiet hours for inbox notices (pure).
//! * [`person`]: the person-detector hook; the default says "not available".
//! * [`settings`]: per-camera settings on disk.
//! * [`watch`]: the service that ties them to the camera hub, the vault, the inbox and the visible indicator.
//!
//! Rules this module keeps (details in `docs/development/CAMERAS.md`):
//!
//! * Nothing analyses a camera without a visible indicator and without the owner having turned it on. Modes: off, while watched (only
//!   when the picture is on a screen or the camera is being recorded: both already show a sign), always (a red sign on every screen).
//! * Detection stops by itself when the owner's vault closes (sign-out): without the key nothing is analysed and nothing is stored.
//! * Privacy masks are black before the detector, the thumbnail and the person hook see a picture. With "also in recordings and
//!   snapshots" ticked they are black before the recorder or a snapshot stores a picture ([`mask_jpeg_for_storage`]).
//! * This computer's own USB camera is never watched in the background.

pub use iyakki_camdetect::{alerts, geom, motion, person, settings};
pub mod events;
pub mod watch;

use std::path::Path;

/// Black out the privacy masks of `camera` in a JPEG about to be stored (a recording, a snapshot), when the person asked for that. A
/// picture that cannot be decoded, masked or encoded is an error: it is never stored unmasked.
pub fn mask_jpeg_for_storage(data: &Path, camera: &str, jpeg: Vec<u8>, encode: &(dyn Fn(u32, u32, &[u8]) -> Vec<u8> + Send + Sync)) -> Result<Vec<u8>, String> {
    let Some(masks) = settings::storage_masks(data, camera) else { return Ok(jpeg) };
    let bmp = iyakki_web::jpeg::decode(&jpeg).map_err(|e| format!("a picture could not be read to apply the privacy masks, so it was not stored ({e})"))?;
    let (w, h, mut rgba) = (bmp.width, bmp.height, bmp.rgba);
    let refs: Vec<&[geom::Pt]> = masks.iter().map(|m| m.as_slice()).collect();
    if !geom::blacken(w, h, &mut rgba, &refs) {
        return Err("a picture was the wrong size to apply the privacy masks, so it was not stored".into());
    }
    let out = encode(w, h, &rgba);
    if out.len() < 4 {
        return Err("a picture could not be written with the privacy masks applied, so it was not stored".into());
    }
    Ok(out)
}

/// The same for a decoded picture (snapshots of cameras that do not send JPEG).
pub fn mask_rgba_for_storage(data: &Path, camera: &str, w: u32, h: u32, rgba: &mut [u8]) -> Result<(), String> {
    let Some(masks) = settings::storage_masks(data, camera) else { return Ok(()) };
    let refs: Vec<&[geom::Pt]> = masks.iter().map(|m| m.as_slice()).collect();
    if geom::blacken(w, h, rgba, &refs) { Ok(()) } else { Err("a picture was the wrong size to apply the privacy masks, so it was not stored".into()) }
}
