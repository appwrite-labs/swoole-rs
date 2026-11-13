//! The resolver record's former native checks, against the Rust API that was under
//! them (README.md maps each): `resolver_guard.cc` and `resolver_guard.py`
//! (`corebase-dns-request-guard`), and the hooks fixture's `rust_dns_request` C++
//! witnesses. This binary defines `getaddrinfo` and `freeaddrinfo`, so the crate's
//! calls bind to the fakes below, which pass through to libc unless this thread set a
//! fake result.
use std::cell::Cell;
use std::ffi::{CStr, c_char, c_int, c_void};
use std::sync::atomic::{AtomicUsize, Ordering};

use libc::{AF_INET, AF_INET6, SOCK_STREAM, addrinfo, sockaddr_in6, sockaddr_storage};
use swoole_rs_corebase::{
    PreparedResolver, ResolverCompletion, ResolverRequest, swr_v1_core_resolver_free,
};

/// What the faked `getaddrinfo` answers on this thread.
#[derive(Clone, Copy)]
enum Fake {
    /// One `AF_INET6` entry whose address is all `0x5a` bytes.
    Filled,
    /// The guard's entry: family 12345 over a zeroed address.
    Unknown,
}

thread_local! {
    static FAKE: Cell<Option<Fake>> = const { Cell::new(None) };
    /// The entry the fake handed out, until the faked `freeaddrinfo` takes it back.
    static ENTRY: Cell<*mut addrinfo> = const { Cell::new(std::ptr::null_mut()) };
    static FREED: Cell<bool> = const { Cell::new(false) };
}

/// libc's own `name`, found once.
fn next(name: &CStr, found: &AtomicUsize) -> *mut c_void {
    let mut address = found.load(Ordering::Relaxed);
    if address == 0 {
        // SAFETY: a NUL-terminated name.
        address = unsafe { libc::dlsym(libc::RTLD_NEXT, name.as_ptr()) }.addr();
        assert_ne!(address, 0, "{name:?}");
        found.store(address, Ordering::Relaxed);
    }
    std::ptr::without_provenance_mut(address)
}

type Getaddrinfo = unsafe extern "C" fn(
    *const c_char,
    *const c_char,
    *const addrinfo,
    *mut *mut addrinfo,
) -> c_int;

/// The guard's `getaddrinfo`: this thread's fake entry, or libc's answer without one.
#[unsafe(no_mangle)]
unsafe extern "C" fn getaddrinfo(
    node: *const c_char,
    service: *const c_char,
    hints: *const addrinfo,
    result: *mut *mut addrinfo,
) -> c_int {
    static REAL: AtomicUsize = AtomicUsize::new(0);
    let Some(fake) = FAKE.try_with(Cell::get).ok().flatten() else {
        let real = next(c"getaddrinfo", &REAL);
        // SAFETY: libc's function of this name and signature, called as this one was.
        return unsafe {
            std::mem::transmute::<*mut c_void, Getaddrinfo>(real)(node, service, hints, result)
        };
    };
    // SAFETY: zero bytes are a value of both C structs.
    let (mut entry, mut address): (addrinfo, sockaddr_storage) = unsafe { std::mem::zeroed() };
    match fake {
        Fake::Filled => {
            entry.ai_family = AF_INET6;
            // SAFETY: the storage's own bytes.
            unsafe { std::ptr::write_bytes(&raw mut address, 0x5a, 1) };
        }
        Fake::Unknown => entry.ai_family = 12345,
    }
    entry.ai_addr = Box::into_raw(Box::new(address)).cast();
    let entry = Box::into_raw(Box::new(entry));
    ENTRY.set(entry);
    // SAFETY: the caller's slot for the list.
    unsafe { result.write(entry) };
    0
}

/// The guard's `freeaddrinfo`: takes back the fake entry, which must be the one handed
/// out, and passes libc's own lists on.
#[unsafe(no_mangle)]
unsafe extern "C" fn freeaddrinfo(list: *mut addrinfo) {
    static REAL: AtomicUsize = AtomicUsize::new(0);
    if FAKE.try_with(Cell::get).ok().flatten().is_none() {
        let real = next(c"freeaddrinfo", &REAL);
        // SAFETY: libc's function of this name and signature, called as this one was.
        unsafe {
            std::mem::transmute::<*mut c_void, unsafe extern "C" fn(*mut addrinfo)>(real)(list);
        }
        return;
    }
    let entry = ENTRY.replace(std::ptr::null_mut());
    assert_eq!(list, entry, "the list the fake handed out");
    // SAFETY: the two boxes the fake `getaddrinfo` leaked.
    unsafe {
        drop(Box::from_raw((*list).ai_addr.cast::<sockaddr_storage>()));
        drop(Box::from_raw(list));
    }
    FREED.set(true);
}

/// A record and the one reference `create` gave its owner, dropped with this.
struct Request(PreparedResolver);

impl Request {
    fn new(family: c_int, socktype: c_int, name: &[u8]) -> Self {
        let prepared = ResolverRequest::create(family, socktype, 0, name, b"");
        assert!(!prepared.owner.is_null(), "the record's allocation");
        Self(prepared)
    }

    fn record(&self) -> &ResolverRequest {
        // SAFETY: live until this drops.
        unsafe { &*self.0.owner }
    }

    /// `request.fields-><field> = value`, as the native views wrote the hints and results.
    fn set(&self, change: impl FnOnce(&mut swoole_rs_corebase::ResolverFields)) {
        // SAFETY: the record's fields, which nothing else reads meanwhile.
        change(unsafe { &mut *self.0.fields });
    }
}

impl Drop for Request {
    fn drop(&mut self) {
        // SAFETY: this request's own reference.
        unsafe { swr_v1_core_resolver_free(self.0.owner) };
    }
}

/// The addresses' bytes.
fn bytes(addresses: &[sockaddr_in6]) -> &[u8] {
    // SAFETY: plain C structs with no padding (28 bytes of integers and byte arrays).
    unsafe { std::slice::from_raw_parts(addresses.as_ptr().cast(), size_of_val(addresses)) }
}

/// `resolver_guard.cc`: a refused diagnostic frees the resolver's list and leaves the
/// record's error, count and addresses as they were; an accepted one publishes the
/// unknown family's zero slot.
#[test]
fn diagnostic_refusal_cleans_up_and_publishes_nothing() {
    let inaccessible = || unreachable!("a failed await reads no request error");
    let failed_await = ResolverCompletion::after_await(-1, 4003, inaccessible, 4003, 712);
    assert!(failed_await.failed && failed_await.error == 712);
    let request = Request::new(AF_INET, SOCK_STREAM, b"127.0.0.1");
    let record = request.record();
    // The guard's `resize(1)` and `memset(0x5a)`: one address of 0x5a bytes.
    FAKE.set(Some(Fake::Filled));
    assert_eq!(record.lookup(|_| unreachable!("a known family")), 0);
    let before = record.results();
    assert_eq!(before.len(), 1);
    assert!(bytes(&before).iter().all(|&byte| byte == 0x5a));
    request.set(|fields| (fields.error, fields.count) = (55, 77));
    FAKE.set(Some(Fake::Unknown));
    FREED.set(false);
    let mut calls = 0;
    let refused = record.lookup(|family| {
        assert_eq!(family, 12345);
        calls += 1;
        false
    });
    assert_eq!(refused, -3);
    assert!(FREED.get() && calls == 1);
    assert!(record.fields().error == 55 && record.fields().count == 77);
    let after = record.results();
    assert_eq!(after.len(), 1);
    assert!(bytes(&after).iter().all(|&byte| byte == 0x5a));
    FREED.set(false);
    let accepted = record.lookup(|family| {
        assert_eq!(family, 12345);
        calls += 1;
        true
    });
    assert_eq!(accepted, 0);
    assert!(FREED.get() && calls == 2);
    assert!(record.fields().error == 0 && record.fields().count == 1);
    let after = record.results();
    assert_eq!(after.len(), 1);
    assert!(bytes(&after).iter().all(|&byte| byte == 0));
    FAKE.set(None);
}

/// `rust_dns_request.numeric_host_owner_and_independent_copies`, over two records in
/// place of the C++ view's copy.
#[test]
fn numeric_host_owner_and_independent_records() {
    let host = Request::new(AF_INET, 0, b"127.0.0.1");
    assert_eq!(host.record().host(), 0);
    assert_eq!(host.record().address(), b"127.0.0.1");
    let other = Request::new(AF_INET6, 0, b"::1");
    assert_eq!(other.record().host(), 0);
    assert_eq!(other.record().address(), b"::1");
    assert_eq!(host.record().name(), b"127.0.0.1");
    assert_eq!(host.record().address(), b"127.0.0.1");
    assert_eq!(other.record().name(), b"::1");
    assert_eq!(other.record().fields().family, AF_INET6);
}

/// The record's part of `rust_dns_request.native_output_append_exception_is_contained`:
/// an `AF_INET6` result read back as its text, offline.
#[test]
fn an_ipv6_result_reads_back_as_its_text() {
    let request = Request::new(AF_INET6, SOCK_STREAM, b"::1");
    let record = request.record();
    FAKE.set(Some(Fake::Filled));
    assert_eq!(record.lookup(|_| unreachable!("a known family")), 0);
    FAKE.set(None);
    assert_eq!(
        record.addresses(),
        [b"5a5a:5a5a:5a5a:5a5a:5a5a:5a5a:5a5a:5a5a".to_vec()]
    );
}

/// `rust_dns_request.hints_vectors_and_failure_publication`: a failed lookup publishes
/// its error and keeps the previous count and addresses.
#[test]
fn hints_vectors_and_failure_publication() {
    let request = Request::new(AF_INET, SOCK_STREAM, b"127.0.0.1");
    let record = request.record();
    assert_eq!(record.fields().count, 0);
    assert_eq!(record.fields().error, 0);
    assert_eq!(record.lookup(|_| true), 0);
    assert!(record.fields().count > 0);
    let count = usize::try_from(record.fields().count).unwrap();
    assert_eq!(record.results().len(), count);
    let saved_count = record.fields().count;
    let saved = record.results();
    assert_eq!(record.addresses()[0], b"127.0.0.1");
    request.set(|fields| fields.family = -123);
    assert_eq!(record.lookup(|_| true), -1);
    assert_ne!(record.fields().error, 0);
    assert_eq!(record.fields().count, saved_count);
    assert_eq!(record.results().len(), saved.len());
    assert_eq!(bytes(&record.results()), bytes(&saved));
}
