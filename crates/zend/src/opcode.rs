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

    /// The handler runs on the thread executing this frame.
    #[must_use]
    pub fn php(self) -> Php<'a> {
        Php(PhantomData)
    }

    /// `EX(func)`: the function the frame runs, which Zend keeps for the call.
    #[must_use]
    pub fn function(self) -> Function<'a> {
        // SAFETY: a live frame.
        Function(unsafe { (*self.ptr).func }, PhantomData)
    }

    /// `EX(opline)`.
    #[must_use]
    pub fn opline(self) -> *const zend_op {
        // SAFETY: a live frame.
        unsafe { (*self.ptr).opline }
    }

    /// `EX(opline) = opline`.
    pub fn jump(self, opline: *const zend_op) {
        // SAFETY: a live frame.
        unsafe { (*self.ptr).opline = opline };
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
