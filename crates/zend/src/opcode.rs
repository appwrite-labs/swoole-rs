//! User opcode handlers (`zend_set_user_opcode_handler`): [`handler`] is the one
//! entry Zend calls, and hands the body a [`Frame`].
use crate::{
    Function, Php, Value,
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

/// A frame's first words, as Zend lays them out: each pointer a `Cell` and `This`
/// a [`Value`], as Zend writes them through pointers on this thread.
#[repr(C)]
struct Head {
    opline: Cell<*const zend_op>,
    _call_return_value: [Cell<*mut c_void>; 2],
    func: Cell<*mut zend_function>,
    this: Value,
    _prev_symbol_table: [Cell<*mut c_void>; 2],
    run_time_cache: Cell<*mut c_void>,
}

const _: () = assert!(
    offset_of!(zend_execute_data, opline) == offset_of!(Head, opline)
        && offset_of!(zend_execute_data, func) == offset_of!(Head, func)
        && offset_of!(zend_execute_data, This) == offset_of!(Head, this)
        && offset_of!(zend_execute_data, run_time_cache) == offset_of!(Head, run_time_cache)
        && size_of::<zend_execute_data>() >= size_of::<Head>()
        && align_of::<zend_execute_data>() >= align_of::<Head>(),
    "a Head is the words a zend_execute_data starts with"
);

impl<'a> Executing<'a> {
    /// The frame's [`Head`].
    fn head(self) -> &'a Head {
        // SAFETY: a live frame for `'a`, per the type, which starts with these
        // words (asserted above); each is a `Cell` or a `Value`, as Zend writes
        // them through pointers on this thread.
        unsafe { &*self.0.cast::<Head>() }
    }

    /// `EX(func)`.
    pub(crate) fn func(self) -> *mut zend_function {
        self.head().func.get()
    }

    /// `EX(This)`: `$this` in its value and the argument count in its `u2`,
    /// which the frame holds for `'a`.
    pub(crate) fn this(self) -> &'a Value {
        &self.head().this
    }

    /// `EX(opline)`.
    fn opline(self) -> *const zend_op {
        self.head().opline.get()
    }

    /// `EX(opline) = at`.
    fn set_opline(self, at: *const zend_op) {
        self.head().opline.set(at);
    }

    /// `EX(run_time_cache)`, left null in its place.
    pub(crate) fn take_run_time_cache(self) -> *mut c_void {
        self.head().run_time_cache.replace(std::ptr::null_mut())
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

    /// The function's instructions, which the handler's frame runs, and the index
    /// of `EX(opline)` among them; None for a function without any.
    fn code(self) -> Option<(&'a [zend_op], usize)> {
        let (first, last) = self.function().instructions()?;
        if first.is_null() {
            return None;
        }
        // SAFETY: the frame's function's op array, `last` instructions from `first`,
        // which Zend leaves unwritten while it runs (the JIT, which rewrites
        // handlers, turns itself off under user opcode handlers).
        let code = unsafe { std::slice::from_raw_parts(first, usize::try_from(last).ok()?) };
        let index = self.opline().addr().wrapping_sub(first.addr()) / size_of::<zend_op>();
        Some((code, index))
    }

    /// `EX(opline)`'s instruction, which the handler was called for.
    #[must_use]
    #[inline]
    pub fn instruction(self) -> Option<&'a zend_op> {
        let (code, index) = self.code()?;
        code.get(index)
    }

    /// `OP_DATA`: the instruction after this one, if the function has one.
    #[must_use]
    pub fn next_instruction(self) -> Option<&'a zend_op> {
        let (code, index) = self.code()?;
        code.get(index.checked_add(1)?)
    }

    /// `EX(opline) += count`, as a handler that consumed `count` instructions
    /// does. Zend executes `EX(opline)` next, so the move stays inside the
    /// function's instructions: one past them leaves the frame where it is (never
    /// so for what Zend compiles, whose op arrays end in a RETURN).
    pub fn skip(self, count: usize) {
        let Some((code, index)) = self.code() else {
            return;
        };
        if index
            .checked_add(count)
            .is_some_and(|next| next < code.len())
        {
            self.executing()
                .set_opline(self.opline().wrapping_add(count));
        }
    }
}

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
pub type Entry = unsafe extern "C" fn(Frame<'_>) -> c_int;

crate::exports! {
/// `H` as Zend's `user_opcode_handler_t`, installed for `H::OPCODE` only.
#[must_use]
pub extern "C" fn handler<H: Handler>(frame: Frame<'_>) -> c_int {
    // zend-sys' shim_stdext.c asserts the two codes.
    match H::run(frame) {
        Next::Continue => 0,
        Next::Dispatch => 2,
    }
}
}
