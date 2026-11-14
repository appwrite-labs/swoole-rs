// SPDX-License-Identifier: Apache-2.0
// Compatibility derived from Swoole v6.2.4 src/wrapper/event.cc and src/reactor/base.cc
// (Swoole contributors).
//! The `Reactor` object: its public fields and backend resources share one Rust
//! lifetime, from its constructor and `swoole_event_init` to its destructor and
//! `swoole_event_free`. The C++ class is methods over this state (`swoole_reactor.h`).
use crate::wait_loop::BackendKind;
use crate::{PollSnapshot, Socket, SocketRecord, handlers::Handlers, registry::Registry};
use libc::{c_int, c_void};
use std::mem::MaybeUninit;
use std::ptr;

pub type ReactorWrite =
    unsafe extern "C-unwind" fn(*mut c_void, *mut SocketRecord, *const c_void, usize) -> isize;
pub type ReactorWritev =
    unsafe extern "C-unwind" fn(*mut c_void, *mut SocketRecord, *const libc::iovec, usize) -> isize;
pub type ReactorClose = unsafe extern "C-unwind" fn(*mut c_void, *mut SocketRecord) -> c_int;

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
    pub write: Option<ReactorWrite>,
    pub writev: Option<ReactorWritev>,
    pub close: Option<ReactorClose>,
    /// `Reactor::defer_tasks`: null (`None`) until a task is deferred.
    pub defer_tasks: Option<Box<crate::ReactorDeferred>>,
    /// `Reactor::destroy_callbacks`.
    pub destroy_callbacks: Option<Box<crate::ReactorDeferred>>,
    pub idle_task: ReactorTask,
    pub future_task: ReactorTask,
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
    /// The `defer_tasks` queues the defer end callback detached and is running, innermost
    /// last: one a task bailed out of stays here for the drop to free.
    #[expect(
        clippy::vec_box,
        reason = "a running queue's address stays put while the list grows, and it moves here from its slot uncopied"
    )]
    pub(crate) running_defer: Vec<Box<crate::ReactorDeferred>>,
}
// swoole_rs_reactor.h asserts the same layout for swr_v1_reactor_head.
const _: () = assert!(
    std::mem::offset_of!(ReactorState, fields) == size_of::<ReactorPrepared>(),
    "Reactor's fields follow its prepared views"
);
#[cfg(target_pointer_width = "64")]
const _: () = assert!(
    size_of::<ReactorPrepared>() == 32 && size_of::<ReactorFields>() == 128,
    "swr_v1_reactor_head's layout"
);

/// The backend's output arrays, allocated once, so the wait loop's pointers to them
/// stay valid until the backend closes.
pub(crate) enum Backend {
    None,
    Poll(PollSnapshot),
    #[cfg(target_os = "linux")]
    Epoll(Vec<MaybeUninit<libc::epoll_event>>),
    #[cfg(any(
        target_os = "macos",
        target_os = "freebsd",
        target_os = "openbsd",
        target_os = "netbsd",
        target_os = "dragonfly"
    ))]
    Kqueue(Vec<MaybeUninit<libc::kevent>>, c_int),
}
impl Drop for ReactorState {
    fn drop(&mut self) {
        self.close_backend();
    }
}

impl ReactorState {
    /// Takes the queues [`swr_v1_reactor_delete`] still has to free out of the state.
    #[expect(clippy::vec_box, reason = "the queues as their slots own them")]
    fn detach_queues(&mut self) -> Vec<Box<crate::ReactorDeferred>> {
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
    /// The head is set by [`ReactorState::into_raw`], once the caller is done with the
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
                write: None,
                writev: None,
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

    /// The state as the pointer its head and callers keep, the head set. Nothing goes
    /// through `owner` afterwards: a write or reborrow through it would end them.
    pub(crate) fn into_raw(owner: Box<Self>) -> *mut Self {
        let owner = Box::leak(owner);
        let state = &raw mut *owner;
        let head = ReactorPrepared {
            owner: state,
            fields: &raw mut owner.fields,
            registry: &raw mut owner.registry,
            handlers: &raw mut owner.handlers,
        };
        // SAFETY: `state` is the live allocation; a raw write leaves the head's pointers.
        unsafe { (*state).head = head };
        state
    }

    /// Opens backend `kind` for `count` events: false when the kernel refuses its
    /// descriptor, errno being the kernel's.
    fn open_backend(&mut self, kind: BackendKind, count: c_int) -> bool {
        #[cfg(target_os = "linux")]
        if kind == BackendKind::Epoll {
            // Upstream's fixed size hint.
            self.backend_fd = swoole_rs_corebase::sys::epoll_create(512);
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
            BackendKind::Kqueue => Backend::Kqueue(output(length), count),
            _ => Backend::Poll(PollSnapshot::new(length)),
        };
        self.backend_limit = count;
        self.fields.native_handle = self.backend_fd;
        self.fields.max_event_num = count.unsigned_abs();
        self.fields.ready = true;
        true
    }
}

/// `count` records for the kernel to fill, aborting as upstream's `bad_alloc` when
/// they could not be allocated.
fn output<T>(count: usize) -> Vec<MaybeUninit<T>> {
    let mut entries = Vec::new();
    if entries.try_reserve_exact(count).is_err() {
        std::alloc::handle_alloc_error(std::alloc::Layout::new::<T>());
    }
    entries.resize_with(count, MaybeUninit::uninit);
    entries
}

/// `SW_GLOBAL_HOOK_ON_REACTOR_CREATE` and `SW_GLOBAL_HOOK_ON_REACTOR_DESTROY`.
const HOOK_ON_REACTOR_CREATE: c_int = 5;
const HOOK_ON_REACTOR_DESTROY: c_int = 9;

#[cfg(feature = "upstream-symbols")]
// crates/tls's `Reactor` statics and crates/hooks's `System::init_reactor`,
// under their upstream symbols; declared, since both depend on this crate.
unsafe extern "C-unwind" {
    #[link_name = "_ZN6swoole7Reactor6_writeEPS0_PNS_7network6SocketEPKvm"]
    fn reactor_write(
        reactor: *mut c_void,
        socket: *mut SocketRecord,
        data: *const c_void,
        length: usize,
    ) -> isize;
    #[link_name = "_ZN6swoole7Reactor7_writevEPS0_PNS_7network6SocketEPK5iovecm"]
    fn reactor_writev(
        reactor: *mut c_void,
        socket: *mut SocketRecord,
        iov: *const libc::iovec,
        count: usize,
    ) -> isize;
    #[link_name = "_ZN6swoole7Reactor6_closeEPS0_PNS_7network6SocketE"]
    fn reactor_close(reactor: *mut c_void, socket: *mut SocketRecord) -> c_int;
    #[link_name = "_ZN6swoole9coroutine6System12init_reactorEPNS_7ReactorE"]
    safe fn system_init_reactor(reactor: crate::Running<'_>);
}
#[cfg(feature = "upstream-symbols")]
// crates/tls's, under its upstream symbol, and crates/clients's; declared,
// since both depend on this crate. Each takes the live reactor's handle, and the
// writable callback is a reactor handler, sound for any event the reactor lends it.
unsafe extern "C" {
    safe fn swr_v1_clients_init_reactor(reactor: crate::Running<'_>);
    #[link_name = "_ZN6swoole7Reactor18_writable_callbackEPS0_PNS_5EventE"]
    safe fn reactor_writable_callback(
        reactor: crate::Running<'_>,
        event: &mut crate::Event,
    ) -> c_int;
}

/// `new Reactor(max_event, type)`: the state `ReactorState::open` makes and, once
/// ready, upstream's write, writev, close and default write handler, the create hook
/// and then the built-in end callbacks and exit conditions. The write functions are
/// crates/tls's, so only the hybrid (`upstream-symbols`) sets them.
#[unsafe(no_mangle)]
pub extern "C-unwind" fn swr_v1_reactor_new(max_event: c_int, kind: c_int) -> *mut c_void {
    let mut owner = ReactorState::open(kind, max_event);
    if !owner.fields.ready {
        return ReactorState::into_raw(owner).cast();
    }
    owner.fields.timeout_msec = -1;
    #[cfg(feature = "upstream-symbols")]
    {
        owner.fields.write = Some(reactor_write);
        owner.fields.writev = Some(reactor_writev);
        owner.fields.close = Some(reactor_close);
        owner.handlers.defaults[0].set(Some(reactor_writable_callback));
    }
    let state = ReactorState::into_raw(owner);
    if swoole_rs_corebase::swoole_isset_hook(HOOK_ON_REACTOR_CREATE) {
        // SAFETY: hooks receive the live `Reactor`.
        unsafe { swoole_rs_corebase::swoole_call_hook(HOOK_ON_REACTOR_CREATE, state.cast()) };
    }
    // SAFETY: the live state; the hook has returned.
    crate::callbacks::install_builtins(unsafe { &mut (*state).callbacks });
    state.cast()
}

/// `delete reactor`: marks it destroyed, runs its destroy callbacks, closes its
/// backend, calls the destroy hook and frees it.
///
/// # Safety
/// `reactor` is from [`swr_v1_reactor_new`] and nothing uses it afterwards.
#[unsafe(no_mangle)]
pub unsafe extern "C-unwind" fn swr_v1_reactor_delete(reactor: *mut c_void) {
    let state = reactor.cast::<ReactorState>();
    // SAFETY: as the caller promises: the live reactor, on its thread, which no
    // reference holds. Each borrow below ends before a callback, hook or disposer runs.
    let handle = unsafe { crate::Running::new(reactor) };
    let cycle_tasks = handle.with(|state| {
        state.fields.destroyed = true;
        crate::event_api::take_cycle_tasks(&mut state.fields)
    });
    // Dropping a task may run its owner's code, which may reach the reactor.
    drop(cycle_tasks);
    crate::deferred::run_destroy(handle);
    handle.with(ReactorState::close_backend);
    if swoole_rs_corebase::swoole_isset_hook(HOOK_ON_REACTOR_DESTROY) {
        // SAFETY: hooks receive the live `Reactor`.
        unsafe { swoole_rs_corebase::swoole_call_hook(HOOK_ON_REACTOR_DESTROY, reactor) };
    }
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
    // SAFETY: as the caller promises: `ReactorState::open`'s allocation, used no more.
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
    // SAFETY: the new reactor, on this thread, which no reference holds.
    let handle = unsafe { crate::Running::new(reactor) };
    if !handle.with(|state| state.fields.ready) {
        // SAFETY: the reactor just made, which nothing else holds or uses again.
        unsafe { swr_v1_reactor_delete(reactor) };
        return -1;
    }
    handle.with(|state| state.fields.wait_exit = flags & SW_EVENTLOOP_WAIT_EXIT != 0);
    system_init_reactor(handle);
    swr_v1_clients_init_reactor(handle);
    swoole_rs_corebase::set_thread_reactor(reactor);
    0
}

/// `int swoole_event_free()`: deletes the thread's `Reactor`, then clears it; `SW_ERR`
/// without one.
#[cfg(feature = "upstream-symbols")]
#[unsafe(export_name = "_Z17swoole_event_freev")]
#[expect(
    clippy::must_use_candidate,
    reason = "callers, `swoole_event_wait` among them, ignore it as upstream's do"
)]
pub extern "C-unwind" fn swoole_event_free() -> c_int {
    let reactor = swoole_rs_corebase::thread_reactor();
    if reactor.is_null() {
        return -1;
    }
    // SAFETY: the thread's reactor, which `swoole_event_init` made; it is cleared below.
    unsafe { swr_v1_reactor_delete(reactor) };
    swoole_rs_corebase::set_thread_reactor(ptr::null_mut());
    0
}
pub(crate) struct ReactorBackendView {
    pub(crate) events: *mut c_void,
    pub(crate) sockets: *mut *mut c_void,
    pub(crate) backend: BackendKind,
}
impl ReactorState {
    /// The backend's arrays, which stay owned until the state is freed.
    pub(crate) fn backend_view(&mut self) -> ReactorBackendView {
        let mut view = ReactorBackendView {
            events: ptr::null_mut(),
            sockets: ptr::null_mut(),
            backend: BackendKind::Poll,
        };
        match &mut self.backend {
            Backend::None => (),
            Backend::Poll(snapshot) => {
                view.events = snapshot.entries().cast();
                view.sockets = snapshot.sockets();
            }
            #[cfg(target_os = "linux")]
            Backend::Epoll(array) => {
                view.events = array.as_mut_ptr().cast();
                view.backend = BackendKind::Epoll;
            }
            #[cfg(any(
                target_os = "macos",
                target_os = "freebsd",
                target_os = "openbsd",
                target_os = "netbsd",
                target_os = "dragonfly"
            ))]
            Backend::Kqueue(array, _) => {
                view.events = array.as_mut_ptr().cast();
                view.backend = BackendKind::Kqueue;
            }
        }
        view
    }
}
/// Actual kernel wait over the owner's stable arrays. No native callbacks run.
// Its one caller, the wait loop, runs it once per iteration (#1435).
#[inline(always)]
pub(crate) fn kernel_wait(
    reactor: crate::Running<'_>,
    deferred: bool,
    entries: &mut c_int,
) -> c_int {
    // SAFETY: the handle's live state, which no reference holds; the kernel wait runs
    // no native callback, so this borrow is the only one.
    let owner = unsafe { &mut *reactor.state() };
    match &mut owner.backend {
        Backend::None => -1,
        Backend::Poll(snapshot) => {
            let count = snapshot.fill(&owner.registry);
            *entries = count;
            let timeout = if deferred {
                0
            } else {
                owner.fields.timeout_msec
            };
            swoole_rs_corebase::sys::poll(snapshot.filled(), timeout)
        }
        #[cfg(target_os = "linux")]
        Backend::Epoll(array) => {
            // SAFETY: a valid owned epoll fd and its writable output array of
            // max_event_num records.
            unsafe {
                libc::epoll_wait(
                    owner.backend_fd,
                    array.as_mut_ptr().cast(),
                    c_int::try_from(owner.fields.max_event_num).unwrap_or(c_int::MAX),
                    if deferred {
                        0
                    } else {
                        owner.fields.timeout_msec
                    },
                )
            }
        }
        #[cfg(any(
            target_os = "macos",
            target_os = "freebsd",
            target_os = "openbsd",
            target_os = "netbsd",
            target_os = "dragonfly"
        ))]
        Backend::Kqueue(array, count) => {
            let timeout = crate::kqueue_timeout(
                owner.fields.timeout_msec,
                owner.fields.timeout_msec <= 0 && deferred,
            );
            // SAFETY: valid owned kqueue, output array of `count` records and optional
            // stack timeout.
            unsafe {
                libc::kevent(
                    owner.backend_fd,
                    ptr::null(),
                    0,
                    array.as_mut_ptr().cast(),
                    *count,
                    timeout.as_ref().map_or(ptr::null(), ptr::from_ref),
                )
            }
        }
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
        socket: Socket<'_>,
        op: crate::ChangeOp,
        events: c_int,
    ) -> bool {
        socket.edit(|record| {
            match op {
                crate::ChangeOp::Add => {
                    if !self.registry.put(record.fd, socket.as_ptr().cast(), events) {
                        return false;
                    }
                    record.events = events;
                    record.removed = false;
                }
                crate::ChangeOp::Set => {
                    record.events = events;
                    self.registry.set(record.fd, events);
                }
                crate::ChangeOp::Del => {
                    record.events = 0;
                    record.removed = true;
                    self.registry.remove(record.fd);
                }
            }
            true
        })
    }

    pub(crate) fn close_backend(&mut self) {
        if self.backend_fd >= 0 {
            swoole_rs_corebase::sys::close(self.backend_fd);
            self.backend_fd = -1;
        }
        // The wait loop no longer holds pointers to the arrays.
        self.backend = Backend::None;
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    /// A state with backend `kind` open for `count` events: `new Reactor` without its
    /// native effects.
    pub(crate) fn state(kind: c_int, count: c_int) -> ReactorPrepared {
        unsafe { (*ReactorState::into_raw(ReactorState::open(kind, count))).head }
    }
    /// [`ReactorState::record`] with the test's socket.
    pub(crate) unsafe fn record(
        owner: *mut ReactorState,
        socket: *mut SocketRecord,
        op: crate::ChangeOp,
        events: c_int,
    ) -> bool {
        unsafe { (*owner).record(Socket::from_ptr(socket).unwrap(), op, events) }
    }
    pub(crate) unsafe fn free(owner: *mut ReactorState) {
        unsafe { swr_v1_reactor_delete(owner.cast()) };
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
                512
            ));
            assert_eq!(socket.events, 512);
            assert!(!socket.removed);
            assert_eq!(
                crate::swr_v1_reactor_registry_count(state.registry.as_ref()),
                1
            );
            record(state.owner, &raw mut socket, crate::ChangeOp::Set, 1024);
            assert_eq!(socket.events, 1024);
            record(state.owner, &raw mut socket, crate::ChangeOp::Del, 0);
            assert_eq!(socket.events, 0);
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
                    512
                ));
                #[cfg(target_os = "linux")]
                if kind == 1 {
                    assert_eq!(
                        crate::registration::epoll_ctl(
                            descriptor,
                            crate::ChangeOp::Add,
                            fds[0],
                            ptr::from_mut(&mut socket).cast(),
                            512
                        ),
                        0
                    );
                }
                #[cfg(not(target_os = "linux"))]
                if kind == 2 {
                    assert_eq!(
                        crate::registration::kqueue_add(
                            descriptor,
                            fds[0],
                            ptr::from_mut(&mut socket).cast(),
                            512
                        ),
                        Ok(())
                    );
                }
                assert_eq!(libc::write(fds[1], b"a".as_ptr().cast(), 1), 1);
                (*state.fields).timeout_msec = 0;
                let view = (*state.owner).backend_view();
                let mut entries = 0;
                assert_eq!(kernel_wait(running(state.owner), false, &mut entries), 1);
                assert_eq!(view.events, (*state.owner).backend_view().events);
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
