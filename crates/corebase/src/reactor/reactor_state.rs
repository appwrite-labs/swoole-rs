// SPDX-License-Identifier: Apache-2.0
// Compatibility derived from Swoole v6.2.4 src/wrapper/event.cc and src/reactor/base.cc
// (Swoole contributors).
//! The `Reactor` object: its public fields and backend resources share one Rust
//! lifetime, from its constructor and `swoole_event_init` to its destructor and
//! `swoole_event_free`. The C++ class is methods over this state (`swoole_reactor.h`).
use crate::reactor::wait_loop::BackendKind;
use crate::reactor::{PollSnapshot, handlers::Handlers, registry::Registry};
use libc::{c_int, c_void};
use std::ptr;
use std::sync::atomic::{AtomicU32, Ordering};

/// `Reactor::close`: frees `socket`, one of the reactor's.
pub type ReactorClose =
    extern "C-unwind" fn(crate::reactor::Running<'_>, crate::reactor::OwnedSocket) -> c_int;

#[repr(C)]
pub struct ReactorFields {
    pub kind: c_int,
    pub ptr: *mut c_void,
    pub native_handle: c_int,
    pub max_event_num: u32,
    pub ready: bool,
    pub running: bool,
    pub start: bool,
    pub once: bool,
    pub wait_exit: bool,
    pub destroyed: bool,
    pub bailout: bool,
    pub timed_out: bool,
    pub timeout_msec: i32,
    pub id: u16,
    pub max_socket: u32,
    pub last_malloc_trim_time: libc::time_t,
    pub close: Option<ReactorClose>,
    // Rust-only fields follow the native head. No native caller reads these.
    /// No queue until a task is deferred; detached before its callbacks run.
    pub defer_tasks: Option<crate::reactor::ReactorDeferred>,
    /// Callbacks run as the reactor is destroyed.
    pub destroy_callbacks: Option<crate::reactor::ReactorDeferred>,
    /// `Reactor::idle_task` and `future_task` (`Swoole\Event::cycle`): none until set.
    pub idle_task: Option<std::rc::Rc<dyn crate::reactor::event_api::CycleTask>>,
    pub future_task: Option<std::rc::Rc<dyn crate::reactor::event_api::CycleTask>>,
}

/// The C++ `Reactor` object (`swr_v1_reactor_head`, which the methods-only class
/// extends): its prepared views of this state, then its public fields, then Rust's own.
#[repr(C)]
pub struct ReactorState {
    pub(crate) head: ReactorPrepared,
    pub(crate) fields: ReactorFields,
    pub(crate) registry: Registry,
    pub(crate) handlers: Handlers,
    pub(crate) backend: Backend,
    pub(crate) backend_fd: c_int,
    pub(crate) backend_limit: c_int,
    pub(crate) callbacks: crate::reactor::callbacks::Callbacks,
    /// The key `fields.ptr` is held under ([`crate::reactor::Running::hold`]), or none.
    pub(crate) object_key: Option<&'static crate::reactor::ReactorObjectKey>,
    /// The `defer_tasks` queues the defer end callback detached and is running, innermost
    /// last: one a task bailed out of stays here for the drop to free.
    pub(crate) running_defer: Vec<crate::reactor::ReactorDeferred>,
    /// Process-unique and nonzero: the mark a socket this registry names carries.
    pub(crate) serial: u32,
}
// swoole_rs_reactor.h asserts the same layout for swr_v1_reactor_head.
const _: () = assert!(
    std::mem::offset_of!(ReactorState, fields) == size_of::<ReactorPrepared>(),
    "Reactor's fields follow its prepared views"
);
#[cfg(target_pointer_width = "64")]
const _: () = assert!(
    size_of::<ReactorPrepared>() == 32 && std::mem::offset_of!(ReactorFields, defer_tasks) == 64,
    "swr_v1_reactor_head's layout"
);

/// The backend's output arrays, allocated once and zeroed, so each entry is a value
/// the wait loop reads in place.
pub(crate) enum Backend {
    None,
    Poll(PollSnapshot),
    #[cfg(target_os = "linux")]
    Epoll(Vec<libc::epoll_event>),
    #[cfg(any(
        target_os = "macos",
        target_os = "freebsd",
        target_os = "openbsd",
        target_os = "netbsd",
        target_os = "dragonfly"
    ))]
    Kqueue(Vec<libc::kevent>),
}
impl Drop for ReactorState {
    fn drop(&mut self) {
        self.close_backend();
    }
}

/// Clears `socket`'s mark if it names the reactor of `serial`: one another reactor has
/// since registered stays that reactor's.
fn unmark(socket: crate::reactor::Socket<'_>, serial: u32) {
    _ = socket
        .mark()
        .compare_exchange(serial, 0, Ordering::Relaxed, Ordering::Relaxed);
}

/// The next reactor's serial.
static SERIALS: AtomicU32 = AtomicU32::new(1);

/// A serial no other reactor of the process has, and never 0, which marks a socket no
/// registry names.
fn next_serial() -> u32 {
    loop {
        let serial = SERIALS.fetch_add(1, Ordering::Relaxed);
        if serial != 0 {
            return serial;
        }
    }
}

impl crate::reactor::Running<'_> {
    /// The reactor's serial ([`ReactorState::serial`]).
    pub(crate) fn serial(self) -> u32 {
        self.with(|state| state.serial)
    }
}

impl ReactorState {
    /// Takes the queues [`swr_v1_reactor_delete`] still has to free out of the state.
    fn detach_queues(&mut self) -> Vec<crate::reactor::ReactorDeferred> {
        let mut queues = std::mem::take(&mut self.running_defer);
        queues.extend(self.fields.destroy_callbacks.take());
        queues.extend(self.fields.defer_tasks.take());
        queues
    }
}
#[repr(C)]
#[derive(Clone, Copy)]
pub struct ReactorPrepared {
    pub owner: *mut ReactorState,
    pub fields: *mut ReactorFields,
    pub registry: *mut Registry,
    pub handlers: *mut Handlers,
}

impl ReactorPrepared {
    /// The views of the `Reactor` at `reactor`, as its head holds them: a `Reactor`
    /// is its state, so they are the addresses of the state's parts. Computing
    /// them reads nothing; using them needs a live reactor.
    #[must_use]
    pub fn of(reactor: *const c_void) -> Self {
        let owner = reactor.cast::<ReactorState>().cast_mut();
        Self {
            owner,
            fields: owner
                .wrapping_byte_add(std::mem::offset_of!(ReactorState, fields))
                .cast(),
            registry: owner
                .wrapping_byte_add(std::mem::offset_of!(ReactorState, registry))
                .cast(),
            handlers: owner
                .wrapping_byte_add(std::mem::offset_of!(ReactorState, handlers))
                .cast(),
        }
    }
}

/// `Reactor::type_` for a requested type (`Reactor::Type`: auto 0, epoll 1, kqueue 2,
/// poll 3), and the backend that opens for it: auto picks epoll, else kqueue when
/// `SwooleG.enable_kqueue`, else poll; a type not compiled in opens poll.
fn select(requested: c_int) -> (c_int, BackendKind) {
    let linux = cfg!(target_os = "linux");
    let bsd = cfg!(any(
        target_os = "macos",
        target_os = "freebsd",
        target_os = "openbsd",
        target_os = "netbsd",
        target_os = "dragonfly"
    ));
    let selected = match requested {
        0 if linux => 1,
        0 if bsd && crate::swr_v1_core_enable_kqueue() => 2,
        0 => 3,
        _ => requested,
    };
    let opened = match selected {
        1 if linux => BackendKind::Epoll,
        2 if bsd => BackendKind::Kqueue,
        _ => BackendKind::Poll,
    };
    (selected, opened)
}

impl ReactorState {
    /// `Reactor::Reactor(max_event, type)` without its native effects: the state with
    /// the backend `requested` selects open for `count` events, or not ready after
    /// logging the kernel's refusal. Allocation failure aborts (upstream's `bad_alloc`),
    /// as does a negative `count` (`bad_array_new_length`).
    /// The head is set by [`crate::reactor::Running::leak`], once the caller is done with the
    /// box.
    pub(crate) fn open(requested: c_int, count: c_int) -> Box<Self> {
        let (kind, opened) = select(requested);
        let mut owner = Box::new(Self {
            head: ReactorPrepared {
                owner: ptr::null_mut(),
                fields: ptr::null_mut(),
                registry: ptr::null_mut(),
                handlers: ptr::null_mut(),
            },
            fields: ReactorFields {
                kind,
                ptr: ptr::null_mut(),
                native_handle: -1,
                max_event_num: 0,
                ready: false,
                running: false,
                start: false,
                once: false,
                wait_exit: false,
                destroyed: false,
                bailout: false,
                timed_out: false,
                timeout_msec: 0,
                id: 0,
                max_socket: 0,
                last_malloc_trim_time: 0,
                close: None,
                defer_tasks: None,
                destroy_callbacks: None,
                idle_task: None,
                future_task: None,
            },
            registry: Registry::default(),
            handlers: Handlers::default(),
            backend: Backend::None,
            backend_fd: -1,
            backend_limit: 0,
            callbacks: crate::reactor::callbacks::Callbacks::default(),
            object_key: None,
            running_defer: Vec::new(),
            serial: next_serial(),
        });
        if !owner.open_backend(opened, count) {
            let (origin, call) = if opened == BackendKind::Epoll {
                (c"ReactorEpoll::ReactorEpoll", "epoll_create")
            } else {
                (c"ReactorKqueue::ReactorKqueue", "kqueue")
            };
            crate::log::sys_warning(origin, format_args!("{call}() failed"));
        }
        owner
    }

    /// Opens backend `kind` for `count` events: false when the kernel refuses its
    /// descriptor, errno being the kernel's.
    fn open_backend(&mut self, kind: BackendKind, count: c_int) -> bool {
        #[cfg(target_os = "linux")]
        if kind == BackendKind::Epoll {
            // Upstream's fixed size hint; Miri has only `epoll_create1`, the same call
            // without it.
            #[cfg(not(miri))]
            let fd = crate::sys::epoll_create(512);
            #[cfg(miri)]
            let fd = crate::sys::epoll_create1(0);
            self.backend_fd = fd;
        }
        #[cfg(any(
            target_os = "macos",
            target_os = "freebsd",
            target_os = "openbsd",
            target_os = "netbsd",
            target_os = "dragonfly"
        ))]
        if kind == BackendKind::Kqueue {
            self.backend_fd = crate::sys::kqueue();
        }
        if kind != BackendKind::Poll && self.backend_fd < 0 {
            return false;
        }
        let Ok(length) = usize::try_from(count) else {
            crate::fatal("a negative event count");
        };
        self.backend = match kind {
            #[cfg(target_os = "linux")]
            BackendKind::Epoll => Backend::Epoll(output(length)),
            #[cfg(any(
                target_os = "macos",
                target_os = "freebsd",
                target_os = "openbsd",
                target_os = "netbsd",
                target_os = "dragonfly"
            ))]
            BackendKind::Kqueue => Backend::Kqueue(output(length)),
            _ => Backend::Poll(PollSnapshot::new(length)),
        };
        self.backend_limit = count;
        self.fields.native_handle = self.backend_fd;
        self.fields.max_event_num = count.unsigned_abs();
        self.fields.ready = true;
        true
    }
}

/// `count` zeroed records for the kernel to fill, aborting as upstream's `bad_alloc`
/// when they could not be allocated.
#[cfg(any(
    target_os = "linux",
    target_os = "macos",
    target_os = "freebsd",
    target_os = "openbsd",
    target_os = "netbsd",
    target_os = "dragonfly"
))]
fn output<T: crate::sys::Zeroed>(count: usize) -> Vec<T> {
    let mut entries = Vec::new();
    if entries.try_reserve_exact(count).is_err() {
        std::alloc::handle_alloc_error(std::alloc::Layout::new::<T>());
    }
    entries.resize_with(count, crate::sys::zeroed);
    entries
}

/// A reactor [`swr_v1_reactor_new`] made, held by the one owner that deletes it
/// ([`swr_v1_reactor_delete`]); one never deleted stays allocated, as a C++ `Reactor *`
/// never deleted does. It is neither `Copy` nor `Clone`, so a reactor is deleted once.
#[must_use]
pub struct OwnedReactor(crate::reactor::Running<'static>);

impl OwnedReactor {
    /// The `Reactor *`, live until the owner deletes it.
    #[must_use]
    pub fn as_ptr(&self) -> *mut c_void {
        self.0.as_ptr()
    }

    /// The reactor's handle, for as long as the owner is borrowed.
    #[must_use]
    pub fn running(&self) -> crate::reactor::Running<'_> {
        self.0
    }
}

/// `new Reactor(max_event, type)`: the state `ReactorState::open` makes and, once
/// ready, upstream's close and default write handler, the create hook and then the
/// built-in end callbacks and exit conditions. Socket I/O is corebase's TLS module.
pub fn swr_v1_reactor_new(max_event: c_int, kind: c_int) -> OwnedReactor {
    let mut owner = ReactorState::open(kind, max_event);
    if !owner.fields.ready {
        return OwnedReactor(crate::reactor::Running::leak(owner));
    }
    owner.fields.timeout_msec = -1;
    owner.fields.close = Some(crate::tls::reactor_write::reactor_close);
    owner.handlers.defaults[0].set(Some(|reactor, event| {
        crate::tls::reactor_write::reactor_writable_callback(reactor, event)
    }));
    let reactor = crate::reactor::Running::leak(owner);
    crate::hook::call(crate::hook::Hook::ReactorCreate(reactor));
    // The hook has returned.
    reactor.with(|state| crate::reactor::callbacks::install_builtins(&mut state.callbacks));
    OwnedReactor(reactor)
}

/// `delete reactor`: marks it destroyed, runs its destroy callbacks, closes its
/// backend, calls the destroy hook and frees it. Each borrow below ends before a
/// callback, hook or disposer runs.
pub fn swr_v1_reactor_delete(OwnedReactor(handle): OwnedReactor) {
    let state = handle.state();
    let cycle_tasks = handle.with(|state| {
        state.fields.destroyed = true;
        crate::reactor::event_api::take_cycle_tasks(&mut state.fields)
    });
    // Dropping a task may run its owner's code, which may reach the reactor.
    drop(cycle_tasks);
    crate::reactor::deferred::run_destroy(handle);
    // The destroy callbacks were the last to delete sockets from the reactor; with its
    // backend gone nothing can, so the sockets it still names are let go, and freeing
    // one from here (a queued free, say) is no longer the reactor's business.
    handle.with(|state| {
        state.release_sockets();
        state.close_backend();
    });
    crate::hook::call(crate::hook::Hook::ReactorDestroy(handle));
    // A disposer can defer again through the reactor's head (a PHP destructor calling
    // `Event::defer`), so the queues are detached and freed until they stay empty, and
    // no reference to the state (a `Drop`'s `&mut self` included) is live meanwhile.
    loop {
        let queues = handle.with(ReactorState::detach_queues);
        if queues.is_empty() {
            break;
        }
        // Detached queues of this state's, which nothing runs now.
        for queue in queues {
            crate::reactor::deferred::deferred_free(&mut Some(queue));
        }
    }
    // SAFETY: the owner's reactor, `ReactorState::open`'s allocation, which the owner
    // gave up by value: deleted once, and used no more.
    drop(unsafe { Box::from_raw(state) });
}

/// `int swoole_event_init(int flags)`: the thread's `Reactor` with
/// `SW_REACTOR_MAXEVENTS` events, waiting to exit under `SW_EVENTLOOP_WAIT_EXIT`, and the
/// coroutine system's and client's handlers set; `SW_ERR` when no backend opens, or
/// while [`swoole_event_free`] deletes the thread's reactor ([`swoole_event_deleting`]).
#[must_use]
pub fn swoole_event_init(flags: c_int) -> c_int {
    const SW_REACTOR_MAXEVENTS: c_int = 4096;
    const SW_EVENTLOOP_WAIT_EXIT: c_int = 1;
    if swoole_event_deleting() {
        return -1;
    }
    let reactor = swr_v1_reactor_new(SW_REACTOR_MAXEVENTS, 0);
    let handle = reactor.running();
    if !handle.with(|state| state.fields.ready) {
        swr_v1_reactor_delete(reactor);
        return -1;
    }
    handle.with(|state| state.fields.wait_exit = flags & SW_EVENTLOOP_WAIT_EXIT != 0);
    crate::reactor::readiness::init_reactor(handle);
    crate::set_thread_reactor(handle.as_ptr());
    // A reactor this replaces stays allocated, as upstream's overwritten pointer did.
    EVENT_REACTOR.set(Some(reactor));
    0
}

thread_local! {
    /// The reactor `swoole_event_init` made on this thread, which `swoole_event_free`
    /// deletes.
    static EVENT_REACTOR: std::cell::Cell<Option<OwnedReactor>> = const {
        std::cell::Cell::new(None)
    };
    /// The reactor `swoole_event_free` is deleting, which the thread no longer names.
    static DELETING: std::cell::Cell<Option<crate::reactor::Running<'static>>> = const {
        std::cell::Cell::new(None)
    };
}

/// Whether `swoole_event_free` is deleting this thread's reactor: code its delete runs
/// (destroy callbacks, PHP destructors of what it frees) finds no reactor, and may not
/// make one.
#[must_use]
pub fn swoole_event_deleting() -> bool {
    DELETING.get().is_some()
}

/// The reactor `swoole_event_free` is deleting, for one thing only: a socket freed
/// meanwhile leaves its registry, as it would leave the thread's.
pub(crate) fn deleting<'a>() -> Option<crate::reactor::Running<'a>> {
    DELETING.get()
}

/// [`DELETING`] for a delete's span: a bailout out of a disposer clears it too.
struct Deleting;

impl Deleting {
    fn enter(reactor: &OwnedReactor) -> Self {
        DELETING.set(Some(reactor.0));
        Self
    }
}

impl Drop for Deleting {
    fn drop(&mut self) {
        DELETING.set(None);
    }
}

/// `swoole_event_free()`: clears the thread's `Reactor`, then deletes it; false
/// without one. Cleared first, so nothing the delete runs reaches it through the thread;
/// destroy callbacks get it as a handle. Only `swoole_event_init` sets a thread's
/// reactor, so it is the one that call made here; a reactor another thread made and
/// shares (a coroutine thread's) is cleared but left to the thread that owns it.
pub fn swoole_event_free() -> bool {
    let Some(reactor) = crate::reactor::event_api::running().map(crate::reactor::Running::as_ptr)
    else {
        return false;
    };
    crate::set_thread_reactor(ptr::null_mut());
    match EVENT_REACTOR.take() {
        Some(owned) if owned.as_ptr() == reactor => {
            let _deleting = Deleting::enter(&owned);
            swr_v1_reactor_delete(owned);
        }
        other => EVENT_REACTOR.set(other),
    }
    true
}
impl ReactorState {
    /// Which kernel wait the backend runs.
    pub(crate) fn backend_kind(&self) -> BackendKind {
        match self.backend {
            Backend::None | Backend::Poll(_) => BackendKind::Poll,
            #[cfg(target_os = "linux")]
            Backend::Epoll(_) => BackendKind::Epoll,
            #[cfg(any(
                target_os = "macos",
                target_os = "freebsd",
                target_os = "openbsd",
                target_os = "netbsd",
                target_os = "dragonfly"
            ))]
            Backend::Kqueue(..) => BackendKind::Kqueue,
        }
    }

    /// poll's entry `index` and the socket it was built from; none past its end or
    /// for another backend.
    #[inline]
    pub(crate) fn poll_entry(
        &self,
        index: usize,
    ) -> Option<(libc::pollfd, crate::reactor::Heap<'static>)> {
        match &self.backend {
            Backend::Poll(snapshot) => snapshot.entry(index),
            _ => None,
        }
    }

    /// epoll's entry `index`; none past its end or for another backend.
    #[cfg(target_os = "linux")]
    #[inline]
    pub(crate) fn epoll_entry(&self, index: usize) -> Option<libc::epoll_event> {
        match &self.backend {
            Backend::Epoll(array) => array.get(index).copied(),
            _ => None,
        }
    }

    /// kqueue's entry `index`; none past its end or for another backend.
    #[cfg(any(
        target_os = "macos",
        target_os = "freebsd",
        target_os = "openbsd",
        target_os = "netbsd",
        target_os = "dragonfly"
    ))]
    #[inline]
    pub(crate) fn kqueue_entry(&self, index: usize) -> Option<libc::kevent> {
        match &self.backend {
            Backend::Kqueue(array) => array.get(index).copied(),
            _ => None,
        }
    }

    /// Where the backend's output array is, which stays put until the backend closes.
    #[cfg(test)]
    fn output_array(&self) -> *const c_void {
        match &self.backend {
            Backend::None => ptr::null(),
            Backend::Poll(snapshot) => snapshot.entries().cast(),
            #[cfg(target_os = "linux")]
            Backend::Epoll(array) => array.as_ptr().cast(),
            #[cfg(any(
                target_os = "macos",
                target_os = "freebsd",
                target_os = "openbsd",
                target_os = "netbsd",
                target_os = "dragonfly"
            ))]
            Backend::Kqueue(array) => array.as_ptr().cast(),
        }
    }
}
/// Actual kernel wait over the owner's stable arrays: the ready count, or the
/// wait's error, errno still set for the warning. No native callbacks run.
// Its one caller, the wait loop, runs it once per iteration (#1435).
#[inline(always)]
pub(crate) fn kernel_wait(
    reactor: crate::reactor::Running<'_>,
    deferred: bool,
    entries: &mut usize,
) -> std::io::Result<usize> {
    // The kernel wait runs no native callback, so the borrow reaches no reactor.
    reactor.with(|owner| backend_wait(owner, deferred, entries))
}

/// [`kernel_wait`] over the borrowed state.
#[inline(always)]
fn backend_wait(
    owner: &mut ReactorState,
    deferred: bool,
    entries: &mut usize,
) -> std::io::Result<usize> {
    let ready = match &mut owner.backend {
        Backend::None => Err(std::io::Error::last_os_error()),
        Backend::Poll(snapshot) => {
            let count = snapshot.fill(&owner.registry);
            *entries = count;
            let timeout = if deferred {
                0
            } else {
                owner.fields.timeout_msec
            };
            crate::sys::poll(snapshot.filled(), timeout)
        }
        #[cfg(target_os = "linux")]
        Backend::Epoll(array) => {
            // Room for `max_event_num` records, which the array holds.
            let room = usize::try_from(owner.fields.max_event_num)
                .map_or(array.len(), |max| max.min(array.len()));
            crate::sys::epoll_wait(
                owner.backend_fd,
                array.get_mut(..room).unwrap_or_default(),
                if deferred {
                    0
                } else {
                    owner.fields.timeout_msec
                },
            )
        }
        #[cfg(any(
            target_os = "macos",
            target_os = "freebsd",
            target_os = "openbsd",
            target_os = "netbsd",
            target_os = "dragonfly"
        ))]
        Backend::Kqueue(array) => {
            let timeout = crate::reactor::kqueue_timeout(
                owner.fields.timeout_msec,
                owner.fields.timeout_msec <= 0 && deferred,
            );
            crate::sys::kevent_wait(owner.backend_fd, array, timeout.as_ref())
        }
    };
    ready.map(|ready| ready.unsigned_abs() as usize)
}
/// The wait loop's initial state transition, over the actual borrowed slots.
pub(crate) const fn before_wait(fields: &mut ReactorFields) {
    fields.start = true;
    fields.running = true;
    if fields.timeout_msec == 0 {
        fields.timeout_msec = -1;
    }
}

impl ReactorState {
    /// `Reactor::_add`, `_set` and `_del`: the registry and the socket's flags
    /// together. False when add cannot allocate, leaving the flags alone.
    pub(crate) fn record(
        &mut self,
        request: crate::reactor::change::Request<'_>,
        events: crate::reactor::Events,
    ) -> bool {
        use crate::reactor::change::Request;
        let socket = match request {
            Request::Add(socket) | Request::Set(socket) => *socket,
            Request::Del(socket) => socket,
        };
        let serial = self.serial;
        socket.edit(|record| {
            match request {
                Request::Add(added) => {
                    let replaced = self.registry.get(record.fd);
                    if !self.registry.put(record.fd, added, events) {
                        return false;
                    }
                    // A socket whose fd closed unremoved is no longer this registry's.
                    if let Some(replaced) = replaced.filter(|old| old.as_ptr() != added.as_ptr()) {
                        unmark(*replaced, serial);
                    }
                    added.mark().store(serial, Ordering::Relaxed);
                    record.events = events;
                    record.removed = false;
                }
                Request::Set(_) => {
                    record.events = events;
                    self.registry.set(record.fd, events);
                }
                Request::Del(deleted) => {
                    record.events = crate::reactor::Events::NONE;
                    record.removed = true;
                    self.registry.remove(record.fd);
                    unmark(deleted, serial);
                }
            }
            true
        })
    }

    /// Forgets `socket` wherever the registry names it, the kernel left alone: for a
    /// socket being freed whose registration `Reactor::del` could not find by its fd.
    pub(crate) fn forget(&mut self, socket: crate::reactor::Socket<'_>) {
        let named: Vec<c_int> = self
            .registry
            .registered()
            .filter(|&(_, entry, _)| entry.as_ptr() == socket.as_ptr())
            .map(|(fd, ..)| fd)
            .collect();
        for fd in named {
            self.registry.remove(fd);
        }
        socket.edit(|record| {
            record.events = crate::reactor::Events::NONE;
            record.removed = true;
        });
        unmark(socket, self.serial);
    }

    /// Lets every socket the registry names go: freeing one no longer deletes it here.
    fn release_sockets(&self) {
        for (_, socket, _) in self.registry.registered() {
            unmark(*socket, self.serial);
        }
    }

    pub(crate) fn close_backend(&mut self) {
        if self.backend_fd >= 0 {
            _ = crate::sys::close(self.backend_fd);
            self.backend_fd = -1;
        }
        // The wait loop no longer holds pointers to the arrays.
        self.backend = Backend::None;
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::reactor::SocketRecord;
    /// A state with backend `kind` open for `count` events: `new Reactor` without its
    /// native effects.
    pub(crate) fn state(kind: c_int, count: c_int) -> ReactorPrepared {
        let reactor = crate::reactor::Running::leak(ReactorState::open(kind, count));
        reactor.with(|state| state.head)
    }
    /// [`ReactorState::record`] with the test's socket.
    pub(crate) unsafe fn record(
        owner: *mut ReactorState,
        socket: *mut SocketRecord,
        op: crate::reactor::ChangeOp,
        events: crate::reactor::Events,
    ) -> bool {
        let socket = crate::reactor::Heap::from_static(
            unsafe { crate::reactor::Socket::from_ptr(socket) }.unwrap(),
        );
        unsafe { (*owner).record(crate::reactor::change::Request::new(socket, op), events) }
    }
    pub(crate) unsafe fn free(owner: *mut ReactorState) {
        swr_v1_reactor_delete(OwnedReactor(running(owner)));
    }
    /// The test's state as the handle its operations take.
    pub(crate) fn running(owner: *mut ReactorState) -> crate::reactor::Running<'static> {
        unsafe { crate::reactor::Running::new(owner.cast()) }
    }
    #[test]
    fn owner_keeps_stable_public_slots_and_socket_registry() {
        unsafe {
            let state = state(3, 4);
            let fields = state.fields;
            assert_eq!(
                state.owner.cast::<ReactorPrepared>(),
                &raw mut (*state.owner).head
            );
            assert_eq!(fields, &raw mut (*state.owner).fields);
            assert_eq!((*fields).kind, 3);
            assert!((*fields).ready);
            assert_eq!((*state.owner).backend_limit, 4);
            let mut socket = SocketRecord::with_fd(123, 0);
            assert!(record(
                state.owner,
                &raw mut socket,
                crate::reactor::ChangeOp::Add,
                crate::reactor::Events::from_bits(512)
            ));
            assert_eq!(socket.events, crate::reactor::Events::from_bits(512));
            assert!(!socket.removed);
            assert_eq!(
                crate::reactor::swr_v1_reactor_registry_count(state.registry.as_ref()),
                1
            );
            record(
                state.owner,
                &raw mut socket,
                crate::reactor::ChangeOp::Set,
                crate::reactor::Events::from_bits(1024),
            );
            assert_eq!(socket.events, crate::reactor::Events::from_bits(1024));
            record(
                state.owner,
                &raw mut socket,
                crate::reactor::ChangeOp::Del,
                crate::reactor::Events::NONE,
            );
            assert_eq!(socket.events, crate::reactor::Events::NONE);
            assert!(socket.removed);
            assert_eq!(
                crate::reactor::swr_v1_reactor_registry_count(state.registry.as_ref()),
                0
            );
            before_wait(&mut (*state.owner).fields);
            assert!((*fields).running && (*fields).start);
            assert_eq!((*fields).timeout_msec, -1);
            free(state.owner);
        }
    }

    /// A heap record over a pipe's read end, added to the reactor for reading, and the
    /// pipe's write end.
    fn registered(reactor: crate::reactor::Running<'_>) -> (crate::reactor::OwnedSocket, c_int) {
        let mut fds = [0; 2];
        assert_eq!(unsafe { libc::pipe(fds.as_mut_ptr()) }, 0);
        let owned = crate::reactor::OwnedSocket::new(Box::new(SocketRecord::with_fd(fds[0], 0)));
        assert!(crate::reactor::reactor_upstream::change(
            reactor,
            owned.heap(),
            crate::reactor::ChangeOp::Add,
            crate::reactor::Events::READ
        ));
        (owned, fds[1])
    }

    /// The mark at the record's byte 332, where native code's layout has room for it.
    fn mark_at_332(socket: crate::reactor::Socket<'_>) -> u32 {
        unsafe { socket.as_ptr().byte_add(332).cast::<u32>().read() }
    }

    #[test]
    #[cfg_attr(all(miri, not(target_os = "linux")), ignore = "Miri has no kqueue")]
    fn freeing_a_registered_socket_deletes_it_from_the_reactor() {
        #[cfg(target_os = "linux")]
        let native_kind = 1;
        #[cfg(not(target_os = "linux"))]
        let native_kind = 2;
        for kind in [3, native_kind] {
            let state = state(kind, 4);
            let reactor = running(state.owner);
            crate::set_thread_reactor(state.owner.cast());
            let (owned, writer) = registered(reactor);
            assert_eq!(mark_at_332(owned.socket()), reactor.serial());
            assert_eq!(reactor.registry(Registry::count), 1);
            let record = Box::leak(owned.into_box());
            assert_eq!(reactor.registry(Registry::count), 0, "the free deleted it");
            assert!(record.removed && record.events == crate::reactor::Events::NONE);
            let fd = record.fd;
            let record = crate::reactor::Socket::new(record);
            assert_eq!(mark_at_332(record), 0);
            // Its descriptor is the kernel's no more: a write wakes nothing.
            assert_eq!(unsafe { libc::write(writer, b"a".as_ptr().cast(), 1) }, 1);
            unsafe { (*state.fields).timeout_msec = 0 };
            let mut entries = 0;
            if kind != 3 {
                assert_eq!(kernel_wait(reactor, true, &mut entries).ok(), Some(0));
            }
            crate::set_thread_reactor(ptr::null_mut());
            unsafe { free(state.owner) };
            drop(unsafe { Box::from_raw(record.as_ptr()) });
            unsafe { libc::close(fd) };
            unsafe { libc::close(writer) };
        }
    }

    #[test]
    fn a_socket_outliving_its_reactor_frees_without_it() {
        let state = state(3, 4);
        let reactor = running(state.owner);
        let (owned, writer) = registered(reactor);
        assert_eq!(mark_at_332(owned.socket()), reactor.serial());
        unsafe { free(state.owner) };
        assert_eq!(
            mark_at_332(owned.socket()),
            0,
            "the reactor's delete let it go"
        );
        // No reactor runs on this thread, and none names the record: the free goes on.
        let record = owned.into_box();
        unsafe { libc::close(record.fd) };
        unsafe { libc::close(writer) };
    }

    #[test]
    fn reactors_have_distinct_nonzero_serials() {
        let (a, b) = (state(3, 1), state(3, 1));
        let (a_serial, b_serial) = (running(a.owner).serial(), running(b.owner).serial());
        assert!(a_serial != 0 && b_serial != 0 && a_serial != b_serial);
        unsafe { free(a.owner) };
        unsafe { free(b.owner) };
    }

    #[test]
    #[cfg_attr(miri, ignore = "Miri can't spawn a process")]
    fn actual_backend_waits_read_pipe_and_close_ownership_before_destroy_hook() {
        if crate::reactor::isolated_process::run_in_child(
            "reactor::reactor_state::tests::actual_backend_waits_read_pipe_and_close_ownership_before_destroy_hook",
        ) {
            return;
        }
        #[cfg(target_os = "linux")]
        let native_kind = 1;
        #[cfg(not(target_os = "linux"))]
        let native_kind = 2;
        for kind in [3, native_kind] {
            unsafe {
                let state = state(kind, 4);
                let descriptor = (*state.owner).backend_fd;
                let mut fds = [0; 2];
                assert_eq!(libc::pipe(fds.as_mut_ptr()), 0);
                let mut socket = SocketRecord::with_fd(fds[0], 0);
                assert!(record(
                    state.owner,
                    &raw mut socket,
                    crate::reactor::ChangeOp::Add,
                    crate::reactor::Events::from_bits(512)
                ));
                #[cfg(target_os = "linux")]
                if kind == 1 {
                    assert_eq!(
                        crate::reactor::registration::epoll_ctl(
                            descriptor,
                            crate::reactor::ChangeOp::Add,
                            fds[0],
                            crate::reactor::Events::from_bits(512)
                        )
                        .ok(),
                        Some(())
                    );
                }
                #[cfg(not(target_os = "linux"))]
                if kind == 2 {
                    assert_eq!(
                        crate::reactor::registration::kqueue_add(
                            descriptor,
                            fds[0],
                            crate::reactor::Events::from_bits(512)
                        ),
                        Ok(())
                    );
                }
                assert_eq!(libc::write(fds[1], b"a".as_ptr().cast(), 1), 1);
                (*state.fields).timeout_msec = 0;
                let events = (*state.owner).output_array();
                let mut entries = 0;
                assert_eq!(
                    kernel_wait(running(state.owner), false, &mut entries).ok(),
                    Some(1)
                );
                assert_eq!(events, (*state.owner).output_array());
                if kind == 3 {
                    assert_eq!(entries, 1);
                }
                // Public handle assignment never transfers or changes private ownership.
                (*state.fields).native_handle = -1;
                assert_eq!((*state.owner).backend_fd, descriptor);
                (*state.owner).close_backend();
                if descriptor >= 0 {
                    assert_eq!(libc::fcntl(descriptor, libc::F_GETFD), -1);
                }
                assert_eq!((*state.fields).native_handle, -1);
                free(state.owner);
                libc::close(fds[0]);
                libc::close(fds[1]);
            }
        }
    }
    #[test]
    #[cfg_attr(miri, ignore = "Miri can't spawn a process")]
    fn owner_drop_preserves_a_reopened_backend_descriptor_number() {
        if crate::reactor::isolated_process::run_in_child(
            "reactor::reactor_state::tests::owner_drop_preserves_a_reopened_backend_descriptor_number",
        ) {
            return;
        }
        #[cfg(target_os = "linux")]
        let kind = 1;
        #[cfg(not(target_os = "linux"))]
        let kind = 2;
        unsafe {
            let state = state(kind, 4);
            let fd = (*state.owner).backend_fd;
            (*state.owner).close_backend();
            assert_eq!((*state.owner).backend_fd, -1);
            assert_eq!((*state.fields).native_handle, fd);
            crate::reactor::isolated_process::replacement_survives(fd, || {
                free(state.owner);
            });
        }
    }
}
