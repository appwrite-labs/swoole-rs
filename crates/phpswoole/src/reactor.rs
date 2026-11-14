//! Reactor fd handlers that run PHP, under the [`Php`] lease of their call.
use std::{ffi::c_int, marker::PhantomData};
use swoole_rs_reactor::event_api::{self, FdHandler};
use swoole_rs_zend::Php;

/// A reactor fd handler that runs PHP: whether it succeeded.
pub trait PhpFdHandler {
    fn handle(php: Php<'_>, fd: c_int) -> bool;
}

struct OnPhp<H>(PhantomData<H>);

impl<H: PhpFdHandler> FdHandler for OnPhp<H> {
    fn handle(fd: c_int) -> c_int {
        // SAFETY: only `set_fd_handler` installs it, on the reactor of a thread
        // running PHP. Only threads running PHP drive that reactor (its own thread,
        // or under thread contexts the coroutine threads sharing it), so this call
        // runs on one, and the lease ends with the call.
        let php = unsafe { Php::assume() };
        if H::handle(php, fd) { 0 } else { -1 }
    }
}

/// `swoole_event_set_handler(fd_type, event, H)` on the reactor of the thread `_php`
/// shows runs PHP.
pub fn set_fd_handler<H: PhpFdHandler>(_php: Php<'_>, fd_type: c_int, event: c_int) {
    event_api::swoole_event_set_fd_handler::<OnPhp<H>>(fd_type, event);
}
