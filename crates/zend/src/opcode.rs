//! User opcode handlers (`zend_set_user_opcode_handler`): [`handler`] is the one
//! entry Zend calls, and hands the body a [`Frame`].
use crate::{
    Function, Php,
    sys::{zend_execute_data, zend_function, zend_op},
};
use std::{
    cell::Cell,
    ffi::{c_int, c_void},
    marker::PhantomData,
    mem::offset_of,
};

/// What a user opcode handler leaves Zend to do with its instruction.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Next {
    /// `ZEND_USER_OPCODE_CONTINUE`: the handler ran the instruction.
    Continue,
    /// `ZEND_USER_OPCODE_DISPATCH`: Zend runs the instruction itself.
    Dispatch,
}

/// A frame executing on this thread, live for `'a`: one Zend passes an
/// internal function's handler, a user opcode handler or an observer. Zend changes
/// its header (its instruction) as it executes, through pointers, so this reads
/// and writes the header's first words as cells ([`Head`]).
#[derive(Clone, Copy)]
pub(crate) struct Executing<'a>(
    pub(crate) *mut zend_execute_data,
    pub(crate) PhantomData<&'a zend_execute_data>,
);

/// A frame's first words, each a `Cell`: `opline`, `call`, `return_value`, `func`.
type Head = [Cell<*mut c_void>; 4];
/// `EX(opline)`'s word in a [`Head`].
const OPLINE: usize = offset_of!(zend_execute_data, opline) / size_of::<*mut c_void>();
/// `EX(func)`'s word in a [`Head`].
const FUNC: usize = offset_of!(zend_execute_data, func) / size_of::<*mut c_void>();

const _: () = assert!(
    offset_of!(zend_execute_data, opline) == OPLINE * size_of::<*mut c_void>()
        && offset_of!(zend_execute_data, func) == FUNC * size_of::<*mut c_void>()
        && OPLINE < 4
        && FUNC < 4
        && size_of::<zend_execute_data>() >= size_of::<Head>()
        && align_of::<zend_execute_data>() >= align_of::<Head>(),
    "a Head is the pointer words a zend_execute_data starts with"
);

impl<'a> Executing<'a> {
    /// The frame's [`Head`].
    fn head(self) -> &'a Head {
        // SAFETY: a live frame for `'a`, per the type, which starts with these
        // pointer words (asserted above); each is a `Cell`, as Zend writes them
        // through pointers on this thread.
        unsafe { &*self.0.cast::<Head>() }
    }

    /// `EX(func)`.
    pub(crate) fn func(self) -> *mut zend_function {
        self.head()[FUNC].get().cast()
    }

    /// `EX(opline)`.
    fn opline(self) -> *const zend_op {
        self.head()[OPLINE].get().cast_const().cast()
    }

    /// `EX(opline) = at`.
    fn set_opline(self, at: *const zend_op) {
        self.head()[OPLINE].set(at.cast_mut().cast());
    }
}

/// A live frame at the instruction of the user opcode handler Zend passed it
/// to, for `'a`: it is running a function's instruction, so it has a function. Only [`handler`] makes one.
#[derive(Clone, Copy)]
#[repr(transparent)]
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
        Function(self.executing().func(), PhantomData)
    }

    /// `EX(opline)`.
    #[must_use]
    pub fn opline(self) -> *const zend_op {
        self.executing().opline()
    }

    /// `EX(opline)`'s instruction, which the handler was called for. Zend never
    /// leaves `EX(opline)` outside its function's instructions; if it did, this
    /// reads as a `NOP` with unused operands, which every handler leaves to Zend.
    #[must_use]
    #[inline]
    pub fn instruction(self) -> &'a zend_op {
        self.ahead(0).unwrap_or(&NOP)
    }

    /// `OP_DATA`: the instruction after this one, if the function has one.
    #[must_use]
    #[inline]
    pub fn next_instruction(self) -> Option<&'a zend_op> {
        self.ahead(1)
    }

    /// The instruction `count` after `EX(opline)`, if it is one of the function's:
    /// part of its op array, which Zend leaves unwritten while it runs (the JIT,
    /// which rewrites handlers, turns itself off under user opcode handlers).
    #[inline]
    fn ahead(self, count: usize) -> Option<&'a zend_op> {
        let (first, last) = self.function().instructions()?;
        let at = self.opline().wrapping_add(count);
        let index = at.addr().wrapping_sub(first.addr()) / size_of::<zend_op>();
        if index >= usize::try_from(last).unwrap_or(0) {
            return None;
        }
        // SAFETY: an instruction of the frame's function, inside its op array.
        Some(unsafe { &*at })
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
        self.executing().set_opline(at.wrapping_add(count));
    }
}

/// What [`Frame::instruction`] reads outside the function's instructions.
const NOP: zend_op = zend_op {
    handler: std::ptr::null(),
    op1: 0,
    op2: 0,
    result: 0,
    extended_value: 0,
    lineno: 0,
    opcode: 0,
    op1_type: 0,
    op2_type: 0,
    result_type: 0,
};

/// A user opcode handler's body: what Zend does next with the instruction
/// `frame` is at, whose opcode is `OPCODE` ([`crate::Minit::opcode_handler`]
/// installs it for that one).
pub trait Handler {
    /// The `ZEND_*` opcode whose instructions `run` is written for.
    const OPCODE: u32;
    fn run(frame: Frame<'_>) -> Next;
}

/// A user opcode handler as Zend stores it (`user_opcode_handler_t`): Zend
/// calls it with the live frame executing on this thread, at an instruction of
/// the opcode it was installed for, which only that call makes a [`Frame`] of.
pub type Entry = extern "C" fn(Frame<'_>) -> c_int;

/// `H` as Zend's `user_opcode_handler_t`, installed for `H::OPCODE` only.
#[must_use]
pub extern "C" fn handler<H: Handler>(frame: Frame<'_>) -> c_int {
    // zend-sys' shim_stdext.c asserts the two codes.
    match H::run(frame) {
        Next::Continue => 0,
        Next::Dispatch => 2,
    }
}
