//! The pure parts of the camera features, with no camera hub, vault or inbox in them (the clock is the small `iyakki-clock` crate).
//!
//! * [`motion`]: the in-house detector on decoded pictures. [`geom`]: zones, privacy masks, polygon checks.
//! * [`alerts`]: rate limits, grouping and quiet hours for notices. [`person`]: the person-detector hook (the default says "not available").
//! * [`settings`]: per-camera settings on disk.
//! * [`camrec_format`]: the encrypted recording and event file format (AES-256-XTS and HMAC from `iyakki-volcrypt`).
//!
//! `iyakki-agent` re-exports each module under its old path (`crate::camdetect::motion`, `crate::camrec_format`) and keeps the watching service (`watch`: hub, inbox, vault).

pub mod alerts;
pub mod camrec_format;
pub mod events;
pub mod geom;
pub mod motion;
pub mod person;
pub mod settings;
