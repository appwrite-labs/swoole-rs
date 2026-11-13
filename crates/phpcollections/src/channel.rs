//! PHP `Swoole\Coroutine\Channel`: class registration, object handlers and every
//! method. The queue is crates/phpcoroutines' channel; values cross it as owned
//! zval copies.
use swoole_rs_corebase::CCast;
use swoole_rs_phpcoroutines::channel::{Channel, Queue, Stats};
use swoole_rs_phpstubs::channel_coro;
use swoole_rs_zend::{
    Call, Custom, CustomClass, Minit, Owned,
    bailout::{self, Bailout},
    method_table,
    sys::ZEND_ACC_PUBLIC,
};

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

static CHANNEL: CustomClass<Chan> = CustomClass::new();

/// `struct ChannelObject { Channel *chan; zend_object std; }`.
struct Chan(Channel<Owned>);

impl Custom for Chan {
    fn class() -> &'static CustomClass<Self> {
        &CHANNEL
    }

    fn new() -> Self {
        Self(Channel::new())
    }

    const DESTRUCTS: bool = true;

    /// `channel_coro_dtor_object`: free every queued value, then the channel.
    fn destruct(&self, _last: bool) {
        self.release(drop);
    }
}

impl Chan {
    /// Give each queued value to `free` (the channel stays meanwhile), then
    /// `Channel::~Channel`: warn about the coroutines still waiting.
    fn release(&self, free: impl Fn(Owned)) {
        if let Some(queue) = self.0.queue() {
            while let Some(value) = queue.take() {
                free(value);
            }
        }
        let Some(stats) = self.0.release().as_ref().map(Queue::stats) else {
            return;
        };
        for (count, role) in [
            (stats.producers, "producers"),
            (stats.consumers, "consumers"),
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
}

impl Drop for Chan {
    /// The free handler when the destructor did not run: upstream deletes the
    /// channel and leaves its values.
    fn drop(&mut self) {
        self.release(std::mem::forget);
    }
}

method_table! {
    static METHODS = [
        c"__construct" => zim_swoole_channel_coro___construct, &channel_coro::arginfo_class_Swoole_Coroutine_Channel___construct, ZEND_ACC_PUBLIC;
        c"push" => zim_swoole_channel_coro_push, &channel_coro::arginfo_class_Swoole_Coroutine_Channel_push, ZEND_ACC_PUBLIC;
        c"pop" => zim_swoole_channel_coro_pop, &channel_coro::arginfo_class_Swoole_Coroutine_Channel_pop, ZEND_ACC_PUBLIC;
        c"isEmpty" => zim_swoole_channel_coro_isEmpty, &channel_coro::arginfo_class_Swoole_Coroutine_Channel_isEmpty, ZEND_ACC_PUBLIC;
        c"isFull" => zim_swoole_channel_coro_isFull, &channel_coro::arginfo_class_Swoole_Coroutine_Channel_isFull, ZEND_ACC_PUBLIC;
        c"close" => zim_swoole_channel_coro_close, &channel_coro::arginfo_class_Swoole_Coroutine_Channel_close, ZEND_ACC_PUBLIC;
        c"stats" => zim_swoole_channel_coro_stats, &channel_coro::arginfo_class_Swoole_Coroutine_Channel_stats, ZEND_ACC_PUBLIC;
        c"length" => zim_swoole_channel_coro_length, &channel_coro::arginfo_class_Swoole_Coroutine_Channel_length, ZEND_ACC_PUBLIC;
    ];
}

swoole_rs_zend::minit!(channel_minit => register);

/// `php_swoole_channel_coro_minit`: register the class, its properties and
/// constants, with `Co\Channel` and `Chan` when short names are on.
fn register(minit: &Minit) -> Result<(), Bailout> {
    minit.register_object::<Chan>(c"Swoole\\Coroutine\\Channel", &METHODS.0, 0);
    if swoole_rs_phpswoole::globals().use_shortname.get() {
        minit.alias(&CHANNEL, c"Co\\Channel")?;
        minit.alias(&CHANNEL, c"Chan")?;
    }
    minit.property_long(&CHANNEL, c"capacity", 0, ZEND_ACC_PUBLIC);
    minit.property_long(&CHANNEL, c"errCode", 0, ZEND_ACC_PUBLIC);
    for (name, value) in CONSTANTS {
        minit.long_constant(name, value);
    }
    Ok(())
}

/// `channel_coro_get_ptr`: the channel, or Swoole's fatal error (which exits).
fn channel(call: &Call) -> Option<Queue<Owned>> {
    let queue = call.custom::<Chan>()?.0.queue();
    if queue.is_none() {
        bailout::defer(swoole_rs_phpswoole::swoole_fatal_error(
            SW_ERROR_WRONG_OPERATION,
            c"must call constructor first",
        ));
    }
    queue
}

/// `$this->errCode = $code`, as the class sees it.
fn set_error(call: &Call, code: i64) {
    call.update_property_long(Some(&CHANNEL), c"errCode", code);
}

/// `__construct(int $size = 1)`: capacity below 1 becomes 1. Upstream neither
/// rejects a second call nor frees the channel it replaces.
fn construct(call: &Call) {
    let Some(chan) = call.custom::<Chan>() else {
        return;
    };
    let Ok(capacity) = call.arity(0, 1).and_then(|()| call.long(1, 1)) else {
        call.set_bool(false);
        return;
    };
    let capacity = capacity.max(1);
    chan.0.open(usize::try_from(capacity).unwrap_or(usize::MAX));
    call.update_property_long(Some(&CHANNEL), c"capacity", capacity);
}

/// `push(mixed $data, float $timeout = -1): bool`: the channel holds a copy
/// with its own reference (`sw_zval_dup`).
fn push(call: &Call) {
    let Some(queue) = channel(call) else { return };
    let Ok(timeout) = call.arity(1, 2).and_then(|()| call.double(2, -1.0)) else {
        call.set_bool(false);
        return;
    };
    let Some(data) = call.value(1) else { return };
    match queue.push(data.copy(), timeout) {
        Ok(()) => {
            set_error(call, ERROR_OK);
            call.set_bool(true);
        }
        Err((value, error)) => {
            set_error(call, error.into());
            drop(value);
            call.set_bool(false);
        }
    }
}

/// `pop(float $timeout = -1): mixed`: the result takes over the copy's reference.
fn pop(call: &Call) {
    let Some(queue) = channel(call) else { return };
    let Ok(timeout) = call.arity(0, 1).and_then(|()| call.double(1, -1.0)) else {
        call.set_bool(false);
        return;
    };
    match queue.pop(timeout) {
        Ok(value) => {
            call.set_owned(value);
            set_error(call, ERROR_OK);
        }
        Err(error) => {
            set_error(call, error.into());
            call.set_bool(false);
        }
    }
}

fn long(size: usize) -> i64 {
    size.c_cast::<i64>()
}

/// `close(): bool`, which wakes every waiter.
fn close(call: &Call) {
    if let Some(queue) = channel(call) {
        call.set_bool(queue.close());
    }
}

/// The methods that read the channel: `length(): int`, `isEmpty(): bool`,
/// `isFull(): bool` and `stats(): array`.
fn query(call: &Call, answer: impl FnOnce(&Call, Stats)) {
    if let Some(stats) = channel(call).as_ref().map(Queue::stats) {
        answer(call, stats);
    }
}

fn length(call: &Call) {
    query(call, |call, stats| call.set_long(long(stats.length)));
}

fn is_empty(call: &Call) {
    query(call, |call, stats| call.set_bool(stats.length == 0));
}

fn is_full(call: &Call) {
    query(call, |call, stats| call.set_bool(stats.full));
}

fn stats(call: &Call) {
    query(call, |call, stats| {
        call.set_array();
        call.add_assoc_long(c"consumer_num", long(stats.consumers));
        call.add_assoc_long(c"producer_num", long(stats.producers));
        call.add_assoc_long(c"queue_num", long(stats.length));
    });
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
