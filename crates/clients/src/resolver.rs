// SPDX-License-Identifier: Apache-2.0
// Compatibility derived from Swoole v6.2.4 src/network/dns.cc and src/os/async_thread.cc
// (Swoole contributors).
//! A resolver request record (crates/corebase) as a Rust owner: the pool handlers'
//! bodies, the completion check and the blocking `network::gethostbyname`.
use core::ffi::{CStr, c_char, c_int, c_void};
use std::ffi::CString;
use swoole_rs_corebase::{
    ResolverRequest, log, swr_v1_clients_gethostbyname, swr_v1_core_resolver_create,
    swr_v1_core_resolver_finish, swr_v1_core_resolver_free, swr_v1_core_resolver_host,
    swr_v1_core_resolver_lookup, swr_v1_core_resolver_next, swr_v1_core_resolver_view,
    swr_v1_core_string_view,
};

/// `SW_ERROR_DNSLOOKUP_RESOLVE_FAILED`.
pub const RESOLVE_FAILED: c_int = 711;
const RESOLVE_TIMEOUT: c_int = 712;
const AIO_TIMEOUT: c_int = 4003;

// SAFETY: libc's.
unsafe extern "C" {
    fn inet_ntop(
        family: c_int,
        source: *const c_void,
        output: *mut c_char,
        size: libc::socklen_t,
    ) -> *const c_char;
}

/// A request record, freed with this.
pub struct Request(*mut ResolverRequest);

impl Request {
    /// A record for `name` and `service`.
    #[must_use]
    pub fn new(
        family: c_int,
        socktype: c_int,
        protocol: c_int,
        name: &[u8],
        service: &[u8],
    ) -> Self {
        // SAFETY: both spans are initialized.
        let prepared = unsafe {
            swr_v1_core_resolver_create(
                family,
                socktype,
                protocol,
                name.as_ptr().cast(),
                name.len(),
                service.as_ptr().cast(),
                service.len(),
            )
        };
        if prepared.owner.is_null() {
            std::alloc::handle_alloc_error(std::alloc::Layout::new::<ResolverRequest>());
        }
        Self(prepared.owner)
    }

    /// The record, for an event's `data`.
    #[must_use]
    pub const fn as_ptr(&self) -> *mut ResolverRequest {
        self.0
    }

    fn get(&self) -> &ResolverRequest {
        // SAFETY: live until this drops.
        unsafe { &*self.0 }
    }

    /// The text address `host` wrote.
    #[must_use]
    pub fn address(&self) -> Vec<u8> {
        address(self.get())
    }

    /// The text addresses `lookup` found.
    #[must_use]
    pub fn addresses(&self) -> Vec<Vec<u8>> {
        let mut cursor = 0;
        let mut found = Vec::new();
        loop {
            // SAFETY: the finished request's addresses.
            let text = unsafe { swr_v1_core_resolver_next(self.get(), &mut cursor) };
            if !text.valid {
                return found;
            }
            let end = text
                .bytes
                .iter()
                .position(|&byte| byte == 0)
                .unwrap_or(text.bytes.len());
            found.push(
                text.bytes
                    .iter()
                    .take(end)
                    .map(|&byte| u8::from_ne_bytes(byte.to_ne_bytes()))
                    .collect(),
            );
        }
    }

    /// `coroutine::async`'s outcome over this request: the error, or none.
    /// `request_error` reads the record's own error, once the pool finished.
    #[must_use]
    pub fn failed(&self, retval: isize, error: c_int, request_error: bool) -> Option<c_int> {
        let slot = if request_error {
            // SAFETY: the record's error slot.
            unsafe { &raw const (*swr_v1_core_resolver_view(self.get()).fields).error }
        } else {
            core::ptr::null()
        };
        // SAFETY: the slot is read only when the pool finished.
        let done = unsafe {
            swr_v1_core_resolver_finish(retval, error, slot, AIO_TIMEOUT, RESOLVE_TIMEOUT)
        };
        done.failed.then_some(done.error)
    }
}

impl Drop for Request {
    fn drop(&mut self) {
        // SAFETY: this request's own reference.
        unsafe { swr_v1_core_resolver_free(self.0) };
    }
}

/// A status below -1 is a refused allocation, which upstream throws as `bad_alloc`.
fn checked(status: c_int) -> isize {
    if status < -1 {
        std::alloc::handle_alloc_error(std::alloc::Layout::new::<u8>());
    }
    status as isize
}

/// The text address `host` wrote into `owner`.
#[must_use]
pub fn address(owner: &ResolverRequest) -> Vec<u8> {
    let view = swr_v1_core_resolver_view(owner);
    // SAFETY: the record's live address slot.
    let text = unsafe { swr_v1_core_string_view(view.address) };
    if text.length == 0 {
        return Vec::new();
    }
    // SAFETY: the slot holds `length` bytes until it changes.
    unsafe { core::slice::from_raw_parts(text.data.cast::<u8>(), text.length) }.to_vec()
}

/// `async::handler_gethostbyname`'s body, on a pool thread: its `(retval, error)`.
#[must_use]
pub fn host(owner: &ResolverRequest) -> (isize, c_int) {
    // SAFETY: the record is exclusive to this handler.
    let status = checked(unsafe { swr_v1_core_resolver_host(owner) });
    (status, if status < 0 { RESOLVE_FAILED } else { 0 })
}

extern "C" fn unknown_family(_: *mut c_void, family: c_int) -> bool {
    log::warning(
        c"getaddrinfo",
        format_args!("unknown socket family[{family}]"),
    );
    true
}

/// `async::handler_getaddrinfo`'s body, on a pool thread: its `(retval, error)`.
#[must_use]
pub fn lookup(owner: &ResolverRequest) -> (isize, c_int) {
    // SAFETY: the record is exclusive to this handler; the warning keeps no context.
    let status = checked(unsafe {
        swr_v1_core_resolver_lookup(owner, unknown_family, core::ptr::null_mut())
    });
    // SAFETY: the record's fields, written above on this thread.
    (status, unsafe {
        (*swr_v1_core_resolver_view(owner).fields).error
    })
}

/// `name` as `std::string::c_str()` reads it: up to its first NUL.
#[must_use]
pub fn c_name(name: &[u8]) -> CString {
    let end = name
        .iter()
        .position(|&byte| byte == 0)
        .unwrap_or(name.len());
    CString::new(name.get(..end).unwrap_or_default()).unwrap_or_default()
}

/// `network::gethostbyname(type, name)`, blocking: the address, or empty with the
/// last error set.
#[must_use]
pub fn gethostbyname(kind: c_int, name: &[u8]) -> Vec<u8> {
    let name = c_name(name);
    let mut binary = [0 as c_char; 16];
    // SAFETY: a NUL-terminated name and room for either family.
    if unsafe { swr_v1_clients_gethostbyname(kind, name.as_ptr(), binary.as_mut_ptr()) } != 0 {
        log::set_last_error(RESOLVE_FAILED);
        return Vec::new();
    }
    let mut text = [0 as c_char; 46];
    // SAFETY: a complete address and bounded output.
    if unsafe { inet_ntop(kind, binary.as_ptr().cast(), text.as_mut_ptr(), 46) }.is_null() {
        return Vec::new();
    }
    CStr::from_bytes_until_nul(&text.map(|byte| u8::from_ne_bytes(byte.to_ne_bytes())))
        .map(|text| text.to_bytes().to_vec())
        .unwrap_or_default()
}
