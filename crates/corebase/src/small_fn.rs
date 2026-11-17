//! A callback of at most two words held inline, its type erased: a timer node's
//! callback and a cancelable yield's canceler, with no allocation.
use core::cell::Cell;
use core::marker::PhantomData;
use core::mem::MaybeUninit;

type Bytes = MaybeUninit<[usize; 2]>;

/// What a [`SmallFn`] holds: run in place while held, or taken out and consumed once.
pub(crate) trait Small<R>: 'static {
    fn run(&self) -> R;
    fn consume(self) -> R;
}

struct Ops<R: 'static> {
    run: fn(&SmallFn<R>) -> R,
    consume: fn(Bytes) -> R,
}

/// An empty slot or one [`Small`] value. `ops` is set exactly while `value` holds the
/// value [`put`](Self::put) wrote, of the type `ops` was made for. A run borrows it in
/// place through `&self`, so nothing writes or moves it meanwhile: `put` refuses a full
/// slot, and only [`take`](Self::take), through `&mut self`, moves it out.
pub(crate) struct SmallFn<R: 'static> {
    value: Cell<Bytes>,
    ops: Cell<Option<&'static Ops<R>>>,
    /// The value may be neither `Send` nor `Sync`.
    thread: PhantomData<*const ()>,
}

/// A value [`SmallFn::take`] moved out; dropping it unconsumed leaks it.
#[must_use]
pub(crate) struct Taken<R: 'static> {
    value: Bytes,
    consume: fn(Bytes) -> R,
    thread: PhantomData<*const ()>,
}

impl<R> SmallFn<R> {
    pub(crate) const fn empty() -> Self {
        Self {
            value: Cell::new(Bytes::uninit()),
            ops: Cell::new(None),
            thread: PhantomData,
        }
    }

    pub(crate) fn is_full(&self) -> bool {
        self.ops.get().is_some()
    }

    /// Holds `value`; a full slot aborts.
    pub(crate) fn put<C: Small<R>>(&self, value: C) {
        fn run<C: Small<R>, R>(slot: &SmallFn<R>) -> R {
            // SAFETY: `slot`'s ops are `C`'s, so it holds the `C` `put` wrote, which
            // stays put while `slot` is borrowed (the type's invariant).
            let value = unsafe { &*slot.value.as_ptr().cast::<C>() };
            value.run()
        }
        fn consume<C: Small<R>, R>(bytes: Bytes) -> R {
            // SAFETY: only `take` makes a `Taken`, of the `C` `put` wrote and `take`
            // moved out with these ops; `Taken::consume` takes it, so it is read once.
            let value = unsafe { bytes.as_ptr().cast::<C>().read() };
            value.consume()
        }
        const {
            assert!(
                size_of::<C>() <= size_of::<Bytes>() && align_of::<C>() <= align_of::<Bytes>(),
                "a small fn holds at most two words, aligned no more than a word"
            );
        };
        if self.is_full() {
            crate::fatal("a small fn is put into a full slot");
        }
        let mut bytes = Bytes::uninit();
        // SAFETY: `C` fits `Bytes`'s size and alignment, as asserted above.
        unsafe { bytes.as_mut_ptr().cast::<C>().write(value) };
        self.value.set(bytes);
        self.ops.set(Some(
            const {
                &Ops {
                    run: run::<C, R>,
                    consume: consume::<C, R>,
                }
            },
        ));
    }

    /// Runs the value in place; none when empty.
    #[inline]
    pub(crate) fn run(&self) -> Option<R> {
        let ops = self.ops.get()?;
        Some((ops.run)(self))
    }

    /// Moves the value out, leaving the slot empty.
    pub(crate) fn take(&mut self) -> Option<Taken<R>> {
        let ops = self.ops.take()?;
        Some(Taken {
            value: self.value.get(),
            consume: ops.consume,
            thread: PhantomData,
        })
    }
}

impl<R> Taken<R> {
    pub(crate) fn consume(self) -> R {
        (self.consume)(self.value)
    }
}

impl<R> core::fmt::Debug for SmallFn<R> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("SmallFn")
            .field("full", &self.is_full())
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::{Small, SmallFn};
    use std::cell::Cell;
    use std::rc::Rc;

    struct Count(Rc<Cell<u32>>);

    impl Small<u32> for Count {
        fn run(&self) -> u32 {
            self.0.set(self.0.get() + 1);
            self.0.get()
        }
        fn consume(self) -> u32 {
            self.0.get() + 100
        }
    }

    #[test]
    fn runs_in_place_then_consumes_once() {
        let runs = Rc::new(Cell::new(0));
        let mut slot = SmallFn::empty();
        assert!(slot.run().is_none());
        slot.put(Count(Rc::clone(&runs)));
        assert_eq!((slot.run(), slot.run()), (Some(1), Some(2)));
        let taken = slot.take().unwrap();
        assert!(!slot.is_full() && slot.take().is_none() && slot.run().is_none());
        assert_eq!(Rc::strong_count(&runs), 2);
        assert_eq!(taken.consume(), 102);
        // `consume` dropped the moved-out value.
        assert_eq!(Rc::strong_count(&runs), 1);
    }

    struct Pair(u32, u32);

    impl Small<u32> for Pair {
        fn run(&self) -> u32 {
            self.0 * 10 + self.1
        }
        fn consume(self) -> u32 {
            self.run()
        }
    }

    #[test]
    fn an_emptied_slot_takes_a_new_value() {
        let mut slot = SmallFn::empty();
        slot.put(Pair(3, 4));
        assert_eq!(slot.take().map(super::Taken::consume), Some(34));
        slot.put(Pair(4, 3));
        assert_eq!(slot.run(), Some(43));
    }
}
