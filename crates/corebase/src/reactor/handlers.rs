//! The reactor's handler tables (upstream `Reactor::read_handler`, `write_handler`,
//! `error_handler` and the two defaults): which `Handler` runs for an
//! fd type's read, write or error readiness.
use crate::reactor::{Event, Events};
use std::cell::Cell;
use std::ffi::{c_int, c_void};
use std::marker::PhantomData;

/// `ReactorHandler`: `int (*)(Reactor *, Event *)`, whose `SW_OK` is true. The loop lends the handler its
/// reactor and the event it prepared for this call. Only Rust registers and calls one.
/// A handler runs only for the fd types and reactors it was registered with.
pub type Handler = fn(Running<'_>, &mut Event) -> bool;

/// How error readiness is dispatched for a descriptor type.
#[derive(Clone, Copy)]
pub enum ErrorHandler {
    /// Use the reactor's default error handler, falling back to read then write.
    ConnectionDefault,
    /// Dispatch errors to this type-specific callback.
    Callback(Handler),
}

/// A getter and setter of one of the reactor's fields, each borrowing its state for
/// the one read or write.
macro_rules! field {
    ($(#[$doc:meta])* $get:ident, $set:ident, $field:ident: $ty:ty) => {
        $(#[$doc])*
        #[cfg(unix)]
        #[inline]
        #[must_use]
        pub fn $get(self) -> $ty {
            self.with(|state| state.fields.$field)
        }

        $(#[$doc])*
        #[cfg(unix)]
        #[inline]
        pub fn $set(self, value: $ty) {
            self.with(|state| state.fields.$field = value);
        }
    };
}

/// A kind of object a reactor's `ptr` holds ([`Running::hold`]), named by the address of
/// a `static`: a crate keeps its key private, so what a reactor holds under it is what
/// that crate gave it.
#[derive(Default)]
pub struct ReactorObjectKey(
    #[expect(
        dead_code,
        reason = "a byte gives each key's static an address of its own"
    )]
    u8,
);

impl ReactorObjectKey {
    #[must_use]
    pub const fn new() -> Self {
        Self(0)
    }
}

/// The `Reactor *` a handler is called with: live, and on this thread, for the call.
#[repr(transparent)]
#[derive(Clone, Copy)]
pub struct Running<'a> {
    raw: *mut c_void,
    live: PhantomData<&'a c_void>,
}

impl Running<'_> {
    /// # Safety
    /// `raw` is a live `Reactor`, on this thread, while the handle is used, and no Rust
    /// reference to its state is held across a call that takes the handle.
    #[must_use]
    pub unsafe fn new(raw: *mut c_void) -> Self {
        Self {
            raw,
            live: PhantomData,
        }
    }

    /// The handle of a reactor just made from `owner`, on this thread, its head set
    /// to the views of where it now stays: live until `swr_v1_reactor_delete` of its
    /// one owner, and borrowed by no reference. The views come from the handle's
    /// pointer, which every later borrow derives from too.
    #[cfg(unix)]
    pub(crate) fn leak(owner: Box<crate::reactor::ReactorState>) -> Running<'static> {
        let reactor = Running {
            raw: Box::into_raw(owner).cast(),
            live: PhantomData,
        };
        let head = crate::reactor::ReactorPrepared::of(reactor.raw);
        reactor.with(|state| state.head = head);
        reactor
    }

    #[must_use]
    pub fn as_ptr(self) -> *mut c_void {
        self.raw
    }

    /// Whether this reactor stopped on a bailout, including during its destroy callbacks.
    #[cfg(unix)]
    #[must_use]
    pub fn bailed_out(self) -> bool {
        self.with(|state| state.fields.bailout)
    }

    /// The reactor's state: live while the handle is, and borrowed by no reference.
    #[cfg(unix)]
    pub(crate) fn state(self) -> *mut crate::reactor::ReactorState {
        self.raw.cast()
    }

    /// `f` over the reactor's handler tables, for one lookup or registration: as
    /// [`Self::with`], `f` reaches no reactor.
    #[cfg(unix)]
    #[inline]
    pub(crate) fn handlers<R>(self, f: impl FnOnce(&Handlers) -> R) -> R {
        self.with(|state| f(&state.handlers))
    }

    /// `f` over the reactor's socket registry, for one lookup: as [`Self::with`], `f`
    /// reaches no reactor, so nothing registers or removes a socket meanwhile.
    #[cfg(unix)]
    #[inline]
    pub(crate) fn registry<R>(self, f: impl FnOnce(&crate::reactor::Registry) -> R) -> R {
        self.with(|state| f(&state.registry))
    }

    /// The reactor's state, borrowed for `f` alone. `f` reaches no reactor (no handle's
    /// method, no `event_api` call, no handler, callback or task): a nested borrow
    /// would be a second `&mut` to the same state, which debug builds refuse.
    #[cfg(unix)]
    #[inline]
    #[track_caller]
    pub(crate) fn with<R>(self, f: impl FnOnce(&mut crate::reactor::ReactorState) -> R) -> R {
        #[cfg(debug_assertions)]
        let _borrowing = {
            assert!(!BORROWED.replace(true), "a reactor's state borrowed twice");
            core::mem::DropGuard::new((), |()| BORROWED.set(false))
        };
        // SAFETY: the handle's live state, which no reference holds across a call that
        // takes the handle (its contract); this borrow ends with `f`, which takes none.
        f(unsafe { &mut *self.state() })
    }

    /// Whether the reactor has deferred tasks waiting (`defer_tasks`).
    #[cfg(unix)]
    #[inline]
    pub(crate) fn has_deferred(self) -> bool {
        self.with(|state| state.fields.defer_tasks.is_some())
    }

    /// `Reactor::ptr`: the object the reactor serves (a server or a process pool).
    #[cfg(unix)]
    #[inline]
    #[must_use]
    pub fn object(self) -> *mut c_void {
        self.with(|state| state.fields.ptr)
    }

    /// `Reactor::ptr = object`, held under no key: [`Self::object_of`] returns it for none.
    #[cfg(unix)]
    #[inline]
    pub fn set_object(self, object: *mut c_void) {
        self.with(|state| {
            state.fields.ptr = object;
            state.object_key = None;
        });
    }

    /// `Reactor::ptr = object`, held under `key`: [`Self::object_of`] with `key` returns
    /// it until the next `hold` or [`Self::set_object`].
    #[cfg(unix)]
    #[inline]
    pub fn hold(self, key: &'static ReactorObjectKey, object: *mut c_void) {
        self.with(|state| {
            state.fields.ptr = object;
            state.object_key = Some(key);
        });
    }

    /// `Reactor::ptr`, when the reactor holds it under `key` ([`Self::hold`]).
    #[cfg(unix)]
    #[inline]
    #[must_use]
    pub fn object_of(self, key: &'static ReactorObjectKey) -> Option<*mut c_void> {
        self.with(|state| {
            let held = state.object_key.is_some_and(|held| std::ptr::eq(held, key));
            held.then_some(state.fields.ptr)
        })
    }
    field!(
        /// `Reactor::id`.
        id, set_id, id: u16
    );
    field!(
        /// `Reactor::running`.
        running, set_running, running: bool
    );
    field!(
        /// `Reactor::wait_exit`.
        wait_exit, set_wait_exit, wait_exit: bool
    );
    field!(
        /// `Reactor::timeout_msec`.
        timeout_msec, set_timeout_msec, timeout_msec: i32
    );
    field!(
        /// `Reactor::destroyed`.
        destroyed, set_destroyed, destroyed: bool
    );
    field!(
        /// `Reactor::max_socket`.
        max_socket, set_max_socket, max_socket: u32
    );
    field!(
        /// `Reactor::close`: how the reactor closes one of its sockets.
        close, set_close, close: Option<crate::reactor::ReactorClose>
    );
}

#[cfg(debug_assertions)]
thread_local! {
    /// Whether a [`Running::with`] borrow is live on this thread.
    static BORROWED: Cell<bool> = const { Cell::new(false) };
}

const FD_TYPES: usize = crate::constants::SW_MAX_FDTYPE as usize;

/// A handler table, also the wait loop's handler kind.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Table {
    Read,
    Write,
    Error,
}

/// Each slot is a cell: a registration needs only the shared tables, as a lookup does.
#[derive(Default)]
pub struct Handlers {
    tables: [Cell<[Option<Handler>; 3]>; FD_TYPES],
    /// Write and error fallbacks (`default_write_handler`, `default_error_handler`).
    pub(crate) defaults: [Cell<Option<Handler>>; 2],
}

/// `Reactor::isset_*_event`'s order: the table an `events` mask names, if any.
fn table(events: Events) -> Option<Table> {
    if events.reads() {
        Some(Table::Read)
    } else if events.intersects(Events::WRITE) {
        Some(Table::Write)
    } else if events.intersects(Events::ERROR) {
        Some(Table::Error)
    } else {
        None
    }
}

impl Handlers {
    /// Replace all three registrations together, including previously installed overrides.
    pub(crate) fn register(
        &self,
        fd_type: c_int,
        read: Option<Handler>,
        write: Option<Handler>,
        error: ErrorHandler,
    ) -> bool {
        let Some(slot) = usize::try_from(fd_type)
            .ok()
            .and_then(|fd| self.tables.get(fd))
        else {
            return false;
        };
        let error = match error {
            ErrorHandler::ConnectionDefault => None,
            ErrorHandler::Callback(handler) => Some(handler),
        };
        slot.set([read, write, error]);
        true
    }

    /// `Reactor::isset_handler`.
    pub(crate) fn isset(&self, fd_type: c_int, events: Events) -> bool {
        table(events).is_some_and(|kind| self.registered(kind, fd_type).is_some())
    }

    fn registered(&self, kind: Table, fd_type: c_int) -> Option<Handler> {
        let row = self.tables.get(usize::try_from(fd_type).ok()?)?.get();
        row.get(kind as usize).copied().flatten()
    }

    /// `default_write_handler` (0) or `default_error_handler` (1).
    pub(crate) fn fallback(&self, index: usize) -> Option<Handler> {
        self.defaults.get(index).and_then(Cell::get)
    }

    /// `Reactor::get_handler(fd_type, event)`; none for an `event` that is not exactly
    /// `SW_EVENT_READ`, `SW_EVENT_WRITE` or `SW_EVENT_ERROR`, where upstream aborts.
    pub(crate) fn of_event(&self, fd_type: c_int, event: Events) -> Option<Handler> {
        let kind = match event {
            Events::READ => Table::Read,
            Events::WRITE => Table::Write,
            Events::ERROR => Table::Error,
            _ => return None,
        };
        self.get(kind, fd_type)
    }

    /// `Reactor::get_handler`: the registered handler, else the write or error default.
    fn get(&self, kind: Table, fd_type: c_int) -> Option<Handler> {
        let registered = self.registered(kind, fd_type);
        match kind {
            Table::Read => registered,
            Table::Write => registered.or(self.defaults[0].get()),
            Table::Error => registered.or(self.defaults[1].get()),
        }
    }

    /// `Reactor::get_error_handler`: error, else read, else write.
    pub(crate) fn dispatch(&self, kind: Table, fd_type: c_int) -> Option<Handler> {
        if kind != Table::Error {
            return self.get(kind, fd_type);
        }
        self.get(Table::Error, fd_type)
            .or_else(|| self.get(Table::Read, fd_type))
            .or_else(|| self.get(Table::Write, fd_type))
    }
}

/// The write (`kind` 1) or error (`kind` 2) default handler.
pub fn swr_v1_reactor_handlers_set_default(
    handlers: &Handlers,
    kind: u32,
    handler: Option<Handler>,
) {
    if let Some(slot) = kind
        .checked_sub(1)
        .and_then(|i| handlers.defaults.get(i as usize))
    {
        slot.set(handler);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn one(_: Running<'_>, _: &mut Event) -> bool {
        true
    }
    fn two(_: Running<'_>, _: &mut Event) -> bool {
        true
    }
    fn three(_: Running<'_>, _: &mut Event) -> bool {
        true
    }

    /// Which of the handlers above `handler` is: 0 for none.
    fn call(handler: Option<Handler>) -> usize {
        let all: [Handler; 3] = [one, two, three];
        handler.map_or(0, |h| {
            1 + all
                .iter()
                .position(|&known| std::ptr::fn_addr_eq(known, h))
                .unwrap()
        })
    }

    #[test]
    fn upstream_lookup_order() {
        let h = Handlers::default();
        assert!(h.register(5, Some(one), None, ErrorHandler::ConnectionDefault));
        assert!(h.isset(5, Events::NONE));
        assert!(h.isset(5, Events::READ));
        assert!(!h.isset(5, Events::WRITE));
        assert!(!h.register(32, Some(two), None, ErrorHandler::ConnectionDefault));
        assert!(!h.isset(5, Events::DEFAULT));

        // Write and error fall back to the defaults; error dispatch then to read.
        assert_eq!(call(h.get(Table::Write, 5)), 0);
        swr_v1_reactor_handlers_set_default(&h, 1, Some(two));
        assert_eq!(call(h.get(Table::Write, 5)), 2);
        assert_eq!(call(h.get(Table::Error, 5)), 0);
        assert_eq!(call(h.dispatch(Table::Error, 5)), 1);
        swr_v1_reactor_handlers_set_default(&h, 2, Some(three));
        assert_eq!(call(h.dispatch(Table::Error, 5)), 3);
        assert_eq!(call(h.fallback(1)), 3);
        assert!(h.fallback(2).is_none());
        assert!(h.register(5, Some(one), Some(one), ErrorHandler::ConnectionDefault));
        assert!(h.isset(5, Events::WRITE | Events::ERROR));
        assert_eq!(call(h.get(Table::Write, 5)), 1);
        // Error-only types: no read handler, the error default still applies.
        assert_eq!(call(h.get(Table::Read, 6)), 0);
        assert_eq!(call(h.dispatch(Table::Error, 6)), 3);
        assert_eq!(call(h.get(Table::Read, -1)), 0);
    }

    #[test]
    fn replacing_a_descriptor_restores_its_selected_fallback() {
        let handlers = Handlers::default();
        swr_v1_reactor_handlers_set_default(&handlers, 2, Some(three));
        handlers.register(5, Some(one), Some(one), ErrorHandler::Callback(two));
        assert_eq!(call(handlers.dispatch(Table::Error, 5)), 2);
        handlers.register(5, Some(one), None, ErrorHandler::ConnectionDefault);
        assert_eq!(call(handlers.dispatch(Table::Error, 5)), 3);
        assert!(!handlers.isset(5, Events::WRITE));
        assert!(!handlers.isset(5, Events::ERROR));
    }

    /// An object held under a key reads back under that key alone, until `set_object`
    /// or another hold replaces it.
    #[test]
    fn a_held_object_reads_back_under_its_key_only() {
        static ONE: ReactorObjectKey = ReactorObjectKey::new();
        static TWO: ReactorObjectKey = ReactorObjectKey::new();
        let state = crate::reactor::reactor_state::tests::state(3, 4);
        let reactor = crate::reactor::reactor_state::tests::running(state.owner);
        let mut object = 0_u8;
        let object = (&raw mut object).cast::<c_void>();
        assert_eq!(reactor.object_of(&ONE), None);
        reactor.hold(&ONE, object);
        assert_eq!(reactor.object(), object);
        assert_eq!(reactor.object_of(&ONE), Some(object));
        assert_eq!(reactor.object_of(&TWO), None);
        reactor.hold(&TWO, object);
        assert_eq!(reactor.object_of(&ONE), None);
        assert_eq!(reactor.object_of(&TWO), Some(object));
        reactor.set_object(object);
        assert_eq!(reactor.object(), object);
        assert_eq!(reactor.object_of(&TWO), None);
        unsafe { crate::reactor::reactor_state::tests::free(state.owner) };
    }
}
