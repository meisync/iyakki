//! The engine of the DEV TOOLS tile: thirteen small tools as plain functions from text to text. The tile (`iyakki-apps`, `apps/devtools`)
//! draws them and owns everything that touches the machine (files, the clipboard, the network tools, saving a picture). Nothing here
//! reads a device or the clock on its own except through [`uuid_time::now_ms`] (the system time) and the random source of
//! [`uuid_time::set_random`].

pub mod b64;
pub mod convert;
pub mod diff;
pub mod hashes;
pub mod json;
pub mod jwt;
pub mod qrcode;
pub mod query;
pub mod regex;
pub mod tools;
pub mod url;
pub mod uuid_time;

#[cfg(test)]
mod tests_qr;
