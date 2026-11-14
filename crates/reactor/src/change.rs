//! `Reactor::add`, `set` and `del` for every backend in one call: upstream's
//! `ReactorEpoll`, `ReactorKqueue` and `ReactorPoll` methods over the Rust reactor
//! state, the checks, kernel calls and registry update included. Native code only
//! logs the warnings, under the upstream function each one names.
use crate::Socket;
use crate::reactor_state::{Backend, ReactorState};
use crate::registration;
use crate::socket_sync::set_errno;
use std::ffi::{CStr, c_int, c_void};

/// Logs `message` as from `origin`: `code` 0 is `swoole_sys_warning`, with errno as the
/// kernel left it; any other is `swoole_error_log(SW_LOG_WARNING, code, ...)`. Then
/// `swoole_print_backtrace_on_error()`.
pub type ChangeLog = fn(origin: &CStr, code: c_int, message: &str);

const SW_ERROR_EVENT_REMOVE_FAILED: c_int = 800;
const SW_ERROR_EVENT_ADD_FAILED: c_int = 801;
#[cfg(not(target_os = "linux"))]
const SW_ERROR_EVENT_UPDATE_FAILED: c_int = 802;
const SW_ERROR_SOCKET_NOT_EXISTS: c_int = 6003;

/// `Reactor::change`'s operations.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u32)]
pub enum ChangeOp {
    Add,
    Set,
    Del,
}

impl ChangeOp {
    /// The op's `uint32_t` in upstream's `Reactor::change`.
    #[must_use]
    pub const fn code(self) -> u32 {
        self as u32
    }

    /// An op from upstream's `uint32_t`: 0 add, 1 set, any other del.
    #[must_use]
    pub const fn from_code(op: u32) -> Self {
        match op {
            0 => Self::Add,
            1 => Self::Set,
            _ => Self::Del,
        }
    }
}

/// `Reactor::interest`'s kinds: `add_event` and `del_event` of an event, then
/// `add_read_event`, `add_write_event`, `remove_read_event` and `remove_write_event`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u32)]
pub enum Interest {
    AddEvent,
    DelEvent,
    AddRead,
    AddWrite,
    RemoveRead,
    RemoveWrite,
}

impl Interest {
    /// The kind's `uint32_t` in upstream's `Reactor::interest`.
    #[must_use]
    pub const fn code(self) -> u32 {
        self as u32
    }

    /// A kind from upstream's `uint32_t`; any past the last is `remove_write_event`.
    #[must_use]
    pub const fn from_code(kind: u32) -> Self {
        match kind {
            0 => Self::AddEvent,
            1 => Self::DelEvent,
            2 => Self::AddRead,
            3 => Self::AddWrite,
            4 => Self::RemoveRead,
            _ => Self::RemoveWrite,
        }
    }
}

fn errno() -> c_int {
    std::io::Error::last_os_error().raw_os_error().unwrap_or(0)
}

/// The socket's fields a change reads, as they were before it.
struct Seen {
    fd: c_int,
    fd_type: c_int,
    events: c_int,
    removed: bool,
    silent_remove: bool,
}

struct Change {
    log: ChangeLog,
    socket: Seen,
    id: u16,
    op: ChangeOp,
    events: c_int,
}

impl Change {
    /// One warning, with `errno` set to `error` for a system warning's text.
    fn warn(&self, origin: &CStr, code: c_int, error: c_int, message: &str) {
        set_errno(error);
        (self.log)(origin, code, message);
    }

    /// `ReactorImpl::after_removal_failure`.
    fn removal_failed(&self, error: c_int) {
        let socket = &self.socket;
        if !socket.silent_remove {
            self.warn(
                c"ReactorImpl::after_removal_failure",
                SW_ERROR_EVENT_REMOVE_FAILED,
                error,
                &format!(
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
        if self.op == ChangeOp::Del && socket.removed {
            self.warn(
                c"ReactorEpoll::del",
                SW_ERROR_EVENT_REMOVE_FAILED,
                errno(),
                &format!(
                    "[Reactor#{id}] failed to delete events[fd={fd}, fd_type={fd_type}], this socket has already been removed"
                ),
            );
            return false;
        }
        if registration::epoll_ctl(epfd, self.op, fd, token, self.events) >= 0 {
            return true;
        }
        let error = errno();
        let events = self.events;
        match self.op {
            ChangeOp::Add => self.warn(
                c"ReactorEpoll::add",
                0,
                error,
                &format!(
                    "[Reactor#{id}] epoll_ctl(epfd={epfd}, EPOLL_CTL_ADD, fd={fd}, fd_type={fd_type}, events={events}) failed"
                ),
            ),
            ChangeOp::Set => self.warn(
                c"ReactorEpoll::set",
                0,
                error,
                &format!(
                    "[Reactor#{id}] epoll_ctl(epfd={epfd}, EPOLL_CTL_MOD, fd={fd}, fd_type={fd_type}, events={events}) failed"
                ),
            ),
            ChangeOp::Del => {
                self.removal_failed(error);
                // Closed before removal: the registry entry still goes.
                if error == libc::EBADF || error == libc::ENOENT {
                    return true;
                }
                self.warn(
                    c"ReactorEpoll::del",
                    0,
                    error,
                    &format!(
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
            ChangeOp::Add => {
                let result = registration::kqueue_add(kq, fd, token, self.events);
                if let Err(failed) = &result {
                    let filter = if failed.write { "write" } else { "read" };
                    self.warn(
                        c"ReactorKqueue::add",
                        SW_ERROR_EVENT_ADD_FAILED,
                        errno(),
                        &format!(
                            "add events[fd={fd}, reactor_id={id}, type={}, events={filter}] failed",
                            socket.fd_type
                        ),
                    );
                }
                result.is_ok()
            }
            ChangeOp::Set => {
                let result = registration::kqueue_set(kq, fd, token, self.events);
                if let Err(failed) = &result {
                    let added = failed.add;
                    self.warn(
                        c"ReactorKqueue::set",
                        if added {
                            SW_ERROR_EVENT_UPDATE_FAILED
                        } else {
                            SW_ERROR_EVENT_REMOVE_FAILED
                        },
                        errno(),
                        &format!(
                            "kqueue->{}({fd}, SW_EVENT_{}) failed",
                            if added { "set" } else { "del" },
                            if failed.write { "WRITE" } else { "READ" }
                        ),
                    );
                }
                result.is_ok()
            }
            ChangeOp::Del => {
                if socket.removed {
                    self.warn(
                        c"ReactorKqueue::del",
                        SW_ERROR_EVENT_REMOVE_FAILED,
                        errno(),
                        &format!("failed to delete event[{fd}], has been removed"),
                    );
                    return false;
                }
                let mut errors = [0; 2];
                let result = registration::kqueue_del(kq, fd, token, socket.events, &mut errors);
                for error in errors.into_iter().filter(|error| *error != 0) {
                    self.removal_failed(error);
                }
                result
            }
        }
    }

    /// `ReactorPoll::add`, `set` and `del` before the registry update.
    fn poll(&self, state: &ReactorState) -> bool {
        let (socket, id, events) = (&self.socket, self.id, self.events);
        let (fd, fd_type) = (socket.fd, socket.fd_type);
        let exists = !state.registry.get(fd).is_null();
        let (origin, code, message) = match self.op {
            ChangeOp::Add if exists => (
                c"ReactorPoll::add",
                SW_ERROR_EVENT_ADD_FAILED,
                format!(
                    "[Reactor#{id}] failed to add events[fd={fd}, fd_type={fd_type}, events={events}], the socket#{fd} is already exists"
                ),
            ),
            ChangeOp::Add
                if usize::try_from(state.backend_limit).ok() == Some(state.registry.count()) =>
            {
                (
                    c"ReactorPoll::add",
                    SW_ERROR_EVENT_ADD_FAILED,
                    format!(
                        "too many sockets, the max events is {}",
                        state.backend_limit
                    ),
                )
            }
            ChangeOp::Set if !exists => (
                c"ReactorPoll::set",
                SW_ERROR_SOCKET_NOT_EXISTS,
                format!(
                    "[Reactor#{id}] failed to set events[fd={fd}, fd_type={fd_type}, events={events}], the socket#{fd} has already been removed"
                ),
            ),
            ChangeOp::Add | ChangeOp::Set => return true,
            ChangeOp::Del if socket.removed => (
                c"ReactorPoll::del",
                SW_ERROR_SOCKET_NOT_EXISTS,
                format!(
                    "[Reactor#{id}] failed to delete events[fd={fd}, fd_type={fd_type}], the socket#{fd} has already been removed"
                ),
            ),
            ChangeOp::Del if !exists => (
                c"ReactorPoll::del",
                SW_ERROR_SOCKET_NOT_EXISTS,
                format!(
                    "[Reactor#{id}] failed to delete events[fd={fd}, fd_type={fd_type}], the socket#{fd} is not exists"
                ),
            ),
            ChangeOp::Del => return true,
        };
        self.warn(origin, code, errno(), &message);
        false
    }
}

/// One change of a socket's registration. An add or set leaves the socket's address in
/// the registry and with the kernel until its removal, so only a socket that is never
/// freed while registered (`'static`) is registered; any live socket may be removed.
#[derive(Clone, Copy)]
pub(crate) enum Request<'a> {
    Add(Socket<'static>),
    Set(Socket<'static>),
    Del(Socket<'a>),
}

impl Request<'_> {
    /// `op` of `socket`.
    pub(crate) const fn new(socket: Socket<'static>, op: ChangeOp) -> Self {
        match op {
            ChangeOp::Add => Self::Add(socket),
            ChangeOp::Set => Self::Set(socket),
            ChangeOp::Del => Self::Del(socket),
        }
    }
}

/// `Reactor::add`, `set` or `del` of the request's socket with `events` on the state's
/// backend: whether it worked, after logging a failure through `log`. An add that
/// cannot allocate is upstream's `bad_alloc`. `log` reaches no reactor.
pub(crate) fn apply(
    state: &mut ReactorState,
    request: Request<'_>,
    events: c_int,
    log: ChangeLog,
) -> bool {
    let (socket, op) = match request {
        Request::Add(socket) => (socket, ChangeOp::Add),
        Request::Set(socket) => (socket, ChangeOp::Set),
        Request::Del(socket) => (socket, ChangeOp::Del),
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
    if done && !state.record(socket, change.op, events) {
        std::alloc::handle_alloc_error(std::alloc::Layout::new::<crate::SocketRecord>());
    }
    done
}

const SW_EVENT_READ: c_int = swoole_rs_corebase::constants::EventType::SW_EVENT_READ as c_int;
const SW_EVENT_WRITE: c_int = swoole_rs_corebase::constants::EventType::SW_EVENT_WRITE as c_int;

/// What a `Reactor` interest helper asks of [`apply`], as upstream decides
/// from the socket's events: the op and its events, or `None` when nothing changes.
pub(crate) fn interest_change(
    record: Socket<'_>,
    kind: Interest,
    event: c_int,
) -> Option<(ChangeOp, c_int)> {
    use Interest::{AddEvent, AddRead, AddWrite, DelEvent, RemoveRead, RemoveWrite};
    let events = record.read(|record| record.events);
    let (other, own) = if matches!(kind, AddRead | RemoveRead) {
        (SW_EVENT_WRITE, SW_EVENT_READ)
    } else {
        (SW_EVENT_READ, SW_EVENT_WRITE)
    };
    let (op, events) = match kind {
        AddEvent if events & event == 0 => (ChangeOp::Set, events | event),
        DelEvent if events & event != 0 => (ChangeOp::Set, events & !event),
        AddEvent | DelEvent => return None,
        AddRead | AddWrite if events & other != 0 => (ChangeOp::Set, events | own),
        AddRead | AddWrite => (ChangeOp::Add, own),
        RemoveRead | RemoveWrite if events & other != 0 => (ChangeOp::Set, events & !own),
        RemoveRead | RemoveWrite => (ChangeOp::Del, 0),
    };
    if op == ChangeOp::Set && !matches!(kind, AddEvent | DelEvent) {
        // Upstream writes the socket's events before the set, so they stay when it fails.
        record.edit(|record| record.events = events);
    }
    Some((op, events))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::SocketRecord;
    use std::cell::RefCell;

    /// [`apply`] of the raw op code (0 add, 1 set, any other del) on the test's state
    /// and socket, which the test keeps live while registered.
    unsafe fn swr_v1_reactor_change(
        owner: *mut ReactorState,
        socket: *mut SocketRecord,
        op: u32,
        events: c_int,
        log: ChangeLog,
    ) -> c_int {
        let socket = unsafe { Socket::from_ptr(socket) }.unwrap();
        let done = apply(
            unsafe { &mut *owner },
            Request::new(socket, ChangeOp::from_code(op)),
            events,
            log,
        );
        if done { 0 } else { -1 }
    }

    const SW_EVENT_READ: c_int = swoole_rs_corebase::constants::EventType::SW_EVENT_READ as c_int;
    const SW_EVENT_WRITE: c_int = swoole_rs_corebase::constants::EventType::SW_EVENT_WRITE as c_int;

    thread_local! {
        static LOGGED: RefCell<Vec<(String, c_int, String)>> = const { RefCell::new(Vec::new()) };
    }

    fn log(origin: &CStr, code: c_int, message: &str) {
        let entry = (
            origin.to_string_lossy().into_owned(),
            code,
            message.to_owned(),
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
            assert!(first.removed);
            assert_eq!(change(&mut first, 2, 0), -1);
            second.removed = false;
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
            assert_eq!(
                crate::swr_v1_reactor_registry_count(state.registry.as_ref()),
                1
            );
            assert_eq!(logged().len(), 0);
            // Closed before removal: one warning, and the registry entry goes.
            libc::close(fds[0]);
            assert_eq!(
                swr_v1_reactor_change(state.owner, &raw mut socket, 2, 0, log),
                0
            );
            assert!(socket.removed);
            assert_eq!(
                crate::swr_v1_reactor_registry_count(state.registry.as_ref()),
                0
            );
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
                let record = Socket::from_ptr(socket).unwrap();
                interest_change(record, kind, event).map_or(0, |(op, events)| {
                    swr_v1_reactor_change(state.owner, socket, op.code(), events, log)
                })
            };
            // add_write_event on a new socket adds it for writing; add_read_event sets both.
            assert_eq!(interest(&mut socket, Interest::AddWrite, 0), 0);
            assert_eq!(socket.events, SW_EVENT_WRITE);
            assert_eq!(interest(&mut socket, Interest::AddRead, 0), 0);
            assert_eq!(socket.events, SW_EVENT_READ | SW_EVENT_WRITE);
            // add_event of a bit it has, and del_event of one it lacks, change nothing.
            assert_eq!(interest(&mut socket, Interest::AddEvent, SW_EVENT_READ), 0);
            assert_eq!(interest(&mut socket, Interest::DelEvent, 1 << 11), 0);
            assert_eq!(interest(&mut socket, Interest::AddEvent, 1 << 11), 0);
            assert_eq!(socket.events, SW_EVENT_READ | SW_EVENT_WRITE | (1 << 11));
            assert_eq!(interest(&mut socket, Interest::DelEvent, 1 << 11), 0);
            // remove_read_event keeps the write interest; remove_write_event then deletes it.
            assert_eq!(interest(&mut socket, Interest::RemoveRead, 0), 0);
            assert_eq!(socket.events, SW_EVENT_WRITE);
            assert_eq!(interest(&mut socket, Interest::RemoveWrite, 0), 0);
            assert_eq!((socket.events, socket.removed), (0, true));
            // A failed set keeps the events upstream wrote first.
            let mut missing = SocketRecord::with_fd(21, 5);
            missing.events = SW_EVENT_READ;
            assert_eq!(interest(&mut missing, Interest::AddWrite, 0), -1);
            assert_eq!(missing.events, SW_EVENT_READ | SW_EVENT_WRITE);
            assert_eq!(logged()[0].0, "ReactorPoll::set");
            assert_eq!(
                crate::swr_v1_reactor_registry_count(state.registry.as_ref()),
                0
            );
            crate::reactor_state::tests::free(state.owner);
        }
    }
}
