//! Panic-free `clamp`.
//!
//! `f32::clamp` and `Ord::clamp` panic when `min > max` (and the float one when a bound is NaN). Layout code
//! computes its bounds from window sizes, and a collapsed area or a rounding difference of 0.0001 is enough to
//! invert them, which killed the AI IDE on a real device ("min > max, or either was NaN. min = 1908.4784,
//! max = 1908.4783"). `clamp_safe` never panics: the lower bound wins when they are inverted, and a NaN value
//! or NaN bound gives the lower bound (or the value when the bounds are fine and it is not NaN).
//!
//! Use it for every clamp whose bounds are not both literal constants; `tests/safeclamp_scan.rs` enforces that.

pub trait SafeClamp: Sized {
    /// `self.max(lo).min(hi.max(lo))`: identical to `clamp` when `lo <= hi`, never panics otherwise.
    fn clamp_safe(self, lo: Self, hi: Self) -> Self;
}

macro_rules! float_impl {
    ($($t:ty),*) => {$(
        impl SafeClamp for $t {
            fn clamp_safe(self, lo: Self, hi: Self) -> Self {
                // `f32::max` ignores a NaN argument, so a NaN bound degrades to the other bound
                let lo = if lo.is_nan() { if hi.is_nan() { 0.0 } else { hi } } else { lo };
                let hi = if hi.is_nan() { lo } else { hi };
                let v = if self.is_nan() { lo } else { self };
                v.max(lo).min(hi.max(lo))
            }
        }
    )*};
}
macro_rules! int_impl {
    ($($t:ty),*) => {$(
        impl SafeClamp for $t {
            fn clamp_safe(self, lo: Self, hi: Self) -> Self { self.max(lo).min(hi.max(lo)) }
        }
    )*};
}
float_impl!(f32, f64);
int_impl!(i8, i16, i32, i64, isize, u8, u16, u32, u64, usize);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_clamp_when_the_bounds_are_in_order() {
        for v in [-5.0f32, 0.0, 0.5, 1.0, 9.0] {
            assert_eq!(v.clamp_safe(0.0, 1.0), v.clamp(0.0, 1.0));
        }
        for v in [-5i32, 0, 3, 10, 99] {
            assert_eq!(v.clamp_safe(0, 10), v.clamp(0, 10));
        }
        assert_eq!(7usize.clamp_safe(2, 5), 5);
        assert_eq!(1u32.clamp_safe(2, 5), 2);
    }

    #[test]
    fn inverted_bounds_do_not_panic_and_the_lower_bound_wins() {
        // the exact numbers from the real-device panic
        assert_eq!(2000.0f32.clamp_safe(1908.4784, 1908.4783), 1908.4784);
        assert_eq!(0.0f32.clamp_safe(1908.4784, 1908.4783), 1908.4784);
        assert_eq!(5i32.clamp_safe(10, 3), 10);
        assert_eq!(0usize.clamp_safe(4, 1), 4);
        assert_eq!(50u32.clamp_safe(4, 1), 4);
    }

    #[test]
    fn nan_never_panics_and_never_escapes() {
        assert_eq!(f32::NAN.clamp_safe(1.0, 2.0), 1.0);
        assert_eq!(3.0f32.clamp_safe(f32::NAN, 2.0), 2.0);
        assert_eq!(0.5f32.clamp_safe(1.0, f32::NAN), 1.0);
        assert!(!f32::NAN.clamp_safe(f32::NAN, f32::NAN).is_nan());
        assert!(!f64::NAN.clamp_safe(f64::NAN, 1.0).is_nan());
        assert_eq!(f32::INFINITY.clamp_safe(0.0, 1.0), 1.0);
        assert_eq!(f32::NEG_INFINITY.clamp_safe(0.0, 1.0), 0.0);
    }
}
