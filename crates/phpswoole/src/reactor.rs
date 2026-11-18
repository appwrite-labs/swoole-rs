//! Reactor fd handlers that run PHP, under the [`Php`] lease of their call.
use std::{ffi::c_int, marker::PhantomData};
use swoole_rs_corebase::reactor::ErrorHandler;
use swoole_rs_corebase::reactor::event_api::{self, FdHandler};
use swoole_rs_zend::Php;

/// A reactor fd handler that runs PHP: whether it succeeded.
pub trait PhpFdHandler {
    fn handle(php: Php<'_>, fd: c_int) -> bool;
}

struct OnPhp<H>(PhantomData<H>);

impl<H: PhpFdHandler> FdHandler for OnPhp<H> {
    fn handle(fd: c_int) -> bool {
        // SAFETY: only `set_fd_handlers` installs it, on the reactor of a thread
        // running PHP. Only threads running PHP drive that reactor (its own thread,
        // or under thread contexts the coroutine threads sharing it), so this call
        // runs on one, and the lease ends with the call.
        let php = unsafe { Php::assume() };
        H::handle(php, fd)
    }
}

/// Register the three PHP callbacks on the reactor belonging to this PHP thread.
pub fn set_fd_handlers<R: PhpFdHandler, W: PhpFdHandler, E: PhpFdHandler>(
    _php: Php<'_>,
    fd_type: c_int,
) {
    event_api::set_handlers(
        fd_type,
        Some(event_api::fd_handler::<OnPhp<R>>),
        Some(event_api::fd_handler::<OnPhp<W>>),
        ErrorHandler::Callback(event_api::fd_handler::<OnPhp<E>>),
    );
}
