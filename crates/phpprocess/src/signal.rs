//! `Process::signal()`: the PHP callable registered for each signal, and the
//! handler Swoole's signal layer runs for them.
use crate::{
    host,
    report::{OPERATION_NOT_SUPPORT, cli, fatal, notice, set_last_error},
};
use std::{
    ffi::{c_int, c_void},
    ptr,
    sync::atomic::{AtomicPtr, Ordering},
};
use swoole_rs_zend::{Call, E_ERROR, E_WARNING, Value, bailout, long_zval, message};

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
const SLOTS: usize = 128;
/// `(zend_long) SIG_IGN`.
pub(crate) const SIG_IGN: i64 = 1;
/// `SIG_IGN` as a `swSignalHandler`.
const IGNORE: usize = 1;

/// `signal_fci_caches`: the callable per signal (null: none).
static CALLABLES: [AtomicPtr<c_void>; SLOTS] = [const { AtomicPtr::new(ptr::null_mut()) }; SLOTS];

/// The slot of `signo`, or `None` outside `0..SW_SIGNO_MAX`.
fn slot(signo: i64) -> Option<&'static AtomicPtr<c_void>> {
    CALLABLES.get(usize::try_from(signo).ok()?)
}

/// `php_swoole_onSignal`: call the signal's callable with its number.
unsafe extern "C" fn on_signal(signo: c_int) {
    let Some(slot) = slot(i64::from(signo)) else {
        return;
    };
    let callable = slot.load(Ordering::Acquire);
    if callable.is_null() {
        return;
    }
    let mut argv = long_zval(i64::from(signo));
    let coroutine = host::enable_coroutine();
    // SAFETY: a live callable from `signal()`, with one argument.
    let called = unsafe { host::callable_call(callable, 1, &raw mut argv, coroutine) };
    if !called && !bailout::pending() {
        fatal(
            E_WARNING,
            &message!("Swoole\\Process: signal [{signo}] handler error"),
        );
    }
    bailout::finish();
}

/// `Process::signal(int $signo, callable|int|null $callback = null): bool`.
pub(crate) fn signal(call: &Call) {
    if !host::main_thread() {
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
    let Some(slot) = slot(signo) else {
        fatal(E_WARNING, &message!("invalid signal number [{signo}]"));
        call.set_bool(false);
        return;
    };
    #[expect(clippy::cast_possible_truncation, reason = "0 <= signo < 128")]
    let number = signo as c_int;
    // SAFETY: a valid signal number.
    let current = unsafe { swoole_signal_get_handler(number) };
    let ours = on_signal as *const () as usize;
    if current != 0 && current != ours && current != IGNORE {
        fatal(
            E_WARNING,
            &message!("signal [{signo}] processor has been registered by the system"),
        );
        call.set_bool(false);
        return;
    }
    let old = slot.load(Ordering::Acquire);
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
            // Reset the handler, then free the callable once the loop is past
            // any dispatch using it.
            host::signal_set(number, 0, false);
            slot.store(ptr::null_mut(), Ordering::Release);
            // SAFETY: the callable this slot owned.
            unsafe { host::callable_defer_free(old) };
            listeners(-1);
            call.set_bool(true);
            return;
        }
        Some(_) if call.value(2).and_then(Value::as_long) == Some(SIG_IGN) => {
            (ptr::null_mut(), IGNORE)
        }
        Some(value) => {
            // SAFETY: a passed argument.
            let callable = unsafe { host::callable_create(value) };
            if callable.is_null() {
                call.set_bool(false);
                return;
            }
            (callable, ours)
        }
    };
    let sync = host::sync_server();
    if !sync {
        host::signal_listen();
    }
    if !old.is_null() {
        // The slot lets go first: a destructor that bails out leaves it empty.
        slot.store(ptr::null_mut(), Ordering::Release);
        if sync {
            // SAFETY: the previous callable, no longer referenced.
            unsafe { host::callable_free(old) };
        } else {
            // SAFETY: as above, freed once the loop is past any dispatch using it.
            unsafe { host::callable_defer_free(old) };
        }
        listeners(-1);
        if bailout::pending() {
            // Upstream stopped here, installing nothing.
            if !callable.is_null() {
                // SAFETY: the callable made above, which nothing holds.
                unsafe { host::callable_free(callable) };
            }
            return;
        }
    }
    if !callable.is_null() {
        listeners(1);
    }
    // In a sync process the old callable's destructor ran, and may have called
    // signal() for this number: this call's callable replaces the one that stored.
    let nested = slot.swap(callable, Ordering::AcqRel);
    if !nested.is_null() {
        // SAFETY: the nested call's callable, which no slot holds now.
        unsafe { host::callable_free(nested) };
        listeners(-1);
    }
    host::signal_set(number, handler, true);
    call.set_bool(true);
}

/// `php_swoole_signal_isset_handler`, which `proc_open`'s hook asks about
/// SIGCHLD (`php_swoole_private.h` declares it inside `BEGIN_EXTERN_C`).
#[unsafe(no_mangle)]
pub extern "C" fn php_swoole_signal_isset_handler(signo: c_int) -> bool {
    let Some(slot) = slot(i64::from(signo)) else {
        fatal(E_WARNING, &message!("invalid signal number [{signo}]"));
        return false;
    };
    !slot.load(Ordering::Acquire).is_null()
}

/// `php_swoole_process_signal_clean`: free every signal callable and forget the
/// listeners, as after a fork, whose `swoole_fork()` removed the inherited handlers.
#[unsafe(no_mangle)]
pub extern "C" fn php_swoole_process_signal_clean() {
    signal_clean();
    bailout::finish();
}

/// [`php_swoole_process_signal_clean`]'s body, which frees every callable and
/// leaves a destructor's bailout pending.
fn signal_clean() {
    for slot in &CALLABLES {
        let callable = slot.swap(ptr::null_mut(), Ordering::AcqRel);
        if !callable.is_null() {
            // SAFETY: a callable `signal()` created and nothing references now.
            unsafe { host::callable_free(callable) };
        }
    }
    swoole_rs_corebase::swr_v1_core_set_signal_listener_num(0);
}

/// `php_swoole_process_clean`: [`php_swoole_process_signal_clean`]; outside thread
/// builds, a process that is not a user worker forgets its worker type. A
/// destructor's bailout stays pending for the caller to stop on.
pub(crate) fn clean() {
    signal_clean();
    host::reset_worker_type();
}

/// `php_swoole_process_rshutdown`.
#[unsafe(no_mangle)]
pub extern "C" fn php_swoole_process_rshutdown() {
    clean();
    bailout::finish();
}
