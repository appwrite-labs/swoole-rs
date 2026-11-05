//! PHP `Swoole\Coroutine\Channel`: class registration, object handlers and every
//! method. The queue is the coroutine module's C++ `Channel`, which C++ hands
//! over as a table of operations; values cross it as emalloc'd zval copies.
use std::{ffi::c_void, ptr, sync::OnceLock};
use swoole_rs_phpstubs::channel_coro;
use swoole_rs_zend::{
    Call, Class, Object, register_long_constant,
    sys::{
        ZEND_ACC_NOT_SERIALIZABLE, swr_zend_efree, swr_zend_emalloc, swr_zval_try_addref,
        swr_zval_try_delref, zend_class_entry, zend_object, zend_object_handlers,
        zend_objects_destroy_object, zval, zval_ptr_dtor,
    },
    unset_property_deny,
};
use swoole_rs_zend::{method_table, sys as z};

/// `swoole::coroutine::Channel`'s operations (`swr_v1_phpcollections_channel_ops`).
#[repr(C)]
pub struct Ops {
    create: unsafe extern "C" fn(capacity: usize) -> *mut c_void,
    destroy: unsafe extern "C" fn(chan: *mut c_void),
    push: unsafe extern "C" fn(chan: *mut c_void, data: *mut c_void, timeout: f64) -> u8,
    pop: unsafe extern "C" fn(chan: *mut c_void, timeout: f64) -> *mut c_void,
    pop_data: unsafe extern "C" fn(chan: *mut c_void) -> *mut c_void,
    error: unsafe extern "C" fn(chan: *mut c_void) -> i32,
    close: unsafe extern "C" fn(chan: *mut c_void) -> u8,
    length: unsafe extern "C" fn(chan: *mut c_void) -> usize,
    is_empty: unsafe extern "C" fn(chan: *mut c_void) -> u8,
    is_full: unsafe extern "C" fn(chan: *mut c_void) -> u8,
    consumers: unsafe extern "C" fn(chan: *mut c_void) -> usize,
    producers: unsafe extern "C" fn(chan: *mut c_void) -> usize,
    /// `swoole_fatal_error(SW_ERROR_WRONG_OPERATION, "must call constructor first")`.
    unconstructed: unsafe extern "C" fn(),
}

/// `Channel::ErrorCode`.
const ERROR_OK: i64 = 0;
const CONSTANTS: [(&std::ffi::CStr, i64); 4] = [
    (c"SWOOLE_CHANNEL_OK", ERROR_OK),
    (c"SWOOLE_CHANNEL_TIMEOUT", -1),
    (c"SWOOLE_CHANNEL_CLOSED", -2),
    (c"SWOOLE_CHANNEL_CANCELED", -3),
];

static OPS: OnceLock<&'static Ops> = OnceLock::new();
static CHANNEL: Class = Class::new();

/// `struct ChannelObject { Channel *chan; zend_object std; }`.
type ChannelObject = Object<*mut c_void>;

fn ops() -> &'static Ops {
    // Methods and handlers run only after MINIT stored the table.
    OPS.get().copied().unwrap_or_else(|| std::process::abort())
}

unsafe extern "C" fn create(ce: *mut zend_class_entry) -> *mut zend_object {
    // SAFETY: Zend calls `create_object` with this class or a subclass of it;
    // a null channel is valid zeroed data.
    unsafe { ChannelObject::create(&CHANNEL, ce) }
}

/// `channel_coro_dtor_object`: run `__destruct`, then free every queued value
/// and the channel.
unsafe extern "C" fn dtor(std: *mut zend_object) {
    // SAFETY: Zend destroys a live object of this class.
    unsafe { zend_objects_destroy_object(std) };
    // SAFETY: as above.
    let chan = unsafe { ChannelObject::data(std) };
    if chan.is_null() {
        return;
    }
    let ops = ops();
    loop {
        // SAFETY: a live channel.
        let data = unsafe { (ops.pop_data)(*chan) }.cast::<zval>();
        if data.is_null() {
            break;
        }
        // SAFETY: `push` queued an emalloc'd zval holding one reference
        // (`sw_zval_free`).
        unsafe { zval_ptr_dtor(data) };
        // SAFETY: as above.
        unsafe { swr_zend_efree(data.cast()) };
    }
    // SAFETY: the channel `__construct` made; nothing else holds it.
    unsafe { (ops.destroy)(*chan) };
    *chan = ptr::null_mut();
}

unsafe extern "C" fn free(std: *mut zend_object) {
    // SAFETY: Zend frees an object this class created.
    let chan = unsafe { *ChannelObject::data(std) };
    if !chan.is_null() {
        // SAFETY: as in `dtor`, when the destructor did not run.
        unsafe { (ops().destroy)(chan) };
    }
    // SAFETY: as above.
    unsafe { ChannelObject::dtor(std) };
}

method_table! {
    static METHODS = [
        c"__construct" => zim_swoole_channel_coro___construct, &channel_coro::arginfo_class_Swoole_Coroutine_Channel___construct, z::ZEND_ACC_PUBLIC;
        c"push" => zim_swoole_channel_coro_push, &channel_coro::arginfo_class_Swoole_Coroutine_Channel_push, z::ZEND_ACC_PUBLIC;
        c"pop" => zim_swoole_channel_coro_pop, &channel_coro::arginfo_class_Swoole_Coroutine_Channel_pop, z::ZEND_ACC_PUBLIC;
        c"isEmpty" => zim_swoole_channel_coro_isEmpty, &channel_coro::arginfo_class_Swoole_Coroutine_Channel_isEmpty, z::ZEND_ACC_PUBLIC;
        c"isFull" => zim_swoole_channel_coro_isFull, &channel_coro::arginfo_class_Swoole_Coroutine_Channel_isFull, z::ZEND_ACC_PUBLIC;
        c"close" => zim_swoole_channel_coro_close, &channel_coro::arginfo_class_Swoole_Coroutine_Channel_close, z::ZEND_ACC_PUBLIC;
        c"stats" => zim_swoole_channel_coro_stats, &channel_coro::arginfo_class_Swoole_Coroutine_Channel_stats, z::ZEND_ACC_PUBLIC;
        c"length" => zim_swoole_channel_coro_length, &channel_coro::arginfo_class_Swoole_Coroutine_Channel_length, z::ZEND_ACC_PUBLIC;
    ];
}

/// MINIT: register the class, its properties and constants as
/// `php_swoole_channel_coro_minit` did, with `Co\Channel` and `Chan` when
/// short names are on.
/// # Safety
/// Called once, at MINIT, with a static operations table.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_phpcollections_channel_minit(
    module_number: i32,
    use_shortname: u8,
    ops: *const Ops,
) {
    // SAFETY: the native MINIT entry lends these original startup arguments.
    let result = unsafe { init_phpcollections_channel_minit(module_number, use_shortname, ops) };
    match result {
        Ok(value) => value,
        Err(error) => {
            error.defer();
            swoole_rs_zend::bailout::finish();
        }
    }
}

unsafe fn init_phpcollections_channel_minit(
    module_number: i32,
    use_shortname: u8,
    ops: *const Ops,
) -> Result<(), swoole_rs_zend::bailout::Bailout> {
    // SAFETY: per the caller, a static table.
    if let Some(ops) = unsafe { ops.as_ref() } {
        OPS.get_or_init(|| ops);
    }
    // SAFETY: MINIT, with a static table.
    unsafe { CHANNEL.register(c"Swoole\\Coroutine\\Channel", METHODS.as_ptr(), configure) };
    if use_shortname != 0 {
        // SAFETY: MINIT, after registration.
        unsafe { CHANNEL.alias(c"Co\\Channel") }?;
        // SAFETY: as above.
        unsafe { CHANNEL.alias(c"Chan") }?;
    }
    // SAFETY: as above.
    unsafe { CHANNEL.property_long(c"capacity", 0) };
    // SAFETY: as above.
    unsafe { CHANNEL.property_long(c"errCode", 0) };
    for (name, value) in CONSTANTS {
        // SAFETY: MINIT, with the module's number.
        unsafe { register_long_constant(name, value, module_number) };
    }

    Ok(())
}

/// Not serializable, clone and unset denied, custom object and destructor.
fn configure(entry: &mut zend_class_entry, handlers: &mut zend_object_handlers) {
    entry.ce_flags |= ZEND_ACC_NOT_SERIALIZABLE;
    handlers.clone_obj = None;
    handlers.unset_property = Some(unset_property_deny);
    ChannelObject::install(entry, handlers, create, free);
    handlers.dtor_obj = Some(dtor);
}

fn this(call: &Call) -> &'static mut *mut c_void {
    // SAFETY: Channel methods are non-static, so `$this` is one of our objects.
    unsafe { ChannelObject::data(call.this()) }
}

/// `channel_coro_get_ptr`: the channel, or Swoole's fatal error (which exits).
fn channel(call: &Call) -> Option<*mut c_void> {
    let chan = *this(call);
    if chan.is_null() {
        // SAFETY: C++'s fatal-error report.
        unsafe { (ops().unconstructed)() };
        return None;
    }
    Some(chan)
}

/// `$this->errCode = $code`, as the class sees it.
fn set_error(call: &Call, code: i64) {
    call.update_property_long(Some(&CHANNEL), c"errCode", code);
}

/// `__construct(int $size = 1)`: capacity below 1 becomes 1. Upstream neither
/// rejects a second call nor frees the channel it replaces.
fn construct(call: &Call) {
    let Ok(capacity) = call.arity(0, 1).and_then(|()| call.long(1, 1)) else {
        call.set_bool(false);
        return;
    };
    let capacity = capacity.max(1);
    let size = usize::try_from(capacity).unwrap_or(usize::MAX);
    // SAFETY: the C++ constructor.
    let chan = unsafe { (ops().create)(size) };
    *this(call) = chan;
    call.update_property_long(Some(&CHANNEL), c"capacity", capacity);
}

/// `push(mixed $data, float $timeout = -1): bool`: the channel holds a new
/// reference in an emalloc'd copy of the zval (`sw_zval_dup`).
fn push(call: &Call) {
    let Some(chan) = channel(call) else { return };
    let Ok(timeout) = call.arity(1, 2).and_then(|()| call.double(2, -1.0)) else {
        call.set_bool(false);
        return;
    };
    let data = call.zval(1);
    // SAFETY: argument 1 is passed and live.
    unsafe { swr_zval_try_addref(data) };
    // SAFETY: Zend's allocator, for one zval.
    let copy = unsafe { swr_zend_emalloc(size_of::<zval>()) }.cast::<zval>();
    // SAFETY: `data` is live.
    let value = unsafe { data.read() };
    // SAFETY: `copy` is fresh; a bitwise copy, as upstream's memcpy.
    unsafe { copy.write(value) };
    let ops = ops();
    // SAFETY: a live channel; may suspend this coroutine.
    if unsafe { (ops.push)(chan, copy.cast(), timeout) } != 0 {
        set_error(call, ERROR_OK);
        call.set_bool(true);
        return;
    }
    // SAFETY: a live channel.
    set_error(call, unsafe { (ops.error)(chan) }.into());
    // SAFETY: the channel refused `copy`, so its reference is ours to drop.
    unsafe { swr_zval_try_delref(copy) };
    // SAFETY: as above.
    unsafe { swr_zend_efree(copy.cast()) };
    call.set_bool(false);
}

/// `pop(float $timeout = -1): mixed`: the result takes over the copy's reference.
fn pop(call: &Call) {
    let Some(chan) = channel(call) else { return };
    let Ok(timeout) = call.arity(0, 1).and_then(|()| call.double(1, -1.0)) else {
        call.set_bool(false);
        return;
    };
    let ops = ops();
    // SAFETY: a live channel; may suspend this coroutine.
    let data = unsafe { (ops.pop)(chan, timeout) }.cast::<zval>();
    if data.is_null() {
        // SAFETY: a live channel.
        set_error(call, unsafe { (ops.error)(chan) }.into());
        call.set_bool(false);
        return;
    }
    // SAFETY: a value `push` queued.
    call.set_value(unsafe { data.read() });
    // SAFETY: as above; the zval moved out.
    unsafe { swr_zend_efree(data.cast()) };
    set_error(call, ERROR_OK);
}

#[expect(clippy::cast_possible_wrap, reason = "queue sizes fit a zend_long")]
fn long(size: usize) -> i64 {
    size as i64
}

/// The methods that read the channel: `close(): bool`, `length(): int`,
/// `isEmpty(): bool`, `isFull(): bool` and `stats(): array`.
macro_rules! queries {
    ($($name:ident($ops:ident, $chan:ident, $call:ident) $body:block)*) => {$(
        fn $name($call: &Call) {
            if let Some($chan) = channel($call) {
                let $ops = ops();
                $body
            }
        }
    )*};
}

// Each body runs on a live channel.
queries! {
    close(ops, chan, call) {
        // SAFETY: see above.
        let closed = unsafe { (ops.close)(chan) };
        call.set_bool(closed != 0);
    }
    length(ops, chan, call) {
        // SAFETY: see above.
        let length = unsafe { (ops.length)(chan) };
        call.set_long(long(length));
    }
    is_empty(ops, chan, call) {
        // SAFETY: see above.
        let empty = unsafe { (ops.is_empty)(chan) };
        call.set_bool(empty != 0);
    }
    is_full(ops, chan, call) {
        // SAFETY: see above.
        let full = unsafe { (ops.is_full)(chan) };
        call.set_bool(full != 0);
    }
    stats(ops, chan, call) {
        call.set_array();
        // SAFETY: see above.
        call.add_assoc_long(c"consumer_num", long(unsafe { (ops.consumers)(chan) }));
        // SAFETY: see above.
        call.add_assoc_long(c"producer_num", long(unsafe { (ops.producers)(chan) }));
        // SAFETY: see above.
        call.add_assoc_long(c"queue_num", long(unsafe { (ops.length)(chan) }));
    }
}

swoole_rs_zend::handlers! {
    zim_swoole_channel_coro___construct => construct;
    zim_swoole_channel_coro_push => push;
    zim_swoole_channel_coro_pop => pop;
    zim_swoole_channel_coro_close => close;
    zim_swoole_channel_coro_length => length;
    zim_swoole_channel_coro_isEmpty => is_empty;
    zim_swoole_channel_coro_isFull => is_full;
    zim_swoole_channel_coro_stats => stats;
}
