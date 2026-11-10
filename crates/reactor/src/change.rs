//! `Reactor::add`, `set` and `del` for every backend in one call: upstream's
//! `ReactorEpoll`, `ReactorKqueue` and `ReactorPoll` methods over the Rust reactor
//! state, the checks, kernel calls and registry update included. Native code only
//! logs the warnings, under the upstream function each one names.
use crate::reactor_state::{Backend, ReactorState};
use crate::registration;
use crate::socket_sync::set_errno;
use crate::{Socket, SocketRecord};
use std::ffi::{CStr, CString, c_char, c_int, c_void};

/// Logs `message` as from `origin`: `code` 0 is `swoole_sys_warning`, with errno as the
/// kernel left it; any other is `swoole_error_log(SW_LOG_WARNING, code, ...)`. Then
/// `swoole_print_backtrace_on_error()`.
pub type ChangeLog = unsafe extern "C" fn(*const c_char, c_int, *const c_char);

const SW_ERROR_EVENT_REMOVE_FAILED: c_int = 800;
const SW_ERROR_EVENT_ADD_FAILED: c_int = 801;
#[cfg(not(target_os = "linux"))]
const SW_ERROR_EVENT_UPDATE_FAILED: c_int = 802;
const SW_ERROR_SOCKET_NOT_EXISTS: c_int = 6003;

const ADD: u32 = 0;
const SET: u32 = 1;

fn errno() -> c_int {
    std::io::Error::last_os_error().raw_os_error().unwrap_or(0)
}

/// The socket's fields a change reads, as they were before it.
struct Seen {
    fd: c_int,
    fd_type: c_int,
    events: c_int,
    removed: u8,
    silent_remove: u8,
}

struct Change {
    log: ChangeLog,
    socket: Seen,
    id: u16,
    op: u32,
    events: c_int,
}

impl Change {
    /// One warning, with `errno` set to `error` for a system warning's text.
    fn warn(&self, origin: &CStr, code: c_int, error: c_int, message: String) {
        let message = CString::new(message).unwrap_or_default();
        set_errno(error);
        // SAFETY: native code's logger with two C strings.
        unsafe { (self.log)(origin.as_ptr(), code, message.as_ptr()) };
    }

    /// `ReactorImpl::after_removal_failure`.
    fn removal_failed(&self, error: c_int) {
        let socket = &self.socket;
        if socket.silent_remove == 0 {
            self.warn(
                c"ReactorImpl::after_removal_failure",
                SW_ERROR_EVENT_REMOVE_FAILED,
                error,
                format!(
                    "failed to delete events[fd={}#{}, type={}, events={}, errno={error}]",
                    socket.fd, self.id, socket.fd_type, socket.events
                ),
            );
        }
    }

    /// `ReactorEpoll::add`, `set` and `del` before the registry update.
    #[cfg(target_os = "linux")]
    fn epoll(&self, epfd: c_int, token: *mut c_void) -> bool {
        let (socket, id) = (&self.socket, self.id);
        let (fd, fd_type) = (socket.fd, socket.fd_type);
        if self.op > SET && socket.removed != 0 {
            self.warn(
                c"ReactorEpoll::del",
                SW_ERROR_EVENT_REMOVE_FAILED,
                errno(),
                format!(
                    "[Reactor#{id}] failed to delete events[fd={fd}, fd_type={fd_type}], this socket has already been removed"
                ),
            );
            return false;
        }
        // SAFETY: the token is the socket, live while registered.
        if unsafe { registration::epoll_ctl(epfd, self.op, fd, token, self.events) } >= 0 {
            return true;
        }
        let error = errno();
        let events = self.events;
        match self.op {
            ADD => self.warn(
                c"ReactorEpoll::add",
                0,
                error,
                format!(
                    "[Reactor#{id}] epoll_ctl(epfd={epfd}, EPOLL_CTL_ADD, fd={fd}, fd_type={fd_type}, events={events}) failed"
                ),
            ),
            SET => self.warn(
                c"ReactorEpoll::set",
                0,
                error,
                format!(
                    "[Reactor#{id}] epoll_ctl(epfd={epfd}, EPOLL_CTL_MOD, fd={fd}, fd_type={fd_type}, events={events}) failed"
                ),
            ),
            _ => {
                self.removal_failed(error);
                // Closed before removal: the registry entry still goes.
                if error == libc::EBADF || error == libc::ENOENT {
                    return true;
                }
                self.warn(
                    c"ReactorEpoll::del",
                    0,
                    error,
                    format!(
                        "[Reactor#{id}] epoll_ctl(epfd={epfd}, EPOLL_CTL_DEL, fd={fd}, fd_type={fd_type}) failed"
                    ),
                );
            }
        }
        false
    }

    /// `ReactorKqueue::add`, `set` and `del` before the registry update.
    #[cfg(any(
        target_os = "macos",
        target_os = "freebsd",
        target_os = "openbsd",
        target_os = "netbsd",
        target_os = "dragonfly"
    ))]
    fn kqueue(&self, kq: c_int, token: *mut c_void) -> bool {
        let (socket, id) = (&self.socket, self.id);
        let fd = socket.fd;
        match self.op {
            ADD => {
                // SAFETY: the token is the socket, live while registered.
                let failed = unsafe { registration::kqueue_add(kq, fd, token, self.events) };
                if failed != 0 {
                    let filter = if failed == 1 { "read" } else { "write" };
                    self.warn(
                        c"ReactorKqueue::add",
                        SW_ERROR_EVENT_ADD_FAILED,
                        errno(),
                        format!(
                            "add events[fd={fd}, reactor_id={id}, type={}, events={filter}] failed",
                            socket.fd_type
                        ),
                    );
                }
                failed == 0
            }
            SET => {
                // SAFETY: as for add.
                let failed = unsafe { registration::kqueue_set(kq, fd, token, self.events) };
                if failed != 0 {
                    let added = failed == 1 || failed == 3;
                    self.warn(
                        c"ReactorKqueue::set",
                        if added {
                            SW_ERROR_EVENT_UPDATE_FAILED
                        } else {
                            SW_ERROR_EVENT_REMOVE_FAILED
                        },
                        errno(),
                        format!(
                            "kqueue->{}({fd}, SW_EVENT_{}) failed",
                            if added { "set" } else { "del" },
                            if failed <= 2 { "READ" } else { "WRITE" }
                        ),
                    );
                }
                failed == 0
            }
            _ => {
                if socket.removed != 0 {
                    self.warn(
                        c"ReactorKqueue::del",
                        SW_ERROR_EVENT_REMOVE_FAILED,
                        errno(),
                        format!("failed to delete event[{fd}], has been removed"),
                    );
                    return false;
                }
                let mut errors = [0; 2];
                // SAFETY: two writable ints.
                let result = unsafe {
                    registration::kqueue_del(kq, fd, token, socket.events, errors.as_mut_ptr())
                };
                for error in errors.into_iter().filter(|error| *error != 0) {
                    self.removal_failed(error);
                }
                result >= 0
            }
        }
    }

    /// `ReactorPoll::add`, `set` and `del` before the registry update.
    fn poll(&self, state: &ReactorState) -> bool {
        let (socket, id, events) = (&self.socket, self.id, self.events);
        let (fd, fd_type) = (socket.fd, socket.fd_type);
        let exists = !state.registry.get(fd).is_null();
        let (origin, code, message) = match self.op {
            ADD if exists => (
                c"ReactorPoll::add",
                SW_ERROR_EVENT_ADD_FAILED,
                format!(
                    "[Reactor#{id}] failed to add events[fd={fd}, fd_type={fd_type}, events={events}], the socket#{fd} is already exists"
                ),
            ),
            ADD if usize::try_from(state.backend_limit).ok() == Some(state.registry.count()) => (
                c"ReactorPoll::add",
                SW_ERROR_EVENT_ADD_FAILED,
                format!(
                    "too many sockets, the max events is {}",
                    state.backend_limit
                ),
            ),
            SET if !exists => (
                c"ReactorPoll::set",
                SW_ERROR_SOCKET_NOT_EXISTS,
                format!(
                    "[Reactor#{id}] failed to set events[fd={fd}, fd_type={fd_type}, events={events}], the socket#{fd} has already been removed"
                ),
            ),
            ADD | SET => return true,
            _ if socket.removed != 0 => (
                c"ReactorPoll::del",
                SW_ERROR_SOCKET_NOT_EXISTS,
                format!(
                    "[Reactor#{id}] failed to delete events[fd={fd}, fd_type={fd_type}], the socket#{fd} has already been removed"
                ),
            ),
            _ if !exists => (
                c"ReactorPoll::del",
                SW_ERROR_SOCKET_NOT_EXISTS,
                format!(
                    "[Reactor#{id}] failed to delete events[fd={fd}, fd_type={fd_type}], the socket#{fd} is not exists"
                ),
            ),
            _ => return true,
        };
        self.warn(origin, code, errno(), message);
        false
    }
}

/// `Reactor::add` (op 0), `set` (1) or `del` (2) of `socket` with `events` on the state's
/// backend: 0, -1 after logging the failure through `log` (or for a null socket), or -2 when
/// add cannot allocate.
///
/// # Safety
/// `owner` is a live state with an open backend and `socket` a live record distinct from it,
/// registered until its removal; `log` neither reads nor changes either.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_reactor_change(
    owner: *mut ReactorState,
    socket: *mut SocketRecord,
    op: u32,
    events: c_int,
    log: ChangeLog,
) -> c_int {
    // SAFETY: as the caller promises.
    let state = unsafe { &mut *owner };
    // SAFETY: as the caller promises.
    let Some(socket) = (unsafe { Socket::from_ptr(socket) }) else {
        return -1;
    };
    let change = Change {
        log,
        socket: socket.read(|record| Seen {
            fd: record.fd,
            fd_type: record.fd_type,
            events: record.events,
            removed: record.removed,
            silent_remove: record.silent_remove,
        }),
        id: state.fields.id,
        op,
        events,
    };
    let token = socket.as_ptr().cast::<c_void>();
    let done = match state.backend {
        Backend::None => false,
        Backend::Poll(_) => change.poll(state),
        #[cfg(target_os = "linux")]
        Backend::Epoll(_) => change.epoll(state.backend_fd, token),
        #[cfg(any(
            target_os = "macos",
            target_os = "freebsd",
            target_os = "openbsd",
            target_os = "netbsd",
            target_os = "dragonfly"
        ))]
        Backend::Kqueue(..) => change.kqueue(state.backend_fd, token),
    };
    if !done {
        return -1;
    }
    state.record(socket, op, events)
}

const SW_EVENT_READ: c_int = 1 << 9;
const SW_EVENT_WRITE: c_int = 1 << 10;

/// `Reactor`'s interest helpers over [`swr_v1_reactor_change`]: `add_event` (kind 0) and
/// `del_event` (1) of `event`, then `add_read_event` (2), `add_write_event` (3),
/// `remove_read_event` (4) and `remove_write_event` (5). Each sets, adds or deletes as
/// upstream decides from the socket's events; its result, or 0 when nothing changes.
///
/// # Safety
/// As for [`swr_v1_reactor_change`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_reactor_interest(
    owner: *mut ReactorState,
    socket: *mut SocketRecord,
    kind: u32,
    event: c_int,
    log: ChangeLog,
) -> c_int {
    // SAFETY: as the caller promises.
    let Some(record) = (unsafe { Socket::from_ptr(socket) }) else {
        return -1;
    };
    let events = record.read(|record| record.events);
    let (other, own) = if kind.is_multiple_of(2) {
        (SW_EVENT_WRITE, SW_EVENT_READ)
    } else {
        (SW_EVENT_READ, SW_EVENT_WRITE)
    };
    let (op, events) = match kind {
        0 if events & event == 0 => (SET, events | event),
        1 if events & event != 0 => (SET, events & !event),
        0 | 1 => return 0,
        2 | 3 if events & other != 0 => (SET, events | own),
        2 | 3 => (ADD, own),
        _ if events & other != 0 => (SET, events & !own),
        _ => (2, 0),
    };
    if op == SET && kind >= 2 {
        // Upstream writes the socket's events before the set, so they stay when it fails.
        record.edit(|record| record.events = events);
    }
    // SAFETY: as the caller promises.
    unsafe { swr_v1_reactor_change(owner, socket, op, events, log) }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    const SW_EVENT_READ: c_int = 1 << 9;
    const SW_EVENT_WRITE: c_int = 1 << 10;

    thread_local! {
        static LOGGED: RefCell<Vec<(String, c_int, String)>> = const { RefCell::new(Vec::new()) };
    }

    unsafe extern "C" fn log(origin: *const c_char, code: c_int, message: *const c_char) {
        // SAFETY: the change passes two C strings.
        let (origin, message) = unsafe { (CStr::from_ptr(origin), CStr::from_ptr(message)) };
        let entry = (
            origin.to_string_lossy().into_owned(),
            code,
            message.to_string_lossy().into_owned(),
        );
        LOGGED.with(|logged| logged.borrow_mut().push(entry));
    }

    fn logged() -> Vec<(String, c_int, String)> {
        LOGGED.with(RefCell::take)
    }

    #[cfg(target_os = "linux")]
    const NATIVE: (c_int, &str) = (1, "ReactorEpoll");
    #[cfg(not(target_os = "linux"))]
    const NATIVE: (c_int, &str) = (2, "ReactorKqueue");

    #[test]
    fn poll_checks_the_registry_and_its_limit() {
        unsafe {
            let state = crate::reactor_state::tests::state(3, 1);
            (*state.fields).id = 3;
            let mut first = SocketRecord::with_fd(10, 5);
            let mut second = SocketRecord::with_fd(11, 5);
            let change = |socket: &mut SocketRecord, op, events| {
                swr_v1_reactor_change(state.owner, socket, op, events, log)
            };
            assert_eq!(change(&mut first, 1, SW_EVENT_READ), -1);
            assert_eq!(change(&mut first, 0, SW_EVENT_READ), 0);
            assert_eq!(first.events, SW_EVENT_READ);
            assert_eq!(change(&mut first, 0, SW_EVENT_READ), -1);
            assert_eq!(change(&mut second, 0, SW_EVENT_READ), -1);
            assert_eq!(change(&mut first, 1, SW_EVENT_WRITE), 0);
            assert_eq!(first.events, SW_EVENT_WRITE);
            assert_eq!(change(&mut first, 2, 0), 0);
            assert_eq!(first.removed, 1);
            assert_eq!(change(&mut first, 2, 0), -1);
            second.removed = 0;
            assert_eq!(change(&mut second, 2, 0), -1);
            let set_missing = "[Reactor#3] failed to set events[fd=10, fd_type=5, events=512], the socket#10 has already been removed";
            let exists = "[Reactor#3] failed to add events[fd=10, fd_type=5, events=512], the socket#10 is already exists";
            assert_eq!(
                logged(),
                [
                    ("ReactorPoll::set".into(), 6003, set_missing.into()),
                    ("ReactorPoll::add".into(), 801, exists.into()),
                    ("ReactorPoll::add".into(), 801, "too many sockets, the max events is 1".into()),
                    (
                        "ReactorPoll::del".into(),
                        6003,
                        "[Reactor#3] failed to delete events[fd=10, fd_type=5], the socket#10 has already been removed".into()
                    ),
                    (
                        "ReactorPoll::del".into(),
                        6003,
                        "[Reactor#3] failed to delete events[fd=11, fd_type=5], the socket#11 is not exists".into()
                    ),
                ]
            );
            crate::reactor_state::tests::free(state.owner);
        }
    }

    #[test]
    #[cfg_attr(miri, ignore = "Miri can't spawn a process")]
    fn kernel_backend_registers_and_reports_removal_failures() {
        if crate::isolated_process::run_in_child(
            "change::tests::kernel_backend_registers_and_reports_removal_failures",
        ) {
            return;
        }
        let (kind, name) = NATIVE;
        unsafe {
            let state = crate::reactor_state::tests::state(kind, 4);
            let mut fds = [0; 2];
            assert_eq!(libc::pipe(fds.as_mut_ptr()), 0);
            let mut socket = SocketRecord::with_fd(fds[0], 5);
            assert_eq!(
                swr_v1_reactor_change(state.owner, &raw mut socket, 0, SW_EVENT_READ, log),
                0
            );
            assert_eq!(
                swr_v1_reactor_change(
                    state.owner,
                    &raw mut socket,
                    1,
                    SW_EVENT_READ | SW_EVENT_WRITE,
                    log
                ),
                0
            );
            assert_eq!(crate::swr_v1_reactor_registry_count(state.registry), 1);
            assert_eq!(logged().len(), 0);
            // Closed before removal: one warning, and the registry entry goes.
            libc::close(fds[0]);
            assert_eq!(
                swr_v1_reactor_change(state.owner, &raw mut socket, 2, 0, log),
                0
            );
            assert_eq!(socket.removed, 1);
            assert_eq!(crate::swr_v1_reactor_registry_count(state.registry), 0);
            let removal = logged();
            assert_ne!(removal.len(), 0);
            assert!(removal.iter().all(|(origin, code, message)| origin
                == "ReactorImpl::after_removal_failure"
                && *code == 800
                && message.starts_with(&format!(
                    "failed to delete events[fd={}#0, type=5, events=1536, errno=",
                    fds[0]
                ))));
            assert_eq!(
                swr_v1_reactor_change(state.owner, &raw mut socket, 2, 0, log),
                -1
            );
            let (origin, code, _) = &logged()[0];
            assert_eq!(
                (origin.as_str(), *code),
                (format!("{name}::del").as_str(), 800)
            );
            // A bad descriptor fails the kernel add.
            let mut bad = SocketRecord::with_fd(-1, 5);
            assert_eq!(
                swr_v1_reactor_change(state.owner, &raw mut bad, 0, SW_EVENT_READ, log),
                -1
            );
            assert_eq!(logged()[0].0, format!("{name}::add"));
            libc::close(fds[1]);
            crate::reactor_state::tests::free(state.owner);
        }
    }

    #[test]
    fn interest_helpers_choose_upstreams_change() {
        unsafe {
            let state = crate::reactor_state::tests::state(3, 4);
            let mut socket = SocketRecord::with_fd(20, 5);
            let interest = |socket: &mut SocketRecord, kind, event| {
                swr_v1_reactor_interest(state.owner, socket, kind, event, log)
            };
            // add_write_event on a new socket adds it for writing; add_read_event sets both.
            assert_eq!(interest(&mut socket, 3, 0), 0);
            assert_eq!(socket.events, SW_EVENT_WRITE);
            assert_eq!(interest(&mut socket, 2, 0), 0);
            assert_eq!(socket.events, SW_EVENT_READ | SW_EVENT_WRITE);
            // add_event of a bit it has, and del_event of one it lacks, change nothing.
            assert_eq!(interest(&mut socket, 0, SW_EVENT_READ), 0);
            assert_eq!(interest(&mut socket, 1, 1 << 11), 0);
            assert_eq!(interest(&mut socket, 0, 1 << 11), 0);
            assert_eq!(socket.events, SW_EVENT_READ | SW_EVENT_WRITE | 1 << 11);
            assert_eq!(interest(&mut socket, 1, 1 << 11), 0);
            // remove_read_event keeps the write interest; remove_write_event then deletes it.
            assert_eq!(interest(&mut socket, 4, 0), 0);
            assert_eq!(socket.events, SW_EVENT_WRITE);
            assert_eq!(interest(&mut socket, 5, 0), 0);
            assert_eq!((socket.events, socket.removed), (0, 1));
            // A failed set keeps the events upstream wrote first.
            let mut missing = SocketRecord::with_fd(21, 5);
            missing.events = SW_EVENT_READ;
            assert_eq!(interest(&mut missing, 3, 0), -1);
            assert_eq!(missing.events, SW_EVENT_READ | SW_EVENT_WRITE);
            assert_eq!(logged()[0].0, "ReactorPoll::set");
            assert_eq!(crate::swr_v1_reactor_registry_count(state.registry), 0);
            crate::reactor_state::tests::free(state.owner);
        }
    }
}
