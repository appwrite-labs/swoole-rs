//! `swoole::MsgQueue` (upstream `src/os/msg_queue.cc` and `include/swoole_msg_queue.h`):
//! open, blocking flag, push, pop, stat, capacity and removal over a `SysV` message
//! queue, with upstream's warnings. Rust allocates and frees the queue; native code
//! holds only its pointer.
use libc::{IPC_CREAT, IPC_NOWAIT, c_int, c_long, c_void};
use swoole_rs_corebase::log;

// The XSI message queue calls, declared here because libc omits them on Darwin.
unsafe extern "C" {
    fn msgget(key: libc::key_t, flags: c_int) -> c_int;
    fn msgsnd(id: c_int, msg: *const c_void, size: usize, flags: c_int) -> c_int;
    fn msgrcv(id: c_int, msg: *mut c_void, size: usize, kind: c_long, flags: c_int) -> isize;
    fn msgctl(id: c_int, cmd: c_int, buf: *mut c_void) -> c_int;
}

fn errno() -> c_int {
    std::io::Error::last_os_error().raw_os_error().unwrap_or(0)
}

/// `swoole_sys_warning`, which sets the last error to errno first.
fn sys_warning(origin: &std::ffi::CStr, message: std::fmt::Arguments<'_>) {
    let error = errno();
    log::set_last_error(error);
    log::sys_warning_errno(error, origin, message);
}

/// Permissions outside (0, 01000) default to 0666.
const fn permissions(perms: c_int) -> c_int {
    if perms <= 0 || perms >= 0o1000 {
        0o666
    } else {
        perms
    }
}

/// Only `IPC_NOWAIT` follows `blocking`.
const fn blocking_flags(flags: c_int, blocking: bool) -> c_int {
    if blocking {
        flags & !IPC_NOWAIT
    } else {
        flags | IPC_NOWAIT
    }
}

/// `swoole::MsgQueue`.
#[repr(C)]
pub struct QueueRecord {
    blocking: bool,
    id: c_int,
    key: libc::key_t,
    flags: c_int,
    perms: c_int,
}

impl QueueRecord {
    /// `MsgQueue(key, blocking, perms)`: `msgget(key, IPC_CREAT | perms)`, warning
    /// when it fails.
    #[must_use]
    pub fn new(key: libc::key_t, blocking: bool, perms: c_int) -> Self {
        let perms = permissions(perms);
        // SAFETY: msgget has no memory preconditions.
        let id = unsafe { msgget(key, IPC_CREAT | perms) };
        if id < 0 {
            sys_warning(c"MsgQueue::MsgQueue", format_args!("msgget() failed"));
        }
        let flags = if id < 0 {
            0
        } else {
            blocking_flags(0, blocking)
        };
        Self {
            blocking,
            id,
            key,
            flags,
            perms,
        }
    }

    #[must_use]
    pub const fn ready(&self) -> bool {
        self.id >= 0
    }

    #[must_use]
    pub const fn id(&self) -> c_int {
        self.id
    }

    /// Change only `IPC_NOWAIT`; the constructor's `blocking` stays as it was.
    pub const fn set_blocking(&mut self, blocking: bool) {
        self.flags = blocking_flags(self.flags, blocking);
    }

    /// Set `msg_qbytes`; warns when `IPC_SET` fails, and is quiet when `IPC_STAT` does.
    #[expect(
        clippy::must_use_candidate,
        reason = "Process::useQueue ignores it, as upstream does"
    )]
    pub fn set_capacity(&self, bytes: usize) -> bool {
        let Some(mut stat) = queue_stat(self.id) else {
            return false;
        };
        stat.msg_qbytes = libc::c_ulong::try_from(bytes).unwrap_or(libc::c_ulong::MAX);
        // SAFETY: IPC_SET reads the `msqid_ds` IPC_STAT filled.
        if unsafe { msgctl(self.id, libc::IPC_SET, (&raw mut stat).cast()) } == 0 {
            return true;
        }
        sys_warning(
            c"MsgQueue::set_capacity",
            format_args!(
                "msgctl(msqid={}, IPC_SET, msg_qbytes={bytes}) failed",
                self.id
            ),
        );
        false
    }

    /// Send a `QueueNode`, a `long` type and then its payload, retrying `EINTR`. A
    /// failure leaves errno as the last error, and warns unless it is `EAGAIN`.
    #[must_use]
    pub fn push(&self, node: &[u8]) -> bool {
        let Some((kind, length)) = node_parts(node) else {
            invalid();
            return false;
        };
        loop {
            // SAFETY: `node` is a `long` type and `length` payload bytes, read for the call.
            if unsafe { msgsnd(self.id, node.as_ptr().cast(), length, self.flags) } == 0 {
                return true;
            }
            match errno() {
                libc::EINTR => {}
                libc::EAGAIN => break,
                _ => {
                    sys_warning(
                        c"MsgQueue::push",
                        format_args!("msgsnd({}, {length}, {kind}) failed", self.id),
                    );
                    break;
                }
            }
        }
        log::set_last_error(errno());
        false
    }

    /// Receive into a `QueueNode` (a `long` type, then room for its payload) a
    /// message of the type it holds. A failure leaves errno as the last error, and
    /// warns unless it is `ENOMSG` or `EINTR`.
    pub fn pop(&self, node: &mut [u8]) -> isize {
        let Some((kind, size)) = node_parts(node) else {
            invalid();
            return -1;
        };
        // SAFETY: `node` is a writable `long` type and `size` payload bytes.
        let result = unsafe { msgrcv(self.id, node.as_mut_ptr().cast(), size, kind, self.flags) };
        if result < 0 {
            let error = errno();
            log::set_last_error(error);
            if !matches!(error, libc::ENOMSG | libc::EINTR) {
                sys_warning(
                    c"MsgQueue::pop",
                    format_args!("msgrcv({}, {size}, {kind}) failed", self.id),
                );
            }
        }
        result
    }

    /// The queue's message count and bytes; `None` leaves errno.
    #[must_use]
    pub fn stat(&self) -> Option<(usize, usize)> {
        let stat = queue_stat(self.id)?;
        let (num, bytes) = (stat.msg_qnum, stat.__msg_cbytes);
        Some((
            usize::try_from(num).unwrap_or(usize::MAX),
            usize::try_from(bytes).unwrap_or(usize::MAX),
        ))
    }

    /// Remove the kernel queue; a failure warns and keeps the id.
    pub fn destroy(&mut self) -> bool {
        // SAFETY: IPC_RMID takes no buffer.
        if unsafe { msgctl(self.id, libc::IPC_RMID, std::ptr::null_mut()) } != 0 {
            sys_warning(
                c"MsgQueue::destroy",
                format_args!("msgctl({}, IPC_RMID) failed", self.id),
            );
            return false;
        }
        self.id = -1;
        true
    }
}

impl Drop for QueueRecord {
    /// A private queue must be destroyed.
    fn drop(&mut self) {
        if self.key == libc::IPC_PRIVATE && self.id >= 0 {
            self.destroy();
        }
    }
}

impl QueueRecord {
    /// `new MsgQueue(key)`, or `None` (after msgget's warning) when it isn't ready.
    #[must_use]
    pub fn open(key: libc::key_t) -> Option<Box<Self>> {
        let queue = Self::new(key, true, 0);
        queue.ready().then(|| Box::new(queue))
    }
}

/// A node shorter than a `long`: `EINVAL` in errno and the last error, as a failed
/// call leaves them.
fn invalid() {
    swoole_rs_corebase::sys::set_errno(libc::EINVAL);
    log::set_last_error(libc::EINVAL);
}

/// A `QueueNode`'s type and payload length, or `None` when it is shorter than a `long`.
fn node_parts(node: &[u8]) -> Option<(c_long, usize)> {
    let (kind, payload) = node.split_first_chunk::<{ size_of::<c_long>() }>()?;
    Some((c_long::from_ne_bytes(*kind), payload.len()))
}

/// `queue->push(node, length)`, which the hooks system tests link by name.
/// # Safety
/// `queue` is live; `node` is a `long` type followed by `length` payload bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_os_msg_queue_push(
    queue: *const QueueRecord,
    node: *const c_void,
    length: usize,
) -> bool {
    // SAFETY: as the caller promises.
    let queue = unsafe { &*queue };
    // SAFETY: as the caller promises.
    let node =
        unsafe { std::slice::from_raw_parts(node.cast::<u8>(), size_of::<c_long>() + length) };
    queue.push(node)
}

/// `queue->pop(node, size)`.
/// # Safety
/// `queue` is live; `node` is a writable `long` type followed by `size` payload bytes.
pub unsafe fn swr_v1_os_msg_queue_pop(
    queue: *const QueueRecord,
    node: *mut c_void,
    size: usize,
) -> isize {
    // SAFETY: as the caller promises.
    let queue = unsafe { &*queue };
    // SAFETY: as the caller promises.
    let node =
        unsafe { std::slice::from_raw_parts_mut(node.cast::<u8>(), size_of::<c_long>() + size) };
    queue.pop(node)
}

/// `struct msqid_ds`, which libc omits on Darwin.
#[cfg(target_os = "linux")]
use libc::msqid_ds as MsqidDs;
#[cfg(target_os = "macos")]
#[repr(C, packed(4))]
#[derive(Clone, Copy)]
struct MsqidDs {
    msg_perm: libc::ipc_perm,
    msg_first: i32,
    msg_last: i32,
    __msg_cbytes: libc::c_ulong,
    msg_qnum: libc::c_ulong,
    msg_qbytes: libc::c_ulong,
    msg_lspid: libc::pid_t,
    msg_lrpid: libc::pid_t,
    msg_stime: libc::time_t,
    msg_pad1: i32,
    msg_rtime: libc::time_t,
    msg_pad2: i32,
    msg_ctime: libc::time_t,
    msg_pad3: i32,
    msg_pad4: [i32; 4],
}

#[cfg(target_os = "macos")]
const _: () = assert!(size_of::<MsqidDs>() == 116, "Darwin's packed msqid_ds");

/// `msgctl(id, IPC_STAT)`.
fn queue_stat(id: c_int) -> Option<MsqidDs> {
    let mut stat = std::mem::MaybeUninit::<MsqidDs>::zeroed();
    // SAFETY: IPC_STAT fills a writable `msqid_ds`.
    let rc = unsafe { msgctl(id, libc::IPC_STAT, stat.as_mut_ptr().cast()) };
    // SAFETY: zeroed is a valid `msqid_ds`, and msgctl filled it on success.
    (rc == 0).then(|| unsafe { stat.assume_init() })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn permissions_and_flags_match_the_native_expressions() {
        for perms in -1024..2048 {
            let expected = if perms <= 0 || perms >= 0o1000 {
                0o666
            } else {
                perms
            };
            assert_eq!(permissions(perms), expected, "perms {perms}");
        }
        for flags in -128..128 {
            assert_eq!(blocking_flags(flags, true), flags & !IPC_NOWAIT);
            assert_eq!(blocking_flags(flags, false), flags | IPC_NOWAIT);
        }
    }

    /// `swoole::QueueNode`.
    #[repr(C)]
    struct QueueNode {
        mtype: c_long,
        mdata: [u8; 8192],
    }

    impl QueueNode {
        /// The node's bytes up to `length` payload bytes, as `msgsnd` reads them.
        fn bytes(&self, length: usize) -> Vec<u8> {
            let mut bytes = self.mtype.to_ne_bytes().to_vec();
            bytes.extend_from_slice(&self.mdata[..length]);
            bytes
        }

        /// Receives into the node as `pop(node, size)` did.
        fn pop(&mut self, queue: &QueueRecord, size: usize) -> isize {
            let mut bytes = self.bytes(size);
            let received = queue.pop(&mut bytes);
            let (kind, payload) = bytes.split_at(size_of::<c_long>());
            self.mtype = c_long::from_ne_bytes(kind.try_into().unwrap());
            self.mdata[..size].copy_from_slice(payload);
            received
        }
    }

    fn mdata(node: &QueueNode) -> &[u8] {
        let end = node.mdata.iter().position(|&b| b == 0).unwrap_or(0);
        &node.mdata[..end]
    }

    // Upstream's core-tests/src/os/msg_queue.cpp, TEST(msg_queue, rbac): its
    // inputs and every assertion. Like upstream's core-tests it runs on Linux:
    // Darwin's default msgmnb (2048) is below its 8192, and raising it needs root.
    #[cfg(target_os = "linux")]
    #[test]
    #[cfg_attr(miri, ignore = "Miri has no msgget")]
    fn msg_queue_rbac() {
        let mut q = QueueRecord::new(0x0095_0001, true, 0);
        assert!(q.ready());
        assert!(q.id() >= 0);
        let mut input = QueueNode {
            mtype: 999,
            mdata: [0; 8192],
        };
        input.mdata[..11].copy_from_slice(b"hello world");
        let len = mdata(&input).len();

        assert!(q.set_capacity(8192));

        // input data
        assert!(q.push(&input.bytes(len)));

        let (queue_num, queue_bytes) = q.stat().expect("stat");
        assert_eq!(queue_num, 1);
        assert!(queue_bytes > 10);

        // output data
        let mut out = QueueNode {
            mtype: 0,
            mdata: [0; 8192],
        };
        assert!(out.pop(&q, out.mdata.len()) > 1);

        let (queue_num, queue_bytes) = q.stat().expect("stat");
        assert_eq!(queue_num, 0);
        assert_eq!(queue_bytes, 0);

        assert_eq!(out.mtype, input.mtype);
        assert_eq!(mdata(&out), mdata(&input));

        assert!(q.destroy());
        assert!(!q.destroy());
        assert_eq!(log::last_error(), libc::EINVAL);

        q.set_blocking(false);

        assert_eq!(out.pop(&q, out.mdata.len()), -1);
        assert_eq!(log::last_error(), libc::EINVAL);

        assert!(!q.push(&input.bytes(len)));
        assert_eq!(log::last_error(), libc::EINVAL);

        assert!(q.stat().is_none());
        assert_eq!(log::last_error(), libc::EINVAL);

        assert!(!q.set_capacity(8192));
        assert_eq!(log::last_error(), libc::EINVAL);
    }

    #[test]
    #[cfg_attr(miri, ignore = "Miri has no msgget")]
    fn a_node_shorter_than_its_type_is_einval_in_errno_and_the_last_error() {
        use swoole_rs_corebase::sys;
        let queue = QueueRecord::new(libc::IPC_PRIVATE, false, 0);
        let mut short = [0u8; size_of::<c_long>() - 1];
        let einval = || {
            let errors = (sys::errno(), log::last_error());
            sys::set_errno(0);
            log::set_last_error(0);
            errors == (libc::EINVAL, libc::EINVAL)
        };
        assert!(!queue.push(&short), "push");
        assert!(einval(), "push's errno and last error");
        assert_eq!(queue.pop(&mut short), -1, "pop");
        assert!(einval(), "pop's errno and last error");
    }

    #[test]
    #[cfg_attr(miri, ignore = "Miri has no msgget")]
    fn a_private_queue_round_trips_reports_empty_quietly_and_is_removed_on_drop() {
        let queue = QueueRecord::new(libc::IPC_PRIVATE, false, 0);
        assert_eq!(queue.perms, 0o666, "default permissions");
        if !queue.ready() {
            return; // SysV IPC unavailable in this sandbox.
        }
        let mut sent = QueueNode {
            mtype: 3,
            mdata: [0; 8192],
        };
        sent.mdata[..7].copy_from_slice(b"payload");
        assert!(queue.push(&sent.bytes(8)), "sent");
        let mut got = QueueNode {
            mtype: 3,
            mdata: [0; 8192],
        };
        assert_eq!(got.pop(&queue, 8), 8, "received");
        assert_eq!(mdata(&got), b"payload");
        assert_eq!(got.pop(&queue, 8), -1, "ENOMSG");
        assert_eq!(log::last_error(), libc::ENOMSG);
        assert!(queue.push(&sent.bytes(8)), "sent again");
        assert_eq!(queue.stat(), Some((1, 8)), "one 8-byte message queued");
        // Lowering the limit needs no privilege.
        assert!(queue.set_capacity(1024), "capacity");
        let limit = queue_stat(queue.id).map(|stat| stat.msg_qbytes);
        assert_eq!(limit, Some(1024), "msg_qbytes");
        let id = queue.id;
        drop(queue);
        assert!(
            queue_stat(id).is_none(),
            "a private queue is removed on drop"
        );
    }
}
