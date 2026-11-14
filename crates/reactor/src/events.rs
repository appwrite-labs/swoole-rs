//! A reactor registration's events: upstream's `SW_EVENT_*` bits, typed.
use core::ffi::c_int;
use swoole_rs_corebase::constants::EventType;

/// The events a socket is registered for, or a handler's event: upstream's
/// `SW_EVENT_*` bits. The same layout as upstream's `int`, which native code reads.
#[repr(transparent)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Events(c_int);

impl Events {
    pub const NONE: Self = Self(EventType::SW_EVENT_NULL as c_int);
    pub const DEFAULT: Self = Self(EventType::SW_EVENT_DEAULT as c_int);
    pub const READ: Self = Self(EventType::SW_EVENT_READ as c_int);
    pub const WRITE: Self = Self(EventType::SW_EVENT_WRITE as c_int);
    pub const RDWR: Self = Self(EventType::SW_EVENT_RDWR as c_int);
    pub const ERROR: Self = Self(EventType::SW_EVENT_ERROR as c_int);
    pub const ONCE: Self = Self(EventType::SW_EVENT_ONCE as c_int);

    /// The bits of upstream's `int`.
    #[must_use]
    pub const fn from_bits(bits: c_int) -> Self {
        Self(bits)
    }

    /// Upstream's `int`.
    #[must_use]
    pub const fn bits(self) -> c_int {
        self.0
    }

    /// Whether it has every event `other` has.
    #[must_use]
    pub const fn contains(self, other: Self) -> bool {
        self.0 & other.0 == other.0
    }

    /// Whether it has any event `other` has.
    #[must_use]
    pub const fn intersects(self, other: Self) -> bool {
        self.0 & other.0 != 0
    }

    #[must_use]
    pub const fn is_empty(self) -> bool {
        self.0 == 0
    }

    /// `Reactor::isset_read_event`: read, or no direction at all.
    #[must_use]
    pub const fn reads(self) -> bool {
        self.0 < Self::DEFAULT.0 || self.intersects(Self::READ)
    }

    /// `Reactor::isset_write_event`.
    #[must_use]
    pub const fn writes(self) -> bool {
        self.intersects(Self::WRITE)
    }

    /// These events less `other`'s.
    #[must_use]
    pub const fn without(self, other: Self) -> Self {
        Self(self.0 & !other.0)
    }
}

impl core::ops::BitOr for Events {
    type Output = Self;
    fn bitor(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }
}

impl core::ops::BitOrAssign for Events {
    fn bitor_assign(&mut self, other: Self) {
        self.0 |= other.0;
    }
}

impl core::ops::BitAnd for Events {
    type Output = Self;
    fn bitand(self, other: Self) -> Self {
        Self(self.0 & other.0)
    }
}

/// Upstream's logs print the `int`.
impl core::fmt::Display for Events {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        self.0.fmt(f)
    }
}
