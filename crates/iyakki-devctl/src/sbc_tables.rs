//! The synthesis window of the 4 and 8 subband filter banks (D = +-K times the prototype lowpass filter, `10 M` taps).
//!
//! `D4_DESIGNED` and `D8_DESIGNED` are DESIGNED, not copied: they are the output of `sbc_design::designed_window` (a documented least-squares
//! design of a pseudo-QMF prototype, no number taken from any other codec), printed to 9 decimals by the ignored test
//! `print_designed_tables`. A test recomputes them and fails if they differ by more than 1e-8, so they cannot be changed by hand.
//! They are NOT the A2DP specification's tables; see docs/development/SBC-TABLES.md for provenance, measurements and status.
//!
//! Which window the codec uses: the designed one, unless the cargo feature `legacy-sbc-window` (on by default until the owner has heard the
//! designed one on a phone) selects the old measured one ([`crate::sbc_tables_legacy`]).
// SPDX-License-Identifier: Apache-2.0

pub static D4_DESIGNED: [f64; 40] = [
    0.000000000, -0.002843598, -0.005959987, -0.009861208, -0.013001302, -0.012681799, -0.005480747, 0.011577105, 
    -0.039184184, -0.073718384, -0.105844704, -0.120690668, -0.100239896, -0.027880303, 0.105874200, 0.298234580, 
    -0.531538001, -0.774657865, -0.988687840, -1.135684725, -1.188047875, -1.135684725, -0.988687840, -0.774657865, 
    0.531538001, 0.298234580, 0.105874200, -0.027880303, -0.100239896, -0.120690668, -0.105844704, -0.073718384, 
    0.039184184, 0.011577105, -0.005480747, -0.012681799, -0.013001302, -0.009861208, -0.005959987, -0.002843598, 
];

pub static D8_DESIGNED: [f64; 80] = [
    0.000000000, -0.001897993, -0.003120356, -0.004712183, -0.006632664, -0.008777649, -0.010967881, -0.012942408, 
    -0.014359732, -0.014809026, -0.013833220, -0.010964825, -0.005774121, 0.002072227, 0.012745661, 0.026188515, 
    -0.042051584, -0.059643878, -0.077902191, -0.095387533, -0.110314080, -0.120614015, -0.124038672, -0.118292923, 
    -0.101196140, -0.070859567, -0.025867141, 0.034555201, 0.110397287, 0.200764411, 0.303831517, 0.416860806, 
    -0.536286888, -0.657868084, -0.776896708, -0.888455672, -0.987704184, -1.070172007, -1.132040206, -1.170386707, 
    -1.183377288, -1.170386707, -1.132040206, -1.070172007, -0.987704184, -0.888455672, -0.776896708, -0.657868084, 
    0.536286888, 0.416860806, 0.303831517, 0.200764411, 0.110397287, 0.034555201, -0.025867141, -0.070859567, 
    -0.101196140, -0.118292923, -0.124038672, -0.120614015, -0.110314080, -0.095387533, -0.077902191, -0.059643878, 
    0.042051584, 0.026188515, 0.012745661, 0.002072227, -0.005774121, -0.010964825, -0.013833220, -0.014809026, 
    -0.014359732, -0.012942408, -0.010967881, -0.008777649, -0.006632664, -0.004712183, -0.003120356, -0.001897993, 
];

/// Where the window in use comes from (shown by tests and by the docs' A/B steps).
#[cfg(feature = "legacy-sbc-window")]
pub const WINDOW_SOURCE: &str = "legacy (measured from a third-party decoder, not for release)";
#[cfg(not(feature = "legacy-sbc-window"))]
pub const WINDOW_SOURCE: &str = "designed (sbc_design, first principles)";

#[cfg(feature = "legacy-sbc-window")]
pub use crate::sbc_tables_legacy::{D4_LEGACY as D4, D8_LEGACY as D8};
#[cfg(not(feature = "legacy-sbc-window"))]
pub use self::{D4_DESIGNED as D4, D8_DESIGNED as D8};
