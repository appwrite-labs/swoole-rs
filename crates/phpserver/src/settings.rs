//! `Swoole\Server::set()`: every option's lookup, conversion, bounds and the
//! order upstream reads them in, written straight into the Server's Rust
//! records, including the build switches config.h would define.
use crate::lifecycle::ServerObject;
use crate::server::{display_errors, fatal, server, set_last_error};
use std::{
    ffi::{CStr, CString, c_int},
    ptr,
    sync::atomic::Ordering,
};
use swoole_rs_server::{server_methods, view};
use swoole_rs_zend::host::callable;
use swoole_rs_zend::{
    Array, Call, E_ERROR, E_WARNING, Value, bailout, message, report,
    sys::{swr_zend_callable_warning, swr_zend_try_facade_update},
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

/// `serv->reload_async = true` (`on_before_start` with coroutines on).
pub(crate) fn set_reload_async(serv: view::Server<'_>) {
    // SAFETY: the server is configured on this thread, with no other reference to the field.
    unsafe { serv.config_mut() }.reload_async = true;
}

/// The Server field `id` (`field::*`), read as C++ reads it.
fn fact(serv: view::Server<'_>, id: u32) -> i64 {
    use field as f;
    let config = serv.config();
    match id {
        f::STARTED => i64::from(serv.is_started()),
        f::WORKER_THREAD => i64::from(serv.is_worker_thread()),
        f::WORKER_MAX_CONCURRENCY => i64::from(config.worker_max_concurrency),
        f::MAX_CONCURRENCY => i64::from(serv.counters().max_concurrency),
        f::ENABLE_COROUTINE => i64::from(config.enable_coroutine),
        f::DISPATCH_MODE => i64::from(config.dispatch_mode),
        f::TASK_IPC_MODE => i64::from(config.task_ipc_mode),
        f::TASK_ENABLE_COROUTINE => i64::from(config.task_enable_coroutine),
        f::CPU_NUM => i64::from(swoole_rs_corebase::swr_v1_core_cpu_num()),
        f::GLOBAL_ENABLE_COROUTINE => i64::from(swoole_rs_corebase::swr_v1_core_enable_coroutine()),
        f::PATH_MAX => i64::from(libc::PATH_MAX),
        f::HAVE_CPU_AFFINITY => i64::from(cfg!(any(
            target_os = "linux",
            target_os = "android",
            target_os = "freebsd"
        ))),
        // configure's probes, as crates/ext/build.rs lists them per target OS;
        // `set()` only reads `HAVE_REUSEPORT` on Linux.
        f::HAVE_REUSE_PORT => i64::from(cfg!(target_os = "linux")),
        f::HAVE_MSGQUEUE => i64::from(cfg!(any(target_os = "linux", target_os = "macos"))),
        // config.m4 links zlib into every build.
        f::HAVE_COMPRESSION | f::HAVE_ZLIB => 1,
        f::THREAD => i64::from(crate::server::THREAD.load(Ordering::Relaxed)),
        _ => 0,
    }
}

/// `serv-><field> = v` for the Server field `id`, converted as C++ assigns it.
///
/// # Safety
/// `serv` has not started.
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "C++'s implicit conversions of the int64_t setting to each field's type"
)]
unsafe fn apply(serv: view::Server<'_>, id: u32, v: i64) {
    use field as f;
    // SAFETY: the server has not started, per the caller; nothing else holds its config.
    let c = unsafe { serv.config_mut() };
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
        f::MAX_CONCURRENCY => unsafe { serv.counters_mut() }.max_concurrency = limit,
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
        f::MAX_CONNECTION => server_methods::set_max_connection(serv, v as u32),
        f::START_SESSION_ID => {
            // SAFETY: per the caller, as above.
            let counters = unsafe { serv.counters_mut() };
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
pub(crate) struct Settings<'a> {
    pub(crate) serv: view::Server<'a>,
    pub(crate) vht: Option<&'a Array>,
}

/// Stop: a bailout is pending (upstream never ran past it).
pub(crate) struct Stop;

pub(crate) type Step = Result<(), Stop>;

pub(crate) fn checked() -> Step {
    if bailout::pending() {
        Err(Stop)
    } else {
        Ok(())
    }
}

impl<'a> Settings<'a> {
    /// The server being configured.
    pub(crate) fn server(&self) -> view::Server<'_> {
        self.serv
    }

    /// `php_swoole_array_get_value(vht, key, ztmp)`: present and not null.
    pub(crate) fn get(&self, key: &CStr) -> Option<&'a Value> {
        self.vht?.get(key.to_bytes())
    }

    fn get_any(&self, keys: &[&CStr]) -> Option<&Value> {
        keys.iter().find_map(|key| self.get(key))
    }

    fn fact(&self, id: u32) -> i64 {
        fact(self.serv, id)
    }

    fn set(&self, id: u32, value: i64) {
        // SAFETY: a live server, not started (set() checked).
        unsafe { apply(self.serv, id, value) };
    }

    pub(crate) fn long(value: &Value) -> Result<i64, Stop> {
        let value = value.long();
        checked().map(|()| value)
    }

    pub(crate) fn size(value: &Value) -> Result<i64, Stop> {
        let value = swoole_rs_phpswoole::php_swoole_parse_to_size(value);
        checked().map(|()| value)
    }

    pub(crate) fn truth(value: &Value) -> i64 {
        i64::from(value.truthy())
    }

    /// `zend::String(value)`, handed to `each` and released; `each` doesn't run
    /// after the conversion's bailout, as upstream's code after it didn't.
    pub(crate) fn string<R>(value: &Value, each: impl FnOnce(&[u8]) -> R) -> Result<R, Stop> {
        let string = value.string();
        checked()?;
        Ok(each(string.bytes()))
    }

    /// `serv-><text> = value` (`view::set_text`'s `index`).
    fn text(&self, index: usize, value: &[u8]) {
        // SAFETY: a live server, not started (set() checked).
        unsafe { self.server().set_text(index, value) };
    }

    /// `insert(value)` into static collection `kind` (`view::insert_static_string`).
    fn insert(&self, kind: u32, value: &[u8]) {
        self.server().insert_static_string(kind, value);
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
    fn string_option(&self, key: &CStr, each: impl FnOnce(&[u8])) -> Step {
        if let Some(value) = self.get(key) {
            Self::string(value, each)?;
        }
        Ok(())
    }

    /// Each value of an array option, as a string; `E_ERROR` otherwise.
    fn list(&self, keys: &[&CStr], error: &CStr, each: impl Fn(&[u8])) -> Step {
        let Some(value) = self.get_any(keys) else {
            return Ok(());
        };
        let Some(array) = value.array() else {
            fatal(E_ERROR, error);
            return Err(Stop);
        };
        let items: Vec<&Value> = array.entries().map(|(_, item)| item).collect();
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
        bailout::defer(report(E_WARNING, message));
    }
    checked()
}

fn set(call: &Call) {
    let serv = server(call);
    let s = Settings { serv, vht: None };
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
    let s = Settings {
        serv,
        vht: vht.array(),
    };
    // Every early exit after an `E_ERROR` or a deferred bailout leaves the
    // handler, whose last step raises it.
    let this = ServerObject::this(call);
    if let Ok(true) = options(&s, this) {
        match finish(this, vht) {
            Ok(()) => call.set_bool(true),
            Err(bailed) => bailed.defer(),
        }
    } else if !bailout::pending() {
        call.set_bool(false);
    }
}

/// `$this->ports[0]->set($settings)` (Port's own method), then the merge into
/// `$this->setting`.
fn finish(this: ServerObject, settings: &Value) -> Result<(), bailout::Bailout> {
    let port = this.port_at(0);
    // SAFETY: the live Port object, its class and the settings argument.
    unsafe {
        bailout::call_method(
            port,
            crate::PORT.entry(),
            b"set",
            ptr::null_mut(),
            &[settings.as_ptr()],
        )
    }?;
    crate::port::merge_setting(crate::server::SERVER.entry(), this.raw(), settings.as_ptr())
        .map(|_| ())
}

/// The options, in upstream's order. `Ok(false)`: return false.
fn options(s: &Settings, this: ServerObject) -> Result<bool, Stop> {
    use field as f;
    // php_swoole_set_global_option, _coroutine_option and _aio_option; the
    // first defers its fatal error for a size that does not parse.
    let Some(vht) = s.vht else {
        return Ok(true);
    };
    if !swoole_rs_phpswoole::set_global(vht) {
        return Err(Stop);
    }
    swoole_rs_phpcoroutines::set_coroutine_option(vht);
    checked()?;
    swoole_rs_phpswoole::php_swoole_set_aio_option(vht);
    s.string_option(c"chroot", |v| s.text(0, v))?;
    s.string_option(c"user", |v| s.text(1, v))?;
    s.string_option(c"group", |v| s.text(2, v))?;
    s.flag(c"daemonize", f::DAEMONIZE);
    s.string_option(c"pid_file", |v| s.text(3, v))?;
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
        let v = value.double();
        checked()?;
        // SAFETY: the live server being configured; nothing else holds its config.
        unsafe { s.server().config_mut() }.send_timeout = v;
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
        // SAFETY: a live setting; `sw_callable_create`, whose bailouts stay deferred.
        let created = unsafe {
            callable::factory(
                value.as_ptr(),
                ptr::null(),
                true,
                ptr::null_mut(),
                swr_zend_callable_warning,
            )
        };
        let set = created.and_then(|handle| {
            if handle.is_null() {
                return Ok(());
            }
            // SAFETY: a live server, not started.
            unsafe { crate::dispatch::set_dispatch_func(s.serv, handle) }
        });
        if set.is_err() {
            bailout::defer(set);
            return Err(Stop);
        }
    }
    workers(s)?;
    http(s, this)
}

/// `bootstrap` into `$this->bootstrap`, and `init_arguments` kept on the object
/// (uncounted, as upstream's `server_object->init_arguments = *ztmp`).
fn thread_options(s: &Settings, this: ServerObject) -> Step {
    if let Some(value) = s.get(c"bootstrap") {
        let (ce, object) = (this.class(), this.raw());
        // SAFETY: `$this` of its class and a live value, which the property
        // copies; the shim traps a bailout.
        let bailed = unsafe {
            swr_zend_try_facade_update(ce, object, c"bootstrap".as_ptr(), 9, value.as_ptr())
        };
        if bailed != 0 {
            bailout::Bailout.defer();
            return Err(Stop);
        }
    }
    if let Some(value) = s.get(c"init_arguments") {
        // SAFETY: a live value.
        let value = unsafe { value.as_ptr().read() };
        this.set_init_arguments(value);
    }
    Ok(())
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
    s.string_option(c"task_tmpdir", |dir| {
        let dir = dir.split(|&b| b == 0).next().unwrap_or_default();
        swoole_rs_phpswoole::set_task_tmpdir(&CString::new(dir).unwrap_or_default());
    })?;
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

#[cfg(any(target_os = "linux", target_os = "android"))]
type CpuSet = libc::cpu_set_t;
#[cfg(target_os = "freebsd")]
type CpuSet = libc::cpuset_t;

/// The CPUs `swoole_get_cpu_affinity` allows this process, or its errno.
#[cfg(any(target_os = "linux", target_os = "android", target_os = "freebsd"))]
fn affinity() -> Result<Vec<c_int>, c_int> {
    /// `CPU_SETSIZE`.
    const CPU_SETSIZE: usize = 1024;
    // SAFETY: a plain bitset, for which zero is the empty set.
    let mut set: CpuSet = unsafe { std::mem::zeroed() };
    if swoole_rs_ossupport::swoole_get_cpu_affinity(&mut set) < 0 {
        return Err(std::io::Error::last_os_error().raw_os_error().unwrap_or(0));
    }
    Ok((0..CPU_SETSIZE)
        // SAFETY: `cpu` is below `CPU_SETSIZE`, inside the set.
        .filter(|&cpu| unsafe { libc::CPU_ISSET(cpu, &set) })
        .filter_map(|cpu| c_int::try_from(cpu).ok())
        .collect())
}

/// No `HAVE_CPU_AFFINITY`: `set()` never asks.
#[cfg(not(any(target_os = "linux", target_os = "android", target_os = "freebsd")))]
#[expect(
    clippy::unnecessary_wraps,
    reason = "the signature the affinity targets share"
)]
fn affinity() -> Result<Vec<c_int>, c_int> {
    Ok(Vec::new())
}

/// `php_swoole_server_set_cpu_affinity_ignore`: the CPUs left to bind workers
/// to once `value`'s are taken out of this process's affinity. False: rejected.
fn cpu_affinity_ignore(s: &Settings, value: &Value) -> Result<bool, Stop> {
    let Some(ignored) = value.array() else {
        fatal(E_ERROR, c"cpu_affinity_ignore must be array");
        return Err(Stop);
    };
    let mut cpus = match affinity() {
        Ok(cpus) => cpus,
        Err(error) => {
            // php_swoole_sys_error(E_WARNING, "sched_getaffinity() failed")
            set_last_error(error);
            if display_errors() {
                let reason = swoole_rs_phpruntime::strerror(error);
                let text = message!(
                    "sched_getaffinity() failed, Error: {}[{error}]",
                    reason.to_string_lossy()
                );
                bailout::defer(report(E_WARNING, text.as_cstr()));
            }
            return checked().map(|()| false);
        }
    };
    let items: Vec<&Value> = ignored.entries().map(|(_, item)| item).collect();
    for item in items {
        let cpu = Settings::long(item)?;
        cpus.retain(|&available| i64::from(available) != cpu);
    }
    if cpus.is_empty() {
        fatal(E_ERROR, c"cpu_affinity_ignore excludes all available CPUs");
        return Err(Stop);
    }
    let bytes = size_of_val(cpus.as_slice());
    let available = swoole_rs_corebase::sw_malloc(bytes).cast::<c_int>();
    // SAFETY: a fresh allocation of `bytes`, apart from `cpus`.
    unsafe { ptr::copy_nonoverlapping(cpus.as_ptr(), available, cpus.len()) };
    // SAFETY: the live server being configured; nothing else holds its config.
    let c = unsafe { s.server().config_mut() };
    c.cpu_affinity_available_num = c_int::try_from(cpus.len()).unwrap_or(c_int::MAX);
    // SAFETY: null or the previous `sw_malloc` list, which nothing else holds.
    unsafe { swoole_rs_corebase::sw_free(c.cpu_affinity_available.cast()) };
    c.cpu_affinity_available = available;
    Ok(true)
}

/// The HTTP, buffer, queue and thread options, and the final checks.
fn http(s: &Settings, this: ServerObject) -> Result<bool, Stop> {
    use field as f;
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
            let created = crate::instance::php_swoole_create_dir(dir);
            if created {
                s.server().set_upload_tmp_dir(dir);
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
            server_methods::set_document_root(s.serv, root);
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
                s.insert(1, name);
            }
        },
    )?;
    s.list(
        &[c"http_compression_types", c"compression_types"],
        c"http_compression_types must be array",
        |kind| {
            if !kind.is_empty() && s.fact(f::HAVE_COMPRESSION) != 0 {
                s.insert(2, kind);
            }
        },
    )?;
    s.list(
        &[c"static_handler_locations"],
        c"static_handler_locations num must be array",
        |location| {
            if location.first() == Some(&b'/') {
                s.insert(0, location);
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
        thread_options(s, this)?;
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
    let Some(value) = s.get(c"url_rewrite_rules") else {
        return Ok(());
    };
    let Some(array) = value.array() else {
        fatal(E_ERROR, c"The `url_rewrite_rules` must be array");
        return Err(Stop);
    };
    for (key, value) in array.entries() {
        let Some(replacement) = value.dereferenced().bytes() else {
            fatal(E_ERROR, c"The `replacement` must be string");
            return Err(Stop);
        };
        // An integer key has no pattern; upstream reads a null string there.
        let pattern = key.unwrap_or_default();
        s.server().add_rewrite_rule(pattern, replacement);
    }
    Ok(())
}

swoole_rs_zend::handlers! {
    zim_swoole_server_set => set;
}
