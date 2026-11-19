//! A callback of at most `N` words (two by default) held inline, its type erased: a
//! timer node's callback, a cancelable yield's canceler and a coroutine's entry, with
//! no allocation.
use core::cell::Cell;
use core::marker::PhantomData;
use core::mem::{ManuallyDrop, MaybeUninit};

type Bytes<const N: usize> = MaybeUninit<[usize; N]>;

/// What a [`SmallFn`] holds: run in place while held, or taken out and consumed once.
pub(crate) trait Small<R>: 'static {
    fn run(&self) -> R;
    fn consume(self) -> R;
}

struct Ops<R: 'static, const N: usize> {
    run: fn(&SmallFn<R, N>) -> R,
    consume: fn(Bytes<N>) -> R,
    discard: Option<fn(Bytes<N>)>,
}

/// An empty slot or one [`Small`] value. `ops` is set exactly while `value` holds the
/// value [`put`](Self::put) wrote, of the type `ops` was made for. A run borrows it in
/// place through `&self`, so nothing writes or moves it meanwhile: `put` refuses a full
/// slot, and only [`take`](Self::take), through `&mut self`, moves it out.
pub(crate) struct SmallFn<R: 'static, const N: usize = 2> {
    value: Cell<Bytes<N>>,
    ops: Cell<Option<&'static Ops<R, N>>>,
    /// The value may be neither `Send` nor `Sync`.
    thread: PhantomData<*const ()>,
}

/// Coroutine entries and cancelers capture only Copy values. Only `put` can
/// populate this slot, so discarding it never requires an erased destructor.
#[derive(Debug)]
pub(crate) struct CopyFn<R: 'static, const N: usize = 2>(ManuallyDrop<SmallFn<R, N>>);

impl<R, const N: usize> CopyFn<R, N> {
    pub(crate) const fn empty() -> Self {
        Self(ManuallyDrop::new(SmallFn::empty()))
    }

    pub(crate) fn put(&self, callback: impl Fn() -> R + Copy + 'static) {
        self.0.put(callback);
    }

    pub(crate) fn run(&self) -> Option<R> {
        self.0.run()
    }

    pub(crate) fn take(&mut self) -> Option<Taken<R, N>> {
        self.0.take()
    }

    #[cfg(test)]
    pub(crate) fn is_full(&self) -> bool {
        self.0.is_full()
    }

    pub(crate) fn clear(&mut self) {
        self.0.ops.set(None);
    }
}

impl<R, F: Fn() -> R + Copy + 'static> Small<R> for F {
    fn run(&self) -> R {
        self()
    }

    fn consume(self) -> R {
        self()
    }
}

/// A value [`SmallFn::take`] moved out, owned until invoked or dropped.
#[must_use]
pub(crate) struct Taken<R: 'static, const N: usize = 2> {
    value: Bytes<N>,
    ops: &'static Ops<R, N>,
    thread: PhantomData<*const ()>,
}

impl<R, const N: usize> SmallFn<R, N> {
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
        fn run<C: Small<R>, R, const N: usize>(slot: &SmallFn<R, N>) -> R {
            // SAFETY: `slot`'s ops are `C`'s, so it holds the `C` `put` wrote, which
            // stays put while `slot` is borrowed (the type's invariant).
            let value = unsafe { &*slot.value.as_ptr().cast::<C>() };
            value.run()
        }
        fn finish<C, T, const N: usize>(bytes: Bytes<N>, action: fn(C) -> T) -> T {
            // SAFETY: the ops installed below are paired with the bytes holding C.
            // Taking the slot transfers ownership, and consuming or dropping it
            // calls exactly one of those ops.
            action(unsafe { bytes.as_ptr().cast::<C>().read() })
        }
        const {
            assert!(
                size_of::<C>() <= size_of::<Bytes<N>>()
                    && align_of::<C>() <= align_of::<Bytes<N>>(),
                "a small fn holds at most its N words, aligned no more than a word"
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
                    run: run::<C, R, N>,
                    consume: |bytes| finish(bytes, C::consume),
                    discard: if core::mem::needs_drop::<C>() {
                        Some(|bytes| finish(bytes, drop::<C>))
                    } else {
                        None
                    },
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
    pub(crate) fn take(&mut self) -> Option<Taken<R, N>> {
        let ops = self.ops.take()?;
        Some(Taken {
            value: self.value.get(),
            ops,
            thread: PhantomData,
        })
    }
}

impl<R, const N: usize> Taken<R, N> {
    pub(crate) fn consume(self) -> R {
        let owned = ManuallyDrop::new(self);
        (owned.ops.consume)(owned.value)
    }
}

impl<R, const N: usize> Drop for SmallFn<R, N> {
    fn drop(&mut self) {
        if let Some(discard) = self.ops.take().and_then(|ops| ops.discard) {
            discard(self.value.get());
        }
    }
}

impl<R, const N: usize> Drop for Taken<R, N> {
    fn drop(&mut self) {
        if let Some(discard) = self.ops.discard {
            discard(self.value);
        }
    }
}

impl<R, const N: usize> core::fmt::Debug for SmallFn<R, N> {
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
            self.0.set(self.0.get() + 100);
            self.0.get()
        }
    }

    #[test]
    fn runs_in_place_then_consumes_once() {
        let runs = Rc::new(Cell::new(0));
        let mut slot: SmallFn<u32> = SmallFn::empty();
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

    #[test]
    fn discarding_a_slot_or_taken_callback_releases_its_capture() {
        let runs = Rc::new(Cell::new(0));
        let mut slot: SmallFn<u32> = SmallFn::empty();
        slot.put(Count(Rc::clone(&runs)));
        drop(slot.take());
        assert_eq!(Rc::strong_count(&runs), 1);
        slot.put(Count(Rc::clone(&runs)));
        drop(slot);
        assert_eq!(Rc::strong_count(&runs), 1);
        assert_eq!(runs.get(), 0);
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
        let mut slot: SmallFn<u32> = SmallFn::empty();
        slot.put(Pair(3, 4));
        assert_eq!(slot.take().map(super::Taken::consume), Some(34));
        slot.put(Pair(4, 3));
        assert_eq!(slot.run(), Some(43));
    }
}
