//! User opcode handlers (`zend_set_user_opcode_handler`): [`handler`] is the one
//! entry Zend calls, and hands the body a [`Frame`].
use crate::{
    Function, Php,
    sys::{zend_execute_data, zend_op},
};
use std::{ffi::c_int, marker::PhantomData};

/// `ZEND_USER_OPCODE_CONTINUE`: the handler ran the instruction.
pub const CONTINUE: c_int = 0;
/// `ZEND_USER_OPCODE_DISPATCH`: Zend runs the instruction itself.
pub const DISPATCH: c_int = 2;

/// A frame executing on this thread, live for `'a`: one Zend passes an
/// internal function's handler, a user opcode handler or an observer. Zend changes it (its instruction, its
/// variables) as it executes, so each read borrows its header only while it
/// copies a field out ([`Executing::read`]).
#[derive(Clone, Copy)]
pub(crate) struct Executing<'a>(
    pub(crate) *mut zend_execute_data,
    pub(crate) PhantomData<&'a zend_execute_data>,
);

impl Executing<'_> {
    /// `f` of the frame's header, which it reads only: no Zend code runs, so
    /// nothing changes the frame, while it is borrowed.
    pub(crate) fn read<R>(self, f: impl FnOnce(&zend_execute_data) -> R) -> R {
        // SAFETY: a live frame, per the type; the borrow ends with `f`.
        f(unsafe { &*self.0 })
    }
}

/// A live frame at the instruction of the user opcode handler Zend passed it
/// to, for `'a`: it is running a function's instruction, so it has a function. Only [`handler`] makes one.
#[derive(Clone, Copy)]
pub struct Frame<'a> {
    ptr: *mut zend_execute_data,
    live: PhantomData<&'a zend_execute_data>,
}

impl<'a> Frame<'a> {
    #[must_use]
    pub fn as_ptr(self) -> *mut zend_execute_data {
        self.ptr
    }

    /// The frame, executing for `'a`.
    fn executing(self) -> Executing<'a> {
        Executing(self.ptr, PhantomData)
    }

    /// The handler runs on the thread executing this frame.
    #[must_use]
    pub fn php(self) -> Php<'a> {
        Php(PhantomData)
    }

    /// `EX(func)`: the function the frame runs, which Zend keeps for the call.
    #[must_use]
    pub fn function(self) -> Function<'a> {
        Function(self.executing().read(|frame| frame.func), PhantomData)
    }

    /// `EX(opline)`.
    #[must_use]
    pub fn opline(self) -> *const zend_op {
        self.executing().read(|frame| frame.opline)
    }

    /// `EX(opline)`'s instruction, which the handler was called for: part of
    /// the function's op array, which Zend leaves unwritten while it runs (the
    /// JIT, which rewrites handlers, turns itself off under user opcode handlers).
    #[must_use]
    #[inline]
    pub fn instruction(self) -> &'a zend_op {
        // SAFETY: a frame at a user opcode handler is at a live instruction.
        unsafe { &*self.opline() }
    }

    /// `OP_DATA`: the instruction after this one, if the function has one.
    #[must_use]
    pub fn next_instruction(self) -> Option<&'a zend_op> {
        let (first, last) = self.function().instructions()?;
        let next = self.opline().wrapping_add(1);
        let index = next.addr().wrapping_sub(first.addr()) / size_of::<zend_op>();
        if index >= usize::try_from(last).unwrap_or(0) {
            return None;
        }
        // SAFETY: an instruction of the frame's function, as for `instruction`.
        Some(unsafe { &*next })
    }

    /// `EX(opline) += count`, as a handler that consumed `count` instructions
    /// does. Zend executes `EX(opline)` next, so the move stays inside the
    /// function's instructions: one past them leaves the frame where it is (never
    /// so for what Zend compiles, whose op arrays end in a RETURN).
    pub fn skip(self, count: usize) {
        let Some((first, last)) = self.function().instructions() else {
            return;
        };
        let at = self.opline();
        let index = at.addr().wrapping_sub(first.addr()) / size_of::<zend_op>();
        if index
            .checked_add(count)
            .is_none_or(|next| next >= usize::try_from(last).unwrap_or(0))
        {
            return;
        }
        // SAFETY: a live frame, and an instruction of its function.
        unsafe { (*self.ptr).opline = at.wrapping_add(count) };
    }
}

/// A user opcode handler's body: `ZEND_USER_OPCODE_*` for the instruction
/// `frame` is at.
pub trait Handler {
    fn run(frame: Frame<'_>) -> c_int;
}

/// `H` as Zend's `user_opcode_handler_t`.
/// # Safety
/// Zend's user opcode handler contract: `frame` is the live frame executing on
/// this thread, at an instruction of an opcode `H` was installed for.
pub unsafe extern "C" fn handler<H: Handler>(frame: *mut zend_execute_data) -> c_int {
    H::run(Frame {
        ptr: frame,
        live: PhantomData,
    })
}
