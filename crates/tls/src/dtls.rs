//! DTLS datagram ownership and BIO observations, independent of native `SSLContext` lifetime.
use core::cell::UnsafeCell;
use core::ffi::{c_int, c_long, c_void};
use core::ptr;
use std::collections::VecDeque;
use std::sync::atomic::{AtomicPtr, Ordering};
use swoole_rs_reactor::SocketRecord;

#[repr(C)]
pub struct DtlsFields {
    pub socket: *mut SocketRecord,
    pub listened: bool,
    pub peek: bool,
}

#[repr(C)]
struct Buffer {
    length: u16,
    data: [u8; 0],
}

struct Datagram(*mut Buffer);
impl Drop for Datagram {
    fn drop(&mut self) {
        // SAFETY: each queued datagram owns one sw_malloc/libc-compatible allocation.
        unsafe { libc::free(self.0.cast()) };
    }
}

pub struct DtlsSession {
    fields: UnsafeCell<DtlsFields>,
    queue: VecDeque<Datagram>,
}

#[repr(C)]
pub struct PreparedDtlsSession {
    pub owner: *mut DtlsSession,
    pub fields: *mut DtlsFields,
}

#[repr(C)]
pub struct DtlsReadView {
    pub length: u16,
    pub empty: bool,
    pub peek: bool,
}

#[unsafe(no_mangle)]
pub extern "C" fn swr_v1_tls_dtls_new(socket: *mut SocketRecord) -> PreparedDtlsSession {
    let owner = Box::into_raw(Box::new(DtlsSession {
        fields: UnsafeCell::new(DtlsFields {
            socket,
            listened: false,
            peek: false,
        }),
        queue: VecDeque::new(),
    }));
    // SAFETY: the fresh Box remains live until dtls_free; the inline slot has a stable address.
    let fields = unsafe { (*owner).fields.get() };
    PreparedDtlsSession { owner, fields }
}

/// # Safety
/// owner is one live handle; native holds no queue borrow while it clears.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_tls_dtls_clear(owner: *mut DtlsSession) {
    // SAFETY: the queue is exclusively accessed for this call.
    let queue = unsafe { &mut (*owner).queue };
    queue.clear();
}

/// # Safety
/// owner came from `dtls_new` and is returned once after native references expire.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_tls_dtls_free(owner: *mut DtlsSession) {
    if !owner.is_null() {
        // SAFETY: native returns its unique allocation after clearing the queue.
        unsafe { drop(Box::from_raw(owner)) };
    }
}

/// # Safety
/// owner is live; buffer is an exclusively transferred sw_malloc-compatible datagram.
/// On allocation failure ownership remains with the caller.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_tls_dtls_append_buffer(
    owner: *mut DtlsSession,
    buffer: *mut c_void,
) -> c_int {
    // SAFETY: the queue is exclusively accessed until the append returns.
    let queue = unsafe { &mut (*owner).queue };
    if queue.try_reserve(1).is_err() {
        return -1;
    }
    queue.push_back(Datagram(buffer.cast()));
    0
}

/// # Safety
/// owner is live; data covers length initialized bytes, with nonnegative native length.
#[unsafe(no_mangle)]
#[expect(
    clippy::cast_possible_truncation,
    reason = "the public native Buffer length is uint16_t"
)]
pub unsafe extern "C" fn swr_v1_tls_dtls_append(
    owner: *mut DtlsSession,
    data: *const u8,
    length: usize,
) -> c_int {
    // SAFETY: this append exclusively accesses the queue.
    let queue = unsafe { &mut (*owner).queue };
    if queue.try_reserve(1).is_err() {
        return -1;
    }
    let Some(size) = core::mem::size_of::<Buffer>().checked_add(length) else {
        return -1;
    };
    // SAFETY: malloc accepts any size and returns null on failure.
    let buffer = unsafe { libc::malloc(size) }.cast::<Buffer>();
    if buffer.is_null() {
        return -1;
    }
    let length = length as u16;
    // SAFETY: malloc provided at least the Buffer header plus length payload bytes.
    unsafe { (*buffer).length = length };
    // SAFETY: the fresh header is valid and its payload immediately follows it.
    let out = unsafe { ptr::addr_of_mut!((*buffer).data).cast::<u8>() };
    if length > 0 {
        // SAFETY: the caller lends the initialized source; fresh storage cannot overlap it.
        unsafe { ptr::copy_nonoverlapping(data, out, usize::from(length)) };
    }
    queue.push_back(Datagram(buffer));
    0
}

/// # Safety
/// owner is live and the native caller does not mutate it during this observation.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_tls_dtls_view(owner: *const DtlsSession) -> DtlsReadView {
    // SAFETY: the queue is read only for this call.
    let queue = unsafe { &(*owner).queue };
    // SAFETY: its inline native scalar group is live.
    let fields = unsafe { (*owner).fields.get() };
    // SAFETY: no native scalar mutation overlaps this call.
    let peek = unsafe { (*fields).peek };
    let length = queue.front().map_or(0, |front| {
        // SAFETY: every queued pointer owns an initialized Buffer.
        unsafe { (*front.0).length }
    });
    DtlsReadView {
        length,
        empty: queue.is_empty(),
        peek,
    }
}

/// # Safety
/// owner is live and data covers requested writable bytes for a nonnegative request.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_tls_dtls_read(
    owner: *mut DtlsSession,
    data: *mut u8,
    requested: c_int,
) -> crate::ReadPlan {
    // SAFETY: this operation exclusively accesses the live queue.
    let queue = unsafe { &mut (*owner).queue };
    let Some(front) = queue.front() else {
        return crate::ReadPlan {
            length: -1,
            consume: 0,
            retry: 1,
        };
    };
    // SAFETY: the native scalar group is inline in this live handle.
    let fields = unsafe { (*owner).fields.get() };
    // SAFETY: native does not mutate its scalar group during this operation.
    let peek = unsafe { (*fields).peek };
    // SAFETY: each queued pointer owns an initialized Buffer header.
    let length = unsafe { (*front.0).length };
    let plan = crate::read_plan(0, length, requested, u8::from(peek));
    if let Ok(length) = usize::try_from(plan.length) {
        // SAFETY: the initialized payload begins immediately after its header.
        let source = unsafe { ptr::addr_of!((*front.0).data).cast::<u8>() };
        if length > 0 {
            // SAFETY: the requested output is writable and the owned front covers length bytes; memmove permits overlap.
            unsafe { ptr::copy(source, data, length) };
        }
        if plan.consume != 0 {
            queue.pop_front();
        }
    }
    plan
}

/// # Safety
/// owner is live and no queue mutation overlaps this observation.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_tls_dtls_length(owner: *const DtlsSession) -> usize {
    // SAFETY: read-only access to the live queue.
    let queue = unsafe { &(*owner).queue };
    queue.iter().fold(0usize, |total, buffer| {
        // SAFETY: each queued pointer owns an initialized Buffer.
        total.wrapping_add(usize::from(unsafe { (*buffer.0).length }))
    })
}

/// Native decodes its actual OpenSSL macro values into the existing stable tags.
/// Shutdown and diagnostic effects are returned to native, outside this Rust frame.
/// # Safety
/// owner is live; ptrval is null or writable for one address union for the peer command.
#[unsafe(no_mangle)]
#[expect(
    clippy::cast_possible_wrap,
    reason = "native BIO_CTRL_PENDING casts wrapping size_t total to long"
)]
pub unsafe extern "C" fn swr_v1_tls_dtls_control(
    owner: *mut DtlsSession,
    tag: u32,
    observed: c_long,
    value: c_long,
    ptrval: *mut c_void,
) -> crate::ControlPlan {
    // SAFETY: inline native scalar storage remains live for this call.
    let fields = unsafe { (*owner).fields.get() };
    let observed = match tag {
        0 => {
            // SAFETY: the queue is only observed during this call.
            c_long::from(unsafe { (*owner).queue.is_empty() })
        }
        2 => {
            // SAFETY: read-only observation of this live queue.
            (unsafe { swr_v1_tls_dtls_length(owner) }) as c_long
        }
        _ => observed,
    };
    if tag == 8 {
        // SAFETY: native lends its scalar group exclusively during this call.
        unsafe { (*fields).peek = value != 0 };
        return crate::ControlPlan {
            result: 1,
            effect: 0,
        };
    }
    if tag == 5 {
        if !ptrval.is_null() {
            // SAFETY: the enclosing native Session retains its network socket.
            let socket = unsafe { (*fields).socket };
            // SAFETY: its initialized info address union is live for this call.
            let source = unsafe { ptr::addr_of!((*socket).info.addr).cast::<u8>() };
            // SAFETY: native's peer destination is writable for the exact native address union.
            unsafe {
                ptr::copy_nonoverlapping(
                    source,
                    ptrval.cast::<u8>(),
                    core::mem::size_of::<swoole_rs_reactor::AddressStorage>(),
                );
            };
        }
        return crate::ControlPlan {
            result: 1,
            effect: 0,
        };
    }
    crate::control_plan(tag, observed)
}

/// # Safety
/// owner is live and its scalar group is exclusively accessed for this completion.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_tls_dtls_listen(owner: *mut DtlsSession, result: c_int) -> u32 {
    let action = crate::listen_plan(result);
    if action == 2 {
        // SAFETY: inline native scalar storage remains live.
        let fields = unsafe { (*owner).fields.get() };
        // SAFETY: the caller lends this flag exclusively for publication.
        unsafe { (*fields).listened = true };
    }
    action
}

static METHODS: AtomicPtr<openssl_sys::BIO_METHOD> = AtomicPtr::new(ptr::null_mut());
#[unsafe(no_mangle)]
pub extern "C" fn swr_v1_tls_dtls_methods() -> *mut openssl_sys::BIO_METHOD {
    METHODS.load(Ordering::Acquire)
}
/// Publish a fully initialized native table, returning the winning table; native frees any loser.
#[unsafe(no_mangle)]
pub extern "C" fn swr_v1_tls_dtls_methods_publish(
    candidate: *mut openssl_sys::BIO_METHOD,
) -> *mut openssl_sys::BIO_METHOD {
    match METHODS.compare_exchange(
        ptr::null_mut(),
        candidate,
        Ordering::AcqRel,
        Ordering::Acquire,
    ) {
        Ok(_) => candidate,
        Err(current) => current,
    }
}
/// Native frees the detached table after Rust returns, with no active BIO users.
#[unsafe(no_mangle)]
pub extern "C" fn swr_v1_tls_dtls_methods_take() -> *mut openssl_sys::BIO_METHOD {
    METHODS.swap(ptr::null_mut(), Ordering::AcqRel)
}

/// # Safety
/// Both handles are live; only destination mutates until this value copy returns.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_tls_dtls_assign(
    destination: *mut DtlsSession,
    source: *const DtlsSession,
) -> c_int {
    if ptr::eq(destination, source) {
        return 0;
    }
    // SAFETY: the source queue is read only while independent datagrams are prepared.
    let source_queue = unsafe { &(*source).queue };
    let mut copied = VecDeque::new();
    if copied.try_reserve(source_queue.len()).is_err() {
        return -1;
    }
    for buffer in source_queue {
        // SAFETY: each source entry owns one initialized native Buffer.
        let length = usize::from(unsafe { (*buffer.0).length });
        // SAFETY: malloc accepts a header plus this initialized datagram length.
        let allocation =
            unsafe { libc::malloc(core::mem::size_of::<Buffer>() + length) }.cast::<Buffer>();
        if allocation.is_null() {
            return -1;
        }
        // SAFETY: the complete source header/payload and fresh destination cover this span, and do not overlap.
        unsafe {
            ptr::copy_nonoverlapping(
                buffer.0.cast::<u8>(),
                allocation.cast::<u8>(),
                core::mem::size_of::<Buffer>() + length,
            );
        };
        copied.push_back(Datagram(allocation));
    }
    // SAFETY: native lends the destination queue exclusively for commit.
    unsafe { (*destination).queue = copied };
    // SAFETY: both inline field slots remain stable for their enclosing handles.
    let source_fields = unsafe { (*source).fields.get() };
    // SAFETY: as above.
    let destination_fields = unsafe { (*destination).fields.get() };
    // SAFETY: the source's scalar values are observed only while native is not mutating them.
    let socket = unsafe { (*source_fields).socket };
    // SAFETY: as above.
    let listened = unsafe { (*source_fields).listened };
    // SAFETY: as above.
    let peek = unsafe { (*source_fields).peek };
    // SAFETY: native lends the destination group exclusively while retaining its address.
    unsafe {
        *destination_fields = DtlsFields {
            socket,
            listened,
            peek,
        }
    };
    0
}

#[cfg(target_pointer_width = "64")]
const _: () = {
    assert!(
        core::mem::size_of::<DtlsFields>() == 16
            && core::mem::offset_of!(DtlsFields, listened) == 8
            && core::mem::offset_of!(DtlsFields, peek) == 9,
        "native Session aliases require the exact canonical DTLS field layout"
    );
    assert!(
        core::mem::size_of::<Buffer>() == 2,
        "native datagram header is one uint16_t"
    );
};

#[cfg(test)]
mod tests {
    use super::*;
    struct Owner(PreparedDtlsSession);
    impl Owner {
        fn new(socket: *mut SocketRecord) -> Self {
            Self(swr_v1_tls_dtls_new(socket))
        }
        fn handle(&self) -> *mut DtlsSession {
            self.0.owner
        }
        fn append(&self, bytes: &[u8]) {
            assert_eq!(
                unsafe { swr_v1_tls_dtls_append(self.handle(), bytes.as_ptr(), bytes.len()) },
                0
            );
        }
        fn read(&self, count: usize) -> (crate::ReadPlan, Vec<u8>) {
            let mut bytes = vec![0x55; count];
            let result = unsafe {
                swr_v1_tls_dtls_read(self.handle(), bytes.as_mut_ptr(), count.try_into().unwrap())
            };
            (result, bytes)
        }
        fn length(&self) -> usize {
            unsafe { swr_v1_tls_dtls_length(self.handle()) }
        }
    }
    impl Drop for Owner {
        fn drop(&mut self) {
            unsafe { swr_v1_tls_dtls_free(self.handle()) };
        }
    }

    #[test]
    fn method_publication_returns_the_adopted_table_and_detaches_it_once() {
        let first = unsafe { openssl_sys::BIO_meth_new(0x480, c"owned first".as_ptr()) };
        let second = unsafe { openssl_sys::BIO_meth_new(0x481, c"owned second".as_ptr()) };
        assert!(!first.is_null() && !second.is_null());
        assert!(swr_v1_tls_dtls_methods().is_null());
        assert_eq!(swr_v1_tls_dtls_methods_publish(first), first);
        assert_eq!(swr_v1_tls_dtls_methods_publish(second), first);
        unsafe { openssl_sys::BIO_meth_free(second) };
        assert_eq!(swr_v1_tls_dtls_methods_take(), first);
        assert!(swr_v1_tls_dtls_methods_take().is_null());
        unsafe { openssl_sys::BIO_meth_free(first) };
    }

    #[test]
    fn whole_datagrams_peek_short_zero_read_and_retry_use_actual_queue_storage() {
        let owner = Owner::new(ptr::null_mut());
        owner.append(b"abcdef");
        owner.append(b"xyz");
        assert_eq!(owner.length(), 9);
        unsafe { (*owner.0.fields).peek = true };
        let (plan, bytes) = owner.read(2);
        assert_eq!((plan.length, plan.consume, plan.retry), (2, 0, 0));
        assert_eq!(bytes, b"ab");
        assert_eq!(owner.length(), 9);
        unsafe { (*owner.0.fields).peek = false };
        assert_eq!(owner.read(2).1, b"ab");
        assert_eq!(owner.length(), 3);
        let (plan, bytes) = owner.read(10);
        assert_eq!(plan.length, 3);
        assert_eq!(&bytes[..3], b"xyz");
        assert_eq!(owner.length(), 0);
        let (plan, _) = owner.read(1);
        assert_eq!((plan.length, plan.retry), (-1, 1));
        owner.append(b"");
        assert!(!unsafe { swr_v1_tls_dtls_view(owner.handle()) }.empty);
        assert_eq!(owner.read(0).0.consume, 1);
        assert!(unsafe { swr_v1_tls_dtls_view(owner.handle()) }.empty);
    }

    #[test]
    fn taken_native_datagram_and_scalar_aliases_have_one_canonical_owner() {
        let mut socket = SocketRecord::new();
        let owner = Owner::new(&raw mut socket);
        let raw = unsafe { libc::malloc(5) }.cast::<Buffer>();
        assert!(!raw.is_null());
        unsafe { (*raw).length = 3 };
        let data = unsafe { ptr::addr_of_mut!((*raw).data).cast::<u8>() };
        unsafe { ptr::copy_nonoverlapping(b"abc".as_ptr(), data, 3) };
        assert_eq!(
            unsafe { swr_v1_tls_dtls_append_buffer(owner.handle(), raw.cast()) },
            0
        );
        assert_eq!(owner.read(3).1, b"abc");
        assert_eq!(unsafe { swr_v1_tls_dtls_listen(owner.handle(), 0) }, 1);
        assert!(!unsafe { (*owner.0.fields).listened });
        assert_eq!(unsafe { swr_v1_tls_dtls_listen(owner.handle(), 1) }, 2);
        assert!(unsafe { (*owner.0.fields).listened });
        unsafe { swr_v1_tls_dtls_control(owner.handle(), 8, 0, -7, ptr::null_mut()) };
        assert!(unsafe { (*owner.0.fields).peek });
        owner.append(b"abc");
        assert_eq!(
            unsafe { swr_v1_tls_dtls_control(owner.handle(), 2, 0, 0, ptr::null_mut()) }.result,
            3
        );
        unsafe { swr_v1_tls_dtls_clear(owner.handle()) };
        assert_eq!(
            unsafe { swr_v1_tls_dtls_control(owner.handle(), 0, 0, 0, ptr::null_mut()) }.result,
            1
        );
    }

    #[test]
    fn value_copies_and_assignment_keep_field_addresses_and_independent_queue_storage() {
        let source = Owner::new(ptr::null_mut());
        source.append(b"first");
        source.append(b"second");
        unsafe { (*source.0.fields).peek = true };
        let copied = Owner::new(ptr::null_mut());
        let fields = copied.0.fields;
        assert_eq!(
            unsafe { swr_v1_tls_dtls_assign(copied.handle(), source.handle()) },
            0
        );
        assert_eq!(copied.0.fields, fields);
        assert_eq!(
            unsafe { swr_v1_tls_dtls_assign(copied.handle(), copied.handle()) },
            0
        );
        unsafe { swr_v1_tls_dtls_clear(source.handle()) };
        assert_eq!(copied.length(), 11);
        assert_eq!(copied.read(5).1, b"first");
        assert_eq!(copied.length(), 11);
        unsafe { (*copied.0.fields).peek = false };
        assert_eq!(copied.read(5).1, b"first");
        assert_eq!(copied.read(6).1, b"second");
        assert_eq!(source.length(), 0);
    }
}
