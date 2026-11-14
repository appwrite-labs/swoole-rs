// SPDX-License-Identifier: Apache-2.0
// Compatibility derived from Swoole v6.2.4 src/network/dns.cc and src/os/async_thread.cc
// (Swoole contributors).
//! A resolver request record (crates/corebase) as a Rust owner: the pool handlers'
//! bodies, the completion check and the blocking `network::gethostbyname`.
use core::ffi::{CStr, c_int};
use std::ffi::CString;
use swoole_rs_corebase::{
    ResolveError, ResolverCompletion, ResolverRequest, log, ntop, swr_v1_core_resolver_free,
};

/// `SW_ERROR_DNSLOOKUP_RESOLVE_FAILED`.
pub const RESOLVE_FAILED: c_int = 711;
const RESOLVE_TIMEOUT: c_int = 712;
const AIO_TIMEOUT: c_int = 4003;

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
        let prepared = ResolverRequest::create(family, socktype, protocol, name, service);
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
        self.get().address()
    }

    /// The text addresses `lookup` found.
    #[must_use]
    pub fn addresses(&self) -> Vec<Vec<u8>> {
        self.get().addresses()
    }

    /// `coroutine::async`'s outcome over this request: the error, or none.
    /// `request_error` reads the record's own error, once the pool finished.
    #[must_use]
    pub fn failed(&self, retval: isize, error: c_int, request_error: bool) -> Option<c_int> {
        let request = || {
            if request_error {
                self.get().fields().error
            } else {
                0
            }
        };
        let done =
            ResolverCompletion::after_await(retval, error, request, AIO_TIMEOUT, RESOLVE_TIMEOUT);
        done.failed.then_some(done.error)
    }
}

impl Drop for Request {
    fn drop(&mut self) {
        // SAFETY: this request's own reference.
        unsafe { swr_v1_core_resolver_free(self.0) };
    }
}

/// The handler's retval: 0, or -1 when the name did not resolve. Anything else is
/// a refused allocation, which upstream throws as `bad_alloc`.
fn checked(result: Result<(), ResolveError>) -> isize {
    match result {
        Ok(()) => 0,
        Err(ResolveError::Failed) => -1,
        Err(ResolveError::Alloc | ResolveError::Unknown) => {
            std::alloc::handle_alloc_error(std::alloc::Layout::new::<u8>())
        }
    }
}

/// The text address `host` wrote into `owner`.
#[must_use]
pub fn address(owner: &ResolverRequest) -> Vec<u8> {
    owner.address()
}

/// `async::handler_gethostbyname`'s body, on a pool thread: its `(retval, error)`.
#[must_use]
pub fn host(owner: &ResolverRequest) -> (isize, c_int) {
    let status = checked(owner.host());
    (status, if status < 0 { RESOLVE_FAILED } else { 0 })
}

fn unknown_family(family: c_int) -> bool {
    log::warning(
        c"getaddrinfo",
        format_args!("unknown socket family[{family}]"),
    );
    true
}

/// `async::handler_getaddrinfo`'s body, on a pool thread: its `(retval, error)`.
#[must_use]
pub fn lookup(owner: &ResolverRequest) -> (isize, c_int) {
    let status = checked(owner.lookup(unknown_family));
    (status, owner.fields().error)
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
    let Some(binary) = swoole_rs_corebase::gethostbyname(kind, &name) else {
        log::set_last_error(RESOLVE_FAILED);
        return Vec::new();
    };
    let Some(text) = binary.first_chunk().and_then(|binary| ntop(kind, binary)) else {
        return Vec::new();
    };
    CStr::from_bytes_until_nul(&text.map(|byte| u8::from_ne_bytes(byte.to_ne_bytes())))
        .map(|text| text.to_bytes().to_vec())
        .unwrap_or_default()
}
