// SPDX-License-Identifier: MIT OR Apache-2.0
//! Core logic of a small boot loader. `no_std` plus `alloc`; no I/O, no drawing, no firmware calls.
//! The UEFI and BIOS programs feed it bytes and key state and act on what it returns.
#![cfg_attr(not(test), no_std)]

extern crate alloc;

pub mod assess;
pub mod choose;
pub mod cmdline;
pub mod config;
pub mod discover;
pub mod item;
pub mod layout;
pub mod menu;
pub mod splash;
