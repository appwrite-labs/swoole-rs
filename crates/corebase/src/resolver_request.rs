//! Owned DNS request strings, hints and address storage. A record is reference
//! counted: its native request holds one reference and each pool copy of an
//! `AsyncEvent` carrying it another. Records contain no callbacks.
use crate::StringSlot;
use crate::resolver::{DIAGNOSTIC_FAILED, gethostbyname, ntop, resolve_addresses};
use core::cell::{Cell, RefCell};
use core::ffi::{c_char, c_int};
use core::sync::atomic::{AtomicUsize, Ordering};
use std::ffi::CString;
use std::ptr;

#[repr(C)]
#[derive(Clone, Copy)]
pub struct ResolverFields {
    pub family: c_int,
    pub socktype: c_int,
    pub protocol: c_int,
    pub error: c_int,
    pub count: c_int,
}
/// Why a [`ResolverRequest`] lookup failed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ResolveError {
    /// The name did not resolve (libc's error is in the fields).
    Failed,
    /// An allocation was refused.
    Alloc,
    /// The unknown-family hook refused the lookup.
    Unknown,
}

/// Its owner writes `fields` and the strings through `PreparedResolver`'s views,
/// never while a method runs.
pub struct ResolverRequest {
    refs: AtomicUsize,
    fields: Cell<ResolverFields>,
    strings: [StringSlot; 3],
    results: RefCell<Vec<libc::sockaddr_in6>>,
}
#[repr(C)]
pub struct PreparedResolver {
    pub owner: *mut ResolverRequest,
    pub fields: *mut ResolverFields,
    pub name: *const StringSlot,
    pub service: *const StringSlot,
    pub address: *const StringSlot,
}

fn zero_address() -> libc::sockaddr_in6 {
    crate::sys::zeroed()
}

/// The slot's bytes. The record's slots change only in `host` and under native
/// code's own synchronization, never while this copies.
fn copy(slot: &StringSlot) -> Vec<u8> {
    slot.bytes().to_vec()
}

/// `bytes` as `std::string::c_str()` reads them: up to the first NUL.
fn c_string(mut bytes: Vec<u8>) -> CString {
    if let Some(end) = bytes.iter().position(|&byte| byte == 0) {
        bytes.truncate(end);
    }
    CString::new(bytes).unwrap_or_default()
}

/// `text` up to its terminator.
fn terminated(text: &[c_char; 46]) -> Vec<u8> {
    let end = text
        .iter()
        .position(|&byte| byte == 0)
        .unwrap_or(text.len());
    text.iter()
        .take(end)
        .map(|&byte| u8::from_ne_bytes(byte.to_ne_bytes()))
        .collect()
}

/// A copy of `addresses`, or none on allocation refusal.
fn copied(addresses: &[libc::sockaddr_in6]) -> Option<Vec<libc::sockaddr_in6>> {
    let mut copy = Vec::new();
    copy.try_reserve_exact(addresses.len()).ok()?;
    copy.extend_from_slice(addresses);
    Some(copy)
}

impl ResolverRequest {
    fn boxed(family: c_int, socktype: c_int, protocol: c_int) -> Box<Self> {
        Box::new(Self {
            refs: AtomicUsize::new(1),
            fields: Cell::new(ResolverFields {
                family,
                socktype,
                protocol,
                error: 0,
                count: 0,
            }),
            strings: std::array::from_fn(|_| StringSlot::default()),
            results: RefCell::new(Vec::new()),
        })
    }

    /// The views over `record`, which the returned owner holds one reference to.
    fn prepared(record: Box<Self>) -> PreparedResolver {
        let state = Box::leak(record);
        let view = swr_v1_core_resolver_view(state);
        // A handle from the mutable borrow, as one from a shared borrow can't free the record.
        PreparedResolver {
            owner: &raw mut *state,
            ..view
        }
    }

    /// A record for `name` and `service` whose owner holds one reference, freed
    /// with `swr_v1_core_resolver_free`; all null on allocation refusal.
    #[must_use]
    pub fn create(
        family: c_int,
        socktype: c_int,
        protocol: c_int,
        name: &[u8],
        service: &[u8],
    ) -> PreparedResolver {
        let mut record = Self::boxed(family, socktype, protocol);
        let [name_slot, service_slot, _] = &mut record.strings;
        if name_slot.set(name) != 0 || service_slot.set(service) != 0 {
            return PreparedResolver {
                owner: ptr::null_mut(),
                fields: ptr::null_mut(),
                name: ptr::null(),
                service: ptr::null(),
                address: ptr::null(),
            };
        }
        Self::prepared(record)
    }

    /// The hints and results.
    #[must_use]
    pub fn fields(&self) -> ResolverFields {
        self.fields.get()
    }

    fn update(&self, change: impl FnOnce(&mut ResolverFields)) {
        let mut fields = self.fields.get();
        change(&mut fields);
        self.fields.set(fields);
    }

    /// The name to resolve.
    #[must_use]
    pub fn name(&self) -> Vec<u8> {
        copy(&self.strings[0])
    }

    /// The text address `host` wrote.
    #[must_use]
    pub fn address(&self) -> Vec<u8> {
        copy(&self.strings[2])
    }

    /// `async::handler_getaddrinfo`'s body: resolves the hints and replaces the
    /// complete result set. A libc error publishes only `error`, keeping the
    /// previous count and addresses. On success count and error publish before
    /// allocation, as native vector assignment does.
    /// # Errors
    /// [`ResolveError::Failed`] for a libc error, [`ResolveError::Alloc`] for an
    /// allocation refusal, [`ResolveError::Unknown`] when `unknown` returned false.
    pub fn lookup(&self, unknown: impl FnMut(c_int) -> bool) -> Result<(), ResolveError> {
        let hints = self.fields();
        let [name, service, _] = &self.strings;
        let name = c_string(copy(name));
        let service = copy(service);
        let service = (!service.is_empty()).then(|| c_string(service));
        let mut found = [zero_address(); 128];
        let result = resolve_addresses(
            &name,
            service.as_deref(),
            (hints.family, hints.socktype, hints.protocol),
            &mut found,
            unknown,
        );
        if result.error == DIAGNOSTIC_FAILED {
            return Err(ResolveError::Unknown);
        }
        self.update(|fields| fields.error = result.error);
        if result.error != 0 {
            return Err(ResolveError::Failed);
        }
        self.update(|fields| fields.count = result.count);
        let count = usize::try_from(result.count).unwrap_or(0);
        let Some(found) = copied(found.get(..count).unwrap_or_default()) else {
            return Err(ResolveError::Alloc);
        };
        self.results.replace(found);
        Ok(())
    }

    /// `async::handler_gethostbyname`'s body: the name's first address, as text,
    /// into the address slot.
    /// # Errors
    /// [`ResolveError::Failed`] when it does not resolve, [`ResolveError::Alloc`] on
    /// an allocation refusal.
    pub fn host(&self) -> Result<(), ResolveError> {
        let family = self.fields().family;
        let text = gethostbyname(family, &c_string(self.name()))
            .and_then(|binary| ntop(family, binary.first_chunk()?));
        let Some(text) = text else {
            return Err(ResolveError::Failed);
        };
        let text = terminated(&text);
        // SAFETY: a slot that nothing reads while this sets it (see `copy`).
        match unsafe { self.strings[2].set_shared(&text) } {
            0 => Ok(()),
            _ => Err(ResolveError::Alloc),
        }
    }

    /// Address `index` as text, or none past the end; `valid` false if it does not
    /// format.
    fn text(&self, index: usize) -> Option<ResolverText> {
        let family = self.fields().family;
        let address = *self.results.borrow().get(index)?;
        let binary = if family == libc::AF_INET6 {
            address.sin6_addr.s6_addr
        } else {
            // The original reads other families through sockaddr_in, whose sin_addr
            // shares sin6_flowinfo's offset.
            let mut binary = [0; 16];
            binary[..4].copy_from_slice(&address.sin6_flowinfo.to_ne_bytes());
            binary
        };
        let binary = binary.map(|byte| c_char::from_ne_bytes(byte.to_ne_bytes()));
        let text = ntop(family, &binary);
        Some(ResolverText {
            bytes: text.unwrap_or([0; 46]),
            valid: text.is_some(),
        })
    }

    /// The text addresses `lookup` found, skipping any that do not format.
    #[must_use]
    pub fn addresses(&self) -> Vec<Vec<u8>> {
        (0..)
            .map_while(|index| self.text(index))
            .filter(|text| text.valid)
            .map(|text| terminated(&text.bytes))
            .collect()
    }

    /// A copy of the socket addresses `lookup` found.
    #[must_use]
    pub fn results(&self) -> Vec<libc::sockaddr_in6> {
        self.results.borrow().clone()
    }
}

/// The record's fields and string slots, from its handle.
pub fn swr_v1_core_resolver_view(owner: &ResolverRequest) -> PreparedResolver {
    let [name, service, address] = &owner.strings;
    PreparedResolver {
        owner: ptr::from_ref(owner).cast_mut(),
        fields: owner.fields.as_ptr(),
        name,
        service,
        address,
    }
}

/// Takes another reference to `owner`.
pub fn swr_v1_core_resolver_retain(owner: &ResolverRequest) {
    owner.refs.fetch_add(1, Ordering::Relaxed);
}

/// Drops one reference; the last frees the record.
/// # Safety
/// owner is null or a reference the caller holds and gives up here.
pub unsafe extern "C" fn swr_v1_core_resolver_free(owner: *mut ResolverRequest) {
    // SAFETY: null or live while the caller's reference stands.
    let Some(record) = (unsafe { owner.as_ref() }) else {
        return;
    };
    if record.refs.fetch_sub(1, Ordering::Release) == 1 {
        core::sync::atomic::fence(Ordering::Acquire);
        // SAFETY: that was the last reference to the leaked box.
        drop(unsafe { Box::from_raw(owner) });
    }
}

/// One address as text; `valid` false if it does not format.
struct ResolverText {
    bytes: [c_char; 46],
    valid: bool,
}

#[repr(C)]
pub struct ResolverCompletion {
    pub error: c_int,
    pub failed: bool,
}
impl ResolverCompletion {
    /// Decides completion after an await returns. A failed await must not read the
    /// worker-owned request error, which may still be changing after timeout, so
    /// `request_error` runs only when `retval != -1`.
    pub fn after_await(
        retval: isize,
        event_error: c_int,
        request_error: impl FnOnce() -> c_int,
        timeout_error: c_int,
        resolution_timeout: c_int,
    ) -> Self {
        Self {
            failed: retval == -1 || request_error() != 0,
            error: if event_error == timeout_error {
                resolution_timeout
            } else {
                event_error
            },
        }
    }
}

#[cfg(test)]
mod completion_tests {
    use super::*;
    #[test]
    fn failed_await_never_reads_worker_error() {
        let inaccessible = || unreachable!("a failed await reads no request error");
        let completion = ResolverCompletion::after_await(-1, 4003, inaccessible, 4003, 712);
        assert!(completion.failed);
        assert_eq!(completion.error, 712);
        let completed_error = 7;
        assert!(ResolverCompletion::after_await(0, 0, || completed_error, 4003, 712).failed);
        assert!(!ResolverCompletion::after_await(1, 0, || 0, 4003, 712).failed);
    }

    #[test]
    fn the_last_reference_frees_the_record() {
        let prepared = ResolverRequest::create(libc::AF_INET, 0, 0, b"", b"");
        let record = unsafe { &*prepared.owner };
        let view = swr_v1_core_resolver_view(record);
        assert_eq!(
            (view.fields, view.address),
            (prepared.fields, prepared.address)
        );
        swr_v1_core_resolver_retain(record);
        unsafe { swr_v1_core_resolver_free(prepared.owner) };
        // The pool copy's reference keeps the record after its request went.
        assert_eq!(record.refs.load(Ordering::Relaxed), 1);
        unsafe { swr_v1_core_resolver_free(prepared.owner) };
        unsafe { swr_v1_core_resolver_free(core::ptr::null_mut()) };
    }
}
