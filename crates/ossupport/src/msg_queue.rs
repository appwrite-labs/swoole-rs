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

    /// Send a `QueueNode` with `length` payload bytes, retrying `EINTR`. A failure
    /// leaves errno as the last error, and warns unless it is `EAGAIN`.
    /// # Safety
    /// `node` points to a `long` type followed by `length` payload bytes.
    #[must_use]
    pub unsafe fn push(&self, node: *const c_void, length: usize) -> bool {
        loop {
            // SAFETY: as the caller promises.
            if unsafe { msgsnd(self.id, node, length, self.flags) } == 0 {
                return true;
            }
            match errno() {
                libc::EINTR => {}
                libc::EAGAIN => break,
                _ => {
                    // SAFETY: the node begins with its `long` type.
                    let kind = unsafe { node.cast::<c_long>().read_unaligned() };
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

    /// Receive into a `QueueNode` a message of the type it holds, up to `size`
    /// payload bytes. A failure leaves errno as the last error, and warns unless
    /// it is `ENOMSG` or `EINTR`.
    /// # Safety
    /// `node` points to a writable `long` type followed by `size` payload bytes.
    pub unsafe fn pop(&self, node: *mut c_void, size: usize) -> isize {
        // SAFETY: the node begins with its `long` type.
        let kind = unsafe { node.cast::<c_long>().read_unaligned() };
        // SAFETY: as the caller promises.
        let result = unsafe { msgrcv(self.id, node, size, kind, self.flags) };
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

/// `queue->push(node, length)`, which the hooks system tests link by name.
/// # Safety
/// `queue` is live; `node` as [`QueueRecord::push`] requires.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_os_msg_queue_push(
    queue: *const QueueRecord,
    node: *const c_void,
    length: usize,
) -> bool {
    // SAFETY: as the caller promises.
    let queue = unsafe { &*queue };
    // SAFETY: as the caller promises.
    unsafe { queue.push(node, length) }
}

/// `queue->pop(node, size)`.
/// # Safety
/// `queue` is live; `node` as [`QueueRecord::pop`] requires.
pub unsafe fn swr_v1_os_msg_queue_pop(
    queue: *const QueueRecord,
    node: *mut c_void,
    size: usize,
) -> isize {
    // SAFETY: as the caller promises.
    let queue = unsafe { &*queue };
    // SAFETY: as the caller promises.
    unsafe { queue.pop(node, size) }
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
        fn ptr(&mut self) -> *mut c_void {
            (&raw mut *self).cast()
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
        assert!(unsafe { q.push(input.ptr(), len) });

        let (queue_num, queue_bytes) = q.stat().expect("stat");
        assert_eq!(queue_num, 1);
        assert!(queue_bytes > 10);

        // output data
        let mut out = QueueNode {
            mtype: 0,
            mdata: [0; 8192],
        };
        assert!(unsafe { q.pop(out.ptr(), out.mdata.len()) } > 1);

        let (queue_num, queue_bytes) = q.stat().expect("stat");
        assert_eq!(queue_num, 0);
        assert_eq!(queue_bytes, 0);

        assert_eq!(out.mtype, input.mtype);
        assert_eq!(mdata(&out), mdata(&input));

        assert!(q.destroy());
        assert!(!q.destroy());
        assert_eq!(log::last_error(), libc::EINVAL);

        q.set_blocking(false);

        assert_eq!(unsafe { q.pop(out.ptr(), out.mdata.len()) }, -1);
        assert_eq!(log::last_error(), libc::EINVAL);

        assert!(!unsafe { q.push(input.ptr(), len) });
        assert_eq!(log::last_error(), libc::EINVAL);

        assert!(q.stat().is_none());
        assert_eq!(log::last_error(), libc::EINVAL);

        assert!(!q.set_capacity(8192));
        assert_eq!(log::last_error(), libc::EINVAL);
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
        assert!(unsafe { queue.push(sent.ptr(), 8) }, "sent");
        let mut got = QueueNode {
            mtype: 3,
            mdata: [0; 8192],
        };
        assert_eq!(unsafe { queue.pop(got.ptr(), 8) }, 8, "received");
        assert_eq!(mdata(&got), b"payload");
        assert_eq!(unsafe { queue.pop(got.ptr(), 8) }, -1, "ENOMSG");
        assert_eq!(log::last_error(), libc::ENOMSG);
        assert!(unsafe { queue.push(sent.ptr(), 8) }, "sent again");
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
