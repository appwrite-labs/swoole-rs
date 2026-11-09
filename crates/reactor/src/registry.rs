//! The reactor's socket registry (upstream `Reactor::sockets_`): each registered fd's
//! native `Socket` and the events it was registered with. Native keeps writing the
//! `Socket`'s own `events` and `removed` fields, which other modules read. Fds are
//! small and dense, so the registry is a flat table indexed by fd.
use crate::owner::{release, try_owner};
use std::ffi::{c_int, c_void};
use std::ptr;
use swoole_rs_memory::flat::Table;

type Sockets = Table<(*mut c_void, c_int)>;

#[derive(Default)]
pub struct Registry {
    sockets: Sockets,
}

impl Registry {
    /// How many fds are registered.
    pub(crate) fn count(&self) -> usize {
        self.sockets.len()
    }
}

/// The table slot for `fd`; none for a negative fd, which is never registered.
fn slot(fd: c_int) -> Option<usize> {
    usize::try_from(fd).ok()
}

/// # Safety
/// `registry` is null (empty) or a live registry no one else is using.
unsafe fn sockets<'a>(registry: *const Registry) -> Option<&'a Sockets> {
    // SAFETY: as the caller promises.
    unsafe { registry.as_ref() }.map(|registry| &registry.sockets)
}

/// A new empty registry, or null if allocation fails.
#[unsafe(no_mangle)]
pub extern "C" fn swr_v1_reactor_registry_new() -> *mut Registry {
    try_owner(Registry::default).map_or(ptr::null_mut(), Box::into_raw)
}

/// # Safety
/// `registry` is null or a live registry from `new`, not used again.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_reactor_registry_free(registry: *mut Registry) {
    // SAFETY: as the caller promises.
    unsafe { release(registry) };
}

/// `Reactor::_add`/`_set`: maps `fd` to `socket` registered for `events`; 0, or -2 if
/// allocation fails or `fd` is negative (the registry is then unchanged).
///
/// # Safety
/// `registry` is live and exclusively lent for this call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_reactor_registry_put(
    registry: *mut Registry,
    fd: c_int,
    socket: *mut c_void,
    events: c_int,
) -> c_int {
    // SAFETY: as the caller promises.
    let registry = unsafe { &mut *registry };
    match usize::try_from(fd) {
        Ok(fd) if registry.sockets.insert(fd, (socket, events)).is_ok() => 0,
        _ => -2,
    }
}

/// `Reactor::_set`: the events `fd` is registered for, if it is.
///
/// # Safety
/// As for [`swr_v1_reactor_registry_put`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_reactor_registry_set(
    registry: *mut Registry,
    fd: c_int,
    events: c_int,
) {
    // SAFETY: as the caller promises.
    let registry = unsafe { &mut *registry };
    if let Some(entry) = slot(fd).and_then(|fd| registry.sockets.get_mut(fd)) {
        entry.1 = events;
    }
}

/// `Reactor::_del`: forgets `fd`.
///
/// # Safety
/// As for [`swr_v1_reactor_registry_put`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_reactor_registry_remove(registry: *mut Registry, fd: c_int) {
    // SAFETY: as the caller promises.
    let registry = unsafe { &mut *registry };
    if let Some(fd) = slot(fd) {
        registry.sockets.remove(fd);
    }
}

/// `Reactor::get_socket`: the socket registered for `fd`, or null.
///
/// # Safety
/// `registry` is null or live.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_reactor_registry_get(
    registry: *const Registry,
    fd: c_int,
) -> *mut c_void {
    // SAFETY: as the caller promises.
    let sockets = unsafe { sockets(registry) };
    sockets
        .zip(slot(fd))
        .and_then(|(sockets, fd)| sockets.get(fd))
        .map_or(ptr::null_mut(), |entry| entry.0)
}

/// `Reactor::get_event_num`: how many fds are registered.
///
/// # Safety
/// `registry` is null or live.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_reactor_registry_count(registry: *const Registry) -> usize {
    // SAFETY: as the caller promises.
    unsafe { sockets(registry) }.map_or(0, Sockets::len)
}

/// `Reactor::foreach_socket`: calls `visit(context, fd, socket)` for each entry, over a
/// copy taken first, so `visit` may change the registry.
///
/// # Safety
/// `registry` is null or live; `visit` is safe to call with `context`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_reactor_registry_foreach(
    registry: *const Registry,
    context: *mut c_void,
    visit: unsafe extern "C" fn(*mut c_void, c_int, *mut c_void),
) -> c_int {
    // SAFETY: as the caller promises.
    let Some(sockets) = (unsafe { sockets(registry) }) else {
        return 0;
    };
    let mut entries = Vec::new();
    if entries.try_reserve_exact(sockets.len()).is_err() {
        return -2;
    }
    entries.extend(sockets.iter().map(|(fd, entry)| (fd, entry.0)));
    for (fd, socket) in entries {
        let Ok(fd) = c_int::try_from(fd) else {
            continue;
        };
        // SAFETY: as the caller promises.
        unsafe { visit(context, fd, socket) };
    }
    0
}

/// The registered fds as pollfds (`translate_events_to_poll`, `revents` cleared) and
/// their sockets, at most `limit`; how many were written.
///
/// # Safety
/// `registry` is null or live; `events` and `sockets` hold `limit` entries.
pub(crate) unsafe fn poll(
    registry: *const Registry,
    events: *mut libc::pollfd,
    sockets: *mut *mut c_void,
    limit: usize,
) -> usize {
    // SAFETY: as the caller promises.
    let Some(registered) = (unsafe { self::sockets(registry) }) else {
        return 0;
    };
    let mut count = 0;
    for (fd, &(socket, registered_events)) in registered.iter().take(limit) {
        let Ok(fd) = c_int::try_from(fd) else {
            continue;
        };
        let entry = libc::pollfd {
            fd,
            events: crate::swr_v1_reactor_poll_events_to(registered_events),
            revents: 0,
        };
        // SAFETY: `count < limit`, within the caller's `events`.
        unsafe { events.wrapping_add(count).write(entry) };
        // SAFETY: as above, for `sockets`.
        unsafe { sockets.wrapping_add(count).write(socket) };
        count += 1;
    }
    count
}

#[cfg(test)]
mod tests {
    use super::*;

    const SW_EVENT_READ: c_int = 1 << 9;
    const SW_EVENT_WRITE: c_int = 1 << 10;

    unsafe extern "C" fn collect(context: *mut c_void, fd: c_int, socket: *mut c_void) {
        let seen = unsafe { &mut *context.cast::<Vec<(c_int, usize)>>() };
        seen.push((fd, socket as usize));
    }

    #[test]
    fn registers_and_snapshots() {
        let registry = swr_v1_reactor_registry_new();
        let socket = |n: usize| n as *mut c_void;
        unsafe {
            assert_eq!(swr_v1_reactor_registry_count(ptr::null()), 0);
            assert!(swr_v1_reactor_registry_get(ptr::null(), 3).is_null());
            assert_eq!(
                swr_v1_reactor_registry_put(registry, 3, socket(30), SW_EVENT_READ),
                0
            );
            assert_eq!(
                swr_v1_reactor_registry_put(registry, 4, socket(40), SW_EVENT_WRITE),
                0
            );
            assert_eq!(
                swr_v1_reactor_registry_put(
                    registry,
                    5,
                    socket(50),
                    SW_EVENT_READ | SW_EVENT_WRITE
                ),
                0
            );
            assert_eq!(
                swr_v1_reactor_registry_put(registry, 4, socket(40), 1 << 11),
                0
            );
            swr_v1_reactor_registry_remove(registry, 5);
            swr_v1_reactor_registry_remove(registry, 9);
            assert_eq!(swr_v1_reactor_registry_count(registry), 2);
            assert_eq!(swr_v1_reactor_registry_get(registry, 4), socket(40));
            assert!(swr_v1_reactor_registry_get(registry, 5).is_null());

            let mut seen: Vec<(c_int, usize)> = Vec::new();
            swr_v1_reactor_registry_foreach(registry, (&raw mut seen).cast(), collect);
            seen.sort_unstable();
            assert_eq!(seen, [(3, 30), (4, 40)]);

            let mut events = [libc::pollfd {
                fd: -1,
                events: 0,
                revents: 7,
            }; 2];
            let mut sockets = [ptr::null_mut(); 2];
            assert_eq!(
                poll(registry, events.as_mut_ptr(), sockets.as_mut_ptr(), 2),
                2
            );
            let mut polled: Vec<_> = events
                .iter()
                .zip(sockets)
                .map(|(e, s)| (e.fd, e.events, e.revents, s as usize))
                .collect();
            polled.sort_unstable();
            assert_eq!(polled, [(3, libc::POLLIN, 0, 30), (4, 0, 0, 40)]);
            assert_eq!(
                poll(registry, events.as_mut_ptr(), sockets.as_mut_ptr(), 1),
                1
            );
            swr_v1_reactor_registry_free(registry);
        }
    }
}
