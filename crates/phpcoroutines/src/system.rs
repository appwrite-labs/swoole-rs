//! `Swoole\Coroutine\System` (`swoole_coroutine_system.cc`): argument
//! parsing, validation and results in Rust; the coroutine-aware system calls
//! stay the C++ `swoole::coroutine::System` the hooks module owns, lent as
//! `swr_v1_phpcoroutines_system_ops`.
use crate::host;
use std::{
    ffi::{CStr, c_char, c_int, c_long},
    mem, ptr,
    sync::OnceLock,
};
use swoole_rs_zend::{
    Call, Class, E_WARNING,
    sys::{IS_ARRAY, zend_array, zend_function_entry, zend_string, zend_value_error, zval},
    type_of, value,
};

/// The system operations and the platform constants they take.
#[repr(C)]
pub struct SystemOps {
    pub(crate) af_inet: c_long,
    pub(crate) af_inet6: c_long,
    pub(crate) sock_stream: c_long,
    pub(crate) ipproto_tcp: c_long,
    pub(crate) event_read: c_long,
    sleep: unsafe extern "C" fn(f64) -> c_int,
    read_file: unsafe extern "C" fn(*const c_char, c_long, *mut zval) -> u8,
    write_file: unsafe extern "C" fn(*const c_char, *const c_char, usize, c_long) -> isize,
    gethostbyname: unsafe extern "C" fn(*const c_char, usize, c_long, f64, *mut zval) -> u8,
    clear_dns_cache: unsafe extern "C" fn(),
    getaddrinfo: unsafe extern "C" fn(
        *const c_char,
        c_long,
        c_long,
        c_long,
        *const c_char,
        f64,
        *mut zval,
    ) -> u8,
    statvfs: unsafe extern "C" fn(*const c_char, *mut [c_long; 11]),
    exec: unsafe extern "C" fn(
        *const c_char,
        u8,
        *mut c_int,
        *mut c_int,
        *mut *mut zend_string,
    ) -> u8,
    main_thread: unsafe extern "C" fn() -> u8,
    wait: unsafe extern "C" fn(c_long, f64, *mut c_int, *mut c_int) -> c_long,
    wait_signal: unsafe extern "C" fn(*const c_long, usize, f64) -> c_int,
    convert_to_fd: unsafe extern "C" fn(*mut zval) -> c_int,
    wait_event: unsafe extern "C" fn(c_int, c_long, f64) -> c_long,
}

static OPS: OnceLock<&'static SystemOps> = OnceLock::new();
static SYSTEM: Class = Class::new();

fn ops() -> &'static SystemOps {
    // Methods run only after MINIT stored the table.
    OPS.get().copied().unwrap_or_else(|| std::process::abort())
}

/// MINIT: `Swoole\Coroutine\System` (`Co\System`), not instantiable.
/// # Safety
/// Called once, at MINIT, with a static `PHP_FE_END`-terminated table and a
/// static operations table.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_phpcoroutines_system_minit(
    methods: *const zend_function_entry,
    ops: *const SystemOps,
) {
    // SAFETY: per the caller, a static table.
    if let Some(ops) = unsafe { ops.as_ref() } {
        OPS.get_or_init(|| ops);
    }
    // SAFETY: MINIT, with the caller's table.
    unsafe { SYSTEM.register_static(c"Swoole\\Coroutine\\System", methods) };
    if host::flag(host::SHORTNAME) {
        // SAFETY: MINIT, after registration.
        unsafe { SYSTEM.alias(c"Co\\System") };
    }
}

swoole_rs_zend::handlers! {
    zim_swoole_coroutine_system_sleep => sleep;
    zim_swoole_coroutine_system_readFile => read_file;
    zim_swoole_coroutine_system_writeFile => write_file;
    zif_swoole_coroutine_gethostbyname => gethostbyname;
    zif_swoole_clear_dns_cache => clear_dns_cache;
    zim_swoole_coroutine_system_getaddrinfo => getaddrinfo;
    zim_swoole_coroutine_system_statvfs => statvfs;
    zim_swoole_coroutine_system_exec => exec;
    zim_swoole_coroutine_system_wait => wait;
    zim_swoole_coroutine_system_waitPid => wait_pid;
    zim_swoole_coroutine_system_waitSignal => wait_signal;
    zim_swoole_coroutine_system_waitEvent => wait_event;
}

/// `SW_TIMER_MIN_SEC`.
const TIMER_MIN_SEC: f64 = 0.001;

/// `sleep(float $seconds)`.
fn sleep(call: &Call) {
    let Ok(seconds) = call.arity(1, 1).and_then(|()| call.double(1, 0.0)) else {
        call.set_bool(false);
        return;
    };
    if seconds < TIMER_MIN_SEC {
        host::fatal_error(E_WARNING, c"Timer must be greater than or equal to 0.001");
        call.set_bool(false);
        return;
    }
    // SAFETY: C++ sleeps the running coroutine.
    call.set_bool(unsafe { (ops().sleep)(seconds) } == 0);
}

/// `zend_value_error(message)`, then false.
fn value_error(call: &Call, message: &CStr) {
    // SAFETY: a NUL-terminated message without format directives.
    unsafe { zend_value_error(message.as_ptr()) };
    call.set_bool(false);
}

/// `readFile(string $filename, int $flag = 0)`.
fn read_file(call: &Call) {
    let parsed = call
        .arity(1, 2)
        .and_then(|()| Ok((call.path(1, b"")?, call.long(2, 0)?)));
    let Ok((path, flags)) = parsed else {
        call.set_bool(false);
        return;
    };
    if path.is_empty() {
        value_error(call, c"Filename cannot be empty");
        return;
    }
    // SAFETY: a NUL-terminated path argument and the return slot.
    if unsafe { (ops().read_file)(path.as_ptr().cast(), flags, call.return_value()) } == 0 {
        call.set_bool(false);
    }
}

/// `writeFile(string $filename, string $fileContent, int $flags = 0)`.
fn write_file(call: &Call) {
    let parsed = call
        .arity(2, 3)
        .and_then(|()| Ok((call.path(1, b"")?, call.string(2, b"")?, call.long(3, 0)?)));
    let Ok((path, data, flags)) = parsed else {
        call.set_bool(false);
        return;
    };
    if path.is_empty() {
        value_error(call, c"Filename cannot be empty");
        return;
    }
    // SAFETY: a NUL-terminated path and the data's bytes.
    let written = unsafe {
        (ops().write_file)(
            path.as_ptr().cast(),
            data.as_ptr().cast(),
            data.len(),
            flags,
        )
    };
    if written < 0 {
        call.set_bool(false);
    } else {
        call.set_long(i64::try_from(written).unwrap_or(i64::MAX));
    }
}

fn family_ok(family: c_long) -> bool {
    family == ops().af_inet || family == ops().af_inet6
}

/// `gethostbyname(string $domain, int $family = AF_INET, float $timeout = -1)`.
fn gethostbyname(call: &Call) {
    host::current_safe();
    let parsed = call.arity(1, 3).and_then(|()| {
        Ok((
            call.string(1, b"")?,
            call.long(2, ops().af_inet)?,
            call.double(3, -1.0)?,
        ))
    });
    let Ok((domain, family, timeout)) = parsed else {
        call.set_bool(false);
        return;
    };
    if domain.is_empty() {
        host::fatal_error(E_WARNING, c"domain name is empty");
        call.set_bool(false);
        return;
    }
    if !family_ok(family) {
        host::fatal_error(
            E_WARNING,
            c"unknown protocol family, must be AF_INET or AF_INET6",
        );
        call.set_bool(false);
        return;
    }
    // SAFETY: the domain's bytes and the return slot.
    let found = unsafe {
        (ops().gethostbyname)(
            domain.as_ptr().cast(),
            domain.len(),
            family,
            timeout,
            call.return_value(),
        )
    };
    if found == 0 {
        call.set_bool(false);
    }
}

/// `swoole_clear_dns_cache()`.
fn clear_dns_cache(_: &Call) {
    // SAFETY: C++ clears the cache.
    unsafe { (ops().clear_dns_cache)() };
}

/// `getaddrinfo(string $domain, int $family = AF_INET, int $socktype = SOCK_STREAM,
/// int $protocol = IPPROTO_TCP, ?string $service = null, float $timeout = -1)`.
fn getaddrinfo(call: &Call) {
    let ops = ops();
    let parsed = call.arity(1, 6).and_then(|()| {
        Ok((
            call.string(1, b"")?,
            call.long(2, ops.af_inet)?,
            call.long(3, ops.sock_stream)?,
            call.long(4, ops.ipproto_tcp)?,
            call.string(5, b"\0")?,
            call.double(6, -1.0)?,
        ))
    });
    let Ok((host_name, family, socktype, protocol, service, timeout)) = parsed else {
        call.set_bool(false);
        return;
    };
    if host_name.is_empty() {
        host::fatal_error(E_WARNING, c"hostname is empty");
        call.set_bool(false);
        return;
    }
    if !family_ok(family) {
        host::fatal_error(
            E_WARNING,
            c"unknown protocol family, must be AF_INET or AF_INET6",
        );
        call.set_bool(false);
        return;
    }
    // SAFETY: NUL-terminated strings and the return slot.
    let found = unsafe {
        (ops.getaddrinfo)(
            host_name.as_ptr().cast(),
            family,
            socktype,
            protocol,
            service.as_ptr().cast(),
            timeout,
            call.return_value(),
        )
    };
    if found == 0 {
        call.set_bool(false);
    }
}

/// `statvfs(string $path)`.
fn statvfs(call: &Call) {
    let Ok(path) = call.arity(1, 1).and_then(|()| call.path(1, b"")) else {
        call.set_bool(false);
        return;
    };
    if path.is_empty() {
        value_error(call, c"Path cannot be empty");
        return;
    }
    let mut fields: [c_long; 11] = [0; 11];
    // SAFETY: a NUL-terminated path and a writable array.
    unsafe { (ops().statvfs)(path.as_ptr().cast(), &raw mut fields) };
    call.set_array();
    let names = [
        c"bsize", c"frsize", c"blocks", c"bfree", c"bavail", c"files", c"ffree", c"favail",
        c"fsid", c"flag", c"namemax",
    ];
    for (name, value) in names.into_iter().zip(fields) {
        call.add_assoc_long(name, value);
    }
}

/// `exec(string $command, bool $get_error_stream = false)`.
fn exec(call: &Call) {
    let parsed = call
        .arity(1, 2)
        .and_then(|()| Ok((call.string(1, b"")?, call.bool(2, false)?)));
    let Ok((command, errors)) = parsed else {
        call.set_bool(false);
        return;
    };
    let (mut code, mut signal, mut output) = (0, 0, ptr::null_mut());
    // SAFETY: a NUL-terminated command and writable outputs.
    let ran = unsafe {
        (ops().exec)(
            command.as_ptr().cast(),
            u8::from(errors),
            &raw mut code,
            &raw mut signal,
            &raw mut output,
        )
    };
    if ran == 0 {
        call.set_bool(false);
        return;
    }
    call.set_array();
    call.add_assoc_long(c"code", code.into());
    call.add_assoc_long(c"signal", signal.into());
    // SAFETY: all-zero is IS_UNDEF, then the output string.
    let mut data: zval = unsafe { mem::zeroed() };
    // SAFETY: the zval takes the new string's reference.
    unsafe { value::set_str(&raw mut data, output) };
    // SAFETY: the return array takes the string.
    unsafe {
        swoole_rs_zend::sys::add_assoc_zval_ex(
            call.return_value(),
            c"output".as_ptr(),
            6,
            &raw mut data,
        );
    }
}

/// `SW_MUST_BE_MAIN_THREAD()`: off the main thread, the method returns true.
fn main_thread(call: &Call) -> bool {
    // SAFETY: C++ checks the thread and sets the last error.
    let main = unsafe { (ops().main_thread)() } != 0;
    if !main {
        call.set_bool(true);
    }
    main
}

/// `swoole_coroutine_system_wait`.
fn wait_for(call: &Call, pid: c_long, timeout: f64) {
    host::current_safe();
    let (mut code, mut signal) = (0, 0);
    // SAFETY: C++ waits in the running coroutine.
    let pid = unsafe { (ops().wait)(pid, timeout, &raw mut code, &raw mut signal) };
    if pid <= 0 {
        call.set_bool(false);
        return;
    }
    call.set_array();
    call.add_assoc_long(c"pid", pid);
    call.add_assoc_long(c"code", code.into());
    call.add_assoc_long(c"signal", signal.into());
}

/// `wait(float $timeout = -1)`.
fn wait(call: &Call) {
    if !main_thread(call) {
        return;
    }
    let Ok(timeout) = call.arity(0, 1).and_then(|()| call.double(1, -1.0)) else {
        call.set_bool(false);
        return;
    };
    wait_for(call, -1, timeout);
}

/// `waitPid(int $pid, float $timeout = -1)`.
fn wait_pid(call: &Call) {
    if !main_thread(call) {
        return;
    }
    let parsed = call
        .arity(1, 2)
        .and_then(|()| Ok((call.long(1, 0)?, call.double(2, -1.0)?)));
    let Ok((pid, timeout)) = parsed else {
        call.set_bool(false);
        return;
    };
    wait_for(call, pid, timeout);
}

/// `waitSignal(int|array $signals, float $timeout = -1)`.
fn wait_signal(call: &Call) {
    if !main_thread(call) {
        return;
    }
    let Ok(timeout) = call.arity(1, 2).and_then(|()| call.double(2, -1.0)) else {
        call.set_bool(false);
        return;
    };
    let given = call.zval(1);
    let mut signals = Vec::new();
    // SAFETY: a passed argument.
    if unsafe { type_of(given) } == IS_ARRAY {
        // SAFETY: an array argument.
        let table: *const zend_array = unsafe { value::table(given) };
        let mut each = |item: *mut zval| {
            // SAFETY: a live element.
            signals.push(unsafe { value::get_long(item) });
            true
        };
        // SAFETY: the array is live and the closure does not modify it.
        unsafe { swoole_rs_zend::each_value(table, &mut each) };
    } else {
        // SAFETY: a passed argument.
        signals.push(unsafe { value::get_long(given) });
    }
    if swoole_rs_zend::bailout::pending() {
        return;
    }
    // Held as a raw box while the coroutine waits, so no frame here owns a drop
    // if a fatal error ends the wait.
    let signals = Box::into_raw(signals.into_boxed_slice());
    // SAFETY: C++ waits for one of the signals; on failure it warns and sets errno.
    let signo = unsafe { (ops().wait_signal)(signals.cast::<c_long>(), signals.len(), timeout) };
    // SAFETY: boxed above, not used again.
    drop(unsafe { Box::from_raw(signals) });
    if signo == -1 {
        call.set_bool(false);
    } else {
        call.set_long(signo.into());
    }
}

/// `waitEvent(mixed $socket, int $events = SWOOLE_EVENT_READ, float $timeout = -1)`.
fn wait_event(call: &Call) {
    let parsed = call
        .arity(1, 3)
        .and_then(|()| Ok((call.long(2, ops().event_read)?, call.double(3, -1.0)?)));
    let Ok((events, timeout)) = parsed else {
        call.set_bool(false);
        return;
    };
    // SAFETY: a passed argument.
    let fd = unsafe { (ops().convert_to_fd)(call.zval(1)) };
    if fd < 0 {
        host::fatal_error(E_WARNING, c"unknown fd type");
        call.set_bool(false);
        return;
    }
    // SAFETY: C++ waits on the descriptor.
    let events = unsafe { (ops().wait_event)(fd, events, timeout) };
    if events < 0 {
        call.set_bool(false);
    } else {
        call.set_long(events);
    }
}
