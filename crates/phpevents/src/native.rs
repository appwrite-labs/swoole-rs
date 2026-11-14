//! What `Swoole\Event` drives outside the crate (once native/phpevents/event.cc's
//! table): the thread's reactor (crates/reactor, with crates/reactor's `Reactor`
//! lifetime, wait and exit check), the server's role, timers, and Swoole's PHP
//! error reports.
use std::ffi::{CStr, c_int};
use swoole_rs_reactor::event_api;
use swoole_rs_zend::{E_ERROR, E_WARNING, Message, Value, bailout::Bailout, contain, report};

/// `SW_ERROR_PHP_FATAL_ERROR` and `SW_ERROR_PHP_RUNTIME_NOTICE`.
const PHP_FATAL_ERROR: c_int = 503;
const PHP_RUNTIME_NOTICE: c_int = 510;
/// `SW_MANAGER`.
const MANAGER: c_int = 3;

pub(crate) fn has_reactor() -> bool {
    !swoole_rs_corebase::thread_reactor().is_null()
}

/// `SWOOLE_G(cli)`.
pub(crate) fn cli() -> bool {
    swoole_rs_phpswoole::globals().cli.get()
}

/// `SWOOLE_G(req_status) == PHP_SWOOLE_RSHUTDOWN_BEGIN`.
pub(crate) fn shutdown_begun() -> bool {
    swoole_rs_phpswoole::globals().req_status.get() == swoole_rs_phpswoole::RSHUTDOWN_BEGIN
}

/// Why async-io is refused in this server process (`is_sync_process`), if it is.
pub(crate) fn server_refusal() -> Option<&'static CStr> {
    if !swoole_rs_phpprocess::sync_server() {
        return None;
    }
    Some(
        if swoole_rs_process::methods::swoole_get_worker_type() == MANAGER {
            c"Unable to use async-io in manager process"
        } else {
            c"Unable to use async-io in task processes, please set `task_enable_coroutine` to true"
        },
    )
}

pub(crate) fn bailed() -> bool {
    event_api::swoole_event_bailed_out()
}

/// `swoole_event_init(SW_EVENTLOOP_WAIT_EXIT)`, then the shutdown function that
/// waits and `Co::set`'s exit condition: whether it made the reactor.
pub(crate) fn create() -> bool {
    let flags = swoole_rs_corebase::constants::EventInitFlag::SW_EVENTLOOP_WAIT_EXIT as c_int;
    if swoole_rs_reactor::swoole_event_init(flags) < 0 {
        return false;
    }
    swoole_rs_phpswoole::php_swoole_register_shutdown_function(c"swoole_event_rshutdown");
    true
}

/// `sw_reactor()->if_exit()`, under a trap: exit conditions may run PHP.
pub(crate) fn if_exit() -> Result<bool, Bailout> {
    contain(event_api::swoole_event_if_exit)
}

/// `sw_reactor()->wait()`, under a trap: callbacks may run PHP.
pub(crate) fn wait() -> Result<c_int, Bailout> {
    contain(event_api::swoole_event_run)
}

/// `swoole_event_free()`, under a trap.
pub(crate) fn free() -> Result<(), Bailout> {
    contain(|| {
        swoole_rs_reactor::swoole_event_free();
    })
}

/// `php_swoole_timer_clear_all()`, under a trap: destructors may run PHP.
pub(crate) fn clear_timers() -> Result<(), Bailout> {
    contain(swoole_rs_phptimer::clear_all_timers).map(drop)
}

/// An ext/sockets `Socket`'s descriptor and whether it was closed, if `value`
/// is one and Swoole was built with sockets support.
pub(crate) fn php_socket_fd(value: &Value) -> Option<(c_int, bool)> {
    use swoole_rs_phpswoole::{Switch, built_with};
    if !built_with(Switch::Sockets) {
        return None;
    }
    value.php_socket_fd()
}

/// `php_swoole_fatal_error(E_WARNING, "%s", message)`.
pub(crate) fn fatal_warning(message: &CStr) -> Result<(), Bailout> {
    fatal(E_WARNING, message)
}

/// `php_swoole_fatal_error(level, "%s", message)`.
pub(crate) fn fatal(level: i32, message: &CStr) -> Result<(), Bailout> {
    swoole_rs_corebase::log::set_last_error(PHP_FATAL_ERROR);
    report(level, message)
}

/// `php_swoole_error(E_WARNING, "%s", message)`.
pub(crate) fn warning(message: &CStr) -> Result<(), Bailout> {
    swoole_rs_corebase::log::set_last_error(PHP_RUNTIME_NOTICE);
    if !swoole_rs_phpswoole::display_errors() {
        return Ok(());
    }
    report(E_WARNING, message)
}

/// `php_swoole_sys_error(level, "%s", message)` with `errno` `code`.
pub(crate) fn system_error(code: c_int, level: i32, message: &CStr) -> Result<(), Bailout> {
    swoole_rs_corebase::log::set_last_error(code);
    if level != E_ERROR && !swoole_rs_phpswoole::display_errors() {
        return Ok(());
    }
    let mut full = Message::new();
    full.push(message.to_bytes());
    full.push(b", Error: ");
    full.push(crate::errno_message(code).as_bytes());
    full.push(format!("[{code}]").as_bytes());
    report(level, full.as_cstr())
}
