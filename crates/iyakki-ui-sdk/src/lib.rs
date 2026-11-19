//! The IYAKKI UI SDK (docs/development/UI-SDK.md, design in UI-SDK-DESIGN.md).
//!
//! An app implements [`app::UiApp`]: `event` and `view`. `view` draws through [`canvas::Canvas`], which is either pixels
//! ([`raster::RasterCanvas`]) or a recorded [`list::DisplayList`] that a browser or client draws. Layout uses size classes
//! ([`size`]); colours come from the Look through [`cx::UiCx`]; the dock, wallpaper, screens and views are in [`desktop`]. The
//! [`host::Host`] drives an app the same way on every target.

#[cfg(feature = "uikit")]
pub mod app;
#[cfg(feature = "uikit")]
pub mod browser;
pub mod assistant;
pub mod cameras;
#[cfg(feature = "uikit")]
pub mod cameraview;
pub mod canvas;
pub mod files;
#[cfg(feature = "uikit")]
pub mod editorview;
pub mod terminal;
#[cfg(feature = "uikit")]
pub mod terminalview;
#[cfg(feature = "uikit")]
pub mod filesview;
#[cfg(feature = "uikit")]
pub mod audioview;
pub mod barcode;
#[cfg(feature = "uikit")]
pub mod barcodeview;
#[cfg(feature = "uikit")]
pub mod mediaview;
pub mod media;
#[cfg(feature = "uikit")]
pub mod chatview;
#[cfg(feature = "uikit")]
pub mod cx;
#[cfg(feature = "uikit")]
pub mod desktop;
pub mod event;
pub mod events;
pub mod geom;
pub mod gesture;
#[cfg(feature = "uikit")]
pub mod host;
pub mod lint;
pub mod path;
pub mod list;
pub mod ports;
#[cfg(feature = "uikit")]
pub mod raster;
pub mod size;
pub mod tokens;
pub mod typeface;
pub mod toolargs;
#[cfg(feature = "uikit")]
pub mod widgets;
#[cfg(feature = "uikit")]
pub mod wire;

/// The SDK's own version: the traits and types above. Additive changes only; a breaking change is a new number and a fixture folder.
pub const UI_SDK_VERSION: u32 = 1;
pub use list::DISPLAY_LIST_VERSION;
/// Re-exported so an SDK app depends on this crate alone.
#[cfg(feature = "uikit")]
pub use iyakki_uikit::{look, skins, theme, wallpaper};

#[cfg(test)]
mod tests;
#[cfg(all(test, feature = "uikit"))]
mod filesview_tests;
#[cfg(all(test, feature = "uikit"))]
mod audioview_tests;
#[cfg(all(test, feature = "uikit"))]
mod mediaview_tests;
#[cfg(all(test, feature = "uikit"))]
mod editorview_tests;
#[cfg(all(test, feature = "uikit"))]
mod terminalview_tests;
#[cfg(all(test, feature = "uikit"))]
mod chatview_tests;
#[cfg(all(test, feature = "uikit"))]
mod image_tests;
#[cfg(all(test, feature = "uikit"))]
mod cameraview_tests;
#[cfg(all(test, feature = "uikit"))]
mod token_tests;
#[cfg(all(test, feature = "uikit"))]
mod type_tests;
#[cfg(all(test, feature = "uikit"))]
mod v3_tests;
