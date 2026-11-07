// SPDX-License-Identifier: MIT OR Apache-2.0
//! Raw UEFI definitions written from the UEFI specification (2.x): `#[repr(C)]` structs, function pointer types, GUIDs and status
//! codes for the subset a boot loader needs. Types only: no entry point, no wrappers, no allocation, no dependencies.
//!
//! Every function pointer uses the `efiapi` calling convention (Microsoft x64 on `x86_64-unknown-uefi`, the C ABI elsewhere) and is
//! `unsafe` to call. Slots this crate does not use are `*const c_void` so offsets stay exact. Struct sizes and field offsets are
//! checked in tests against the x86_64 layout of the specification.
#![no_std]

pub mod base;
pub mod proto;
pub mod tables;

pub use base::*;
pub use proto::*;
pub use tables::*;

/// Check field offsets of a `#[repr(C)]` type and its size: `check_layout!(Type, size, { field: offset, ... })`.
/// Used by the tests; exported so users can assert on their own extensions.
#[macro_export]
macro_rules! check_layout {
    ($t:ty, $size:expr, { $($f:ident : $off:expr),* $(,)? }) => {{
        assert_eq!(core::mem::size_of::<$t>(), $size, "size of {}", stringify!($t));
        $( assert_eq!(core::mem::offset_of!($t, $f), $off, "offset of {}::{}", stringify!($t), stringify!($f)); )*
    }};
}
