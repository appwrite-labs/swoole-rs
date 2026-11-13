//! `Process::signal()`: the PHP callable registered for each signal, and the
//! handler Swoole's signal layer runs for them.
use crate::{
    host::{self, Callable, Slot},
    report::{OPERATION_NOT_SUPPORT, cli, fatal, notice, set_last_error},
};
use std::ffi::c_int;
use swoole_rs_zend::{Call, E_ERROR, E_WARNING, Owned, bailout, message};

use swoole_rs_reactor::signal_upstream::{self, Handler};

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

/// `signal_fci_caches`: the callable per signal.
static CALLABLES: [Slot; SLOTS] = [const { Slot::empty() }; SLOTS];

/// The slot of `signo`, or `None` outside `0..SW_SIGNO_MAX`.
fn slot(signo: i64) -> Option<&'static Slot> {
    CALLABLES.get(usize::try_from(signo).ok()?)
}

/// `php_swoole_onSignal`: call the signal's callable with its number.
extern "C" fn on_signal(signo: c_int) {
    let Some(slot) = slot(i64::from(signo)) else {
        return;
    };
    if !slot.is_set() {
        return;
    }
    let coroutine = host::enable_coroutine();
    // The callable `signal()` stored, borrowed from its slot, not taken.
    let Some(called) = slot.call(&[&Owned::long(i64::from(signo))], coroutine) else {
        return;
    };
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
    let callback = call.value(2).filter(|value| !value.is_null());
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
    let current = signal_upstream::get(number);
    let ours = Handler::new(on_signal);
    if current != Handler::NONE && current != ours && current != Handler::IGNORE {
        fatal(
            E_WARNING,
            &message!("signal [{signo}] processor has been registered by the system"),
        );
        call.set_bool(false);
        return;
    }
    let (callable, handler) = match callback {
        None => {
            if !slot.is_set() {
                notice(
                    E_WARNING,
                    &message!("unable to find the callback of signal [{signo}]"),
                );
                call.set_bool(false);
                return;
            }
            // Reset the handler, then free the callable once the loop is past
            // any dispatch using it.
            host::signal_set(number, Handler::NONE, false);
            if let Some(old) = slot.replace(None) {
                host::callable_defer_free(old);
            }
            listeners(-1);
            call.set_bool(true);
            return;
        }
        Some(value) if value.as_long() == Some(SIG_IGN) => (None, Handler::IGNORE),
        Some(value) => {
            let Some(callable) = Callable::create(value) else {
                call.set_bool(false);
                return;
            };
            (Some(callable), ours)
        }
    };
    let sync = host::sync_server();
    if !sync {
        host::signal_listen();
    }
    // The slot lets go of the previous callable first: a destructor that bails
    // out leaves it empty.
    if let Some(old) = slot.replace(None) {
        if sync {
            drop(old);
        } else {
            // Freed once the loop is past any dispatch using it.
            host::callable_defer_free(old);
        }
        listeners(-1);
        if bailout::pending() {
            // Upstream stopped here, installing nothing; the callable made
            // above drops.
            return;
        }
    }
    if callable.is_some() {
        listeners(1);
    }
    // In a sync process the old callable's destructor ran, and may have called
    // signal() for this number: this call's callable replaces the one that stored.
    if let Some(nested) = slot.replace(callable) {
        drop(nested);
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
    slot.is_set()
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
        drop(slot.replace(None));
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
