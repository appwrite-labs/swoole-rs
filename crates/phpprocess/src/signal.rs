//! `Process::signal()`: the PHP callable registered for each signal, and the
//! handler Swoole's signal layer runs for them.
use crate::{
    host::host,
    report::{OPERATION_NOT_SUPPORT, cli, fatal, notice, set_last_error},
};
use std::{
    ffi::{c_int, c_void},
    ptr,
    sync::atomic::{AtomicPtr, Ordering},
};
use swoole_rs_zend::{Call, E_ERROR, E_WARNING, long_zval, message, sys::IS_LONG, type_of};

// SAFETY: crates/reactor's `swoole_signal_get_handler`, under its upstream symbol.
unsafe extern "C" {
    #[link_name = "_Z25swoole_signal_get_handleri"]
    fn swoole_signal_get_handler(signo: c_int) -> usize;
}

/// `SwooleG.signal_listener_num += delta`.
fn listeners(delta: i32) {
    use swoole_rs_corebase::{
        swr_v1_core_set_signal_listener_num, swr_v1_core_signal_listener_num,
    };
    swr_v1_core_set_signal_listener_num(
        swr_v1_core_signal_listener_num().wrapping_add_signed(delta),
    );
}

/// `SW_SIGNO_MAX`.
const SIGNO_MAX: i64 = 128;
const SLOTS: usize = 128;
/// `(zend_long) SIG_IGN`.
pub(crate) const SIG_IGN: i64 = 1;

/// `signal_fci_caches`: the `zend::Callable` per signal (null: none).
static CALLABLES: [AtomicPtr<c_void>; SLOTS] = [const { AtomicPtr::new(ptr::null_mut()) }; SLOTS];

fn slot(signo: i64) -> &'static AtomicPtr<c_void> {
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "callers checked 0 <= signo < SIGNO_MAX"
    )]
    &CALLABLES[signo as usize]
}

/// `php_swoole_onSignal`: call the signal's callable with its number.
unsafe extern "C" fn on_signal(signo: c_int) {
    let callable = slot(i64::from(signo)).load(Ordering::Acquire);
    if callable.is_null() {
        return;
    }
    let mut argv = long_zval(i64::from(signo));
    let host = host();
    // SAFETY: C++'s reader of the coroutine setting.
    let coroutine = unsafe { (host.enable_coroutine)() };
    // SAFETY: a live callable from `signal()`, with one argument.
    let called = unsafe { (host.callable_call)(callable, 1, &raw mut argv, coroutine) };
    if !called {
        fatal(
            E_WARNING,
            &message!("Swoole\\Process: signal [{signo}] handler error"),
        );
    }
    swoole_rs_zend::bailout::finish();
}

/// `Process::signal(int $signo, ?callable $callback = null): bool`.
pub(crate) fn signal(call: &Call) {
    let host = host();
    // SAFETY: C++'s reader of the thread's identity.
    if !unsafe { (host.main_thread)() } {
        set_last_error(OPERATION_NOT_SUPPORT);
        call.set_bool(true);
        return;
    }
    let Ok(signo) = call.arity(1, 2).and_then(|()| call.long(1, 0)) else {
        call.set_bool(false);
        return;
    };
    let callback = call.zval_or_null(2);
    // SAFETY: C++'s reader of an ini flag.
    if !cli() {
        fatal(
            E_ERROR,
            &message!("Swoole\\Process::signal can only be used in CLI mode"),
        );
        call.set_bool(false);
        return;
    }
    if !(0..SIGNO_MAX).contains(&signo) {
        fatal(E_WARNING, &message!("invalid signal number [{signo}]"));
        call.set_bool(false);
        return;
    }
    #[expect(clippy::cast_possible_truncation, reason = "0 <= signo < 128")]
    let number = signo as c_int;
    // SAFETY: a valid signal number.
    let current = unsafe { swoole_signal_get_handler(number) };
    let ours: unsafe extern "C" fn(c_int) = on_signal;
    if current != 0 && current != ours as usize {
        fatal(
            E_WARNING,
            &message!("signal [{signo}] processor has been registered by the system"),
        );
        call.set_bool(false);
        return;
    }
    let old = slot(signo).load(Ordering::Acquire);
    let (callable, handler) = match callback {
        None => {
            if old.is_null() {
                notice(
                    E_WARNING,
                    &message!("unable to find the callback of signal [{signo}]"),
                );
                call.set_bool(false);
                return;
            }
            // SAFETY: C++ resets the handler, then frees the callable once the
            // loop is past any dispatch using it.
            unsafe {
                (host.signal_set)(number, None, false);
            }
            slot(signo).store(ptr::null_mut(), Ordering::Release);
            // SAFETY: as above.
            unsafe { (host.callable_defer_free)(old) };
            listeners(-1);
            call.set_bool(true);
            return;
        }
        // SAFETY: a passed argument.
        Some(value) if unsafe { type_of(value) } == IS_LONG && is_ignore(value) => {
            (ptr::null_mut(), None)
        }
        Some(value) => {
            // SAFETY: a passed argument.
            let callable = unsafe { (host.callable_create)(value) };
            if callable.is_null() {
                call.set_bool(false);
                return;
            }
            (callable, Some(ours))
        }
    };
    // SAFETY: C++'s reader of the server's mode.
    let sync = unsafe { (host.sync_server)() };
    if !sync {
        // SAFETY: C++ creates the reactor and its exit condition.
        unsafe { (host.signal_listen)() };
    }
    if old.is_null() {
        listeners(1);
    } else if sync {
        // SAFETY: the previous callable, no longer referenced.
        unsafe { (host.callable_free)(old) };
    } else {
        // SAFETY: as above, freed once the loop is past any dispatch using it.
        unsafe { (host.callable_defer_free)(old) };
    }
    slot(signo).store(callable, Ordering::Release);
    // SAFETY: a valid signal and handler.
    unsafe { (host.signal_set)(number, handler, true) };
    call.set_bool(true);
}

fn is_ignore(value: *mut swoole_rs_zend::sys::zval) -> bool {
    // SAFETY: an IS_LONG zval.
    let value = unsafe { (*value).value };
    // SAFETY: as above.
    unsafe { value.lval == SIG_IGN }
}

/// `php_swoole_signal_isset_handler`.
#[unsafe(no_mangle)]
pub extern "C" fn swr_v1_phpprocess_signal_isset(signo: c_int) -> bool {
    if !(0..SIGNO_MAX).contains(&i64::from(signo)) {
        fatal(E_WARNING, &message!("invalid signal number [{signo}]"));
        return false;
    }
    !slot(i64::from(signo)).load(Ordering::Acquire).is_null()
}

/// `php_swoole_process_clean`: free every signal callable; outside thread
/// builds, a process that is not a user worker forgets its worker type.
#[unsafe(no_mangle)]
pub extern "C" fn swr_v1_phpprocess_clean() {
    let host = host();
    for slot in &CALLABLES {
        let callable = slot.swap(ptr::null_mut(), Ordering::AcqRel);
        if !callable.is_null() {
            // SAFETY: a callable `signal()` created and nothing references now.
            unsafe { (host.callable_free)(callable) };
        }
    }
    // SAFETY: C++'s worker-type reset.
    unsafe { (host.reset_worker_type)() };
}
