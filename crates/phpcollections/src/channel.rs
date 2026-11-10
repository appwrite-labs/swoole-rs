//! PHP `Swoole\Coroutine\Channel`: class registration, object handlers and every
//! method. The queue is the coroutine crate's Channel, run on PHP's coroutines;
//! values cross it as emalloc'd zval copies.
use std::{ffi::c_void, ptr};
use swoole_rs_coroutine::chan::{self as queue, CANCELED, Channel, Effects, TIMEDOUT};
use swoole_rs_phpcoroutines as co;
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

/// `Channel::ErrorCode`.
const ERROR_OK: i64 = 0;
const CONSTANTS: [(&std::ffi::CStr, i64); 4] = [
    (c"SWOOLE_CHANNEL_OK", ERROR_OK),
    (c"SWOOLE_CHANNEL_TIMEOUT", -1),
    (c"SWOOLE_CHANNEL_CLOSED", -2),
    (c"SWOOLE_CHANNEL_CANCELED", -3),
];
const SW_ERROR_WRONG_OPERATION: i32 = 509;
const SW_ERROR_CO_HAS_BEEN_DISCARDED: i32 = 10003;
const SW_LOG_WARNING: i32 = 4;

static CHANNEL: Class = Class::new();

/// `struct ChannelObject { Channel *chan; zend_object std; }`.
type ChannelObject = Object<*mut Channel>;

/// One coroutine's wait, on its own stack while it is suspended: upstream's
/// `TimeoutMessage`, which the timer and the cancel function reach by pointer.
/// Under thread contexts they run on another thread than the waiting coroutine.
struct Wait {
    ch: *mut Channel,
    role: u32,
    co: *mut c_void,
    timer: *mut c_void,
    timedout: bool,
}

/// `timer_callback` and the cancel function: remove the wait from its queue
/// and resume it; a timeout also marks it timed out.
///
/// # Safety
/// `wait` is a suspended coroutine's [`Wait`].
unsafe fn wake(wait: *mut c_void, timeout: bool) {
    // SAFETY: the suspended coroutine's frame, live until it resumes.
    let w = unsafe { &mut *wait.cast::<Wait>() };
    if timeout {
        w.timedout = true;
        w.timer = ptr::null_mut();
    }
    // SAFETY: a channel outlives its waiters.
    unsafe { (*w.ch).remove(w.role, w.co) };
    let parked = w.co;
    // SAFETY: a parked coroutine; this frame owns nothing that needs a drop.
    unsafe { co::wake(parked, false) };
}

unsafe extern "C-unwind" fn on_timeout(wait: *mut c_void) {
    // SAFETY: the wait deletes its timer before it ends.
    unsafe { wake(wait, true) };
}

unsafe extern "C-unwind" fn on_cancel(_: *mut c_void, wait: *mut c_void) -> bool {
    // SAFETY: a cancel function is reachable only while its wait is suspended.
    unsafe { wake(wait, false) };
    true
}

/// PHP's coroutines for one operation; `co` is null for close.
struct Php {
    co: *mut c_void,
    timeout: f64,
}

impl Php {
    /// `Coroutine::get_current_safe()`: a fatal error outside a coroutine.
    fn current(timeout: f64) -> Self {
        Self {
            co: co::current_safe(),
            timeout,
        }
    }
}

impl Effects for Php {
    fn current(&self) -> *mut c_void {
        self.co
    }

    unsafe fn wait(&mut self, ch: *mut Channel, role: u32) -> u32 {
        let mut state = Wait {
            ch,
            role,
            co: self.co,
            timer: ptr::null_mut(),
            timedout: false,
        };
        // Only through this pointer from here on: the callbacks write it too.
        let wait = &raw mut state;
        if self.timeout > 0.0 {
            // SAFETY: on_timeout takes this frame's wait, which deletes the timer before it ends.
            let timer = unsafe { co::timer_add(self.timeout, wait.cast(), on_timeout) };
            // SAFETY: this frame's wait.
            unsafe { (*wait).timer = timer };
        }
        // SAFETY: the running coroutine; this frame owns nothing that needs a drop.
        unsafe { co::yield_cancelable(self.co, on_cancel, wait.cast()) };
        // SAFETY: as above, and no callback can reach it any more.
        let Wait {
            timer, timedout, ..
        } = unsafe { wait.read() };
        if !timer.is_null() {
            // SAFETY: still armed: the timeout would have cleared it.
            unsafe { co::timer_del(timer) };
        }
        (if co::is_canceled(self.co) {
            CANCELED
        } else {
            0
        }) | (if timedout { TIMEDOUT } else { 0 })
    }

    fn resume(&mut self, waiter: *mut c_void) {
        // SAFETY: a waiter just taken from a queue; this frame owns nothing that needs a drop.
        unsafe { co::wake(waiter, false) };
    }
}

/// `Channel::~Channel`: warns about the coroutines still waiting, then frees it.
fn destroy(chan: *mut Channel) {
    // SAFETY: `__construct` made it; nothing else holds it.
    let chan = unsafe { Box::from_raw(chan) };
    for (count, role) in [
        (chan.producer_num(), "producers"),
        (chan.consumer_num(), "consumers"),
    ] {
        if count > 0 {
            swoole_rs_corebase::log::error_log(
                SW_LOG_WARNING,
                SW_ERROR_CO_HAS_BEEN_DISCARDED,
                c"Channel::~Channel",
                format_args!("channel is destroyed, {count} {role} will be discarded"),
            );
        }
    }
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
    loop {
        // SAFETY: a live channel.
        let data = unsafe { (**chan).pop_data() }.cast::<zval>();
        if data.is_null() {
            break;
        }
        // SAFETY: `push` queued an emalloc'd zval holding one reference
        // (`sw_zval_free`).
        unsafe { zval_ptr_dtor(data) };
        // SAFETY: as above.
        unsafe { swr_zend_efree(data.cast()) };
    }
    destroy(*chan);
    *chan = ptr::null_mut();
}

unsafe extern "C" fn free(std: *mut zend_object) {
    // SAFETY: Zend frees an object this class created.
    let chan = unsafe { *ChannelObject::data(std) };
    if !chan.is_null() {
        // As in `dtor`, when the destructor did not run.
        destroy(chan);
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

/// MINIT: register the class, its properties and constants as upstream's
/// `php_swoole_channel_coro_minit` did, with `Co\Channel` and `Chan` when
/// short names are on.
/// # Safety
/// Called once, at MINIT.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn php_swoole_channel_coro_minit(module_number: i32) {
    // SAFETY: the native MINIT entry lends these original startup arguments.
    let result = unsafe { init_phpcollections_channel_minit(module_number) };
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
) -> Result<(), swoole_rs_zend::bailout::Bailout> {
    // SAFETY: MINIT, with a static table.
    unsafe { CHANNEL.register(c"Swoole\\Coroutine\\Channel", METHODS.as_ptr(), configure) };
    if swoole_rs_phpswoole::globals().use_shortname.get() {
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

fn this(call: &Call) -> &'static mut *mut Channel {
    // SAFETY: Channel methods are non-static, so `$this` is one of our objects.
    unsafe { ChannelObject::data(call.this()) }
}

/// `channel_coro_get_ptr`: the channel, or Swoole's fatal error (which exits).
fn channel(call: &Call) -> Option<*mut Channel> {
    let chan = *this(call);
    if chan.is_null() {
        // SAFETY: the fatal error exits; this frame owns nothing that needs a drop.
        unsafe {
            swoole_rs_corebase::fatal_error(
                SW_ERROR_WRONG_OPERATION,
                c"must call constructor first",
            );
        }
        return None;
    }
    Some(chan)
}

/// `$this->errCode = $code`, as the class sees it.
fn set_error(call: &Call, code: i64) {
    call.update_property_long(Some(&CHANNEL), c"errCode", code);
}

/// The channel's last error.
fn error(chan: *mut Channel) -> i64 {
    // SAFETY: a live channel.
    unsafe { (*chan).error() }.into()
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
    *this(call) = Box::into_raw(Box::new(Channel::new(size)));
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
    let mut php = Php::current(timeout);
    // SAFETY: a live channel; may suspend this coroutine.
    if unsafe { queue::push(chan, &mut php, copy.cast()) } {
        set_error(call, ERROR_OK);
        call.set_bool(true);
        return;
    }
    set_error(call, error(chan));
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
    let mut php = Php::current(timeout);
    // SAFETY: a live channel; may suspend this coroutine.
    let data = unsafe { queue::pop(chan, &mut php) }.cast::<zval>();
    if data.is_null() {
        set_error(call, error(chan));
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

/// `close(): bool`, which wakes every waiter.
fn close(call: &Call) {
    if let Some(chan) = channel(call) {
        let mut php = Php {
            co: ptr::null_mut(),
            timeout: 0.0,
        };
        // SAFETY: a live channel.
        call.set_bool(unsafe { queue::close(chan, &mut php) });
    }
}

/// The methods that read the channel: `length(): int`, `isEmpty(): bool`,
/// `isFull(): bool` and `stats(): array`.
macro_rules! queries {
    ($($name:ident($chan:ident, $call:ident) $body:block)*) => {$(
        fn $name($call: &Call) {
            if let Some(chan) = channel($call) {
                // SAFETY: a live channel, which no other borrow reaches meanwhile.
                let $chan = unsafe { &*chan };
                $body
            }
        }
    )*};
}

queries! {
    length(chan, call) {
        call.set_long(long(chan.length()));
    }
    is_empty(chan, call) {
        call.set_bool(chan.is_empty());
    }
    is_full(chan, call) {
        call.set_bool(chan.is_full());
    }
    stats(chan, call) {
        call.set_array();
        call.add_assoc_long(c"consumer_num", long(chan.consumer_num()));
        call.add_assoc_long(c"producer_num", long(chan.producer_num()));
        call.add_assoc_long(c"queue_num", long(chan.length()));
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
