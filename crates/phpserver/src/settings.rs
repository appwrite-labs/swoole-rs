//! `Swoole\Server::set()`: every option's lookup, conversion, bounds and the
//! order upstream reads them in. C++ keeps the Server's fields and the effects
//! that need its C++ API, reached by field id through the host table
//! (`native/process/swoole_rs_phpserver_settings.h`).
use crate::server::{display_errors, fatal, server, set_last_error};
use std::{
    ffi::{CStr, c_char, c_int, c_void},
    ptr,
    sync::atomic::{AtomicPtr, Ordering},
};
use swoole_rs_server::{server_methods, view};
use swoole_rs_zend::{
    Call, E_ERROR, E_WARNING, bailout, message, php_error,
    string::bytes,
    sys::{
        IS_ARRAY, IS_NULL, IS_REFERENCE, swr_zend_hash_foreach_key, swr_zend_string_release,
        zend_hash_str_find, zend_is_true, zend_object, zend_string, zend_ulong, zval,
    },
    type_of,
    value::{get_double, get_long, get_string, table},
};

/// `SW_ERROR_PHP_RUNTIME_NOTICE`, `SW_ERROR_SERVER_UNRELATED_THREAD`.
const SW_ERROR_PHP_RUNTIME_NOTICE: c_int = 510;
const SW_ERROR_SERVER_UNRELATED_THREAD: c_int = 9016;
/// `SW_WORKER_MIN_REQUEST`.
const WORKER_MIN_REQUEST: i64 = 10;

/// Server fields by id (`SWR_PHPSERVER_SET_*`).
mod field {
    pub(super) const DAEMONIZE: u32 = 1;
    pub(super) const REACTOR_NUM: u32 = 2;
    pub(super) const SINGLE_THREAD: u32 = 3;
    pub(super) const WORKER_NUM: u32 = 4;
    pub(super) const MAX_WAIT_TIME: u32 = 5;
    pub(super) const MAX_QUEUED_BYTES: u32 = 6;
    pub(super) const MAX_CONCURRENCY: u32 = 7;
    pub(super) const WORKER_MAX_CONCURRENCY: u32 = 8;
    pub(super) const ENABLE_COROUTINE: u32 = 9;
    pub(super) const DISPATCH_MODE: u32 = 10;
    pub(super) const SEND_YIELD: u32 = 11;
    pub(super) const DISCARD_TIMEOUT_REQUEST: u32 = 12;
    pub(super) const ENABLE_UNSAFE_EVENT: u32 = 13;
    pub(super) const ENABLE_DELAY_RECEIVE: u32 = 14;
    pub(super) const ENABLE_REUSE_PORT: u32 = 15;
    pub(super) const TASK_OBJECT: u32 = 16;
    pub(super) const EVENT_OBJECT: u32 = 17;
    pub(super) const TASK_ENABLE_COROUTINE: u32 = 18;
    pub(super) const TASK_WORKER_NUM: u32 = 19;
    pub(super) const TASK_IPC_MODE: u32 = 20;
    pub(super) const TASK_MAX_REQUEST: u32 = 21;
    pub(super) const TASK_MAX_REQUEST_GRACE: u32 = 22;
    pub(super) const MAX_CONNECTION: u32 = 23;
    pub(super) const START_SESSION_ID: u32 = 24;
    pub(super) const HEARTBEAT_CHECK_INTERVAL: u32 = 25;
    pub(super) const MAX_REQUEST: u32 = 26;
    pub(super) const MAX_REQUEST_GRACE: u32 = 27;
    pub(super) const RELOAD_ASYNC: u32 = 28;
    pub(super) const OPEN_CPU_AFFINITY: u32 = 29;
    pub(super) const HTTP_PARSE_COOKIE: u32 = 30;
    pub(super) const HTTP_PARSE_POST: u32 = 31;
    pub(super) const HTTP_PARSE_FILES: u32 = 32;
    pub(super) const HTTP_COMPRESSION: u32 = 33;
    pub(super) const HTTP_COMPRESSION_LEVEL: u32 = 34;
    pub(super) const COMPRESSION_MIN_LENGTH: u32 = 35;
    pub(super) const WEBSOCKET_COMPRESSION: u32 = 36;
    pub(super) const UPLOAD_MAX_FILESIZE: u32 = 37;
    pub(super) const ENABLE_STATIC_HANDLER: u32 = 38;
    pub(super) const HTTP_AUTOINDEX: u32 = 39;
    pub(super) const INPUT_BUFFER_SIZE: u32 = 40;
    pub(super) const OUTPUT_BUFFER_SIZE: u32 = 41;
    pub(super) const MESSAGE_QUEUE_KEY: u32 = 42;
    /// Read-only facts.
    pub(super) const CPU_NUM: u32 = 100;
    pub(super) const GLOBAL_ENABLE_COROUTINE: u32 = 101;
    pub(super) const WORKER_THREAD: u32 = 102;
    pub(super) const STARTED: u32 = 103;
    /// Build switches: `HAVE_REUSEPORT` on Linux, `HAVE_CPU_AFFINITY`,
    /// `SW_HAVE_COMPRESSION`, `SW_HAVE_ZLIB`, `HAVE_MSGQUEUE`, `SW_THREAD`.
    pub(super) const HAVE_REUSE_PORT: u32 = 110;
    pub(super) const HAVE_CPU_AFFINITY: u32 = 111;
    pub(super) const HAVE_COMPRESSION: u32 = 112;
    pub(super) const HAVE_ZLIB: u32 = 113;
    pub(super) const HAVE_MSGQUEUE: u32 = 114;
    pub(super) const THREAD: u32 = 115;
    pub(super) const PATH_MAX: u32 = 116;
}

/// Server string settings and lists (`SWR_PHPSERVER_TEXT_*`).
mod text {
    pub(super) const CHROOT: u32 = 1;
    pub(super) const USER: u32 = 2;
    pub(super) const GROUP: u32 = 3;
    pub(super) const PID_FILE: u32 = 4;
    pub(super) const TASK_TMPDIR: u32 = 5;
    pub(super) const DOCUMENT_ROOT: u32 = 7;
    pub(super) const INDEX_FILE: u32 = 8;
    pub(super) const COMPRESSION_TYPE: u32 = 9;
    pub(super) const STATIC_LOCATION: u32 = 10;
}

/// `swr_v1_phpserver_settings_host`.
#[repr(C)]
pub struct SettingsHost {
    get: unsafe extern "C" fn(*mut c_void, u32) -> i64,
    text: unsafe extern "C" fn(*mut c_void, u32, *const c_char, usize),
    rewrite: unsafe extern "C" fn(*mut c_void, *const c_char, usize, *const c_char, usize),
    parse_size: unsafe extern "C" fn(*mut zval) -> i64,
    global_options: unsafe extern "C" fn(*mut c_void) -> bool,
    dispatch_func: unsafe extern "C" fn(*mut c_void, *mut zval) -> bool,
    cpu_affinity: unsafe extern "C" fn(*mut c_int, usize, *mut c_int) -> c_int,
    set_cpu_affinity: unsafe extern "C" fn(*mut c_void, *const c_int, usize),
    strerror: unsafe extern "C" fn(c_int) -> *const c_char,
    create_dir: unsafe extern "C" fn(*const c_char, usize) -> c_int,
    thread_options: unsafe extern "C" fn(*mut zend_object, *mut c_void) -> bool,
}

static HOST: AtomicPtr<SettingsHost> = AtomicPtr::new(ptr::null_mut());

fn host() -> &'static SettingsHost {
    // SAFETY: MINIT installs C++'s static table before any method runs.
    unsafe { &*HOST.load(Ordering::Acquire) }
}

/// MINIT: the table `set()` calls.
/// # Safety
/// `table` lives for the process.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_phpserver_settings_init(table: *const SettingsHost) {
    HOST.store(table.cast_mut(), Ordering::Release);
}

/// `serv->reload_async = true` (`on_before_start` with coroutines on).
pub(crate) fn set_reload_async(serv: *mut c_void) {
    // SAFETY: a live server.
    unsafe { view::config(serv) }.reload_async = true;
}

/// The Server field `id` (`field::*`), read as C++ reads it.
///
/// # Safety
/// `serv` is a live server.
unsafe fn fact(serv: *mut c_void, id: u32) -> i64 {
    use field as f;
    // SAFETY: per the caller.
    let config = unsafe { view::config(serv) };
    match id {
        // SAFETY: per the caller.
        f::STARTED => i64::from(unsafe { view::is_started(serv) }),
        f::WORKER_MAX_CONCURRENCY => i64::from(config.worker_max_concurrency),
        // SAFETY: per the caller.
        f::MAX_CONCURRENCY => i64::from(unsafe { view::counters(serv) }.max_concurrency),
        f::ENABLE_COROUTINE => i64::from(config.enable_coroutine),
        f::DISPATCH_MODE => i64::from(config.dispatch_mode),
        f::TASK_IPC_MODE => i64::from(config.task_ipc_mode),
        f::TASK_ENABLE_COROUTINE => i64::from(config.task_enable_coroutine),
        // SAFETY: per the caller.
        _ => unsafe { (host().get)(serv, id) },
    }
}

/// `serv-><field> = v` for the Server field `id`, converted as C++ assigns it.
///
/// # Safety
/// `serv` is a live server that has not started.
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "C++'s implicit conversions of the int64_t setting to each field's type"
)]
unsafe fn apply(serv: *mut c_void, id: u32, v: i64) {
    use field as f;
    // SAFETY: per the caller.
    let c = unsafe { view::config(serv) };
    let on = v != 0;
    // `0` is unlimited for both concurrency limits.
    let limit = if v as u32 == 0 { u32::MAX } else { v as u32 };
    match id {
        f::DAEMONIZE => c.daemonize = on,
        f::REACTOR_NUM => c.reactor_num = v as u16,
        f::SINGLE_THREAD => c.single_thread = on,
        f::WORKER_NUM => c.worker_num = v as u32,
        f::MAX_WAIT_TIME => c.max_wait_time = v as u32,
        f::MAX_QUEUED_BYTES => c.max_queued_bytes = v as u32,
        // SAFETY: per the caller; the counters are mapped before set() runs.
        f::MAX_CONCURRENCY => unsafe { view::counters_mut(serv) }.max_concurrency = limit,
        f::WORKER_MAX_CONCURRENCY => c.worker_max_concurrency = limit,
        f::ENABLE_COROUTINE => c.enable_coroutine = on,
        f::DISPATCH_MODE => c.dispatch_mode = v as u8,
        f::SEND_YIELD => c.send_yield = on,
        f::DISCARD_TIMEOUT_REQUEST => c.discard_timeout_request = on,
        f::ENABLE_UNSAFE_EVENT => c.enable_unsafe_event = on,
        f::ENABLE_DELAY_RECEIVE => c.enable_delay_receive = on,
        f::ENABLE_REUSE_PORT => c.enable_reuse_port = on,
        f::TASK_OBJECT => c.task_object = on,
        f::EVENT_OBJECT => c.event_object = on,
        f::TASK_ENABLE_COROUTINE => c.task_enable_coroutine = on,
        f::TASK_WORKER_NUM => c.task_worker_num = v as u32,
        f::TASK_IPC_MODE => c.task_ipc_mode = v as u8,
        f::TASK_MAX_REQUEST => c.task_max_request = v as u32,
        f::TASK_MAX_REQUEST_GRACE => c.task_max_request_grace = v as u32,
        // SAFETY: per the caller.
        f::MAX_CONNECTION => unsafe { server_methods::set_max_connection(serv, v as u32) },
        f::START_SESSION_ID => {
            // SAFETY: per the caller, as above.
            let counters = unsafe { view::counters_mut(serv) };
            counters.session_round = v.min(i64::from(u32::MAX));
        }
        f::HEARTBEAT_CHECK_INTERVAL => c.heartbeat_check_interval = v as u16,
        f::MAX_REQUEST => c.max_request = v as u32,
        f::MAX_REQUEST_GRACE => c.max_request_grace = v as u32,
        f::RELOAD_ASYNC => c.reload_async = on,
        f::OPEN_CPU_AFFINITY => c.open_cpu_affinity = on,
        f::HTTP_PARSE_COOKIE => c.http_parse_cookie = on,
        f::HTTP_PARSE_POST => c.http_parse_post = on,
        f::HTTP_PARSE_FILES => c.http_parse_files = on,
        f::HTTP_COMPRESSION => c.http_compression = on,
        f::HTTP_COMPRESSION_LEVEL => c.http_compression_level = v as u8,
        f::COMPRESSION_MIN_LENGTH => c.compression_min_length = v as u32,
        f::WEBSOCKET_COMPRESSION => c.websocket_compression = on,
        f::UPLOAD_MAX_FILESIZE => c.upload_max_filesize = v as usize,
        f::ENABLE_STATIC_HANDLER => c.enable_static_handler = on,
        f::HTTP_AUTOINDEX => c.http_autoindex = on,
        f::INPUT_BUFFER_SIZE => c.input_buffer_size = v as u32,
        f::OUTPUT_BUFFER_SIZE => c.output_buffer_size = v as u32,
        f::MESSAGE_QUEUE_KEY => c.message_queue_key = v as u64,
        _ => unreachable!("not a Server field: {id}"),
    }
}

/// The settings array and the Server being configured.
struct Settings {
    serv: *mut c_void,
    vht: *mut zval,
}

/// Stop: a bailout is pending (upstream never ran past it).
struct Stop;

type Step = Result<(), Stop>;

fn checked() -> Step {
    if bailout::pending() {
        Err(Stop)
    } else {
        Ok(())
    }
}

impl Settings {
    /// `php_swoole_array_get_value(vht, key, ztmp)`: present and not null.
    fn get(&self, key: &CStr) -> Option<*mut zval> {
        // SAFETY: an array argument.
        let vht = unsafe { table(self.vht) };
        // SAFETY: a live table and a NUL-terminated key.
        let found = unsafe { zend_hash_str_find(vht, key.as_ptr(), key.count_bytes()) };
        // SAFETY: a found value is live.
        (!found.is_null() && unsafe { type_of(found) } != IS_NULL).then_some(found)
    }

    fn get_any(&self, keys: &[&CStr]) -> Option<*mut zval> {
        keys.iter().find_map(|key| self.get(key))
    }

    fn fact(&self, id: u32) -> i64 {
        // SAFETY: a live server.
        unsafe { fact(self.serv, id) }
    }

    fn set(&self, id: u32, value: i64) {
        // SAFETY: a live server, not started (set() checked).
        unsafe { apply(self.serv, id, value) };
    }

    fn long(value: *mut zval) -> Result<i64, Stop> {
        // SAFETY: a live setting.
        let value = unsafe { get_long(value) };
        checked().map(|()| value)
    }

    fn size(value: *mut zval) -> Result<i64, Stop> {
        // SAFETY: a live setting; C++'s `php_swoole_parse_to_size`.
        let value = unsafe { (host().parse_size)(value) };
        checked().map(|()| value)
    }

    fn truth(value: *mut zval) -> i64 {
        // SAFETY: a live setting.
        i64::from(unsafe { zend_is_true(value) })
    }

    /// `zend::String(value)`, handed to `each` and released.
    fn string<R>(value: *mut zval, each: impl FnOnce(&[u8]) -> R) -> Result<R, Stop> {
        // SAFETY: a live setting; an owned reference, released below.
        let string = unsafe { get_string(value) };
        // SAFETY: live until released.
        let result = each(unsafe { bytes(string) });
        // SAFETY: the reference taken above.
        unsafe { swr_zend_string_release(string) };
        checked().map(|()| result)
    }

    fn text(&self, id: u32, value: &[u8]) {
        // SAFETY: a live server and bytes.
        unsafe { (host().text)(self.serv, id, value.as_ptr().cast(), value.len()) };
    }

    /// A boolean option.
    fn flag(&self, key: &CStr, id: u32) {
        if let Some(value) = self.get(key) {
            self.set(id, Self::truth(value));
        }
    }

    /// An integer option clamped to `[min, max]`.
    fn clamped(&self, key: &CStr, id: u32, min: i64, max: i64) -> Step {
        if let Some(value) = self.get(key) {
            self.set(id, Self::long(value)?.clamp(min, max));
        }
        Ok(())
    }

    /// A string option.
    fn string_option(&self, key: &CStr, id: u32) -> Step {
        if let Some(value) = self.get(key) {
            Self::string(value, |bytes| self.text(id, bytes))?;
        }
        Ok(())
    }

    /// Each value of an array option, as a string; `E_ERROR` otherwise.
    fn list(&self, keys: &[&CStr], error: &CStr, each: impl Fn(&[u8])) -> Step {
        let Some(value) = self.get_any(keys) else {
            return Ok(());
        };
        // SAFETY: a live setting.
        if unsafe { type_of(value) } != IS_ARRAY {
            fatal(E_ERROR, error);
            return Err(Stop);
        }
        let mut items = Vec::new();
        // SAFETY: a live array setting.
        let array = unsafe { table(value) };
        // SAFETY: a live array; the values are only collected.
        unsafe {
            swoole_rs_zend::each_value(array, |item| {
                items.push(item);
                true
            });
        }
        for item in items {
            Self::string(item, &each)?;
        }
        Ok(())
    }
}

/// `php_swoole_error(E_WARNING, message)`.
fn notice(message: &CStr) -> Step {
    set_last_error(SW_ERROR_PHP_RUNTIME_NOTICE);
    if display_errors() {
        // SAFETY: a warning from a handler.
        bailout::defer(unsafe { php_error(E_WARNING, message) });
    }
    checked()
}

fn set(call: &Call) {
    let serv = server(call);
    let s = Settings {
        serv,
        vht: ptr::null_mut(),
    };
    if s.fact(field::WORKER_THREAD) != 0 {
        set_last_error(SW_ERROR_SERVER_UNRELATED_THREAD);
        return call.set_bool(false);
    }
    if s.fact(field::STARTED) != 0 {
        // SAFETY: `$this` is live.
        let name = unsafe { swoole_rs_zend::class_name(call.this()) };
        let text = message!(
            "server is running, unable to execute {}->set",
            name.to_string_lossy()
        );
        fatal(E_WARNING, text.as_cstr());
        return call.set_bool(false);
    }
    let Ok(vht) = call.arity(1, 1).and_then(|()| call.array(1)) else {
        return call.set_bool(false);
    };
    let s = Settings { serv, vht };
    // Every early exit after an `E_ERROR` or a deferred bailout leaves the
    // handler, whose last step raises it.
    if let Ok(true) = options(&s, call.this()) {
        match finish(call.this(), vht) {
            Ok(()) => call.set_bool(true),
            Err(bailed) => bailed.defer(),
        }
    } else if !bailout::pending() {
        call.set_bool(false);
    }
}

/// `$this->ports[0]->set($settings)` (Port's own method), then the merge into
/// `$this->setting`.
fn finish(this: *mut zend_object, settings: *mut zval) -> Result<(), bailout::Bailout> {
    // SAFETY: `$this` is a Server object.
    let port = unsafe { crate::lifecycle::swr_v1_phpserver_port(this, 0) };
    // SAFETY: the primary port's object zval.
    let port = unsafe { swoole_rs_zend::object_of(port) };
    // SAFETY: the live Port object, its class and the settings argument.
    unsafe {
        bailout::call_method(
            port,
            crate::PORT.entry(),
            b"set",
            ptr::null_mut(),
            &[settings],
        )
    }?;
    crate::port::merge_setting(crate::server::SERVER.entry(), this, settings).map(|_| ())
}

/// The options, in upstream's order. `Ok(false)`: return false.
fn options(s: &Settings, this: *mut zend_object) -> Result<bool, Stop> {
    use field as f;
    let host = host();
    // php_swoole_set_global_option, _coroutine_option and _aio_option.
    // SAFETY: the settings argument.
    if unsafe { (host.global_options)(s.vht.cast()) } {
        bailout::defer(Err(bailout::Bailout));
        return Err(Stop);
    }
    s.string_option(c"chroot", text::CHROOT)?;
    s.string_option(c"user", text::USER)?;
    s.string_option(c"group", text::GROUP)?;
    s.flag(c"daemonize", f::DAEMONIZE);
    s.string_option(c"pid_file", text::PID_FILE)?;
    if let Some(value) = s.get(c"reactor_num") {
        let v = Settings::long(value)?.clamp(0, i64::from(u16::MAX));
        s.set(f::REACTOR_NUM, if v == 0 { s.fact(f::CPU_NUM) } else { v });
    }
    s.flag(c"single_thread", f::SINGLE_THREAD);
    if let Some(value) = s.get(c"worker_num") {
        let v = Settings::long(value)?.clamp(0, i64::from(u32::MAX));
        s.set(f::WORKER_NUM, if v == 0 { s.fact(f::CPU_NUM) } else { v });
    }
    s.clamped(c"max_wait_time", f::MAX_WAIT_TIME, 0, i64::from(u32::MAX))?;
    if let Some(value) = s.get(c"max_queued_bytes") {
        s.set(
            f::MAX_QUEUED_BYTES,
            Settings::size(value)?.clamp(0, i64::from(u32::MAX)),
        );
    }
    if let Some(value) = s.get(c"max_concurrency") {
        let v = Settings::long(value)?.clamp(1, i64::from(u32::MAX));
        s.set(f::MAX_CONCURRENCY, v);
        if s.fact(f::WORKER_MAX_CONCURRENCY) == i64::from(u32::MAX) {
            s.set(f::WORKER_MAX_CONCURRENCY, s.fact(f::MAX_CONCURRENCY));
        }
    }
    s.clamped(
        c"worker_max_concurrency",
        f::WORKER_MAX_CONCURRENCY,
        1,
        i64::from(u32::MAX),
    )?;
    match s.get(c"enable_coroutine") {
        Some(value) => s.set(f::ENABLE_COROUTINE, Settings::truth(value)),
        None => s.set(f::ENABLE_COROUTINE, s.fact(f::GLOBAL_ENABLE_COROUTINE)),
    }
    if let Some(value) = s.get(c"send_timeout") {
        // SAFETY: a live setting.
        let v = unsafe { get_double(value) };
        checked()?;
        // SAFETY: a live server.
        unsafe { view::config(s.serv) }.send_timeout = v;
    }
    s.clamped(c"dispatch_mode", f::DISPATCH_MODE, 0, i64::from(u8::MAX))?;
    match s.get(c"send_yield") {
        Some(value) => {
            let on = Settings::truth(value);
            s.set(f::SEND_YIELD, on);
            let mode = s.fact(f::DISPATCH_MODE);
            if on != 0 && !(mode == 2 || mode == 4) {
                notice(c"'send_yield' option can only be set when using dispatch_mode=2/4")?;
                s.set(f::SEND_YIELD, 0);
            }
        }
        None => s.set(f::SEND_YIELD, s.fact(f::ENABLE_COROUTINE)),
    }
    if let Some(value) = s.get(c"dispatch_func") {
        // SAFETY: a live server and setting; true if a bailout was trapped.
        if unsafe { (host.dispatch_func)(s.serv, value) } {
            bailout::defer(Err(bailout::Bailout));
            return Err(Stop);
        }
    }
    workers(s)?;
    http(s, this)
}

/// The task, worker and connection options.
fn workers(s: &Settings) -> Step {
    use field as f;
    s.flag(c"discard_timeout_request", f::DISCARD_TIMEOUT_REQUEST);
    s.flag(c"enable_unsafe_event", f::ENABLE_UNSAFE_EVENT);
    s.flag(c"enable_delay_receive", f::ENABLE_DELAY_RECEIVE);
    if s.fact(f::HAVE_REUSE_PORT) != 0 {
        s.flag(c"enable_reuse_port", f::ENABLE_REUSE_PORT);
    }
    if let Some(value) = s.get_any(&[c"task_use_object", c"task_object"]) {
        s.set(f::TASK_OBJECT, Settings::truth(value));
    }
    if let Some(value) = s.get(c"event_object") {
        let on = Settings::truth(value);
        s.set(f::EVENT_OBJECT, on);
        if on != 0 {
            s.set(f::TASK_OBJECT, 1);
        }
    }
    s.flag(c"task_enable_coroutine", f::TASK_ENABLE_COROUTINE);
    s.clamped(
        c"task_worker_num",
        f::TASK_WORKER_NUM,
        0,
        i64::from(u32::MAX),
    )?;
    s.clamped(c"task_ipc_mode", f::TASK_IPC_MODE, 0, i64::from(u8::MAX))?;
    s.string_option(c"task_tmpdir", text::TASK_TMPDIR)?;
    if let Some(value) = s.get(c"task_max_request") {
        let v = Settings::long(value)?.clamp(0, i64::from(u32::MAX));
        s.set(f::TASK_MAX_REQUEST, v);
        if let Some(grace) = s.get(c"task_max_request_grace") {
            s.set(
                f::TASK_MAX_REQUEST_GRACE,
                Settings::long(grace)?.clamp(0, i64::from(u32::MAX)),
            );
        } else if v > WORKER_MIN_REQUEST {
            s.set(f::TASK_MAX_REQUEST_GRACE, v / 2);
        }
    }
    if let Some(value) = s.get_any(&[c"max_connection", c"max_conn"]) {
        s.set(
            f::MAX_CONNECTION,
            Settings::long(value)?.clamp(0, i64::from(u32::MAX)),
        );
    }
    if let Some(value) = s.get(c"start_session_id") {
        s.set(f::START_SESSION_ID, Settings::long(value)?);
    }
    if let Some(value) = s.get(c"heartbeat_check_interval") {
        s.set(
            f::HEARTBEAT_CHECK_INTERVAL,
            Settings::long(value)?.clamp(0, i64::from(u16::MAX)),
        );
    } else if let Some(value) = s.get(c"heartbeat_idle_time") {
        let v = Settings::long(value)?;
        s.set(f::HEARTBEAT_CHECK_INTERVAL, if v > 2 { v / 2 } else { 1 });
    }
    if let Some(value) = s.get(c"max_request") {
        let v = Settings::long(value)?.clamp(0, i64::from(u32::MAX));
        s.set(f::MAX_REQUEST, v);
        if let Some(grace) = s.get(c"max_request_grace") {
            s.set(
                f::MAX_REQUEST_GRACE,
                Settings::long(grace)?.clamp(0, i64::from(u32::MAX)),
            );
        } else if v > WORKER_MIN_REQUEST {
            s.set(f::MAX_REQUEST_GRACE, v / 2);
        }
    }
    s.flag(c"reload_async", f::RELOAD_ASYNC);
    s.flag(c"open_cpu_affinity", f::OPEN_CPU_AFFINITY);
    if s.fact(f::HAVE_CPU_AFFINITY) != 0
        && let Some(value) = s.get(c"cpu_affinity_ignore")
        && !cpu_affinity_ignore(s, value)?
    {
        return Err(Stop);
    }
    Ok(())
}

/// `CPU_SETSIZE`.
const CPU_SETSIZE: usize = 1024;

/// `php_swoole_server_set_cpu_affinity_ignore`: the CPUs left to bind workers
/// to once `value`'s are taken out of this process's affinity. False: rejected.
fn cpu_affinity_ignore(s: &Settings, value: *mut zval) -> Result<bool, Stop> {
    let host = host();
    // SAFETY: a live setting.
    if unsafe { type_of(value) } != IS_ARRAY {
        fatal(E_ERROR, c"cpu_affinity_ignore must be array");
        return Err(Stop);
    }
    let mut cpus = vec![0; CPU_SETSIZE];
    let mut error = 0;
    // SAFETY: room for every CPU and a writable error.
    let count = unsafe { (host.cpu_affinity)(cpus.as_mut_ptr(), cpus.len(), &raw mut error) };
    let Ok(count) = usize::try_from(count) else {
        // php_swoole_sys_error(E_WARNING, "sched_getaffinity() failed")
        set_last_error(error);
        if display_errors() {
            // SAFETY: plain state.
            let reason = unsafe { (host.strerror)(error) };
            // SAFETY: Swoole's static error strings.
            let reason = unsafe { CStr::from_ptr(reason) };
            let text = message!(
                "sched_getaffinity() failed, Error: {}[{error}]",
                reason.to_string_lossy()
            );
            // SAFETY: a warning from a handler.
            bailout::defer(unsafe { php_error(E_WARNING, text.as_cstr()) });
        }
        return checked().map(|()| false);
    };
    cpus.truncate(count);
    // SAFETY: an array setting.
    let ignored = unsafe { table(value) };
    let mut items = Vec::new();
    // SAFETY: a live table; the values are only collected.
    unsafe {
        swoole_rs_zend::each_value(ignored, |item| {
            items.push(item);
            true
        });
    }
    for item in items {
        let cpu = Settings::long(item)?;
        cpus.retain(|&available| i64::from(available) != cpu);
    }
    if cpus.is_empty() {
        fatal(E_ERROR, c"cpu_affinity_ignore excludes all available CPUs");
        return Err(Stop);
    }
    // SAFETY: a live server and the CPUs left.
    unsafe { (host.set_cpu_affinity)(s.serv, cpus.as_ptr(), cpus.len()) };
    Ok(true)
}

/// The HTTP, buffer, queue and thread options, and the final checks.
fn http(s: &Settings, this: *mut zend_object) -> Result<bool, Stop> {
    use field as f;
    let host = host();
    s.flag(c"http_parse_cookie", f::HTTP_PARSE_COOKIE);
    s.flag(c"http_parse_post", f::HTTP_PARSE_POST);
    s.flag(c"http_parse_files", f::HTTP_PARSE_FILES);
    if s.fact(f::HAVE_COMPRESSION) != 0 {
        s.flag(c"http_compression", f::HTTP_COMPRESSION);
        if let Some(value) = s.get_any(&[
            c"http_compression_level",
            c"compression_level",
            c"http_gzip_level",
        ]) {
            s.set(
                f::HTTP_COMPRESSION_LEVEL,
                Settings::long(value)?.clamp(0, i64::from(u8::MAX)),
            );
        }
        if let Some(value) = s.get_any(&[c"http_compression_min_length", c"compression_min_length"])
        {
            s.set(f::COMPRESSION_MIN_LENGTH, Settings::size(value)?);
        }
    }
    if s.fact(f::HAVE_ZLIB) != 0 {
        s.flag(c"websocket_compression", f::WEBSOCKET_COMPRESSION);
    }
    if let Some(value) = s.get(c"upload_tmp_dir") {
        let created = Settings::string(value, |dir| {
            // SAFETY: the directory's bytes.
            let created = unsafe { (host.create_dir)(dir.as_ptr().cast(), dir.len()) } >= 0;
            if created {
                // SAFETY: the live server.
                unsafe { view::set_upload_tmp_dir(s.serv, dir) };
            } else {
                let shown = dir.split(|&b| b == 0).next().unwrap_or_default();
                let text = message!(
                    "Unable to create upload_tmp_dir[{}]",
                    String::from_utf8_lossy(shown)
                );
                fatal(E_ERROR, text.as_cstr());
            }
            created
        })?;
        if !created {
            return Err(Stop);
        }
    }
    if let Some(value) = s.get(c"upload_max_filesize") {
        s.set(f::UPLOAD_MAX_FILESIZE, Settings::size(value)?);
    }
    s.flag(c"enable_static_handler", f::ENABLE_STATIC_HANDLER);
    if let Some(value) = s.get(c"document_root") {
        let path_max = s.fact(f::PATH_MAX);
        let fits = Settings::string(value, |root| {
            if i64::try_from(root.len()).unwrap_or(i64::MAX) >= path_max {
                let text = message!("The length of document_root must be less than {path_max}");
                fatal(E_ERROR, text.as_cstr());
                return false;
            }
            s.text(text::DOCUMENT_ROOT, root);
            true
        })?;
        if !fits {
            return Err(Stop);
        }
    }
    s.flag(c"http_autoindex", f::HTTP_AUTOINDEX);
    s.list(
        &[c"http_index_files"],
        c"http_index_files must be array",
        |name| {
            if !name.is_empty() {
                s.text(text::INDEX_FILE, name);
            }
        },
    )?;
    s.list(
        &[c"http_compression_types", c"compression_types"],
        c"http_compression_types must be array",
        |kind| {
            if !kind.is_empty() {
                s.text(text::COMPRESSION_TYPE, kind);
            }
        },
    )?;
    s.list(
        &[c"static_handler_locations"],
        c"static_handler_locations num must be array",
        |location| {
            if location.first() == Some(&b'/') {
                s.text(text::STATIC_LOCATION, location);
            }
        },
    )?;
    rewrite_rules(s)?;
    if let Some(value) = s.get_any(&[c"input_buffer_size", c"buffer_input_size"]) {
        s.set(
            f::INPUT_BUFFER_SIZE,
            Settings::size(value)?.clamp(0, i64::from(u32::MAX)),
        );
    }
    if let Some(value) = s.get_any(&[c"output_buffer_size", c"buffer_output_size"]) {
        s.set(
            f::OUTPUT_BUFFER_SIZE,
            Settings::size(value)?.clamp(0, i64::from(u32::MAX)),
        );
    }
    if let Some(value) = s.get(c"message_queue_key") {
        let v = Settings::long(value)?;
        // A positive SysV key must fit key_t's unsigned counterpart.
        if v > i64::from(u32::MAX) {
            fatal(E_ERROR, c"message_queue_key is out of range");
            return Err(Stop);
        }
        s.set(f::MESSAGE_QUEUE_KEY, v.max(0));
    }
    if s.fact(f::THREAD) != 0 {
        // bootstrap and init_arguments.
        // SAFETY: `$this` and the settings.
        if unsafe { (host.thread_options)(this, s.vht.cast()) } {
            bailout::defer(Err(bailout::Bailout));
            return Err(Stop);
        }
    }
    let ipc = s.fact(f::TASK_IPC_MODE);
    let queue = ipc == 2 || ipc == 3;
    if s.fact(f::HAVE_MSGQUEUE) == 0 && queue {
        fatal(E_ERROR, c"not support `sysvmsg`");
        return Err(Stop);
    }
    if s.fact(f::TASK_ENABLE_COROUTINE) != 0 && queue {
        fatal(
            E_ERROR,
            c"cannot use msgqueue when `task_enable_coroutine` is enable",
        );
        return Err(Stop);
    }
    Ok(true)
}

/// `url_rewrite_rules`: string keys to string replacements.
fn rewrite_rules(s: &Settings) -> Step {
    unsafe extern "C" fn each(
        _index: zend_ulong,
        key: *mut zend_string,
        value: *mut zval,
        context: *mut c_void,
    ) -> bool {
        // SAFETY: the `Rules` passed below, live for the loop.
        let rules = unsafe { &mut *context.cast::<Rules>() };
        let mut value = value;
        // SAFETY: a live value; ZVAL_DEREF.
        if unsafe { type_of(value) } == IS_REFERENCE {
            // SAFETY: a live value.
            let inner = unsafe { (*value).value };
            // SAFETY: a reference zval holds `ref`.
            let reference = unsafe { inner.ref_ };
            // SAFETY: a live reference.
            value = unsafe { &raw mut (*reference).val };
        }
        // SAFETY: as above.
        let Some(replacement) = (unsafe { swoole_rs_zend::string_of(value) }) else {
            rules.bad = true;
            return false;
        };
        // An integer key has no pattern; upstream reads a null string there.
        let pattern: &[u8] = if key.is_null() {
            b""
        } else {
            // SAFETY: a live key.
            unsafe { bytes(key) }
        };
        // SAFETY: a live server and the rule's bytes, which C++ copies.
        unsafe {
            (host().rewrite)(
                rules.settings.serv,
                pattern.as_ptr().cast(),
                pattern.len(),
                replacement.as_ptr().cast(),
                replacement.len(),
            );
        }
        true
    }
    struct Rules<'a> {
        settings: &'a Settings,
        bad: bool,
    }
    let Some(value) = s.get(c"url_rewrite_rules") else {
        return Ok(());
    };
    // SAFETY: a live setting.
    if unsafe { type_of(value) } != IS_ARRAY {
        fatal(E_ERROR, c"The `url_rewrite_rules` must be array");
        return Err(Stop);
    }
    let mut rules = Rules {
        settings: s,
        bad: false,
    };
    // SAFETY: a live array; `each` neither modifies it nor unwinds.
    let array = unsafe { table(value) };
    // SAFETY: a live array; `each` neither modifies it nor unwinds.
    unsafe { swr_zend_hash_foreach_key(array, Some(each), (&raw mut rules).cast()) };
    if rules.bad {
        fatal(E_ERROR, c"The `replacement` must be string");
        return Err(Stop);
    }
    Ok(())
}

swoole_rs_zend::handlers! {
    zim_swoole_server_set => set;
}
