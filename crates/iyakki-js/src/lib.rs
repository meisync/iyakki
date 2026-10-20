//! JavaScript for the reader browser.
//!
//! QuickJS-ng (MIT) is compiled to WebAssembly (`dist/js/build.sh`) and run inside wasmtime with a fuel budget, a memory
//! cap and a wall-clock deadline, so a hostile or broken script cannot take the browser down. The engine only knows the
//! JavaScript language. Everything else a page expects (`console`, timers, `localStorage`, `fetch`, `document`,
//! cookies) is a [`WebModule`]: a piece of JavaScript that defines the globals, plus host code that answers its calls.
//! A page is given exactly the modules its site is allowed to use.

pub mod engine;
pub mod modules;
pub mod page;
mod cache;
mod sha256;

pub use engine::{warm_up, JsEngine, JsError, Limits, WebModule};
pub use page::{LivePage, renderer, renderer_with, run_page, run_page_with, PageReport, PageResult, SiteData, Sites};
