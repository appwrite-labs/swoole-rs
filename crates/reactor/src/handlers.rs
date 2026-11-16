//! The reactor's handler tables (upstream `Reactor::read_handler`, `write_handler`,
//! `error_handler` and the two defaults): which `Handler` runs for an
//! fd type's read, write or error readiness.
use crate::{Event, Events};
use std::cell::Cell;
use std::ffi::{c_int, c_void};
use std::marker::PhantomData;

/// `ReactorHandler`: `int (*)(Reactor *, Event *)`. The loop lends the handler its
/// reactor and the event it prepared for this call. Only Rust registers and calls one.
/// A handler runs only for the fd types and reactors it was registered with.
pub type Handler = fn(Running<'_>, &mut Event) -> c_int;

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

    /// The handle of a reactor just made from `owner`, on this thread: live until
    /// `swr_v1_reactor_delete` of its one owner, and borrowed by no reference.
    #[cfg(unix)]
    pub(crate) fn leak(owner: Box<crate::ReactorState>) -> Running<'static> {
        Running {
            raw: crate::ReactorState::into_raw(owner).cast(),
            live: PhantomData,
        }
    }

    #[must_use]
    pub fn as_ptr(self) -> *mut c_void {
        self.raw
    }

    /// The reactor's state: live while the handle is, and borrowed by no reference.
    #[cfg(unix)]
    pub(crate) fn state(self) -> *mut crate::ReactorState {
        self.raw.cast()
    }

    /// The reactor's handler tables, for one lookup or registration: the borrow of the
    /// handle ends before any call that takes it.
    #[cfg(unix)]
    #[inline]
    pub(crate) fn handlers(&self) -> &Handlers {
        let handlers = self
            .state()
            .wrapping_byte_add(std::mem::offset_of!(crate::ReactorState, handlers))
            .cast::<Handlers>();
        // SAFETY: the handle's live tables, which no mutable reference holds (the
        // handle's contract); their slots are cells, so sharing them is enough to set one.
        unsafe { &*handlers }
    }

    /// The reactor's socket registry, for one lookup: the borrow of the handle ends
    /// before any call that takes it, so before anything registers or removes a socket.
    #[cfg(unix)]
    pub(crate) fn registry(&self) -> &crate::Registry {
        let registry = self
            .state()
            .wrapping_byte_add(std::mem::offset_of!(crate::ReactorState, registry))
            .cast::<crate::Registry>();
        // SAFETY: the handle's live registry, which no mutable reference holds (the
        // handle's contract).
        unsafe { &*registry }
    }

    /// The reactor's state, borrowed for `f` alone. `f` reaches no reactor (no handle's
    /// method, no `event_api` call, no handler, callback or task): a nested borrow
    /// would be a second `&mut` to the same state, which debug builds refuse.
    #[cfg(unix)]
    #[inline]
    #[track_caller]
    pub(crate) fn with<R>(self, f: impl FnOnce(&mut crate::ReactorState) -> R) -> R {
        #[cfg(debug_assertions)]
        let _borrowing = {
            assert!(!BORROWED.replace(true), "a reactor's state borrowed twice");
            Borrowing
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
        close, set_close, close: Option<crate::ReactorClose>
    );
}

#[cfg(debug_assertions)]
thread_local! {
    /// Whether a [`Running::with`] borrow is live on this thread.
    static BORROWED: Cell<bool> = const { Cell::new(false) };
}

/// A [`Running::with`] borrow, which ends when `f` returns or unwinds.
#[cfg(debug_assertions)]
struct Borrowing;

#[cfg(debug_assertions)]
impl Drop for Borrowing {
    fn drop(&mut self) {
        BORROWED.set(false);
    }
}

const FD_TYPES: usize = swoole_rs_corebase::constants::SW_MAX_FDTYPE as usize;

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
    tables: [[Cell<Option<Handler>>; FD_TYPES]; 3],
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
    /// `Reactor::set_handler`: false when `events` names no table (upstream asserts) or
    /// `fd_type` is out of range.
    pub(crate) fn set(&self, fd_type: c_int, events: Events, handler: Option<Handler>) -> bool {
        self.slot(fd_type, events)
            .map(|slot| slot.set(handler))
            .is_some()
    }

    /// `Reactor::isset_handler`.
    pub(crate) fn isset(&self, fd_type: c_int, events: Events) -> bool {
        table(events).is_some_and(|kind| self.registered(kind, fd_type).is_some())
    }

    fn slot(&self, fd_type: c_int, events: Events) -> Option<&Cell<Option<Handler>>> {
        let table = table(events)?;
        self.tables
            .get(table as usize)?
            .get(usize::try_from(fd_type).ok()?)
    }

    fn registered(&self, kind: Table, fd_type: c_int) -> Option<Handler> {
        let row = self.tables.get(kind as usize)?;
        row.get(usize::try_from(fd_type).ok()?)?.get()
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

    fn one(_: Running<'_>, _: &mut Event) -> c_int {
        1
    }
    fn two(_: Running<'_>, _: &mut Event) -> c_int {
        2
    }
    fn three(_: Running<'_>, _: &mut Event) -> c_int {
        3
    }

    fn call(handler: Option<Handler>) -> c_int {
        let mut event = Event::empty();
        handler.map_or(0, |h| {
            h(unsafe { Running::new(std::ptr::null_mut()) }, &mut event)
        })
    }

    #[test]
    fn upstream_lookup_order() {
        let mut handlers = Handlers::default();
        let h = &raw mut handlers;
        unsafe {
            // A mask below SW_EVENT_DEAULT is a read registration, as upstream.
            assert!((*h).set(5, Events::NONE, Some(one)));
            assert!((*h).isset(5, Events::READ));
            assert!(!(*h).isset(5, Events::WRITE));
            assert!(!(*h).set(5, Events::DEFAULT, Some(two)));
            assert!(!(*h).set(32, Events::READ, Some(two)));
            assert!(!(*h).isset(5, Events::DEFAULT));

            // Write and error fall back to the defaults; error dispatch then to read.
            assert_eq!(call((*h).get(Table::Write, 5)), 0);
            swr_v1_reactor_handlers_set_default(&*h, 1, Some(two));
            assert_eq!(call((*h).get(Table::Write, 5)), 2);
            assert_eq!(call((*h).get(Table::Error, 5)), 0);
            assert_eq!(call((*h).dispatch(Table::Error, 5)), 1);
            swr_v1_reactor_handlers_set_default(&*h, 2, Some(three));
            assert_eq!(call((*h).dispatch(Table::Error, 5)), 3);
            assert_eq!(call((*h).fallback(1)), 3);
            assert!((*h).fallback(2).is_none());
            assert!((*h).set(5, Events::WRITE | Events::ERROR, Some(one)));
            assert_eq!(call((*h).get(Table::Write, 5)), 1);
            // Error-only types: no read handler, the error default still applies.
            assert_eq!(call((*h).get(Table::Read, 6)), 0);
            assert_eq!(call((*h).dispatch(Table::Error, 6)), 3);
            assert_eq!(call((*h).get(Table::Read, -1)), 0);
        }
    }

    /// An object held under a key reads back under that key alone, until `set_object`
    /// or another hold replaces it.
    #[test]
    fn a_held_object_reads_back_under_its_key_only() {
        static ONE: ReactorObjectKey = ReactorObjectKey::new();
        static TWO: ReactorObjectKey = ReactorObjectKey::new();
        let state = crate::reactor_state::tests::state(3, 4);
        let reactor = crate::reactor_state::tests::running(state.owner);
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
        unsafe { crate::reactor_state::tests::free(state.owner) };
    }
}
