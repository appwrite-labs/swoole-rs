// SPDX-License-Identifier: Apache-2.0
//! C's implicit conversions between number types, named at each use.
//!
//! Upstream converts between `int`, `long`, `size_t`, `uint32_t` and `double` without
//! a cast. Rust's `as` does what C does: an integer keeps the target's low bits, a
//! float truncates toward zero (and saturates). The pedantic cast lints flag every
//! `as` that could truncate or flip a sign; `x.c_cast::<u32>()` marks a conversion
//! that is C's on purpose, so a bare `as` still stands out.
#![expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_possible_wrap,
    clippy::cast_precision_loss,
    clippy::cast_lossless,
    reason = "the one place C's conversions are spelled `as`"
)]

/// `T::cast_from(s)` is `s as T`.
pub trait CastFrom<S> {
    /// `s as Self`.
    fn cast_from(s: S) -> Self;
}

/// `x.c_cast::<T>()` converts as C would: `x as T`.
pub trait CCast: Sized {
    /// `self as T`.
    #[inline(always)]
    fn c_cast<T: CastFrom<Self>>(self) -> T {
        T::cast_from(self)
    }
}

macro_rules! c_cast {
    ($s:ty => $($t:ty),*) => {
        impl CCast for $s {}
        $(
            impl CastFrom<$s> for $t {
                #[inline(always)]
                fn cast_from(s: $s) -> Self {
                    s as Self
                }
            }
        )*
    };
}
c_cast!(i8 => i16, i32, i64, i128, isize, u8, u16, u32, u64, u128, usize, f32, f64);
c_cast!(i16 => i8, i32, i64, i128, isize, u8, u16, u32, u64, u128, usize, f32, f64);
c_cast!(i32 => i8, i16, i64, i128, isize, u8, u16, u32, u64, u128, usize, f32, f64);
c_cast!(i64 => i8, i16, i32, i128, isize, u8, u16, u32, u64, u128, usize, f32, f64);
c_cast!(i128 => i8, i16, i32, i64, isize, u8, u16, u32, u64, u128, usize, f32, f64);
c_cast!(isize => i8, i16, i32, i64, i128, u8, u16, u32, u64, u128, usize, f32, f64);
c_cast!(u8 => i8, i16, i32, i64, i128, isize, u16, u32, u64, u128, usize, f32, f64);
c_cast!(u16 => i8, i16, i32, i64, i128, isize, u8, u32, u64, u128, usize, f32, f64);
c_cast!(u32 => i8, i16, i32, i64, i128, isize, u8, u16, u64, u128, usize, f32, f64);
c_cast!(u64 => i8, i16, i32, i64, i128, isize, u8, u16, u32, u128, usize, f32, f64);
c_cast!(u128 => i8, i16, i32, i64, i128, isize, u8, u16, u32, u64, usize, f32, f64);
c_cast!(usize => i8, i16, i32, i64, i128, isize, u8, u16, u32, u64, u128, f32, f64);
c_cast!(f32 => i8, i16, i32, i64, i128, isize, u8, u16, u32, u64, u128, usize, f64);
c_cast!(f64 => i8, i16, i32, i64, i128, isize, u8, u16, u32, u64, u128, usize, f32);

#[cfg(test)]
mod tests {
    use super::CCast;

    #[test]
    fn converts_as_c_does() {
        assert_eq!(70_000_i64.c_cast::<i32>(), 70_000, "fits");
        assert_eq!(0x1_0000_0005_u64.c_cast::<u32>(), 5, "keeps the low bits");
        assert_eq!((-1_i32).c_cast::<u32>(), u32::MAX, "reinterprets the sign");
        assert_eq!(2.9_f64.c_cast::<i64>(), 2, "truncates toward zero");
        assert_eq!(
            3_u64.c_cast::<f64>().to_bits(),
            3.0_f64.to_bits(),
            "widens to double"
        );
    }
}
