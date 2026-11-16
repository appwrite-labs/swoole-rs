// SPDX-License-Identifier: Apache-2.0
// Compatibility derived from Swoole v6.2.4 src/wrapper/event.cc and src/reactor/base.cc
// (Swoole contributors).
//! The `Reactor` object: its public fields and backend resources share one Rust
//! lifetime, from its constructor and `swoole_event_init` to its destructor and
//! `swoole_event_free`. The C++ class is methods over this state (`swoole_reactor.h`).
use crate::wait_loop::BackendKind;
use crate::{PollSnapshot, handlers::Handlers, registry::Registry};
use libc::{c_int, c_void};
use std::ptr;
use swoole_rs_corebase::HookType;

/// `Reactor::close`: frees `socket`, one of the reactor's.
pub type ReactorClose = extern "C-unwind" fn(crate::Running<'_>, crate::OwnedSocket) -> c_int;

/// `Reactor::idle_task` and `future_task` (`Swoole\Event::cycle`): unset until set.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct ReactorTask {
    pub callback: Option<crate::DeferCall>,
    pub data: *mut c_void,
}

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
    pub idle_task: ReactorTask,
    pub future_task: ReactorTask,
    // Rust-only fields follow the native head. No native caller reads these queues.
    /// No queue until a task is deferred; detached before its callbacks run.
    pub defer_tasks: Option<crate::ReactorDeferred>,
    /// Callbacks run as the reactor is destroyed.
    pub destroy_callbacks: Option<crate::ReactorDeferred>,
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
    pub(crate) callbacks: crate::callbacks::Callbacks,
    /// The key `fields.ptr` is held under ([`crate::Running::hold`]), or none.
    pub(crate) object_key: Option<&'static crate::ReactorObjectKey>,
    /// The `defer_tasks` queues the defer end callback detached and is running, innermost
    /// last: one a task bailed out of stays here for the drop to free.
    pub(crate) running_defer: Vec<crate::ReactorDeferred>,
}
// swoole_rs_reactor.h asserts the same layout for swr_v1_reactor_head.
const _: () = assert!(
    std::mem::offset_of!(ReactorState, fields) == size_of::<ReactorPrepared>(),
    "Reactor's fields follow its prepared views"
);
#[cfg(target_pointer_width = "64")]
const _: () = assert!(
    size_of::<ReactorPrepared>() == 32 && std::mem::offset_of!(ReactorFields, defer_tasks) == 96,
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

impl ReactorState {
    /// Takes the queues [`swr_v1_reactor_delete`] still has to free out of the state.
    fn detach_queues(&mut self) -> Vec<crate::ReactorDeferred> {
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
        0 if bsd && swoole_rs_corebase::swr_v1_core_enable_kqueue() => 2,
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
    /// The head is set by [`crate::Running::leak`], once the caller is done with the
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
                idle_task: ReactorTask {
                    callback: None,
                    data: ptr::null_mut(),
                },
                future_task: ReactorTask {
                    callback: None,
                    data: ptr::null_mut(),
                },
            },
            registry: Registry::default(),
            handlers: Handlers::default(),
            backend: Backend::None,
            backend_fd: -1,
            backend_limit: 0,
            callbacks: crate::callbacks::Callbacks::default(),
            object_key: None,
            running_defer: Vec::new(),
        });
        if !owner.open_backend(opened, count) {
            let (origin, call) = if opened == BackendKind::Epoll {
                (c"ReactorEpoll::ReactorEpoll", "epoll_create")
            } else {
                (c"ReactorKqueue::ReactorKqueue", "kqueue")
            };
            swoole_rs_corebase::log::sys_warning(origin, format_args!("{call}() failed"));
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
            let fd = swoole_rs_corebase::sys::epoll_create(512);
            #[cfg(miri)]
            let fd = swoole_rs_corebase::sys::epoll_create1(0);
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
            self.backend_fd = swoole_rs_corebase::sys::kqueue();
        }
        if kind != BackendKind::Poll && self.backend_fd < 0 {
            return false;
        }
        let Ok(length) = usize::try_from(count) else {
            swoole_rs_corebase::fatal("a negative event count");
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
fn output<T: swoole_rs_corebase::sys::Zeroed>(count: usize) -> Vec<T> {
    let mut entries = Vec::new();
    if entries.try_reserve_exact(count).is_err() {
        std::alloc::handle_alloc_error(std::alloc::Layout::new::<T>());
    }
    entries.resize_with(count, swoole_rs_corebase::sys::zeroed);
    entries
}

/// A reactor [`swr_v1_reactor_new`] made, held by the one owner that deletes it
/// ([`swr_v1_reactor_delete`]); one never deleted stays allocated, as a C++ `Reactor *`
/// never deleted does. It is neither `Copy` nor `Clone`, so a reactor is deleted once.
#[must_use]
pub struct OwnedReactor(crate::Running<'static>);

impl OwnedReactor {
    /// The `Reactor *`, live until the owner deletes it.
    #[must_use]
    pub fn as_ptr(&self) -> *mut c_void {
        self.0.as_ptr()
    }

    /// The reactor's handle, for as long as the owner is borrowed.
    #[must_use]
    pub fn running(&self) -> crate::Running<'_> {
        self.0
    }
}

/// `new Reactor(max_event, type)`: the state `ReactorState::open` makes and, once
/// ready, upstream's close and default write handler, the create hook and then the
/// built-in end callbacks and exit conditions. Those two are crates/tls's, which
/// `swoole_init` registers with its writes ([`crate::event_api::set_writers`]).
pub fn swr_v1_reactor_new(max_event: c_int, kind: c_int) -> OwnedReactor {
    let mut owner = ReactorState::open(kind, max_event);
    if !owner.fields.ready {
        return OwnedReactor(crate::Running::leak(owner));
    }
    owner.fields.timeout_msec = -1;
    if let Some(writers) = crate::event_api::registered_writers() {
        owner.fields.close = Some(writers.close);
        owner.handlers.defaults[0].set(Some(writers.writable));
    }
    let reactor = crate::Running::leak(owner);
    reactor_hook(HookType::ON_REACTOR_CREATE, reactor);
    // The hook has returned.
    reactor.with(|state| crate::callbacks::install_builtins(&mut state.callbacks));
    OwnedReactor(reactor)
}

/// The `kind` hooks (`ON_REACTOR_CREATE` or `ON_REACTOR_DESTROY`) over `reactor`, if
/// any is set.
fn reactor_hook(kind: HookType, reactor: crate::Running<'_>) {
    if swoole_rs_corebase::swoole_isset_hook(kind) {
        // SAFETY: the reactor hooks take a `Reactor *`, and the handle's is live.
        unsafe { swoole_rs_corebase::swoole_call_hook(kind, reactor.as_ptr()) };
    }
}

/// `delete reactor`: marks it destroyed, runs its destroy callbacks, closes its
/// backend, calls the destroy hook and frees it. Each borrow below ends before a
/// callback, hook or disposer runs.
pub fn swr_v1_reactor_delete(OwnedReactor(handle): OwnedReactor) {
    let state = handle.state();
    let cycle_tasks = handle.with(|state| {
        state.fields.destroyed = true;
        crate::event_api::take_cycle_tasks(&mut state.fields)
    });
    // Dropping a task may run its owner's code, which may reach the reactor.
    drop(cycle_tasks);
    crate::deferred::run_destroy(handle);
    handle.with(ReactorState::close_backend);
    reactor_hook(HookType::ON_REACTOR_DESTROY, handle);
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
            crate::deferred::deferred_free(&mut Some(queue));
        }
    }
    // SAFETY: the owner's reactor, `ReactorState::open`'s allocation, which the owner
    // gave up by value: deleted once, and used no more.
    drop(unsafe { Box::from_raw(state) });
}

/// `int swoole_event_init(int flags)`: the thread's `Reactor` with
/// `SW_REACTOR_MAXEVENTS` events, waiting to exit under `SW_EVENTLOOP_WAIT_EXIT`, and the
/// coroutine system's and client's handlers set; `SW_ERR` when no backend opens.
#[cfg(feature = "upstream-symbols")]
#[unsafe(export_name = "_Z17swoole_event_initi")]
#[must_use]
pub extern "C-unwind" fn swoole_event_init(flags: c_int) -> c_int {
    const SW_REACTOR_MAXEVENTS: c_int = 4096;
    const SW_EVENTLOOP_WAIT_EXIT: c_int = 1;
    let reactor = swr_v1_reactor_new(SW_REACTOR_MAXEVENTS, 0);
    let handle = reactor.running();
    if !handle.with(|state| state.fields.ready) {
        swr_v1_reactor_delete(reactor);
        return -1;
    }
    handle.with(|state| state.fields.wait_exit = flags & SW_EVENTLOOP_WAIT_EXIT != 0);
    crate::event_api::init_reactor(handle);
    swoole_rs_corebase::set_thread_reactor(handle.as_ptr());
    // A reactor this replaces stays allocated, as upstream's overwritten pointer did.
    EVENT_REACTOR.set(Some(reactor));
    0
}

#[cfg(feature = "upstream-symbols")]
thread_local! {
    /// The reactor `swoole_event_init` made on this thread, which `swoole_event_free`
    /// deletes.
    static EVENT_REACTOR: std::cell::Cell<Option<OwnedReactor>> = const {
        std::cell::Cell::new(None)
    };
}

/// `swoole_event_free()`: deletes the thread's `Reactor`, then clears it; false
/// without one. Only `swoole_event_init` sets a thread's reactor, so it is the one that
/// call made here; a reactor another thread made and shares (a coroutine thread's) is
/// cleared but left to the thread that owns it.
#[cfg(feature = "upstream-symbols")]
#[expect(
    clippy::must_use_candidate,
    reason = "callers, `swoole_event_wait` among them, ignore it as upstream's do"
)]
pub fn swoole_event_free() -> bool {
    let Some(reactor) = crate::event_api::running() else {
        return false;
    };
    match EVENT_REACTOR.take() {
        Some(owned) if owned.as_ptr() == reactor.as_ptr() => swr_v1_reactor_delete(owned),
        other => EVENT_REACTOR.set(other),
    }
    swoole_rs_corebase::set_thread_reactor(ptr::null_mut());
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
    ) -> Option<(libc::pollfd, crate::Socket<'static>)> {
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
    reactor: crate::Running<'_>,
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
        Backend::None => -1,
        Backend::Poll(snapshot) => {
            let count = snapshot.fill(&owner.registry);
            *entries = count;
            let timeout = if deferred {
                0
            } else {
                owner.fields.timeout_msec
            };
            return swoole_rs_corebase::sys::poll(snapshot.filled(), timeout)
                .map(|ready| ready.unsigned_abs() as usize);
        }
        #[cfg(target_os = "linux")]
        Backend::Epoll(array) => {
            let timeout = if deferred {
                0
            } else {
                owner.fields.timeout_msec
            };
            let epfd = owner.backend_fd;
            // `max_event_num` records, which a caller may lower (to 0: EINVAL), never
            // past the array.
            let limit = array
                .len()
                .min(usize::try_from(owner.fields.max_event_num).unwrap_or(usize::MAX));
            let (array, _) = array.split_at_mut(limit);
            swoole_rs_corebase::lent::lend_out(array, |events, count| {
                wait::epoll_wait(epfd, events, count, timeout)
            })
        }
        #[cfg(any(
            target_os = "macos",
            target_os = "freebsd",
            target_os = "openbsd",
            target_os = "netbsd",
            target_os = "dragonfly"
        ))]
        Backend::Kqueue(array) => {
            let timeout = crate::kqueue_timeout(
                owner.fields.timeout_msec,
                owner.fields.timeout_msec <= 0 && deferred,
            );
            let kq = owner.backend_fd;
            swoole_rs_corebase::lent::lend_out(array, |events, count| {
                wait::kevent(
                    kq,
                    None,
                    wait::NoChanges::Zero,
                    events,
                    count,
                    timeout.as_ref(),
                )
            })
        }
    };
    usize::try_from(ready).map_err(|_| std::io::Error::last_os_error())
}

/// The backends' kernel waits, over an output array lent with its length.
mod wait {
    #[cfg(any(
        target_os = "macos",
        target_os = "freebsd",
        target_os = "openbsd",
        target_os = "netbsd",
        target_os = "dragonfly"
    ))]
    /// No value: a null-only change list.
    pub(super) enum Nothing {}

    /// The length of a null change list.
    #[cfg(any(
        target_os = "macos",
        target_os = "freebsd",
        target_os = "openbsd",
        target_os = "netbsd",
        target_os = "dragonfly"
    ))]
    #[repr(i32)]
    pub(super) enum NoChanges {
        Zero = 0,
    }

    // libc's. Each writes at most the lent count of records to the lent array and reads
    // nothing else but the optional timeout; `kevent` reads no change from its null list.
    // They keep nothing, so they are declared `safe`.
    unsafe extern "C" {
        #[cfg(target_os = "linux")]
        pub(super) safe fn epoll_wait<'id>(
            epfd: libc::c_int,
            events: swoole_rs_corebase::lent::Out<'id, libc::epoll_event>,
            count: swoole_rs_corebase::lent::Count<'id, libc::c_int>,
            timeout: libc::c_int,
        ) -> libc::c_int;
        #[cfg(any(
            target_os = "macos",
            target_os = "freebsd",
            target_os = "openbsd",
            target_os = "netbsd",
            target_os = "dragonfly"
        ))]
        pub(super) safe fn kevent<'id>(
            kq: libc::c_int,
            changes: Option<&'static Nothing>,
            change_count: NoChanges,
            events: swoole_rs_corebase::lent::Out<'id, libc::kevent>,
            count: swoole_rs_corebase::lent::Count<'id, libc::c_int>,
            timeout: Option<&libc::timespec>,
        ) -> libc::c_int;
    }
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
        request: crate::change::Request<'_>,
        events: crate::Events,
    ) -> bool {
        use crate::change::Request;
        let (Request::Add(socket) | Request::Set(socket) | Request::Del(socket)) = request;
        socket.edit(|record| {
            match request {
                Request::Add(added) => {
                    if !self.registry.put(record.fd, added, events) {
                        return false;
                    }
                    record.events = events;
                    record.removed = false;
                }
                Request::Set(_) => {
                    record.events = events;
                    self.registry.set(record.fd, events);
                }
                Request::Del(_) => {
                    record.events = crate::Events::NONE;
                    record.removed = true;
                    self.registry.remove(record.fd);
                }
            }
            true
        })
    }

    pub(crate) fn close_backend(&mut self) {
        if self.backend_fd >= 0 {
            _ = swoole_rs_corebase::sys::close(self.backend_fd);
            self.backend_fd = -1;
        }
        // The wait loop no longer holds pointers to the arrays.
        self.backend = Backend::None;
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::SocketRecord;
    /// A state with backend `kind` open for `count` events: `new Reactor` without its
    /// native effects.
    pub(crate) fn state(kind: c_int, count: c_int) -> ReactorPrepared {
        let reactor = crate::Running::leak(ReactorState::open(kind, count));
        reactor.with(|state| state.head)
    }
    /// [`ReactorState::record`] with the test's socket.
    pub(crate) unsafe fn record(
        owner: *mut ReactorState,
        socket: *mut SocketRecord,
        op: crate::ChangeOp,
        events: crate::Events,
    ) -> bool {
        let socket = unsafe { crate::Socket::from_ptr(socket) }.unwrap();
        unsafe { (*owner).record(crate::change::Request::new(socket, op), events) }
    }
    pub(crate) unsafe fn free(owner: *mut ReactorState) {
        swr_v1_reactor_delete(OwnedReactor(running(owner)));
    }
    /// The test's state as the handle its operations take.
    pub(crate) fn running(owner: *mut ReactorState) -> crate::Running<'static> {
        unsafe { crate::Running::new(owner.cast()) }
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
                crate::ChangeOp::Add,
                crate::Events::from_bits(512)
            ));
            assert_eq!(socket.events, crate::Events::from_bits(512));
            assert!(!socket.removed);
            assert_eq!(
                crate::swr_v1_reactor_registry_count(state.registry.as_ref()),
                1
            );
            record(
                state.owner,
                &raw mut socket,
                crate::ChangeOp::Set,
                crate::Events::from_bits(1024),
            );
            assert_eq!(socket.events, crate::Events::from_bits(1024));
            record(
                state.owner,
                &raw mut socket,
                crate::ChangeOp::Del,
                crate::Events::NONE,
            );
            assert_eq!(socket.events, crate::Events::NONE);
            assert!(socket.removed);
            assert_eq!(
                crate::swr_v1_reactor_registry_count(state.registry.as_ref()),
                0
            );
            before_wait(&mut (*state.owner).fields);
            assert!((*fields).running && (*fields).start);
            assert_eq!((*fields).timeout_msec, -1);
            free(state.owner);
        }
    }
    #[test]
    #[cfg_attr(miri, ignore = "Miri can't spawn a process")]
    fn actual_backend_waits_read_pipe_and_close_ownership_before_destroy_hook() {
        if crate::isolated_process::run_in_child(
            "reactor_state::tests::actual_backend_waits_read_pipe_and_close_ownership_before_destroy_hook",
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
                    crate::ChangeOp::Add,
                    crate::Events::from_bits(512)
                ));
                #[cfg(target_os = "linux")]
                if kind == 1 {
                    assert_eq!(
                        crate::registration::epoll_ctl(
                            descriptor,
                            crate::ChangeOp::Add,
                            fds[0],
                            ptr::from_mut(&mut socket).cast(),
                            crate::Events::from_bits(512)
                        )
                        .ok(),
                        Some(())
                    );
                }
                #[cfg(not(target_os = "linux"))]
                if kind == 2 {
                    assert_eq!(
                        crate::registration::kqueue_add(
                            descriptor,
                            fds[0],
                            ptr::from_mut(&mut socket).cast(),
                            crate::Events::from_bits(512)
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
        if crate::isolated_process::run_in_child(
            "reactor_state::tests::owner_drop_preserves_a_reopened_backend_descriptor_number",
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
            crate::isolated_process::replacement_survives(fd, || {
                free(state.owner);
            });
        }
    }
}
