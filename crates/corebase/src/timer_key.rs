//! [`Held`]: a timer as its holders keep it, in the word a `TimerNode *` took.

use core::ffi::c_long;
use core::num::NonZeroU64;
use core::sync::atomic::{AtomicU64, Ordering::Relaxed};

/// Bits of [`Held`] for the id; the generation takes the other 16.
const ID_BITS: u32 = 48;
const ID_MASK: u64 = (1 << ID_BITS) - 1;

/// A timer's id with the generation of the timer that issued it (crates/timer), packed in
/// one word. A timer never reuses an id and checks the generation, so deleting a held
/// timer that has fired, or whose timer is gone, does nothing. Any value is sound to
/// delete: at worst it names another timer of this thread, as any id does.
/// `Option<Held>` is one word, zero for none.
#[repr(transparent)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Held(NonZeroU64);

impl Held {
    /// Timer `id` of the timer of `generation`, if the id is positive and fits in 48 bits.
    #[must_use]
    pub fn new(generation: u16, id: c_long) -> Option<Self> {
        let id = u64::try_from(id)
            .ok()
            .filter(|id| (1..=ID_MASK).contains(id))?;
        NonZeroU64::new((u64::from(generation) << ID_BITS) | id).map(Self)
    }

    /// Timer `id` (its low 48 bits) of generation 0, which no timer has: it names none,
    /// for a holder's tests.
    #[must_use]
    pub const fn detached(id: c_long) -> Self {
        match NonZeroU64::new(id.cast_unsigned() & ID_MASK) {
            Some(bits) => Self(bits),
            None => Self(NonZeroU64::MIN),
        }
    }

    /// The generation of the timer that issued it.
    #[must_use]
    pub fn generation(self) -> u16 {
        u16::try_from(self.0.get() >> ID_BITS).unwrap_or(0)
    }

    /// The timer's id.
    #[must_use]
    pub fn id(self) -> c_long {
        c_long::try_from(self.0.get() & ID_MASK).unwrap_or(0)
    }
}

/// An `Option<Held>` a shared record keeps, in the word a `TimerNode *` took.
#[derive(Debug, Default)]
#[repr(transparent)]
pub struct AtomicHeld(AtomicU64);

impl AtomicHeld {
    #[must_use]
    pub const fn new() -> Self {
        Self(AtomicU64::new(0))
    }

    pub fn store(&self, held: Option<Held>) {
        self.0.store(held.map_or(0, |held| held.0.get()), Relaxed);
    }

    /// The held timer, left empty.
    pub fn take(&self) -> Option<Held> {
        NonZeroU64::new(self.0.swap(0, Relaxed)).map(Held)
    }

    #[must_use]
    pub fn is_some(&self) -> bool {
        self.0.load(Relaxed) != 0
    }
}

#[cfg(test)]
mod tests {
    use super::{AtomicHeld, Held};

    #[test]
    fn packs_the_generation_and_a_positive_id_that_fits() {
        let held = Held::new(7, 42).unwrap();
        assert_eq!((held.generation(), held.id()), (7, 42));
        let top = Held::new(u16::MAX, (1 << 48) - 1).unwrap();
        assert_eq!((top.generation(), top.id()), (u16::MAX, (1 << 48) - 1));
        assert_eq!(Held::new(1, 1 << 48), None);
        assert_eq!(Held::new(1, 0), None);
        assert_eq!(Held::new(1, -1), None);
        assert_eq!(Held::detached(3), Held::new(0, 3).unwrap());
        assert_eq!(size_of::<Option<Held>>(), 8);
    }

    #[test]
    fn an_atomic_slot_takes_what_it_stored_once() {
        let slot = AtomicHeld::new();
        assert!(!slot.is_some());
        slot.store(Held::new(2, 9));
        assert!(slot.is_some());
        assert_eq!(slot.take(), Held::new(2, 9));
        assert_eq!(slot.take(), None);
    }
}
