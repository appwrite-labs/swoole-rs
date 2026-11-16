//! Shared operations for integer-backed flag types.

/// Implement the common operations of a flag newtype over its integer storage.
/// Unknown bits are retained when reading a native field.
#[macro_export]
macro_rules! impl_flags {
    ($name:ident, $bits:ty) => {
        impl $name {
            pub const NONE: Self = Self(0);

            #[must_use]
            pub const fn from_bits(bits: $bits) -> Self {
                Self(bits)
            }

            #[must_use]
            pub const fn bits(self) -> $bits {
                self.0
            }

            #[must_use]
            pub const fn contains(self, other: Self) -> bool {
                self.0 & other.0 == other.0
            }

            #[must_use]
            pub const fn union(self, other: Self) -> Self {
                Self(self.0 | other.0)
            }
        }

        impl core::ops::BitOr for $name {
            type Output = Self;
            fn bitor(self, other: Self) -> Self {
                self.union(other)
            }
        }
    };
}
