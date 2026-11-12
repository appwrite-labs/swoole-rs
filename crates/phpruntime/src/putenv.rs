//! `putenv` for Swoole's threads (ZTS builds, `SW_THREAD`): PHP's own keeps
//! its strings per request, so a thread exiting would leave `environ`
//! pointing at freed memory. These strings live for the process, one per
//! name, under PHP's environment lock.
use std::{
    collections::HashMap,
    ffi::CString,
    sync::{LazyLock, Mutex, PoisonError},
};
use swoole_rs_zend::{Call, sys::swr_zend_runtime_env_lock};

swoole_rs_zend::handlers! {
    zif_swoole_putenv => putenv;
}

/// Each name's latest `NAME=value` (or bare `NAME`) string.
static ENVIRON: LazyLock<Mutex<HashMap<Vec<u8>, CString>>> = LazyLock::new(Mutex::default);

/// `putenv(string $assignment): bool`.
fn putenv(call: &Call) {
    let Ok(setting) = call.arity(1, 1).and_then(|()| call.string(1, b"")) else {
        return;
    };
    if setting.first().is_none_or(|&first| first == b'=') {
        call.argument_value_error(1, c"must have a valid syntax");
        return;
    }
    // as C reads it: up to the first NUL
    let text = setting.split(|&b| b == 0).next().unwrap_or_default();
    let assignment = text.iter().position(|&b| b == b'=');
    let key = assignment
        .and_then(|end| setting.get(..end))
        .unwrap_or(setting);
    let stored = CString::new(text).unwrap_or_default();

    // SAFETY: PHP's environment lock, released below.
    unsafe { swr_zend_runtime_env_lock(true) };
    let mut environ = ENVIRON.lock().unwrap_or_else(PoisonError::into_inner);
    let entry = environ.entry(key.to_vec()).insert_entry(stored);
    let pointer = entry.get().as_ptr().cast_mut();
    if assignment.is_none() {
        // no '=' unsets the name
        // SAFETY: a NUL-terminated name.
        unsafe { libc::unsetenv(pointer) };
    }
    // SAFETY: the string stays in ENVIRON until this name is set again.
    let set = assignment.is_none() || unsafe { libc::putenv(pointer) } == 0;
    if set && key.eq_ignore_ascii_case(b"TZ") {
        // TZ changed; under the environment lock.
        tzset();
    }
    drop(environ);
    // SAFETY: taken above.
    unsafe { swr_zend_runtime_env_lock(false) };
    call.set_bool(set);
}

unsafe extern "C" {
    /// libc's `tzset`: no arguments, and it only reads the environment.
    safe fn tzset();
}
