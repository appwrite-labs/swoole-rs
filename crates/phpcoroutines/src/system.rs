//! `Swoole\Coroutine\System` (upstream's `swoole_coroutine_system.cc`):
//! argument parsing, validation and results in Rust. The coroutine-aware
//! system calls stay the C++ `swoole::coroutine::System`: its static members
//! by their own symbols, and `swr_v1_system_*` (in `crates/hooks/src/resolve.rs`)
//! where a C++ string or list carries the result.
use crate::host;
use std::{
    ffi::{CStr, CString, c_char, c_int, c_long, c_void},
    mem, ptr, slice,
};
use swoole_rs_buffer::string;
use swoole_rs_corebase::log::last_error;
use swoole_rs_hooks::exec::exec as exec_into;
use swoole_rs_phpstubs::coroutine_system;
use swoole_rs_reactor::wait_upstream::{system_wait, system_waitpid};
use swoole_rs_zend::{
    Call, Class, E_WARNING,
    sys::{IS_ARRAY, zend_value_error, zval},
    type_of, value,
};
use swoole_rs_zend::{method_table, sys as z};

use swoole_rs_hooks::resolve::{swr_v1_system_getaddrinfo, swr_v1_system_gethostbyname};
use swoole_rs_hooks::sleep::system_sleep;
use swoole_rs_hooks::stdio::swoole_coroutine_statvfs;
use swoole_rs_hooks::wait_event::system_wait_event;
// `swoole_async_dns_lookup_coro()`, which crates/phpswoole defines.
pub(crate) use swoole_rs_phpswoole::zif_swoole_async_dns_lookup_coro;

unsafe extern "C" {
    // crates/phpevents': declared, since crates/phpevents depends on this crate.
    fn php_swoole_convert_to_fd(value: *mut zval) -> c_int;
}

/// `SW_EVENT_READ`.
const EVENT_READ: c_long = 1 << 9;
/// `SW_FILE_LOCK`, the `FILE_LOCK` constant.
const FILE_LOCK: c_long = libc::LOCK_EX as c_long;
/// `SW_FILE_APPEND`: PHP's `FILE_APPEND`.
const FILE_APPEND: c_long = 1 << 3;
/// `SW_ERROR_OPERATION_NOT_SUPPORT`.
const SW_ERROR_OPERATION_NOT_SUPPORT: c_int = 507;

static SYSTEM: Class = Class::new();

method_table! {
    static METHODS = [
        c"gethostbyname" => zif_swoole_coroutine_gethostbyname, &coroutine_system::arginfo_class_Swoole_Coroutine_System_gethostbyname, z::ZEND_ACC_PUBLIC | z::ZEND_ACC_STATIC;
        c"dnsLookup" => zif_swoole_async_dns_lookup_coro, &coroutine_system::arginfo_class_Swoole_Coroutine_System_dnsLookup, z::ZEND_ACC_PUBLIC | z::ZEND_ACC_STATIC;
        c"exec" => zim_swoole_coroutine_system_exec, &coroutine_system::arginfo_class_Swoole_Coroutine_System_exec, z::ZEND_ACC_PUBLIC | z::ZEND_ACC_STATIC;
        c"sleep" => zim_swoole_coroutine_system_sleep, &coroutine_system::arginfo_class_Swoole_Coroutine_System_sleep, z::ZEND_ACC_PUBLIC | z::ZEND_ACC_STATIC;
        c"getaddrinfo" => zim_swoole_coroutine_system_getaddrinfo, &coroutine_system::arginfo_class_Swoole_Coroutine_System_getaddrinfo, z::ZEND_ACC_PUBLIC | z::ZEND_ACC_STATIC;
        c"statvfs" => zim_swoole_coroutine_system_statvfs, &coroutine_system::arginfo_class_Swoole_Coroutine_System_statvfs, z::ZEND_ACC_PUBLIC | z::ZEND_ACC_STATIC;
        c"readFile" => zim_swoole_coroutine_system_readFile, &coroutine_system::arginfo_class_Swoole_Coroutine_System_readFile, z::ZEND_ACC_PUBLIC | z::ZEND_ACC_STATIC;
        c"writeFile" => zim_swoole_coroutine_system_writeFile, &coroutine_system::arginfo_class_Swoole_Coroutine_System_writeFile, z::ZEND_ACC_PUBLIC | z::ZEND_ACC_STATIC;
        c"wait" => zim_swoole_coroutine_system_wait, &coroutine_system::arginfo_class_Swoole_Coroutine_System_wait, z::ZEND_ACC_PUBLIC | z::ZEND_ACC_STATIC;
        c"waitPid" => zim_swoole_coroutine_system_waitPid, &coroutine_system::arginfo_class_Swoole_Coroutine_System_waitPid, z::ZEND_ACC_PUBLIC | z::ZEND_ACC_STATIC;
        c"waitSignal" => zim_swoole_coroutine_system_waitSignal, &coroutine_system::arginfo_class_Swoole_Coroutine_System_waitSignal, z::ZEND_ACC_PUBLIC | z::ZEND_ACC_STATIC;
        c"waitEvent" => zim_swoole_coroutine_system_waitEvent, &coroutine_system::arginfo_class_Swoole_Coroutine_System_waitEvent, z::ZEND_ACC_PUBLIC | z::ZEND_ACC_STATIC;
    ];
}

/// MINIT: `Swoole\Coroutine\System` (`Co\System`), not
/// instantiable, and the `FILE_LOCK` constant.
/// # Safety
/// Called once, at MINIT, with the extension's module number.
pub unsafe extern "C" fn php_swoole_coroutine_system_minit(module_number: c_int) {
    // SAFETY: MINIT, with a static table.
    unsafe { SYSTEM.register_static(c"Swoole\\Coroutine\\System", METHODS.as_ptr()) };
    if host::flag(host::SHORTNAME) {
        // SAFETY: MINIT, after registration.
        if let Err(error) = unsafe { SYSTEM.alias(c"Co\\System") } {
            error.defer();
            swoole_rs_zend::bailout::finish();
        }
    }
    // SAFETY: MINIT, with the module number.
    unsafe { swoole_rs_zend::register_long_constant(c"FILE_LOCK", FILE_LOCK, module_number) };
}

/// `RETVAL_STRINGL`: `out` (an empty zval) takes a copy of the string.
unsafe extern "C" fn put_string(out: *mut c_void, data: *const c_char, length: usize) {
    // SAFETY: C++ lends `length` readable bytes.
    let bytes = unsafe { slice::from_raw_parts(data.cast::<u8>(), length) };
    // SAFETY: `out` holds nothing that needs releasing.
    unsafe { value::set_str(out.cast(), swoole_rs_zend::string::copy(bytes)) };
}

/// `add_next_index_stringl`, after `array_init` if `out` is not yet an array.
unsafe extern "C" fn push_string(out: *mut c_void, data: *const c_char, length: usize) {
    let out = out.cast::<zval>();
    // SAFETY: the return slot.
    if unsafe { type_of(out) } != IS_ARRAY {
        // SAFETY: an empty return slot.
        unsafe { value::array_init(out) };
    }
    // SAFETY: C++ lends `length` readable bytes.
    let bytes = unsafe { slice::from_raw_parts(data.cast::<u8>(), length) };
    // SAFETY: an array; it takes the new string.
    unsafe { swoole_rs_zend::push_string(out, swoole_rs_zend::string::copy(bytes)) };
}

/// C's implicit `long` to `int` conversion, as upstream's calls make it.
#[expect(
    clippy::cast_possible_truncation,
    reason = "upstream's implicit long to int conversion"
)]
fn int(value: c_long) -> c_int {
    value as c_int
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
    call.set_bool(unsafe { system_sleep(seconds) } == 0);
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
    let Ok(path) = CString::new(path) else {
        call.set_bool(false);
        return;
    };
    match swoole_rs_hooks::file_io::read_file(&path, flags & FILE_LOCK != 0) {
        // SAFETY: a new string, whose reference the return slot takes.
        Some(content) => unsafe { call.set_string(swoole_rs_zend::string::copy(&content)) },
        None => call.set_bool(false),
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
    let mode = if flags & FILE_APPEND != 0 {
        libc::O_APPEND
    } else {
        libc::O_TRUNC
    };
    let Ok(path) = CString::new(path) else {
        call.set_bool(false);
        return;
    };
    let written = swoole_rs_hooks::file_io::write_file(
        &path,
        data,
        flags & FILE_LOCK != 0,
        libc::O_NOFOLLOW | mode,
    );
    if written < 0 {
        call.set_bool(false);
    } else {
        call.set_long(i64::try_from(written).unwrap_or(i64::MAX));
    }
}

fn family_ok(family: c_long) -> bool {
    family == AF_INET || family == c_long::from(libc::AF_INET6)
}

const AF_INET: c_long = libc::AF_INET as c_long;

/// `gethostbyname(string $domain, int $family = AF_INET, float $timeout = -1)`.
fn gethostbyname(call: &Call) {
    host::current_safe();
    let parsed = call.arity(1, 3).and_then(|()| {
        Ok((
            call.string(1, b"")?,
            call.long(2, AF_INET)?,
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
    let (name, length, out) = (
        domain.as_ptr().cast(),
        domain.len(),
        call.return_value().cast(),
    );
    // SAFETY: the domain's bytes and the empty return slot.
    if !unsafe { swr_v1_system_gethostbyname(name, length, family, timeout, put_string, out) } {
        call.set_bool(false);
    }
}

/// `swoole_clear_dns_cache()`.
fn clear_dns_cache(_: &Call) {
    swoole_rs_hooks::dns_cache::swr_v1_hook_dns_cache_clear();
}

/// `getaddrinfo(string $domain, int $family = AF_INET, int $socktype = SOCK_STREAM,
/// int $protocol = IPPROTO_TCP, ?string $service = null, float $timeout = -1)`.
fn getaddrinfo(call: &Call) {
    let parsed = call.arity(1, 6).and_then(|()| {
        Ok((
            call.string(1, b"")?,
            call.long(2, AF_INET)?,
            call.long(3, c_long::from(libc::SOCK_STREAM))?,
            call.long(4, c_long::from(libc::IPPROTO_TCP))?,
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
    // SAFETY: NUL-terminated strings and the empty return slot.
    let found = unsafe {
        swr_v1_system_getaddrinfo(
            host_name.as_ptr().cast(),
            family,
            socktype,
            protocol,
            service.as_ptr().cast(),
            timeout,
            push_string,
            call.return_value().cast(),
        )
    };
    if !found {
        call.set_bool(false);
    }
}

/// Upstream's `(long)` cast of each `statvfs` field.
#[expect(clippy::cast_possible_wrap, reason = "upstream's (long) cast")]
fn field(value: impl Into<u64>) -> c_long {
    value.into() as c_long
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
    // SAFETY: all-zero is a valid statvfs.
    let mut stat: libc::statvfs = unsafe { mem::zeroed() };
    // SAFETY: a NUL-terminated path and a writable struct; on failure it stays zeroed.
    unsafe { swoole_coroutine_statvfs(path.as_ptr().cast(), &raw mut stat) };
    call.set_array();
    let fields = [
        (c"bsize", field(stat.f_bsize)),
        (c"frsize", field(stat.f_frsize)),
        (c"blocks", field(stat.f_blocks)),
        (c"bfree", field(stat.f_bfree)),
        (c"bavail", field(stat.f_bavail)),
        (c"files", field(stat.f_files)),
        (c"ffree", field(stat.f_ffree)),
        (c"favail", field(stat.f_favail)),
        (c"fsid", field(stat.f_fsid)),
        (c"flag", field(stat.f_flag)),
        (c"namemax", field(stat.f_namemax)),
    ];
    for (name, value) in fields {
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
    let mut status = 0;
    // SAFETY: a fresh String, freed below.
    let buffer = unsafe { string::new_sized_string(1024, ptr::null()) };
    // SAFETY: live until the delete below, and only this frame's.
    let output = unsafe { &mut *buffer };
    // SAFETY: a zend string is NUL-terminated.
    let command = unsafe { CStr::from_ptr(command.as_ptr().cast()) };
    // SAFETY: in a method call, which raises the fatal error outside a coroutine.
    let done = unsafe { exec_into(command, errors, output, &mut status) };
    let mut output_value = swoole_rs_zend::UNDEF;
    if done {
        // SAFETY: the String's bytes, copied into an empty slot.
        unsafe {
            put_string(
                (&raw mut output_value).cast(),
                output.str.cast(),
                output.length,
            );
        };
    }
    // SAFETY: from new_sized_string, not used again.
    unsafe { string::delete_string(buffer) };
    if !done {
        call.set_bool(false);
        return;
    }
    call.set_array();
    call.add_assoc_long(c"code", libc::WEXITSTATUS(status).into());
    call.add_assoc_long(c"signal", libc::WTERMSIG(status).into());
    // SAFETY: the return array takes the string.
    unsafe {
        swoole_rs_zend::sys::add_assoc_zval_ex(
            call.return_value(),
            c"output".as_ptr(),
            6,
            &raw mut output_value,
        );
    }
}

/// `SW_MUST_BE_MAIN_THREAD_EX(RETURN_TRUE)`: with `SW_THREAD`, off the main
/// thread, the method sets the last error and returns true.
fn main_thread(call: &Call) -> bool {
    if host::flag(host::THREAD) && !host::flag(host::MAIN_THREAD) {
        host::set_last_error(SW_ERROR_OPERATION_NOT_SUPPORT);
        call.set_bool(true);
        return false;
    }
    true
}

/// `swoole_coroutine_system_wait`.
fn wait_for(call: &Call, pid: c_long, timeout: f64) {
    host::current_safe();
    let mut status = 0;
    let pid = c_long::from(if pid < 0 {
        // SAFETY: waits in the running coroutine.
        unsafe { system_wait(&raw mut status, timeout) }
    } else {
        // SAFETY: as above.
        unsafe { system_waitpid(int(pid), &raw mut status, 0, timeout) }
    });
    if pid <= 0 {
        call.set_bool(false);
        return;
    }
    call.set_array();
    call.add_assoc_long(c"pid", pid);
    call.add_assoc_long(c"code", libc::WEXITSTATUS(status).into());
    call.add_assoc_long(c"signal", libc::WTERMSIG(status).into());
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
    let mut signals = Vec::new();
    if let Some(given) = call.value(1) {
        if let Some(array) = given.array() {
            signals.extend(array.entries().map(|(_, item)| item.long()));
        } else {
            signals.push(given.long());
        }
    }
    if swoole_rs_zend::bailout::pending() {
        return;
    }
    // Held as a raw box while the coroutine waits, so no frame here owns a drop
    // if a fatal error ends the wait.
    let signals = Box::into_raw(signals.into_boxed_slice());
    // SAFETY: boxed above; only this call reads it.
    let signo = swoole_rs_hooks::wait_signal::system_wait_signal(unsafe { &*signals }, timeout);
    // SAFETY: boxed above, not used again.
    drop(unsafe { Box::from_raw(signals) });
    if signo != -1 {
        call.set_long(signo.into());
        return;
    }
    let error = last_error();
    if error == libc::EBUSY {
        host::fatal_error(
            E_WARNING,
            c"Unable to wait signal, async signal listener has been registered",
        );
    } else if error == libc::EINVAL {
        host::fatal_error(E_WARNING, c"Invalid signal in the given list");
    }
    swoole_rs_corebase::sys::set_errno(last_error());
    call.set_bool(false);
}

/// `waitEvent(mixed $socket, int $events = SWOOLE_EVENT_READ, float $timeout = -1)`.
fn wait_event(call: &Call) {
    let parsed = call
        .arity(1, 3)
        .and_then(|()| Ok((call.long(2, EVENT_READ)?, call.double(3, -1.0)?)));
    let Ok((events, timeout)) = parsed else {
        call.set_bool(false);
        return;
    };
    // SAFETY: a passed argument.
    let fd = unsafe { php_swoole_convert_to_fd(call.zval(1)) };
    if fd < 0 {
        host::fatal_error(E_WARNING, c"unknown fd type");
        call.set_bool(false);
        return;
    }
    // SAFETY: waits on the descriptor in the running coroutine.
    let events = unsafe { system_wait_event(fd, int(events), timeout) };
    if events < 0 {
        call.set_bool(false);
    } else {
        call.set_long(events.into());
    }
}
