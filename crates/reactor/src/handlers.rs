//! The reactor's handler tables (upstream `Reactor::read_handler`, `write_handler`,
//! `error_handler` and the two defaults): which native `ReactorHandler` runs for an
//! fd type's read, write or error readiness.
use std::ffi::{c_int, c_void};
use std::ptr;

/// `ReactorHandler`: `int (*)(Reactor *, Event *)`.
pub type Handler = unsafe extern "C" fn(*mut c_void, *mut c_void) -> c_int;

/// `SW_MAX_FDTYPE`.
const FD_TYPES: usize = 32;
const SW_EVENT_DEAULT: c_int = 1 << 8;
const SW_EVENT_READ: c_int = 1 << 9;
const SW_EVENT_WRITE: c_int = 1 << 10;
const SW_EVENT_ERROR: c_int = 1 << 11;

/// Table indexes, also the wait loop's handler kinds.
pub(crate) const READ: usize = 0;
pub(crate) const WRITE: usize = 1;
pub(crate) const ERROR: usize = 2;

#[derive(Default)]
pub struct Handlers {
    tables: [[Option<Handler>; FD_TYPES]; 3],
    /// Write and error fallbacks (`default_write_handler`, `default_error_handler`).
    pub(crate) defaults: [Option<Handler>; 2],
}

/// `Reactor::isset_*_event`'s order: the table an `events` mask names, if any.
fn table(events: c_int) -> Option<usize> {
    if events < SW_EVENT_DEAULT || events & SW_EVENT_READ != 0 {
        Some(READ)
    } else if events & SW_EVENT_WRITE != 0 {
        Some(WRITE)
    } else if events & SW_EVENT_ERROR != 0 {
        Some(ERROR)
    } else {
        None
    }
}

impl Handlers {
    /// `Reactor::set_handler`: false when `events` names no table (upstream asserts) or
    /// `fd_type` is out of range.
    pub(crate) fn set(&mut self, fd_type: c_int, events: c_int, handler: Option<Handler>) -> bool {
        self.slot(fd_type, events)
            .map(|slot| *slot = handler)
            .is_some()
    }

    /// `Reactor::isset_handler`.
    pub(crate) fn isset(&self, fd_type: c_int, events: c_int) -> bool {
        table(events).is_some_and(|kind| self.registered(kind, fd_type).is_some())
    }

    fn slot(&mut self, fd_type: c_int, events: c_int) -> Option<&mut Option<Handler>> {
        let table = table(events)?;
        self.tables
            .get_mut(table)?
            .get_mut(usize::try_from(fd_type).ok()?)
    }

    fn registered(&self, kind: usize, fd_type: c_int) -> Option<Handler> {
        let row = self.tables.get(kind)?;
        *row.get(usize::try_from(fd_type).ok()?)?
    }

    /// `Reactor::get_handler`: the registered handler, else the write or error default.
    fn get(&self, kind: usize, fd_type: c_int) -> Option<Handler> {
        let registered = self.registered(kind, fd_type);
        match kind {
            WRITE => registered.or(self.defaults[0]),
            ERROR => registered.or(self.defaults[1]),
            _ => registered,
        }
    }

    /// `Reactor::get_error_handler`: error, else read, else write.
    pub(crate) fn dispatch(&self, kind: usize, fd_type: c_int) -> Option<Handler> {
        if kind != ERROR {
            return self.get(kind, fd_type);
        }
        self.get(ERROR, fd_type)
            .or_else(|| self.get(READ, fd_type))
            .or_else(|| self.get(WRITE, fd_type))
    }
}

/// `Reactor::get_handler` (`kind` 0 read, 1 write, 2 error) and, for `error_only`
/// false with kind 2, `get_error_handler`'s fallbacks.
#[unsafe(no_mangle)]
pub extern "C" fn swr_v1_reactor_handlers_get(
    handlers: &Handlers,
    kind: u32,
    fd_type: c_int,
    error_only: bool,
) -> Option<Handler> {
    let kind = usize::try_from(kind).ok()?;
    if error_only {
        handlers.get(kind, fd_type)
    } else {
        handlers.dispatch(kind, fd_type)
    }
}

/// The write (`kind` 1) or error (`kind` 2) default handler.
#[unsafe(no_mangle)]
pub extern "C" fn swr_v1_reactor_handlers_set_default(
    handlers: &mut Handlers,
    kind: u32,
    handler: Option<Handler>,
) {
    if let Some(slot) = kind
        .checked_sub(1)
        .and_then(|i| handlers.defaults.get_mut(i as usize))
    {
        *slot = handler;
    }
}

/// The write (`kind` 1) or error (`kind` 2) default's slot, which lives as long as
/// `handlers` (upstream's `default_write_handler` / `default_error_handler` members
/// read it), or null for another `kind`.
#[unsafe(no_mangle)]
pub extern "C" fn swr_v1_reactor_handlers_default(
    handlers: &Handlers,
    kind: u32,
) -> *const Option<Handler> {
    kind.checked_sub(1)
        .and_then(|i| handlers.defaults.get(i as usize))
        .map_or(ptr::null(), ptr::from_ref)
}

#[cfg(test)]
mod tests {
    use super::*;

    unsafe extern "C" fn one(_: *mut c_void, _: *mut c_void) -> c_int {
        1
    }
    unsafe extern "C" fn two(_: *mut c_void, _: *mut c_void) -> c_int {
        2
    }
    unsafe extern "C" fn three(_: *mut c_void, _: *mut c_void) -> c_int {
        3
    }

    fn call(handler: Option<Handler>) -> c_int {
        handler.map_or(0, |h| unsafe { h(ptr::null_mut(), ptr::null_mut()) })
    }

    #[test]
    fn upstream_lookup_order() {
        let mut handlers = Handlers::default();
        let h = &raw mut handlers;
        unsafe {
            // A mask below SW_EVENT_DEAULT is a read registration, as upstream.
            assert!((*h).set(5, 0, Some(one)));
            assert!((*h).isset(5, SW_EVENT_READ));
            assert!(!(*h).isset(5, SW_EVENT_WRITE));
            assert!(!(*h).set(5, SW_EVENT_DEAULT, Some(two)));
            assert!(!(*h).set(32, SW_EVENT_READ, Some(two)));
            assert!(!(*h).isset(5, SW_EVENT_DEAULT));

            // Write and error fall back to the defaults; error dispatch then to read.
            assert_eq!(call(swr_v1_reactor_handlers_get(&*h, 1, 5, true)), 0);
            swr_v1_reactor_handlers_set_default(&mut *h, 1, Some(two));
            assert_eq!(call(swr_v1_reactor_handlers_get(&*h, 1, 5, true)), 2);
            assert_eq!(call(swr_v1_reactor_handlers_get(&*h, 2, 5, true)), 0);
            assert_eq!(call(swr_v1_reactor_handlers_get(&*h, 2, 5, false)), 1);
            swr_v1_reactor_handlers_set_default(&mut *h, 2, Some(three));
            assert_eq!(call(swr_v1_reactor_handlers_get(&*h, 2, 5, false)), 3);
            assert_eq!(call(*swr_v1_reactor_handlers_default(&*h, 2)), 3);
            assert!(swr_v1_reactor_handlers_default(&*h, 3).is_null());
            assert!((*h).set(5, SW_EVENT_WRITE | SW_EVENT_ERROR, Some(one)));
            assert_eq!(call(swr_v1_reactor_handlers_get(&*h, 1, 5, true)), 1);
            // Error-only types: no read handler, the error default still applies.
            assert_eq!(call(swr_v1_reactor_handlers_get(&*h, 0, 6, true)), 0);
            assert_eq!(call(swr_v1_reactor_handlers_get(&*h, 2, 6, false)), 3);
            assert_eq!(call(swr_v1_reactor_handlers_get(&*h, 0, -1, true)), 0);
        }
    }
}
