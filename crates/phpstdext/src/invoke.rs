//! Methods on arrays, strings and streams: the `INIT_METHOD_CALL` handler
//! routes `$value->name(...)` to `swoole_call_{array,string,stream}_method`,
//! which calls the function the method names with `$value` first; and the
//! `swoole_*` wrappers that move that first argument where the function
//! expects it.
use crate::{
    Frame, Next, Op,
    aliases::{self, Receiver},
};
use std::{
    ffi::{CStr, c_void},
    ptr,
    sync::atomic::{AtomicPtr, Ordering},
};
use swoole_rs_zend::{
    Call, Emalloc, Owned, Value, Zvals,
    bailout::Bailout,
    opcode::Handler,
    sys::{
        IS_ARRAY, IS_CONST, IS_RESOURCE, IS_STRING, IS_VAR, swr_zend_zval_release, zend_function,
    },
};

unsafe extern "C" {
    /// `zend_vm_stack_push_call_frame(ZEND_CALL_NESTED_FUNCTION, function,
    /// num_args, object)`, which Zend always pushes, given `run_time_cache`
    /// and linked as `frame`'s next call (`EX(call)`).
    #[link_name = "swr_zend_push_nested_call"]
    fn push_nested_call(
        frame: Frame<'_>,
        function: *mut zend_function,
        num_args: u32,
        object: *mut c_void,
        run_time_cache: *mut c_void,
    );
}

/// Upstream's `CallInfo`: what the handler hands the method call's frame in
/// `run_time_cache`. Hold the method name and receiver until dispatch finishes.
/// The method also releases the original receiver reference for a `VAR` operand.
struct CallInfo {
    func: Owned,
    this: Owned,
    op1_type: u8,
}

/// The method dispatchers, by `Receiver`: copies of
/// `swoole_call_{array,string,stream}_method` that no function table names, so
/// the `INIT_METHOD_CALL` handler's frames are the only ones that run
/// [`call_method`].
static METHODS: [AtomicPtr<zend_function>; 3] = [const { AtomicPtr::new(ptr::null_mut()) }; 3];

/// Take the method dispatchers out of PHP's reach (MINIT): the handler keeps a
/// copy of each, and the function PHP code can name refuses a call.
pub(crate) fn install(minit: &swoole_rs_zend::Minit) {
    for (slot, name) in METHODS.iter().zip([
        c"swoole_call_array_method",
        c"swoole_call_string_method",
        c"swoole_call_stream_method",
    ]) {
        slot.store(minit.unnamed_copy(name), Ordering::Relaxed);
        _ = minit.replace_handler(name, zif_swoole_method_called_directly);
    }
}

/// A dispatcher called by name: it has no method call to run.
fn called_directly(frame: &Call) {
    frame.throw_error(c"A method dispatcher runs a method call on an array, string or stream; it cannot be called directly");
}

swoole_rs_zend::handlers! {
    zif_swoole_method_called_directly => called_directly;
}

/// The `INIT_METHOD_CALL` user opcode handler.
pub(crate) struct MethodCall;

impl Handler for MethodCall {
    const OPCODE: u32 = swoole_rs_zend::sys::ZEND_INIT_METHOD_CALL;
    fn run(frame: Frame<'_>) -> Next {
        let Some(op) = Op::current(frame) else {
            return Next::Dispatch;
        };
        let Some(object) = op.op1() else {
            return Next::Dispatch;
        };
        // The method name: op2's literal, `RT_CONSTANT(opline, opline->op2)`,
        // which lives as long as the script. A name computed at run time is
        // left to Zend.
        if u32::from(op.op2_type) != IS_CONST {
            return Next::Dispatch;
        }
        let Some(name) = op.op2() else {
            return Next::Dispatch;
        };
        let receiver = match object.dereferenced().kind() {
            IS_ARRAY => Receiver::Array,
            IS_STRING => Receiver::String,
            IS_RESOURCE => Receiver::Stream,
            _ => return Next::Dispatch,
        };
        // Zend's request allocator, freed by the method call. Keep valid values
        // across the separate opcode dispatch rather than unowned zval bits.
        let info = Box::into_raw_with_allocator(Box::new_in(
            CallInfo {
                func: name.copy(),
                this: object.copy(),
                op1_type: op.op1_type,
            },
            Emalloc::default(),
        ))
        .0;
        let method = match receiver {
            Receiver::Array => &METHODS[0],
            Receiver::String => &METHODS[1],
            Receiver::Stream => &METHODS[2],
        }
        .load(Ordering::Relaxed);
        // SAFETY: a registered function; the new frame passes `object` as
        // upstream does, and Zend runs it as this live frame's next call.
        unsafe {
            push_nested_call(
                frame,
                method,
                op.extended_value,
                object.as_ptr().cast(),
                info.cast(),
            );
        };
        frame.skip(1);
        Next::Continue
    }
}

/// `prefix`, then `ZEND_CALL_ARG(frame, 1)` through `ZEND_CALL_NUM_ARGS(frame)`:
/// held values, lent to Zend for the nested call.
fn arguments(prefix: &[&Value], frame: &Call) -> Vec<Owned> {
    let passed = (1..=frame.num_args())
        .filter_map(|n| frame.value(n))
        .map(Value::copy);
    prefix
        .iter()
        .map(|value| value.copy())
        .chain(passed)
        .collect()
}

/// Upstream's `call_method`: call the function `receiver`'s method names with
/// `$this` first, in the dispatcher's frame, which the `INIT_METHOD_CALL`
/// handler pushed with its `CallInfo`.
fn call_method(receiver: Receiver, frame: &Call) {
    let allocation = frame.take_run_time_cache().cast::<CallInfo>();
    // SAFETY: the handler's initialized allocation: only its frames run a
    // dispatcher (`install`). The frame's `run_time_cache` alone pointed to it,
    // until the take above, and it is freed below.
    let mut info = unsafe { Box::from_raw_in(allocation, Emalloc::default()) };
    // op2 of INIT_METHOD_CALL is the method name, a string literal.
    let name = info.func.str();
    let alias = name.and_then(|name| aliases::find(receiver, name.bytes()));
    let outcome = match alias {
        None => {
            let kind = info.this.type_name().to_bytes();
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
        Some(alias) => call_alias(alias, &mut info, frame),
    };
    drop(info);
    swoole_rs_zend::bailout::defer(outcome);
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
    // Keep the receiver and arguments live throughout the nested call.
    let argv = arguments(&[&info.this], frame);
    frame.call_known(function, Zvals::owned(&argv))?;
    if u32::from(info.op1_type) == IS_VAR {
        let receiver = info.this.as_ptr().cast();
        // SAFETY: the receiver's reference (`CallInfo::this`), released once,
        // inside the trap: its destructors may bail out.
        swoole_rs_zend::contain_with(|| unsafe { swr_zend_zval_release(receiver) })?;
    }
    Ok(())
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
        swoole_rs_corebase::utilities::move_first_argument(&mut argv, self.position);
        let outcome = frame.call_known(target, Zvals::owned(&argv));
        drop(argv);
        swoole_rs_zend::bailout::defer(outcome);
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
            $($handler => wrapped::$handler;)*
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

/// The method dispatchers the `INIT_METHOD_CALL` handler pushes.
fn array_method(frame: &Call) {
    call_method(Receiver::Array, frame);
}

fn string_method(frame: &Call) {
    call_method(Receiver::String, frame);
}

fn stream_method(frame: &Call) {
    call_method(Receiver::Stream, frame);
}

swoole_rs_zend::handlers! {
    zif_swoole_call_array_method => array_method;
    zif_swoole_call_string_method => string_method;
    zif_swoole_call_stream_method => stream_method;
}
