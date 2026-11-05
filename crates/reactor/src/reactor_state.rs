//! Reactor public state and backend resources share one Rust lifetime. Native
//! reference fields are borrowed slots, never owning copies or callback proxies.
use crate::{SocketRecord, handlers::Handlers, registry::Registry};
use libc::{c_int, c_void};
use std::ptr;

pub type ReactorWrite =
    unsafe extern "C" fn(*mut c_void, *mut SocketRecord, *const c_void, usize) -> isize;
pub type ReactorWritev =
    unsafe extern "C" fn(*mut c_void, *mut SocketRecord, *const libc::iovec, usize) -> isize;
pub type ReactorClose = unsafe extern "C" fn(*mut c_void, *mut SocketRecord) -> c_int;

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
}

pub struct ReactorState {
    pub(crate) fields: ReactorFields,
    pub(crate) registry: Registry,
    pub(crate) handlers: Handlers,
    pub(crate) backend: Backend,
    pub(crate) backend_fd: c_int,
    pub(crate) backend_limit: c_int,
}

pub(crate) enum Backend {
    None,
    Poll(*mut crate::PollSnapshot),
    #[cfg(target_os = "linux")]
    Epoll(*mut crate::EpollEvents),
    #[cfg(any(
        target_os = "macos",
        target_os = "freebsd",
        target_os = "openbsd",
        target_os = "netbsd",
        target_os = "dragonfly"
    ))]
    Kqueue(*mut crate::KqueueEvents, c_int),
}
impl Drop for ReactorState {
    fn drop(&mut self) {
        self.close_backend();
    }
}
#[repr(C)]
pub struct ReactorPrepared {
    pub owner: *mut ReactorState,
    pub fields: *mut ReactorFields,
    pub registry: *mut Registry,
    pub handlers: *mut Handlers,
}
#[unsafe(no_mangle)]
pub extern "C" fn swr_v1_reactor_state_new() -> ReactorPrepared {
    let Some(mut state) = crate::owner::try_owner(|| ReactorState {
        fields: ReactorFields {
            kind: 0,
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
        },
        registry: Registry::default(),
        handlers: Handlers::default(),
        backend: Backend::None,
        backend_fd: -1,
        backend_limit: 0,
    }) else {
        return ReactorPrepared {
            owner: ptr::null_mut(),
            fields: ptr::null_mut(),
            registry: ptr::null_mut(),
            handlers: ptr::null_mut(),
        };
    };
    ReactorPrepared {
        fields: &raw mut state.fields,
        registry: &raw mut state.registry,
        handlers: &raw mut state.handlers,
        owner: Box::into_raw(state),
    }
}
/// # Safety
/// owner is null or the unique allocation returned by `state_new`; views have ended.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_reactor_state_free(owner: *mut ReactorState) {
    // SAFETY: forwards unique ownership.
    unsafe { crate::owner::release(owner) };
}

/// Open the selected backend (`Reactor::Type` values). 0 ready,1 kernel failure,
/// 2 allocation refusal,3 negative array bound; errno is the kernel's on failure.
/// # Safety
/// owner is live/exclusive, with no backend installed yet.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_reactor_state_open(
    owner: *mut ReactorState,
    kind: c_int,
    count: c_int,
) -> c_int {
    // SAFETY: exclusive initialization, with no native callback.
    let owner = unsafe { &mut *owner };
    owner.fields.kind = kind;
    #[cfg(target_os = "linux")]
    if kind == 1 {
        // SAFETY: the fixed hint matches the original epoll_create argument.
        owner.backend_fd = unsafe { libc::epoll_create(512) };
    }
    #[cfg(any(
        target_os = "macos",
        target_os = "freebsd",
        target_os = "openbsd",
        target_os = "netbsd",
        target_os = "dragonfly"
    ))]
    if kind == 2 {
        // SAFETY: kqueue creates a new descriptor owned by this state.
        owner.backend_fd = unsafe { libc::kqueue() };
    }
    if kind != 3 && owner.backend_fd < 0 {
        return 1;
    }
    let Ok(length) = usize::try_from(count) else {
        return 3;
    };
    owner.backend = match kind {
        #[cfg(target_os = "linux")]
        1 => {
            let array = crate::swr_v1_reactor_epoll_new(length);
            if array.is_null() {
                return 2;
            }
            Backend::Epoll(array)
        }
        #[cfg(any(
            target_os = "macos",
            target_os = "freebsd",
            target_os = "openbsd",
            target_os = "netbsd",
            target_os = "dragonfly"
        ))]
        2 => {
            let array = crate::swr_v1_reactor_kqueue_new(length);
            if array.is_null() {
                return 2;
            }
            Backend::Kqueue(array, count)
        }
        _ => {
            let array = crate::swr_v1_reactor_poll_new(length);
            if array.is_null() {
                return 2;
            }
            Backend::Poll(array)
        }
    };
    owner.backend_limit = count;
    owner.fields.native_handle = owner.backend_fd;
    owner.fields.max_event_num = count.unsigned_abs();
    owner.fields.ready = true;
    0
}
#[repr(C)]
pub struct ReactorBackendView {
    pub events: *mut c_void,
    pub sockets: *mut *mut c_void,
    pub backend: u32,
}
/// # Safety
/// owner is live; borrowed arrays remain owned until `state_free`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_reactor_state_backend(
    owner: *mut ReactorState,
) -> ReactorBackendView {
    // SAFETY: the state is live; no reference crosses a callback.
    let backend = unsafe { &(*owner).backend };
    let mut view = ReactorBackendView {
        events: ptr::null_mut(),
        sockets: ptr::null_mut(),
        backend: 0,
    };
    match *backend {
        Backend::None => (),
        Backend::Poll(array) => {
            // SAFETY: this state's live snapshot.
            view.events = unsafe { crate::swr_v1_reactor_poll_data(array) }.cast();
            // SAFETY: the same snapshot owns the socket array.
            view.sockets = unsafe { crate::swr_v1_reactor_poll_sockets(array) };
        }
        #[cfg(target_os = "linux")]
        Backend::Epoll(array) => {
            // SAFETY: the live epoll output array.
            view.events = unsafe { crate::swr_v1_reactor_epoll_data(array) }.cast();
            view.backend = 1;
        }
        #[cfg(any(
            target_os = "macos",
            target_os = "freebsd",
            target_os = "openbsd",
            target_os = "netbsd",
            target_os = "dragonfly"
        ))]
        Backend::Kqueue(array, _) => {
            // SAFETY: the live kevent output array.
            view.events = unsafe { crate::swr_v1_reactor_kqueue_data(array) }.cast();
            view.backend = 2;
        }
    }
    view
}
/// Actual kernel wait over the owner's stable arrays. No native callbacks run.
/// # Safety
/// owner is live/exclusive; entries is writable for one int.
pub(crate) unsafe fn kernel_wait(
    owner: *mut ReactorState,
    deferred: bool,
    entries: *mut c_int,
) -> c_int {
    // SAFETY: the kernel operation exclusively borrows state, without native callbacks.
    let owner = unsafe { &mut *owner };
    match owner.backend {
        Backend::None => -1,
        Backend::Poll(array) => {
            // SAFETY: state's live snapshot and registry.
            let count =
                unsafe { crate::swr_v1_reactor_poll_fill(array, &raw const owner.registry) };
            // SAFETY: the caller lends an entry-count slot.
            unsafe { entries.write(count) };
            // SAFETY: the array has capacity for the bounded registry snapshot.
            let data = unsafe { crate::swr_v1_reactor_poll_data(array) };
            // SAFETY: poll initializes revents in the first count valid records.
            unsafe {
                libc::poll(
                    data,
                    libc::nfds_t::from(count.unsigned_abs()),
                    if deferred {
                        0
                    } else {
                        owner.fields.timeout_msec
                    },
                )
            }
        }
        #[cfg(target_os = "linux")]
        Backend::Epoll(array) => {
            // SAFETY: the live array has max_event_num writable records.
            let data = unsafe { crate::swr_v1_reactor_epoll_data(array) };
            // SAFETY: a valid owned epoll fd and its writable output array.
            unsafe {
                libc::epoll_wait(
                    owner.backend_fd,
                    data,
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
            let timeout = crate::swr_v1_reactor_kqueue_timeout(
                owner.fields.timeout_msec,
                u32::from(owner.fields.timeout_msec <= 0 && deferred),
            );
            let time = libc::timespec {
                tv_sec: timeout.seconds,
                tv_nsec: timeout.nanoseconds,
            };
            // SAFETY: the live array has max_event_num writable records.
            let data = unsafe { crate::swr_v1_reactor_kqueue_data(array) };
            // SAFETY: valid owned kqueue, output array and optional stack timeout.
            unsafe {
                libc::kevent(
                    owner.backend_fd,
                    ptr::null(),
                    0,
                    data,
                    count,
                    if timeout.finite != 0 {
                        &raw const time
                    } else {
                        ptr::null()
                    },
                )
            }
        }
    }
}
/// The wait loop's initial state transition, over the actual borrowed slots.
/// # Safety
/// owner is live/exclusive and no native callback is active.
pub(crate) unsafe fn before_wait(owner: *mut ReactorState) {
    // SAFETY: the caller lends the state exclusively.
    let fields = unsafe { &mut (*owner).fields };
    fields.start = true;
    fields.running = true;
    if fields.timeout_msec == 0 {
        fields.timeout_msec = -1;
    }
}

/// Close backend resources at the native impl destruction point, before destroy hooks.
/// # Safety
/// owner is live/exclusive; backend arrays have no active wait or consumer.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_reactor_state_close_backend(owner: *mut ReactorState) {
    // SAFETY: exclusive state, no callback or kernel wait is active.
    let owner = unsafe { &mut *owner };
    owner.close_backend();
}

/// Private backend descriptor, independent of the public compatibility slot.
/// # Safety
/// owner is a live state.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_reactor_state_descriptor(owner: *const ReactorState) -> c_int {
    // SAFETY: immutable observation of a live state.
    unsafe { (*owner).backend_fd }
}
/// Keep registry and socket flags in one operation. op: add=0,set=1,delete=2.
/// Returns -2 only when add cannot allocate, without changing the socket flags.
/// # Safety
/// State and socket are distinct live/exclusive records; no native effect runs.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_reactor_state_socket(
    owner: *mut ReactorState,
    socket: *mut SocketRecord,
    op: u32,
    events: c_int,
) -> c_int {
    // SAFETY: live exclusively lent records, not accessed by any callback.
    let state = unsafe { &mut *owner };
    // SAFETY: socket is distinct from the state and exclusively lent.
    state.record(unsafe { &mut *socket }, op, events)
}

impl ReactorState {
    /// `Reactor::_add` (0), `_set` (1) and `_del` (2): the registry and the socket's
    /// flags together. -2 when add cannot allocate, leaving the flags alone.
    pub(crate) fn record(&mut self, socket: &mut SocketRecord, op: u32, events: c_int) -> c_int {
        match op {
            0 => {
                // SAFETY: the registry is exclusively lent; the token stays live while registered.
                let result = unsafe {
                    crate::swr_v1_reactor_registry_put(
                        &raw mut self.registry,
                        socket.fd,
                        ptr::from_mut(socket).cast(),
                        events,
                    )
                };
                if result != 0 {
                    return result;
                }
                socket.events = events;
                socket.removed = 0;
            }
            1 => {
                socket.events = events;
                // SAFETY: the registry is exclusively lent.
                unsafe {
                    crate::swr_v1_reactor_registry_set(&raw mut self.registry, socket.fd, events);
                };
            }
            _ => {
                socket.events = 0;
                socket.removed = 1;
                // SAFETY: the registry is exclusively lent.
                unsafe { crate::swr_v1_reactor_registry_remove(&raw mut self.registry, socket.fd) };
            }
        }
        0
    }

    fn close_backend(&mut self) {
        if self.backend_fd >= 0 {
            // SAFETY: this state owns the descriptor and invalidates ownership immediately.
            unsafe { libc::close(self.backend_fd) };
            self.backend_fd = -1;
        }
        let backend = std::mem::replace(&mut self.backend, Backend::None);
        match backend {
            Backend::None => (),
            Backend::Poll(array) => {
                // SAFETY: detached uniquely owned snapshot; all views have ended.
                unsafe { crate::swr_v1_reactor_poll_free(array) };
            }
            #[cfg(target_os = "linux")]
            Backend::Epoll(array) => {
                // SAFETY: detached uniquely owned output array; all views have ended.
                unsafe { crate::swr_v1_reactor_epoll_free(array) };
            }
            #[cfg(any(
                target_os = "macos",
                target_os = "freebsd",
                target_os = "openbsd",
                target_os = "netbsd",
                target_os = "dragonfly"
            ))]
            Backend::Kqueue(array, _) => {
                // SAFETY: detached uniquely owned output array; all views have ended.
                unsafe { crate::swr_v1_reactor_kqueue_free(array) };
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn owner_keeps_stable_public_slots_and_socket_registry() {
        unsafe {
            let state = swr_v1_reactor_state_new();
            assert!(!state.owner.is_null());
            let fields = state.fields;
            assert_eq!((*fields).native_handle, -1);
            assert_eq!(swr_v1_reactor_state_open(state.owner, 3, 4), 0);
            assert_eq!(state.fields, fields);
            assert_eq!((*state.owner).backend_limit, 4);
            let mut socket = SocketRecord::with_fd(123, 0);
            assert_eq!(
                swr_v1_reactor_state_socket(state.owner, &raw mut socket, 0, 512),
                0
            );
            assert_eq!(socket.events, 512);
            assert_eq!(socket.removed, 0);
            assert_eq!(crate::swr_v1_reactor_registry_count(state.registry), 1);
            swr_v1_reactor_state_socket(state.owner, &raw mut socket, 1, 1024);
            assert_eq!(socket.events, 1024);
            swr_v1_reactor_state_socket(state.owner, &raw mut socket, 2, 0);
            assert_eq!(socket.events, 0);
            assert_eq!(socket.removed, 1);
            assert_eq!(crate::swr_v1_reactor_registry_count(state.registry), 0);
            before_wait(state.owner);
            assert!((*fields).running && (*fields).start);
            assert_eq!((*fields).timeout_msec, -1);
            swr_v1_reactor_state_free(state.owner);
        }
    }
    #[test]
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
                let state = swr_v1_reactor_state_new();
                assert_eq!(swr_v1_reactor_state_open(state.owner, kind, 4), 0);
                let descriptor = swr_v1_reactor_state_descriptor(state.owner);
                let mut fds = [0; 2];
                assert_eq!(libc::pipe(fds.as_mut_ptr()), 0);
                let mut socket = SocketRecord::with_fd(fds[0], 0);
                assert_eq!(
                    swr_v1_reactor_state_socket(state.owner, &raw mut socket, 0, 512),
                    0
                );
                #[cfg(target_os = "linux")]
                if kind == 1 {
                    assert_eq!(
                        crate::registration::epoll_ctl(
                            descriptor,
                            0,
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
                        0
                    );
                }
                assert_eq!(libc::write(fds[1], b"a".as_ptr().cast(), 1), 1);
                (*state.fields).timeout_msec = 0;
                let view = swr_v1_reactor_state_backend(state.owner);
                let mut entries = 0;
                assert_eq!(kernel_wait(state.owner, false, &raw mut entries), 1);
                assert_eq!(
                    view.events,
                    swr_v1_reactor_state_backend(state.owner).events
                );
                if kind == 3 {
                    assert_eq!(entries, 1);
                }
                // Public handle assignment never transfers or changes private ownership.
                (*state.fields).native_handle = -1;
                assert_eq!(swr_v1_reactor_state_descriptor(state.owner), descriptor);
                swr_v1_reactor_state_close_backend(state.owner);
                if descriptor >= 0 {
                    assert_eq!(libc::fcntl(descriptor, libc::F_GETFD), -1);
                }
                assert_eq!((*state.fields).native_handle, -1);
                swr_v1_reactor_state_free(state.owner);
                libc::close(fds[0]);
                libc::close(fds[1]);
            }
        }
    }
    #[test]
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
            let state = swr_v1_reactor_state_new();
            assert_eq!(swr_v1_reactor_state_open(state.owner, kind, 4), 0);
            let fd = swr_v1_reactor_state_descriptor(state.owner);
            swr_v1_reactor_state_close_backend(state.owner);
            assert_eq!(swr_v1_reactor_state_descriptor(state.owner), -1);
            assert_eq!((*state.fields).native_handle, fd);
            crate::isolated_process::replacement_survives(fd, || {
                swr_v1_reactor_state_free(state.owner);
            });
        }
    }
}
