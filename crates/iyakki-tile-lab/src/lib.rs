//! The LAB tile (alpha.9 P5): pictures, vision and video frames against the models the person has, as a crate that depends on the toolkit
//! (`iyakki-uikit`) and the pure model crate `iyakki-lab-model` only. Everything it needs from the rest of the system is [`port::LabPort`] and
//! the [`app::Rig`] it hands over (the picture and vision backends, the frame source, the camera, the screen and the picture decoders); the shell
//! adapter (`crates/iyakki-apps/src/apps/lab.rs`) and the real port (`lab_port.rs`) are the only code that names the services.

pub mod app;
pub mod pic;
pub mod port;
pub mod ui;

#[cfg(test)]
mod fake_tests;

pub use app::{Flow, LabApp, Rig, Tab, ACCENT};
pub use pic::{PictureCodec, Prepared};
pub use port::LabPort;
