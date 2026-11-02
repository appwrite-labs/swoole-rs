//! Methods on arrays, strings and streams: the `INIT_METHOD_CALL` handler
//! routes `$value->name(...)` to `swoole_call_{array,string,stream}_method`,
//! which calls the function the method names with `$value` first; and the
//! `swoole_*` wrappers that move that first argument where the function
//! expects it.
use crate::{
    CONTINUE, DISPATCH,
    aliases::{self, Receiver},
    deref, ex_var, raise, rt_constant,
};
use std::{
    ffi::{CStr, c_int},
    mem::offset_of,
    ptr,
    sync::atomic::{AtomicPtr, Ordering},
};
use swoole_rs_zend::{
    bailout::{Bailout, trap},
    string,
    sys::{
        IS_ARRAY, IS_CONST, IS_RESOURCE, IS_STRING, IS_UNUSED, IS_VAR,
        swr_zend_compiled_function_find, swr_zend_efree, swr_zend_emalloc, swr_zend_function_find,
        swr_zend_push_nested_call, swr_zend_try_call_known, swr_zend_zval_release,
        zend_execute_data, zend_function, zend_throw_error, zend_zval_type_name, zval,
    },
    type_of,
    value::str_of,
};

/// Upstream's `CallInfo`: what the handler hands the method call's frame in
/// `run_time_cache`.
#[repr(C)]
struct CallInfo {
    func: zval,
    this: zval,
    op1_type: u8,
}

/// `swoole_call_{array,string,stream}_method`, by `Receiver`.
static METHODS: [AtomicPtr<zend_function>; 3] = [const { AtomicPtr::new(ptr::null_mut()) }; 3];

/// Look up the method dispatchers (MINIT).
/// # Safety
/// At MINIT.
pub(crate) unsafe fn install() {
    for (slot, name) in METHODS.iter().zip([
        c"swoole_call_array_method",
        c"swoole_call_string_method",
        c"swoole_call_stream_method",
    ]) {
        slot.store(compiled_function(name), Ordering::Relaxed);
    }
}

/// `zend::get_function(CG(function_table), name)`.
pub(crate) fn compiled_function(name: &CStr) -> *mut zend_function {
    // SAFETY: Zend copies nothing and reads `count_bytes()` bytes.
    unsafe { swr_zend_compiled_function_find(name.as_ptr(), name.count_bytes()) }
}

/// The `INIT_METHOD_CALL` user opcode handler.
/// # Safety
/// Zend's user opcode handler contract: a live frame at this opline.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_phpstdext_method_call(frame: *mut zend_execute_data) -> c_int {
    // SAFETY: a live frame.
    let opline = unsafe { (*frame).opline };
    // SAFETY: the frame's current opline.
    let op = unsafe { &*opline };
    let object = match u32::from(op.op1_type) {
        // SAFETY: a constant operand.
        IS_CONST => unsafe { rt_constant(opline, op.op1) },
        IS_UNUSED => return DISPATCH,
        // SAFETY: a variable operand holds its slot.
        _ => ex_var(frame, unsafe { op.op1.var }),
    };
    // SAFETY: the operand's live zval.
    let target = unsafe { deref(object) };
    // SAFETY: as above.
    let receiver = match unsafe { type_of(target) } {
        IS_ARRAY => Receiver::Array,
        IS_STRING => Receiver::String,
        IS_RESOURCE => Receiver::Stream,
        _ => return DISPATCH,
    };
    // SAFETY: Zend's request allocator, freed by the method call.
    let info = unsafe { swr_zend_emalloc(size_of::<CallInfo>()) }.cast::<CallInfo>();
    // SAFETY: the method name, op2's constant.
    let name = unsafe { rt_constant(opline, op.op2) };
    // SAFETY: a live literal; zvals are plain copies, as upstream's are.
    let func = unsafe { *name };
    // SAFETY: the operand's live zval.
    let this = unsafe { *object };
    // SAFETY: a fresh allocation of a `CallInfo`.
    unsafe {
        info.write(CallInfo {
            func,
            this,
            op1_type: op.op1_type,
        });
    }
    let method = METHODS[receiver as usize].load(Ordering::Relaxed);
    // SAFETY: a registered function; the frame passes `object` as upstream does.
    let call = unsafe { swr_zend_push_nested_call(method, op.extended_value, object.cast()) };
    // SAFETY: the frame just pushed.
    unsafe { (*call).run_time_cache = info.cast() };
    // SAFETY: this live frame.
    let previous = unsafe { (*frame).call };
    // SAFETY: the frame just pushed.
    unsafe { (*call).prev_execute_data = previous };
    // SAFETY: this live frame.
    unsafe { (*frame).call = call };
    // SAFETY: as above.
    unsafe { (*frame).opline = opline.wrapping_add(1) };
    CONTINUE
}

/// `ZEND_CALL_FRAME_SLOT`: zvals before a frame's first argument.
const FRAME_SLOT: usize = size_of::<zend_execute_data>().div_ceil(size_of::<zval>());

/// `ZEND_CALL_ARG(frame, 1)` and `ZEND_CALL_NUM_ARGS(frame)`.
/// # Safety
/// `frame` is a live call frame.
pub(crate) unsafe fn arguments(frame: *mut zend_execute_data) -> (*mut zval, usize) {
    // SAFETY: per the caller.
    let this = unsafe { (*frame).This };
    // SAFETY: a call frame's `This.u2` holds the argument count.
    let count = unsafe { this.u2.num_args } as usize;
    (frame.cast::<zval>().wrapping_add(FRAME_SLOT), count)
}

/// `ecalloc(count, sizeof(zval))`.
fn zvals(count: usize) -> *mut zval {
    // SAFETY: Zend's request allocator; the caller frees it.
    unsafe { swr_zend_emalloc(count * size_of::<zval>()) }.cast()
}

/// `zend_call_known_function(function, ..., retval, count, argv)` under
/// `zend_try`, then free `argv`.
/// # Safety
/// `function` is live; `argv` holds `count` zvals from [`zvals`].
unsafe fn call(
    function: *mut zend_function,
    value: *mut zval,
    argv: *mut zval,
    count: usize,
) -> Result<(), Bailout> {
    // A frame's argument count plus one at most.
    let count = u32::try_from(count).unwrap_or(u32::MAX);
    // SAFETY: per the caller.
    let bailed = unsafe { swr_zend_try_call_known(function, value, count, argv) };
    // SAFETY: allocated by `zvals`; the callee keeps no pointer to it.
    unsafe { swr_zend_efree(argv.cast()) };
    if bailed != 0 { Err(Bailout) } else { Ok(()) }
}

/// Upstream's `call_method`: call the function `receiver`'s method names with
/// `$this` first.
/// # Safety
/// A frame the `INIT_METHOD_CALL` handler pushed.
unsafe fn call_method(receiver: Receiver, frame: *mut zend_execute_data, value: *mut zval) {
    // SAFETY: the handler stored its `CallInfo` here.
    let info = unsafe { (*frame).run_time_cache }.cast::<CallInfo>();
    // SAFETY: op2 of INIT_METHOD_CALL is the method name, a string literal.
    let name = unsafe { str_of(info.wrapping_byte_add(offset_of!(CallInfo, func)).cast()) };
    // SAFETY: a live literal.
    let alias = aliases::find(receiver, unsafe { string::bytes(name) });
    let outcome = match alias {
        None => {
            // SAFETY: the receiver's live zval.
            let kind = unsafe { zend_zval_type_name(this_of(info)) };
            // SAFETY: printf arguments: two NUL-terminated strings.
            unsafe {
                zend_throw_error(
                    ptr::null_mut(),
                    c"The method `%s` is undefined on %s".as_ptr(),
                    string::val(name),
                    kind,
                );
            }
            Ok(())
        }
        // SAFETY: as above.
        Some(alias) => unsafe { call_alias(alias, info, frame, value) },
    };
    // SAFETY: the handler's allocation; nothing else holds it.
    unsafe { swr_zend_efree(info.cast()) };
    // SAFETY: a live frame.
    unsafe { (*frame).run_time_cache = ptr::null_mut() };
    raise(&outcome);
}

/// # Safety
/// As [`call_method`]; `info` is its `CallInfo`.
unsafe fn call_alias(
    alias: &CStr,
    info: *mut CallInfo,
    frame: *mut zend_execute_data,
    value: *mut zval,
) -> Result<(), Bailout> {
    // SAFETY: Zend reads `count_bytes()` bytes.
    let function = unsafe { swr_zend_function_find(alias.as_ptr(), alias.count_bytes()) };
    if function.is_null() {
        // SAFETY: a printf format with one NUL-terminated string.
        unsafe {
            zend_throw_error(
                ptr::null_mut(),
                c"The function `%s` is undefined".as_ptr(),
                alias.as_ptr(),
            );
        }
        return Ok(());
    }
    // SAFETY: a live frame.
    let (args, count) = unsafe { arguments(frame) };
    let argv = zvals(count + 1);
    // SAFETY: `argv` holds `count + 1` zvals; plain copies, as upstream's.
    unsafe { ptr::copy_nonoverlapping(this_of(info), argv, 1) };
    // SAFETY: the frame's `count` arguments into the rest of `argv`.
    unsafe { ptr::copy_nonoverlapping(args, argv.wrapping_add(1), count) };
    // SAFETY: a live function and `argv` from `zvals`.
    unsafe { call(function, value, argv, count + 1) }?;
    // SAFETY: the handler's copy of op1's type.
    if u32::from(unsafe { (*info).op1_type }) == IS_VAR {
        // SAFETY: a C body releasing the receiver, which may run destructors.
        unsafe { trap(swr_zend_zval_release, this_of(info).cast()) }?;
    }
    Ok(())
}

/// The `this` field of a `CallInfo`.
fn this_of(info: *mut CallInfo) -> *mut zval {
    info.wrapping_byte_add(offset_of!(CallInfo, this)).cast()
}

/// Upstream's `move_first_element`: the first argument moves to `position`.
/// With `position` or fewer arguments the first argument moves to the end, and
/// the target function reports the missing arguments itself.
/// # Safety
/// `src` and `dst` hold `count` zvals.
unsafe fn move_first(src: *const zval, dst: *mut zval, count: usize, position: usize) {
    let Some(last) = count.checked_sub(1) else {
        return;
    };
    let position = position.min(last);
    // SAFETY: per the caller, `position + 1 <= count`.
    unsafe { ptr::copy_nonoverlapping(src.wrapping_add(1), dst, position) };
    // SAFETY: as above.
    unsafe { ptr::copy_nonoverlapping(src, dst.wrapping_add(position), 1) };
    // SAFETY: as above.
    unsafe {
        ptr::copy_nonoverlapping(
            src.wrapping_add(position + 1),
            dst.wrapping_add(position + 1),
            last - position,
        );
    }
}

/// `SW_CREATE_PHP_FUNCTION_WRAPPER`: call `name` with the first argument moved
/// to `position`.
/// # Safety
/// A live internal function frame.
unsafe fn forward(
    function: &AtomicPtr<zend_function>,
    name: &CStr,
    position: usize,
    frame: *mut zend_execute_data,
    value: *mut zval,
) {
    let mut target = function.load(Ordering::Relaxed);
    if target.is_null() {
        target = compiled_function(name);
        function.store(target, Ordering::Relaxed);
    }
    // SAFETY: a live frame.
    let (args, count) = unsafe { arguments(frame) };
    let argv = zvals(count);
    // SAFETY: both hold `count` zvals.
    unsafe { move_first(args, argv, count, position) };
    // SAFETY: a registered core function and `argv` from `zvals`.
    raise(&unsafe { call(target, value, argv, count) });
}

macro_rules! wrappers {
    ($($handler:ident => $function:literal, $position:literal;)*) => {$(
        /// A `SW_CREATE_PHP_FUNCTION_WRAPPER` function.
        /// # Safety
        /// Zend's handler contract.
        #[unsafe(no_mangle)]
        pub(crate) unsafe extern "C" fn $handler(frame: *mut zend_execute_data, value: *mut zval) {
            static FUNCTION: AtomicPtr<zend_function> = AtomicPtr::new(ptr::null_mut());
            // SAFETY: per Zend's handler contract.
            unsafe { forward(&FUNCTION, $function, $position, frame, value) };
        }
    )*};
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
        let mut dst = vec![unsafe { std::mem::zeroed::<zval>() }; count];
        // SAFETY: both hold `count` zvals.
        unsafe { move_first(src.as_ptr(), dst.as_mut_ptr(), count, position) };
        dst.iter().map(|slot| unsafe { slot.value.lval }).collect()
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
