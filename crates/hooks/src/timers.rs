//! The coroutine socket's timers and timeouts: the read and write timer slots
//! (in [`SocketState`]) and which one a firing timer belongs to, `TimeoutSetter`'s
//! save/set/restore of the selected timeouts, and `connect(addr)`'s whole loop:
//! the `connect(2)` retries, the wait for writability and the `SO_ERROR` check.
use crate::shutdown::{SocketState, swr_v1_hook_socket_connect};
use crate::{swr_v1_hook_connect_action, swr_v1_hook_connect_option};
use core::ffi::{c_int, c_void};

const EVENT_READ: u32 = 1 << 9;
const EVENT_WRITE: u32 = 1 << 10;

/// The read slot for `SW_EVENT_READ`, else the write slot.
fn slot(event: u32) -> usize {
    usize::from(event != EVENT_READ)
}

/// The timer slot (a native `TimerNode *`) for `event`'s direction: reading for
/// `SW_EVENT_READ`, else writing.
///
/// # Safety
/// `state` is a live handle; the slot lives as long as it.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_hook_socket_timer(
    state: *mut SocketState,
    event: u32,
) -> *mut *mut c_void {
    // SAFETY: the caller's live state; the slot stays inside its allocation.
    unsafe { &raw mut (*state).timers[slot(event)] }
}

/// A timer fired: clears the slot holding `node` and returns its direction
/// (`SW_EVENT_READ` first), or 0 when neither slot holds it.
///
/// # Safety
/// `state` is a live handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_hook_socket_timeout(
    state: *mut SocketState,
    node: *mut c_void,
) -> u32 {
    // SAFETY: the caller's live state.
    let timers = unsafe { &mut (*state).timers };
    for (index, event) in [EVENT_READ, EVENT_WRITE].into_iter().enumerate() {
        if timers[index] == node {
            timers[index] = core::ptr::null_mut();
            return event;
        }
    }
    0
}

/// `SW_TIMEOUT_DNS`, `_CONNECT`, `_READ` and `_WRITE`, in `timeout_type_list`'s order.
const TIMEOUT_TYPES: [c_int; 4] = [1, 2, 4, 8];

#[repr(C)]
pub struct TimeoutOps {
    /// The socket's timeout of one type, and setting it.
    pub get: unsafe extern "C" fn(*mut c_void, c_int) -> f64,
    pub set: unsafe extern "C" fn(*mut c_void, f64, c_int),
}

/// `TimeoutSetter`: unless `timeout` is 0, for each type in `types`, saves the
/// socket's timeout in `original` (indexed as `timeout_type_list`) and sets
/// `timeout` where it differs; with `restore`, puts back each saved timeout that
/// differed instead.
///
/// # Safety
/// `ops` is live, `socket` is what its effects take, and `original` holds four
/// writable doubles.
#[unsafe(no_mangle)]
#[expect(
    clippy::float_cmp,
    reason = "upstream compares the requested and saved timeouts exactly"
)]
pub unsafe extern "C" fn swr_v1_hook_timeouts(
    ops: *const TimeoutOps,
    socket: *mut c_void,
    timeout: f64,
    types: c_int,
    original: *mut f64,
    restore: bool,
) {
    if timeout == 0.0 {
        return;
    }
    // SAFETY: the caller's live table.
    let ops = unsafe { &*ops };
    for (index, kind) in TIMEOUT_TYPES.into_iter().enumerate() {
        if types & kind == 0 {
            continue;
        }
        // SAFETY: one of the caller's four doubles.
        let saved = unsafe { original.add(index) };
        if restore {
            // SAFETY: as above.
            let value = unsafe { *saved };
            if timeout != value {
                // SAFETY: the live socket's timeout.
                unsafe { (ops.set)(socket, value, kind) };
            }
            continue;
        }
        // SAFETY: the live socket's timeout.
        let value = unsafe { (ops.get)(socket, kind) };
        // SAFETY: as above.
        unsafe { *saved = value };
        if timeout != value {
            // SAFETY: the live socket's timeout.
            unsafe { (ops.set)(socket, timeout, kind) };
        }
    }
}

#[repr(C)]
pub struct ConnectAddrOps {
    pub socket: *mut c_void,
    pub state: *mut SocketState,
    /// The socket's descriptor.
    pub fd: c_int,
    /// `is_available(SW_EVENT_RDWR)`, `set_err(code)` and `is_closed()`.
    pub available: unsafe extern "C" fn(*mut c_void) -> bool,
    pub set_err: unsafe extern "C" fn(*mut c_void, c_int),
    pub closed: unsafe extern "C" fn(*mut c_void) -> bool,
    /// Waits to write under the connect timeout.
    pub wait: unsafe extern "C" fn(*mut c_void) -> bool,
    /// `get_option(SOL_SOCKET, SO_ERROR)` into `errCode`, and `errCode`.
    pub so_error: unsafe extern "C" fn(*mut c_void) -> c_int,
    pub error: unsafe extern "C" fn(*mut c_void) -> c_int,
    /// `get_name()`.
    pub name: unsafe extern "C" fn(*mut c_void),
}

fn errno() -> c_int {
    std::io::Error::last_os_error().raw_os_error().unwrap_or(0)
}

/// `Socket::connect(addr, addrlen)`.
///
/// # Safety
/// `ops` is live for the call and `addr` holds `length` readable bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_hook_connect_addr(
    ops: *const ConnectAddrOps,
    addr: *const libc::sockaddr,
    length: libc::socklen_t,
) -> bool {
    // SAFETY: the caller passes a live table (see above).
    let ops = unsafe { &*ops };
    // SAFETY: the live socket's availability.
    if !unsafe { (ops.available)(ops.socket) } {
        return false;
    }
    let action = loop {
        // SAFETY: the caller's descriptor and address.
        let result = unsafe { libc::connect(ops.fd, addr, length) };
        let error = errno();
        let action = swr_v1_hook_connect_action(
            result,
            result < 0 && error == libc::EINTR,
            result < 0 && error == libc::EINPROGRESS,
        );
        if action != 1 {
            break action;
        }
    };
    if action < 0 {
        // SAFETY: the live socket's error.
        unsafe { (ops.set_err)(ops.socket, errno()) };
        return false;
    }
    if action > 0 {
        // SAFETY: the live socket's wait.
        if !unsafe { (ops.wait)(ops.socket) } {
            // SAFETY: the live socket.
            if unsafe { (ops.closed)(ops.socket) } {
                // SAFETY: the live socket's error.
                unsafe { (ops.set_err)(ops.socket, libc::ECONNABORTED) };
            }
            return false;
        }
        // SAFETY: the live socket's pending error.
        let result = unsafe { (ops.so_error)(ops.socket) };
        // SAFETY: the live socket's error code.
        let error = unsafe { (ops.error)(ops.socket) };
        if swr_v1_hook_connect_option(result, if result < 0 { 0 } else { error }) {
            // SAFETY: the live socket's error.
            unsafe { (ops.set_err)(ops.socket, error) };
            return false;
        }
    }
    // SAFETY: the live state.
    unsafe { swr_v1_hook_socket_connect(ops.state) };
    // SAFETY: the live socket.
    unsafe { (ops.name)(ops.socket) };
    // SAFETY: the live socket's error.
    unsafe { (ops.set_err)(ops.socket, 0) };
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shutdown::{swr_v1_hook_socket_state_free, swr_v1_hook_socket_state_new};
    use std::cell::RefCell;

    #[test]
    fn timer_slots() {
        let state = swr_v1_hook_socket_state_new();
        let mut node = 0u8;
        let node: *mut c_void = (&raw mut node).cast();
        // SAFETY: a live state.
        unsafe {
            let read = swr_v1_hook_socket_timer(state, EVENT_READ);
            let write = swr_v1_hook_socket_timer(state, EVENT_WRITE);
            assert_ne!(read, write);
            assert_eq!(
                write,
                swr_v1_hook_socket_timer(state, EVENT_READ | EVENT_WRITE)
            );
            assert_eq!(
                swr_v1_hook_socket_timeout(state, node),
                0,
                "an unknown node"
            );
            *write = node;
            assert_eq!(swr_v1_hook_socket_timeout(state, node), EVENT_WRITE);
            assert!((*write).is_null());
            *read = node;
            assert_eq!(swr_v1_hook_socket_timeout(state, node), EVENT_READ);
            assert!((*read).is_null());
            swr_v1_hook_socket_state_free(state);
        }
    }

    thread_local! {
        static TIMEOUTS: RefCell<[f64; 4]> = const { RefCell::new([1.0, 2.0, 3.0, 4.0]) };
        static SETS: RefCell<Vec<(f64, c_int)>> = const { RefCell::new(Vec::new()) };
    }

    unsafe extern "C" fn get(_: *mut c_void, kind: c_int) -> f64 {
        TIMEOUTS.with(|t| t.borrow()[kind.trailing_zeros() as usize])
    }
    unsafe extern "C" fn set(_: *mut c_void, value: f64, kind: c_int) {
        SETS.with(|s| s.borrow_mut().push((value, kind)));
        TIMEOUTS.with(|t| t.borrow_mut()[kind.trailing_zeros() as usize] = value);
    }

    #[test]
    fn timeouts_save_set_and_restore() {
        let ops = TimeoutOps { get, set };
        let mut original = [0.0; 4];
        // SAFETY: a local table and array.
        unsafe {
            swr_v1_hook_timeouts(
                &raw const ops,
                std::ptr::null_mut(),
                3.0,
                4 | 8,
                original.as_mut_ptr(),
                false,
            );
        }
        assert_eq!(original, [0.0, 0.0, 3.0, 4.0]);
        assert_eq!(
            SETS.with(|s| s.borrow().clone()),
            [(3.0, 8)],
            "the read timeout already matched"
        );
        SETS.with(|s| s.borrow_mut().clear());
        // SAFETY: as above.
        unsafe {
            swr_v1_hook_timeouts(
                &raw const ops,
                std::ptr::null_mut(),
                3.0,
                4 | 8,
                original.as_mut_ptr(),
                true,
            );
        }
        assert_eq!(SETS.with(|s| s.borrow().clone()), [(4.0, 8)]);
        SETS.with(|s| s.borrow_mut().clear());
        // SAFETY: as above.
        unsafe {
            swr_v1_hook_timeouts(
                &raw const ops,
                std::ptr::null_mut(),
                0.0,
                15,
                original.as_mut_ptr(),
                false,
            );
        }
        assert!(
            SETS.with(|s| s.borrow().is_empty()),
            "a zero timeout changes nothing"
        );
    }

    thread_local! {
        static LOG: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
        static WAIT: RefCell<bool> = const { RefCell::new(true) };
        static ERROR: RefCell<c_int> = const { RefCell::new(0) };
    }

    unsafe extern "C" fn available(_: *mut c_void) -> bool {
        true
    }
    unsafe extern "C" fn set_err(_: *mut c_void, code: c_int) {
        LOG.with(|l| l.borrow_mut().push(format!("err {code}")));
    }
    unsafe extern "C" fn closed(_: *mut c_void) -> bool {
        true
    }
    unsafe extern "C" fn wait(_: *mut c_void) -> bool {
        LOG.with(|l| l.borrow_mut().push("wait".into()));
        WAIT.with(|w| *w.borrow())
    }
    unsafe extern "C" fn so_error(_: *mut c_void) -> c_int {
        0
    }
    unsafe extern "C" fn error(_: *mut c_void) -> c_int {
        ERROR.with(|e| *e.borrow())
    }
    unsafe extern "C" fn name(_: *mut c_void) {
        LOG.with(|l| l.borrow_mut().push("name".into()));
    }

    fn connect_to(fd: c_int, addr: &libc::sockaddr_in) -> (bool, Vec<String>, bool) {
        LOG.with(|l| l.borrow_mut().clear());
        let state = swr_v1_hook_socket_state_new();
        let ops = ConnectAddrOps {
            socket: std::ptr::null_mut(),
            state,
            fd,
            available,
            set_err,
            closed,
            wait,
            so_error,
            error,
            name,
        };
        // SAFETY: a local table and address.
        let ok = unsafe {
            swr_v1_hook_connect_addr(
                &raw const ops,
                std::ptr::from_ref(addr).cast(),
                libc::socklen_t::try_from(size_of::<libc::sockaddr_in>()).unwrap(),
            )
        };
        // SAFETY: the live state.
        let connected = unsafe { crate::shutdown::swr_v1_hook_socket_flags(state) }.connected;
        // SAFETY: frees the state made above.
        unsafe { swr_v1_hook_socket_state_free(state) };
        (ok, LOG.with(|l| l.borrow().clone()), connected)
    }

    fn loopback(port: u16) -> libc::sockaddr_in {
        // SAFETY: an all-zero sockaddr_in is valid.
        let mut addr: libc::sockaddr_in = unsafe { std::mem::zeroed() };
        addr.sin_family = libc::sa_family_t::try_from(libc::AF_INET).unwrap();
        addr.sin_port = port.to_be();
        addr.sin_addr.s_addr = u32::from_be_bytes([127, 0, 0, 1]).to_be();
        addr
    }

    #[test]
    fn connect_now_or_after_a_wait() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        // A blocking socket connects at once.
        // SAFETY: a new TCP socket.
        let fd = unsafe { libc::socket(libc::AF_INET, libc::SOCK_STREAM, 0) };
        let (ok, log, connected) = connect_to(fd, &loopback(port));
        assert!(ok && connected);
        assert_eq!(log, ["name", "err 0"]);
        // A bad descriptor fails with its errno.
        let (ok, log, connected) = connect_to(-1, &loopback(port));
        assert!(!ok && !connected);
        assert_eq!(log, [format!("err {}", libc::EBADF)]);
        // SAFETY: the socket made above.
        unsafe { libc::close(fd) };
        // A nonblocking socket waits; a failed wait on a closed socket is ECONNABORTED.
        // SAFETY: a new TCP socket made nonblocking.
        let fd = unsafe { libc::socket(libc::AF_INET, libc::SOCK_STREAM, 0) };
        // SAFETY: as above.
        unsafe { libc::fcntl(fd, libc::F_SETFL, libc::O_NONBLOCK) };
        WAIT.with(|w| *w.borrow_mut() = false);
        let (ok, log, _) = connect_to(fd, &loopback(port));
        WAIT.with(|w| *w.borrow_mut() = true);
        if log.first().is_some_and(|e| e == "wait") {
            assert!(!ok);
            assert_eq!(
                log,
                ["wait".to_owned(), format!("err {}", libc::ECONNABORTED)]
            );
        }
        // SAFETY: the socket made above.
        unsafe { libc::close(fd) };
    }
}
