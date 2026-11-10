//! What `Swoole\Event` drives outside the crate (once native/phpevents/event.cc's
//! table): the thread's reactor and its sockets (crates/reactor, with
//! crates/reactor's `Reactor` lifetime, wait and exit check), the
//! server's role, timers, and Swoole's PHP error reports.
use std::ffi::{CStr, c_int, c_void};
use swoole_rs_corebase::constants::{EventType, FdType};
use swoole_rs_reactor::{
    DeferCall, DeferDispose, ReactorFields, ReactorPrepared, ReactorTask, SocketRecord, event_api,
};
use swoole_rs_zend::{
    E_ERROR, E_WARNING, Message,
    bailout::{self, Bailout, trap},
    php_error,
    sys::{self as z, swr_zend_callable_warning},
};

unsafe extern "C-unwind" {
    #[link_name = "_Z17swoole_event_initi"]
    fn swoole_event_init(flags: c_int) -> c_int;
    #[link_name = "_Z17swoole_event_freev"]
    fn swoole_event_free() -> c_int;
    #[link_name = "_ZN6swoole7Reactor7if_exitEv"]
    fn reactor_if_exit(reactor: *mut c_void) -> bool;
    #[link_name = "_ZN6swoole7Reactor4waitEv"]
    fn reactor_wait(reactor: *mut c_void) -> c_int;
    // crates/coroutine.
    fn swoole_coroutine_is_in() -> u8;
}

// crates/phphelpers' `SW_ZSTR_KNOWN` table.
use swoole_rs_phphelpers::sw_zend_known_strings;

/// `SW_ERROR_PHP_FATAL_ERROR` and `SW_ERROR_PHP_RUNTIME_NOTICE`.
const PHP_FATAL_ERROR: c_int = 503;
const PHP_RUNTIME_NOTICE: c_int = 510;
/// `SW_TASK_WORKER` and `SW_MANAGER`.
const TASK_WORKER: c_int = 4;
const MANAGER: c_int = 3;
const FD_USER: c_int = FdType::SW_FD_USER as c_int;
const READ: c_int = EventType::SW_EVENT_READ as c_int;
const WRITE: c_int = EventType::SW_EVENT_WRITE as c_int;
const ERROR: c_int = EventType::SW_EVENT_ERROR as c_int;

/// `sw_reactor()`, unless it is null.
fn reactor() -> Option<ReactorPrepared> {
    let reactor = swoole_rs_corebase::thread_reactor();
    // SAFETY: null or the thread's live `Reactor`, which begins with its prepared state.
    unsafe { reactor.cast::<ReactorPrepared>().as_ref() }.copied()
}

pub(crate) fn has_reactor() -> bool {
    !swoole_rs_corebase::thread_reactor().is_null()
}

/// The thread's reactor's fields. Callers have checked [`has_reactor`].
fn fields() -> &'static mut ReactorFields {
    let Some(reactor) = reactor() else {
        std::process::abort()
    };
    // SAFETY: the live reactor's fields; no other borrow of them is held.
    unsafe { &mut *reactor.fields }
}

/// `SWOOLE_G(cli)`.
pub(crate) fn cli() -> bool {
    swoole_rs_phpswoole::globals().cli.get()
}

/// `SWOOLE_G(req_status) == PHP_SWOOLE_RSHUTDOWN_BEGIN`.
pub(crate) fn shutdown_begun() -> bool {
    swoole_rs_phpswoole::globals().req_status.get() == swoole_rs_phpswoole::RSHUTDOWN_BEGIN
}

/// Why async-io is refused in this server process, if it is.
pub(crate) fn server_refusal() -> Option<&'static CStr> {
    let serv = swoole_rs_corebase::swr_v1_core_server();
    if serv.is_null() {
        return None;
    }
    match swoole_rs_process::methods::swoole_get_worker_type() {
        // SAFETY: `serv` is the live server here.
        TASK_WORKER if !unsafe { swoole_rs_server::view::Server::from_raw(serv) }.config().task_enable_coroutine => Some(
            c"Unable to use async-io in task processes, please set `task_enable_coroutine` to true",
        ),
        MANAGER => Some(c"Unable to use async-io in manager process"),
        _ => None,
    }
}

/// `php_swoole_is_fatal_error()`.
pub(crate) fn fatal_error() -> bool {
    let (mut message, mut file, mut line) = (std::ptr::null(), std::ptr::null(), 0);
    // SAFETY: writable slots; the executor's last error is only read.
    unsafe { z::swr_zend_last_fatal_error(&raw mut message, &raw mut file, &raw mut line) }
}

/// `Coroutine::get_current() != nullptr`.
pub(crate) fn in_coroutine() -> bool {
    // SAFETY: no preconditions.
    unsafe { swoole_coroutine_is_in() != 0 }
}

pub(crate) fn bailed() -> bool {
    fields().bailout
}

pub(crate) fn set_running(running: bool) {
    fields().running = running;
}

pub(crate) fn set_once(once: bool) {
    fields().once = once;
}

/// `swoole_event_init(SW_EVENTLOOP_WAIT_EXIT)`, then the shutdown function that
/// waits and `Co::set`'s exit condition: whether it made the reactor.
pub(crate) fn create() -> bool {
    let flags = swoole_rs_corebase::constants::EventInitFlag::SW_EVENTLOOP_WAIT_EXIT as c_int;
    // SAFETY: this thread has no reactor.
    if unsafe { swoole_event_init(flags) } < 0 {
        return false;
    }
    swoole_rs_phpswoole::php_swoole_register_shutdown_function(c"swoole_event_rshutdown");
    true
}

/// The user fd type's read, write and error handlers, unless they are set.
pub(crate) fn install_handlers() {
    // SAFETY: the thread's live reactor.
    if unsafe { event_api::swoole_event_isset_handler(FD_USER, READ) } {
        return;
    }
    for (event, handler) in [
        (READ, crate::readable as swoole_rs_reactor::Handler),
        (WRITE, crate::writable),
        (ERROR, crate::error),
    ] {
        // SAFETY: as above.
        unsafe { event_api::swoole_event_set_handler(FD_USER, event, Some(handler)) };
    }
}

unsafe extern "C" fn if_exit_body(exit: *mut c_void) {
    let reactor = swoole_rs_corebase::thread_reactor();
    // SAFETY: the live reactor; exit conditions may run PHP, under the caller's trap.
    let can_exit = unsafe { reactor_if_exit(reactor) };
    // SAFETY: the caller's flag.
    unsafe { exit.cast::<bool>().write(can_exit) };
}

unsafe extern "C" fn wait_body(status: *mut c_void) {
    let reactor = swoole_rs_corebase::thread_reactor();
    // SAFETY: the live reactor; callbacks may run PHP, under the caller's trap.
    let result = unsafe { reactor_wait(reactor) };
    // SAFETY: the caller's slot.
    unsafe { status.cast::<c_int>().write(result) };
}

unsafe extern "C" fn free_body(_: *mut c_void) {
    // SAFETY: deletes the thread's reactor, which nothing here borrows.
    unsafe { swoole_event_free() };
}

/// Destructors may run PHP, under the caller's trap.
unsafe extern "C" fn timers_body(_: *mut c_void) {
    let _cleared = swoole_rs_phptimer::clear_all_timers();
}

/// `sw_reactor()->if_exit()`, under a trap.
pub(crate) fn if_exit() -> Result<bool, Bailout> {
    let mut exit = false;
    // SAFETY: a frame that owns nothing, writing the local flag.
    unsafe { trap(if_exit_body, (&raw mut exit).cast()) }?;
    Ok(exit)
}

/// `sw_reactor()->wait()`, under a trap.
pub(crate) fn wait() -> Result<c_int, Bailout> {
    let mut status = 0;
    // SAFETY: as above.
    unsafe { trap(wait_body, (&raw mut status).cast()) }?;
    Ok(status)
}

/// `swoole_event_free()`, under a trap.
pub(crate) fn free() -> Result<(), Bailout> {
    // SAFETY: as above.
    unsafe { trap(free_body, std::ptr::null_mut()) }
}

/// `php_swoole_timer_clear_all()`, under a trap.
pub(crate) fn clear_timers() -> Result<(), Bailout> {
    // SAFETY: as above.
    unsafe { trap(timers_body, std::ptr::null_mut()) }
}

/// `SW_ZSTR_KNOWN(SW_ZEND_STR_FD)`, `_SOCK` (1) or `_PIPE` (2).
pub(crate) fn property_name(index: u8) -> *mut z::zend_string {
    // SAFETY: MINIT filled the table.
    let table = unsafe { sw_zend_known_strings };
    // SAFETY: its entries 1 to 3 are fd, sock and pipe.
    unsafe { *table.wrapping_add(1 + usize::from(index)) }
}

/// An ext/sockets `Socket`'s descriptor and whether it was closed, if `value`
/// is one and Swoole was built with sockets support.
/// # Safety
/// `value` is a live object zval.
pub(crate) unsafe fn php_socket_fd(value: *mut z::zval) -> Option<(c_int, bool)> {
    use swoole_rs_phpswoole::{Switch, built_with};
    if !built_with(Switch::Sockets) {
        return None;
    }
    let mut closed = 0;
    // SAFETY: per the caller.
    let fd = unsafe { z::swr_zend_event_socket_fd(value, &raw mut closed) };
    (fd != -2).then_some((fd, closed != 0))
}

/// `make_socket(fd, SW_FD_USER)`, non-blocking, carrying `object`.
pub(crate) fn socket_make(fd: c_int, object: *mut c_void) -> *mut SocketRecord {
    let socket = swoole_rs_reactor::socket_upstream::make_socket_fd(fd, FD_USER);
    // SAFETY: the record just made, which nothing else holds.
    let record = unsafe { &mut *socket };
    swoole_rs_reactor::socket_upstream::socket_set_fd_option(record, 1, -1);
    record.object = object;
    socket
}

/// `socket->free()`, after `socket->fd = -1` when `detach_fd`.
/// # Safety
/// `socket` is a record no reactor holds, not used again.
pub(crate) unsafe fn socket_free_record(socket: *mut SocketRecord, detach_fd: bool) {
    if detach_fd {
        // SAFETY: per the caller.
        unsafe { (*socket).fd = -1 };
    }
    // SAFETY: per the caller.
    unsafe { swoole_rs_tls::upstream::socket_free(socket) };
}

/// `Reactor::defer(call, data)` on this thread's reactor; `dispose(data)` if the
/// reactor is freed before the call runs.
pub(crate) fn defer(call: DeferCall, data: *mut c_void, dispose: DeferDispose) {
    // SAFETY: the live reactor's fields.
    unsafe { swoole_rs_reactor::defer(fields(), call, data, Some(dispose)) };
}

/// The reactor's future (`before`) or idle task.
pub(crate) fn cycle_task(before: bool) -> &'static mut ReactorTask {
    let fields = fields();
    if before {
        &mut fields.future_task
    } else {
        &mut fields.idle_task
    }
}

/// `php_swoole_fatal_error(E_WARNING, "%s", message)`.
pub(crate) fn fatal_warning(message: &CStr) -> Result<(), Bailout> {
    // SAFETY: the C reporter runs inside Zend's C trap.
    unsafe { bailout::report(swr_zend_callable_warning, message) }
}

/// `php_swoole_fatal_error(level, "%s", message)`.
pub(crate) fn fatal(level: i32, message: &CStr) -> Result<(), Bailout> {
    swoole_rs_corebase::log::set_last_error(PHP_FATAL_ERROR);
    // SAFETY: a NUL-terminated message, reported inside Zend's C trap.
    unsafe { php_error(level, message) }
}

/// `php_swoole_error(E_WARNING, "%s", message)`.
pub(crate) fn warning(message: &CStr) -> Result<(), Bailout> {
    swoole_rs_corebase::log::set_last_error(PHP_RUNTIME_NOTICE);
    if !swoole_rs_phpswoole::display_errors() {
        return Ok(());
    }
    // SAFETY: as above.
    unsafe { php_error(E_WARNING, message) }
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
    full.push(strerror(code).to_bytes());
    full.push(format!("[{code}]").as_bytes());
    // SAFETY: as above.
    unsafe { php_error(level, full.as_cstr()) }
}

/// `strerror(code)`.
pub(crate) fn strerror(code: c_int) -> &'static CStr {
    // SAFETY: no preconditions.
    let reason = unsafe { libc::strerror(code) };
    // SAFETY: a NUL-terminated message, not freed.
    unsafe { CStr::from_ptr(reason) }
}
