//! Methods on arrays, strings and streams: the `INIT_METHOD_CALL` handler
//! routes `$value->name(...)` to `swoole_call_{array,string,stream}_method`,
//! which calls the function the method names with `$value` first; and the
//! `swoole_*` wrappers that move that first argument where the function
//! expects it.
use crate::{
    CONTINUE, DISPATCH, Frame,
    aliases::{self, Receiver},
    offset,
};
use std::{
    ffi::{CStr, c_int},
    mem::offset_of,
    ptr, slice,
    sync::atomic::{AtomicPtr, Ordering},
};
use swoole_rs_zend::{
    bailout::{Bailout, trap},
    string,
    sys::{
        IS_ARRAY, IS_RESOURCE, IS_STRING, IS_VAR, swr_zend_compiled_function_find, swr_zend_efree,
        swr_zend_emalloc, swr_zend_function_find, swr_zend_push_nested_call,
        swr_zend_try_call_known, swr_zend_zval_release, zend_execute_data, zend_function,
        zend_throw_error, zend_zval_type_name, zval,
    },
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
pub unsafe extern "C" fn swr_v1_phpstdext_method_call(frame: *mut zend_execute_data) -> c_int {
    // SAFETY: per Zend's contract.
    let frame = unsafe { Frame::new(frame) };
    let op = frame.op();
    let Some(object) = op.op1() else {
        return DISPATCH;
    };
    let receiver = match object.dereferenced().kind() {
        IS_ARRAY => Receiver::Array,
        IS_STRING => Receiver::String,
        IS_RESOURCE => Receiver::Stream,
        _ => return DISPATCH,
    };
    // SAFETY: Zend's request allocator, freed by the method call.
    let info = unsafe { swr_zend_emalloc(size_of::<CallInfo>()) }.cast::<CallInfo>();
    // The method name: op2's literal, `RT_CONSTANT(opline, opline->op2)` as upstream reads it.
    let name = op
        .at
        .wrapping_byte_offset(offset(op.op2).cast_signed() as isize)
        .cast::<zval>();
    // SAFETY: a live literal; zvals are plain copies, as upstream's are.
    let func = unsafe { name.read() };
    // SAFETY: the operand's live zval.
    let this = unsafe { object.as_ptr().read() };
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
    // SAFETY: a registered function; the frame passes `object` as upstream does.
    let call =
        unsafe { swr_zend_push_nested_call(method, op.extended_value, object.as_ptr().cast()) };
    // SAFETY: the frame just pushed.
    unsafe { (*call).run_time_cache = info.cast() };
    // SAFETY: this live frame.
    let previous = unsafe { (*frame.as_ptr()).call };
    // SAFETY: the frame just pushed.
    unsafe { (*call).prev_execute_data = previous };
    // SAFETY: this live frame.
    unsafe { (*frame.as_ptr()).call = call };
    frame.jump(op.at.wrapping_add(1));
    CONTINUE
}

/// `ZEND_CALL_FRAME_SLOT`: zvals before a frame's first argument.
const FRAME_SLOT: usize = size_of::<zend_execute_data>().div_ceil(size_of::<zval>());

/// `ZEND_CALL_ARG(frame, 1)` through `ZEND_CALL_NUM_ARGS(frame)`.
/// # Safety
/// `frame` is a live call frame, which outlives `'a`.
unsafe fn arguments<'a>(frame: *mut zend_execute_data) -> &'a [zval] {
    // SAFETY: per the caller.
    let this = unsafe { (*frame).This };
    // SAFETY: a call frame's `This.u2` holds the argument count.
    let count = unsafe { this.u2.num_args } as usize;
    let first = frame.cast::<zval>().wrapping_add(FRAME_SLOT);
    // SAFETY: the frame's arguments follow its header.
    unsafe { slice::from_raw_parts(first, count) }
}

/// `zend_call_known_function(function, ..., retval, count, argv)` under
/// `zend_try`.
/// # Safety
/// `function` is live and `value` a writable return slot.
unsafe fn call(
    function: *mut zend_function,
    value: *mut zval,
    argv: &mut [zval],
) -> Result<(), Bailout> {
    // A frame's argument count plus one at most.
    let count = u32::try_from(argv.len()).unwrap_or(u32::MAX);
    // SAFETY: per the caller; Zend copies the arguments and keeps no pointer to `argv`.
    let bailed = unsafe { swr_zend_try_call_known(function, value, count, argv.as_mut_ptr()) };
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
    swoole_rs_zend::bailout::raise(outcome);
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
    // SAFETY: per the caller, the handler's `CallInfo`; a plain copy, as upstream's.
    let this = unsafe { this_of(info).read() };
    // SAFETY: a live frame.
    let mut argv = [slice::from_ref(&this), unsafe { arguments(frame) }].concat();
    // SAFETY: a live function and the caller's return slot.
    unsafe { call(function, value, &mut argv) }?;
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
fn move_first(args: &[zval], position: usize) -> Vec<zval> {
    let Some((first, rest)) = args.split_first() else {
        return Vec::new();
    };
    let (before, after) = rest.split_at(position.min(rest.len()));
    [before, slice::from_ref(first), after].concat()
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
    let mut argv = move_first(unsafe { arguments(frame) }, position);
    // SAFETY: a registered core function and the caller's return slot.
    swoole_rs_zend::bailout::raise(unsafe { call(target, value, &mut argv) });
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
        move_first(&src, position)
            .iter()
            .map(|slot| unsafe { slot.value.lval })
            .collect()
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
