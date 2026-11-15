//! Methods on arrays, strings and streams: the `INIT_METHOD_CALL` handler
//! routes `$value->name(...)` to `swoole_call_{array,string,stream}_method`,
//! which calls the function the method names with `$value` first; and the
//! `swoole_*` wrappers that move that first argument where the function
//! expects it.
use crate::{
    CONTINUE, DISPATCH, Frame, Op,
    aliases::{self, Receiver},
};
use std::{
    ffi::{CStr, c_int, c_void},
    ptr, slice,
    sync::atomic::{AtomicPtr, Ordering},
};
use swoole_rs_zend::{
    Call, Value,
    bailout::{Bailout, trap},
    opcode::Handler,
    sys::{
        IS_ARRAY, IS_CONST, IS_RESOURCE, IS_STRING, IS_VAR, swr_zend_efree, swr_zend_zval_release,
        zend_execute_data, zend_function, zval,
    },
    value,
};

unsafe extern "C" {
    /// `zend_vm_stack_push_call_frame(ZEND_CALL_NESTED_FUNCTION, function,
    /// num_args, object)`: the new frame on the VM stack, which Zend always
    /// pushes, for as long as the caller says it lives.
    #[link_name = "swr_zend_push_nested_call"]
    fn push_nested_call<'a>(
        function: *mut zend_function,
        num_args: u32,
        object: *mut c_void,
    ) -> &'a mut zend_execute_data;
}

/// Upstream's `CallInfo`: what the handler hands the method call's frame in
/// `run_time_cache`. `this` is the receiver's live zval, which the method call
/// releases once when it was a `VAR` operand.
#[repr(C)]
struct CallInfo {
    func: zval,
    this: zval,
    op1_type: u8,
}

/// `swoole_call_{array,string,stream}_method`, by `Receiver`.
static METHODS: [AtomicPtr<zend_function>; 3] = [const { AtomicPtr::new(ptr::null_mut()) }; 3];

/// Look up the method dispatchers (MINIT).
pub(crate) fn install(minit: &swoole_rs_zend::Minit) {
    for (slot, name) in METHODS.iter().zip([
        c"swoole_call_array_method",
        c"swoole_call_string_method",
        c"swoole_call_stream_method",
    ]) {
        slot.store(minit.compiled_function(name), Ordering::Relaxed);
    }
}

/// The `INIT_METHOD_CALL` user opcode handler.
pub(crate) struct MethodCall;

impl Handler for MethodCall {
    fn run(frame: Frame<'_>) -> c_int {
        let op = Op::current(frame);
        let Some(object) = op.op1() else {
            return DISPATCH;
        };
        // The method name: op2's literal, `RT_CONSTANT(opline, opline->op2)`,
        // which lives as long as the script. A name computed at run time is
        // left to Zend.
        if u32::from(op.op2_type) != IS_CONST {
            return DISPATCH;
        }
        let Some(name) = op.op2() else {
            return DISPATCH;
        };
        let receiver = match object.dereferenced().kind() {
            IS_ARRAY => Receiver::Array,
            IS_STRING => Receiver::String,
            IS_RESOURCE => Receiver::Stream,
            _ => return DISPATCH,
        };
        // Zend's request allocator, freed by the method call.
        let info = value::emalloc(size_of::<CallInfo>()).cast::<CallInfo>();
        // A plain copy of the literal, as upstream's.
        let func = name.copy_value();
        let this = object.copy_value();
        // SAFETY: a fresh allocation of a `CallInfo`.
        unsafe {
            info.write(CallInfo {
                func,
                this,
                op1_type: op.op1_type,
            });
        }
        let method = match receiver {
            Receiver::Array => &METHODS[0],
            Receiver::String => &METHODS[1],
            Receiver::Stream => &METHODS[2],
        }
        .load(Ordering::Relaxed);
        // SAFETY: a registered function; the frame passes `object` as upstream
        // does. The frame just pushed lives until its call ends, and nothing
        // else reaches it until this frame's `call` names it below.
        let pushed = unsafe { push_nested_call(method, op.extended_value, object.as_ptr().cast()) };
        pushed.run_time_cache = info.cast();
        // SAFETY: this live frame's header; the operands borrowed above are slots
        // after it, and no Zend code runs while it is borrowed.
        let current = unsafe { &mut *frame.as_ptr() };
        pushed.prev_execute_data = current.call;
        current.call = ptr::from_mut(pushed);
        frame.skip(1);
        CONTINUE
    }
}

/// `prefix`, then `ZEND_CALL_ARG(frame, 1)` through `ZEND_CALL_NUM_ARGS(frame)`:
/// plain copies, as upstream's.
fn arguments(prefix: &[zval], frame: &Call) -> Vec<zval> {
    let passed = (1..=frame.num_args())
        .filter_map(|n| frame.value(n))
        .map(Value::copy_value);
    prefix.iter().copied().chain(passed).collect()
}

/// Upstream's `call_method`: call the function `receiver`'s method names with
/// `$this` first.
/// # Safety
/// A frame the `INIT_METHOD_CALL` handler pushed.
unsafe fn call_method(receiver: Receiver, frame: *mut zend_execute_data, value: *mut zval) {
    // SAFETY: the handler stored its `CallInfo` here.
    let allocation = unsafe { (*frame).run_time_cache }.cast::<CallInfo>();
    // SAFETY: the handler's initialized allocation, which only this frame's
    // `run_time_cache` points to until it is freed below.
    let info = unsafe { &mut *allocation };
    // op2 of INIT_METHOD_CALL is the method name, a string literal.
    let name = Value::of(&mut info.func).str();
    let alias = name.and_then(|name| aliases::find(receiver, name.bytes()));
    let outcome = match alias {
        None => {
            let kind = Value::of(&mut info.this).type_name().to_bytes();
            // "(null)" is what Zend's printf shows for a null string.
            let name = name.map_or(&b"(null)"[..], |name| name.bytes());
            swoole_rs_zend::throw_error(&crate::typed::message(&[
                b"The method `",
                name,
                b"` is undefined on ",
                kind,
            ]));
            Ok(())
        }
        Some(alias) => {
            // SAFETY: per the caller, the dispatcher's own frame and return slot.
            let frame = unsafe { Call::new(frame, value) };
            call_alias(alias, info, &frame)
        }
    };
    // SAFETY: the handler's allocation; nothing else holds it.
    unsafe { swr_zend_efree(allocation.cast()) };
    // SAFETY: a live frame.
    unsafe { (*frame).run_time_cache = ptr::null_mut() };
    swoole_rs_zend::bailout::raise(outcome);
}

/// Call `alias` in `frame`, the dispatcher's, with `info`'s receiver first.
fn call_alias(alias: &CStr, info: &mut CallInfo, frame: &Call) -> Result<(), Bailout> {
    // `zend_hash_str_find_ptr(EG(function_table), ...)`: a function table holds
    // each function as a pointer.
    let Some(function) = frame.php().function(alias.to_bytes()) else {
        swoole_rs_zend::throw_error(&crate::typed::message(&[
            b"The function `",
            alias.to_bytes(),
            b"` is undefined",
        ]));
        return Ok(());
    };
    // A plain copy, as upstream's.
    let this = info.this;
    let mut argv = arguments(slice::from_ref(&this), frame);
    frame.call_known(function, &mut argv)?;
    if u32::from(info.op1_type) == IS_VAR {
        // SAFETY: a C body releasing the receiver (`CallInfo::this`), which
        // may run destructors.
        unsafe { trap(swr_zend_zval_release, (&raw mut info.this).cast()) }?;
    }
    Ok(())
}

/// Upstream's `move_first_element`: the first argument moves to `position`.
/// With `position` or fewer arguments the first argument moves to the end, and
/// the target function reports the missing arguments itself.
fn move_first(args: &mut [zval], position: usize) {
    let Some(last) = args.len().checked_sub(1) else {
        return;
    };
    if let Some(moved) = args.get_mut(..=position.min(last)) {
        moved.rotate_left(1);
    }
}

/// A `SW_CREATE_PHP_FUNCTION_WRAPPER` function: it calls `name` with its first
/// argument moved to `position`.
struct Wrapper {
    name: &'static CStr,
    position: usize,
}

impl Wrapper {
    /// Call the function with the arguments of `frame`, the wrapper's own.
    fn forward(&self, frame: &Call) {
        // A core function, which this thread's compiler always knows.
        let Some(target) = frame.php().compiled_function(self.name) else {
            return;
        };
        let mut argv = arguments(&[], frame);
        move_first(&mut argv, self.position);
        swoole_rs_zend::bailout::raise(frame.call_known(target, &mut argv));
    }
}

macro_rules! wrappers {
    ($($handler:ident => $function:literal, $position:literal;)*) => {
        /// The `SW_CREATE_PHP_FUNCTION_WRAPPER` functions' bodies.
        mod wrapped {
            use super::{Call, Wrapper};
            $(
                pub(super) fn $handler(frame: &Call) {
                    const WRAPPER: Wrapper = Wrapper {
                        name: $function,
                        position: $position,
                    };
                    WRAPPER.forward(frame);
                }
            )*
        }
        swoole_rs_zend::handlers! {
            leaf: $($handler => wrapped::$handler;)*
        }
    };
}

wrappers! {
    zif_swoole_array_search => c"array_search", 1;
    zif_swoole_array_contains => c"in_array", 1;
    zif_swoole_array_join => c"implode", 1;
    zif_swoole_array_key_exists => c"array_key_exists", 1;
    zif_swoole_array_map => c"array_map", 1;
    zif_swoole_array_replace_str => c"str_replace", 2;
    zif_swoole_array_ireplace_str => c"str_ireplace", 2;
    zif_swoole_str_split => c"explode", 1;
    zif_swoole_hash => c"hash", 1;
    zif_swoole_str_replace => c"str_replace", 2;
    zif_swoole_str_ireplace => c"str_ireplace", 2;
}

macro_rules! dispatchers {
    ($($handler:ident => $receiver:expr;)*) => {$(
        /// A method dispatcher the `INIT_METHOD_CALL` handler pushes.
        /// # Safety
        /// Zend's handler contract, in a frame that handler pushed.
        #[unsafe(no_mangle)]
        pub(crate) unsafe extern "C" fn $handler(frame: *mut zend_execute_data, value: *mut zval) {
            // SAFETY: per the caller.
            unsafe { call_method($receiver, frame, value) };
        }
    )*};
}

dispatchers! {
    zif_swoole_call_array_method => Receiver::Array;
    zif_swoole_call_string_method => Receiver::String;
    zif_swoole_call_stream_method => Receiver::Stream;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn moved(count: usize, position: usize) -> Vec<i64> {
        let mut src = vec![unsafe { std::mem::zeroed::<zval>() }; count];
        for (index, slot) in src.iter_mut().enumerate() {
            slot.value.lval = i64::try_from(index).unwrap();
        }
        move_first(&mut src, position);
        src.iter().map(|slot| unsafe { slot.value.lval }).collect()
    }

    #[test]
    fn first_argument_moves_to_its_position() {
        assert_eq!(moved(3, 1), [1, 0, 2]);
        assert_eq!(moved(4, 2), [1, 2, 0, 3]);
        assert_eq!(moved(3, 2), [1, 2, 0]);
        assert_eq!(moved(1, 1), [0], "too few arguments: to the end");
        assert_eq!(moved(2, 2), [1, 0]);
        assert_eq!(moved(0, 1), Vec::<i64>::new());
    }
}
