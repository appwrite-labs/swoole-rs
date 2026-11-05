//! `swoole::MsgQueue` (upstream `src/os/msg_queue.cc`): open, blocking flag,
//! push, pop, stat and capacity over a `SysV` message queue. Rust defines and
//! initializes the queue record; the inline methods in `swoole_msg_queue.h` keep
//! the logs; each call returns right after a failing syscall with its errno intact.
use libc::{c_int, c_long, c_void};

// The XSI message queue calls, declared here because libc omits them on Darwin.
unsafe extern "C" {
    fn msgget(key: libc::key_t, flags: c_int) -> c_int;
    fn msgsnd(id: c_int, msg: *const c_void, size: usize, flags: c_int) -> c_int;
    fn msgrcv(id: c_int, msg: *mut c_void, size: usize, kind: c_long, flags: c_int) -> isize;
    fn msgctl(id: c_int, cmd: c_int, buf: *mut c_void) -> c_int;
}

use libc::{IPC_CREAT, IPC_NOWAIT};

fn errno() -> c_int {
    std::io::Error::last_os_error().raw_os_error().unwrap_or(0)
}

/// Permissions outside (0, 01000) default to 0666.
const fn permissions(perms: c_int) -> c_int {
    if perms <= 0 || perms >= 0o1000 {
        0o666
    } else {
        perms
    }
}

#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MsgOpen {
    pub id: c_int,
    pub perms: c_int,
}

/// `msgget(key, IPC_CREAT | perms)` with the defaulted permissions.
#[unsafe(no_mangle)]
pub extern "C" fn swr_v1_os_msg_open(key: libc::key_t, perms: c_int) -> MsgOpen {
    let perms = permissions(perms);
    // SAFETY: msgget has no memory preconditions.
    let id = unsafe { msgget(key, IPC_CREAT | perms) };
    MsgOpen { id, perms }
}

/// `MsgQueue::set_blocking`: only `IPC_NOWAIT` changes.
#[unsafe(no_mangle)]
pub extern "C" fn swr_v1_os_msg_flags(flags: c_int, blocking: bool) -> c_int {
    if blocking {
        flags & !IPC_NOWAIT
    } else {
        flags | IPC_NOWAIT
    }
}

/// `MsgQueue::push`: retries `EINTR`. 0 sent, 1 failed with `EAGAIN` (quiet),
/// 2 failed (native warns).
/// # Safety
/// `msg` points to a message of `length` payload bytes after its `long` type.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_os_msg_push(
    id: c_int,
    msg: *const c_void,
    length: usize,
    flags: c_int,
) -> c_int {
    loop {
        // SAFETY: `msg` is a valid message per the contract.
        if unsafe { msgsnd(id, msg, length, flags) } == 0 {
            return 0;
        }
        match errno() {
            libc::EINTR => {}
            libc::EAGAIN => return 1,
            _ => return 2,
        }
    }
}

/// `MsgQueue::destroy`: `msgctl(id, IPC_RMID)`; false leaves errno for the warning.
#[unsafe(no_mangle)]
pub extern "C" fn swr_v1_os_msg_remove(id: c_int) -> bool {
    // SAFETY: IPC_RMID takes no buffer.
    unsafe { msgctl(id, libc::IPC_RMID, std::ptr::null_mut()) == 0 }
}

#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MsgPop {
    pub result: isize,
    pub warn: bool,
}

/// `MsgQueue::pop`: one `msgrcv`; failures other than `ENOMSG`/`EINTR` warn.
/// # Safety
/// `msg` points to writable room for a `long` type and `size` payload bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_os_msg_pop(
    id: c_int,
    msg: *mut c_void,
    size: usize,
    kind: c_long,
    flags: c_int,
) -> MsgPop {
    // SAFETY: `msg` is writable per the contract.
    let result = unsafe { msgrcv(id, msg, size, kind, flags) };
    MsgPop {
        result,
        warn: result < 0 && !matches!(errno(), libc::ENOMSG | libc::EINTR),
    }
}

/// Canonical `MsgQueue` storage. Native owns the enclosing object's lifetime and
/// borrows this POD record; ordinary C++ value copies preserve the original
/// queue-ID copy behavior without introducing allocation or shared ownership.
#[repr(C)]
pub struct QueueRecord {
    pub blocking: bool,
    pub id: c_int,
    pub key: libc::key_t,
    pub flags: c_int,
    pub perms: c_int,
}

/// Initialize a queue and publish its exact opened ID, flags and permissions.
/// # Safety
/// `queue` is aligned writable storage for a new `QueueRecord`, not concurrently used.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_os_msg_queue_init(
    queue: *mut QueueRecord,
    key: libc::key_t,
    blocking: bool,
    perms: c_int,
) -> bool {
    let opened = swr_v1_os_msg_open(key, perms);
    let flags = if opened.id < 0 {
        0
    } else {
        swr_v1_os_msg_flags(0, blocking)
    };
    let record = QueueRecord {
        blocking,
        id: opened.id,
        key,
        flags,
        perms: opened.perms,
    };
    // SAFETY: initialization owns this new native object's entire canonical storage.
    unsafe { queue.write(record) };
    opened.id >= 0
}

/// Change only `IPC_NOWAIT`, as the original method did; the constructor's blocking
/// field is deliberately not rewritten by later flag changes.
/// # Safety
/// `queue` is live and exclusively loaned for this configuration update.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_os_msg_queue_blocking(queue: *mut QueueRecord, blocking: bool) {
    // SAFETY: the caller serializes this live queue's flag mutation.
    let queue = unsafe { &mut *queue };
    queue.flags = swr_v1_os_msg_flags(queue.flags, blocking);
}

/// Destroy the kernel queue; a failed removal preserves its ID and errno.
/// # Safety
/// `queue` is live and exclusively loaned for removal.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_os_msg_queue_remove(queue: *mut QueueRecord) -> bool {
    // SAFETY: the caller exclusively lends its live queue record for this leaf.
    let queue = unsafe { &mut *queue };
    if !swr_v1_os_msg_remove(queue.id) {
        return false;
    }
    queue.id = -1;
    true
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

/// `MsgQueue::stat`: the queue's message count and bytes; false leaves errno.
/// # Safety
/// `queue` is live; `num` and `bytes` are writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_os_msg_queue_stat(
    queue: *const QueueRecord,
    num: *mut usize,
    bytes: *mut usize,
) -> bool {
    // SAFETY: a live queue record.
    let Some(stat) = queue_stat(unsafe { (*queue).id }) else {
        return false;
    };
    // SAFETY: writable per the contract.
    unsafe { num.write(usize::try_from(stat.msg_qnum).unwrap_or(usize::MAX)) };
    // SAFETY: as above.
    unsafe { bytes.write(usize::try_from(stat.__msg_cbytes).unwrap_or(usize::MAX)) };
    true
}

/// `MsgQueue::set_capacity`: 0 set, 1 `IPC_STAT` failed (quiet), 2 `IPC_SET`
/// failed (native warns); errno intact.
/// # Safety
/// `queue` is live.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_os_msg_queue_set_capacity(
    queue: *const QueueRecord,
    bytes: usize,
) -> c_int {
    // SAFETY: a live queue record.
    let id = unsafe { (*queue).id };
    let Some(mut stat) = queue_stat(id) else {
        return 1;
    };
    stat.msg_qbytes = libc::c_ulong::try_from(bytes).unwrap_or(libc::c_ulong::MAX);
    // SAFETY: IPC_SET reads the `msqid_ds` IPC_STAT filled.
    if unsafe { msgctl(id, libc::IPC_SET, (&raw mut stat).cast()) } == 0 {
        0
    } else {
        2
    }
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
            assert_eq!(
                swr_v1_os_msg_flags(flags, true),
                flags & !IPC_NOWAIT,
                "blocking"
            );
            assert_eq!(
                swr_v1_os_msg_flags(flags, false),
                flags | IPC_NOWAIT,
                "nonblocking"
            );
        }
    }

    #[repr(C)]
    struct Message {
        kind: c_long,
        data: [u8; 8],
    }

    #[test]
    fn a_private_queue_round_trips_and_reports_empty_quietly() {
        let queue = swr_v1_os_msg_open(libc::IPC_PRIVATE, 0);
        assert_eq!(queue.perms, 0o666, "default permissions");
        if queue.id < 0 {
            return; // SysV IPC unavailable in this sandbox.
        }
        let sent = Message {
            kind: 3,
            data: *b"payload\0",
        };
        let flags = swr_v1_os_msg_flags(0, false);
        assert_eq!(
            unsafe { swr_v1_os_msg_push(queue.id, (&raw const sent).cast(), 8, flags) },
            0,
            "sent"
        );
        let mut got = Message {
            kind: 3,
            data: [0; 8],
        };
        let pop = unsafe { swr_v1_os_msg_pop(queue.id, (&raw mut got).cast(), 8, 3, flags) };
        assert_eq!(
            (pop, got.data),
            (
                MsgPop {
                    result: 8,
                    warn: false
                },
                *b"payload\0"
            ),
            "received"
        );
        let pop = unsafe { swr_v1_os_msg_pop(queue.id, (&raw mut got).cast(), 8, 3, flags) };
        assert_eq!(
            pop,
            MsgPop {
                result: -1,
                warn: false
            },
            "ENOMSG is quiet"
        );
        let record = QueueRecord {
            blocking: false,
            id: queue.id,
            key: libc::IPC_PRIVATE,
            flags,
            perms: queue.perms,
        };
        assert_eq!(
            unsafe { swr_v1_os_msg_push(queue.id, (&raw const sent).cast(), 8, flags) },
            0,
            "sent again"
        );
        let (mut num, mut bytes) = (0, 0);
        assert!(
            unsafe { swr_v1_os_msg_queue_stat(&raw const record, &raw mut num, &raw mut bytes) },
            "stat"
        );
        assert_eq!((num, bytes), (1, 8), "one 8-byte message queued");
        // Lowering the limit needs no privilege.
        assert_eq!(
            unsafe { swr_v1_os_msg_queue_set_capacity(&raw const record, 1024) },
            0,
            "capacity"
        );
        let limit = queue_stat(queue.id).map(|stat| stat.msg_qbytes);
        assert_eq!(limit, Some(1024), "msg_qbytes");
        assert_eq!(
            unsafe { swr_v1_os_msg_pop(queue.id, (&raw mut got).cast(), 8, 3, flags) }.result,
            8,
            "drained"
        );
        assert!(swr_v1_os_msg_remove(queue.id), "removed");
        assert!(!swr_v1_os_msg_remove(queue.id), "already removed");
        let pop = unsafe { swr_v1_os_msg_pop(queue.id, (&raw mut got).cast(), 8, 3, flags) };
        assert_eq!(
            pop,
            MsgPop {
                result: -1,
                warn: true
            },
            "a removed queue warns"
        );
        let (mut num, mut bytes) = (0, 0);
        assert!(
            !unsafe { swr_v1_os_msg_queue_stat(&raw const record, &raw mut num, &raw mut bytes) },
            "a removed queue has no stat"
        );
        assert_eq!(
            unsafe { swr_v1_os_msg_queue_set_capacity(&raw const record, 1024) },
            1,
            "nor a capacity"
        );
    }
}
