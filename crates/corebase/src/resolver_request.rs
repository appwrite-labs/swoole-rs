//! Owned DNS request strings, hints and address storage. A record is reference
//! counted: its native request holds one reference and each pool copy of an
//! `AsyncEvent` carrying it another. Records contain no callbacks.
use crate::{StringSlot, swr_v1_core_string_set, swr_v1_core_string_view};
use core::ffi::{c_char, c_int};
use core::sync::atomic::{AtomicUsize, Ordering};
use std::{cell::UnsafeCell, ptr};
unsafe extern "C" {
    fn inet_ntop(
        family: c_int,
        source: *const core::ffi::c_void,
        output: *mut c_char,
        capacity: libc::socklen_t,
    ) -> *const c_char;
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct ResolverFields {
    pub family: c_int,
    pub socktype: c_int,
    pub protocol: c_int,
    pub error: c_int,
    pub count: c_int,
}
pub struct ResolverRequest {
    refs: AtomicUsize,
    fields: UnsafeCell<ResolverFields>,
    strings: [StringSlot; 3],
    results: UnsafeCell<Vec<libc::sockaddr_in6>>,
}
#[repr(C)]
pub struct PreparedResolver {
    pub owner: *mut ResolverRequest,
    pub fields: *mut ResolverFields,
    pub name: *const StringSlot,
    pub service: *const StringSlot,
    pub address: *const StringSlot,
}
#[unsafe(no_mangle)]
pub extern "C" fn swr_v1_core_resolver_new(
    family: c_int,
    socktype: c_int,
    protocol: c_int,
) -> PreparedResolver {
    let state = Box::leak(Box::new(ResolverRequest {
        refs: AtomicUsize::new(1),
        fields: UnsafeCell::new(ResolverFields {
            family,
            socktype,
            protocol,
            error: 0,
            count: 0,
        }),
        strings: std::array::from_fn(|_| StringSlot::default()),
        results: UnsafeCell::new(Vec::new()),
    }));
    let view = swr_v1_core_resolver_view(state);
    // A handle from the mutable borrow, as one from a shared borrow can't free the record.
    PreparedResolver {
        owner: &raw mut *state,
        ..view
    }
}

/// The record's fields and string slots, from its handle.
pub fn swr_v1_core_resolver_view(owner: &ResolverRequest) -> PreparedResolver {
    let [name, service, address] = &owner.strings;
    PreparedResolver {
        owner: ptr::from_ref(owner).cast_mut(),
        fields: owner.fields.get(),
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
#[unsafe(no_mangle)]
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

#[repr(C)]
pub struct ResolverAddresses {
    pub data: *mut libc::sockaddr_in6,
    pub length: usize,
    pub capacity: usize,
}
/// # Safety
/// owner is live and synchronized; no mutation occurs while its span is used.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_core_resolver_span(owner: &ResolverRequest) -> ResolverAddresses {
    let storage = owner.results.get();
    // SAFETY: a shared borrow only while reading the span metadata.
    let addresses = unsafe { &*storage };
    ResolverAddresses {
        data: if addresses.capacity() == 0 {
            ptr::null_mut()
        } else {
            addresses.as_ptr().cast_mut()
        },
        length: addresses.len(),
        capacity: addresses.capacity(),
    }
}
/// Resize the actual Rust vector. -4 is length overflow; -2 allocation refusal.
/// # Safety
/// owner is live and exclusively writable, with no outstanding span accesses.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_core_resolver_resize(
    owner: &ResolverRequest,
    length: usize,
) -> c_int {
    if length > isize::MAX.unsigned_abs() / size_of::<libc::sockaddr_in6>() {
        return -4;
    }
    let storage = owner.results.get();
    // SAFETY: no callbacks occur while this vector is borrowed.
    let addresses = unsafe { &mut *storage };
    if length > addresses.len() && addresses.try_reserve(length - addresses.len()).is_err() {
        return -2;
    }
    addresses.resize_with(length, || {
        // SAFETY: sockaddr_in6 is a POD address, whose zero state is valid.
        unsafe { std::mem::zeroed() }
    });
    0
}
/// Assignment accepts overlap, including self-assignment, and leaves old storage
/// intact on allocation refusal.
/// # Safety
/// owner is exclusive; data holds length readable addresses until this returns.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_core_resolver_assign(
    owner: &ResolverRequest,
    data: *const libc::sockaddr_in6,
    length: usize,
) -> c_int {
    if length > isize::MAX.unsigned_abs() / size_of::<libc::sockaddr_in6>() {
        return -4;
    }
    let mut replacement = Vec::new();
    if replacement.try_reserve_exact(length).is_err() {
        return -2;
    }
    if length != 0 {
        // SAFETY: the source span remains live until replacement is complete.
        replacement.extend_from_slice(unsafe { std::slice::from_raw_parts(data, length) });
    }
    // Only now mutate the caller's address owner, after copying overlaps.
    let storage = owner.results.get();
    // SAFETY: synchronized exclusive access; old addresses contain no callbacks.
    unsafe { *storage = replacement };
    0
}

/// Resolve the request's hints and replace its complete result set. Errors from
/// libc publish only error, retaining the previous count and vector. On success,
/// count/error publish before allocation, matching native vector assignment.
/// # Safety
/// owner is exclusive; unknown catches native exceptions and returns false on failure; context stays live throughout the call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_core_resolver_lookup(
    owner: &ResolverRequest,
    unknown: unsafe extern "C" fn(*mut core::ffi::c_void, c_int) -> bool,
    context: *mut core::ffi::c_void,
) -> c_int {
    let fields = owner.fields.get();
    // SAFETY: copy every hint before any external resolver effect.
    let hints = unsafe { *fields };
    let [name, service, _] = &owner.strings;
    // SAFETY: the first slot contains the terminated hostname.
    let name = unsafe { swr_v1_core_string_view(name) };
    // SAFETY: the service remains stable until resolution returns.
    let service = unsafe { swr_v1_core_string_view(service) };
    // SAFETY: every byte of POD sockaddr_in6 may be zero.
    let mut found: [libc::sockaddr_in6; 128] = unsafe { std::mem::zeroed() };
    let diagnostic = |family| {
        // SAFETY: the injected leaf catches all exceptions and borrows live context.
        unsafe { unknown(context, family) }
    };
    // SAFETY: stable strings, bounded initialized output, and contained diagnostic.
    let result = unsafe {
        crate::resolver::resolve_addresses(
            name.data,
            if service.length == 0 {
                ptr::null()
            } else {
                service.data
            },
            hints.family,
            hints.socktype,
            hints.protocol,
            found.as_mut_ptr(),
            found.len(),
            diagnostic,
        )
    };
    if result.error == crate::resolver::DIAGNOSTIC_FAILED {
        return -3;
    }
    // SAFETY: update error for both success and failure, as the native API does.
    unsafe { (*fields).error = result.error };
    if result.error != 0 {
        return -1;
    }
    // SAFETY: count commits before potentially failing address assignment.
    unsafe { (*fields).count = result.count };
    // SAFETY: the bounded stack result is readable until assignment returns.
    unsafe {
        swr_v1_core_resolver_assign(
            owner,
            found.as_ptr(),
            usize::try_from(result.count).unwrap_or(0),
        )
    }
}

/// # Safety
/// owner is exclusive.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_core_resolver_host(owner: &ResolverRequest) -> c_int {
    // SAFETY: copy the family without holding a Rust borrow across libc.
    let family = unsafe { (*owner.fields.get()).family };
    let [name, _, address] = &owner.strings;
    // SAFETY: the first slot's terminated name remains unmodified through libc.
    let name = unsafe { swr_v1_core_string_view(name) };
    let mut binary = [0 as c_char; 46];
    // SAFETY: stable terminated name and output large enough for both families.
    if unsafe { crate::swr_v1_clients_gethostbyname(family, name.data, binary.as_mut_ptr()) } != 0 {
        return -1;
    }
    let mut text = [0 as c_char; 46];
    // SAFETY: inet_ntop receives a complete binary address and bounded output.
    let result = unsafe { inet_ntop(family, binary.as_ptr().cast(), text.as_mut_ptr(), 46) };
    if result.is_null() {
        return -1;
    }
    // SAFETY: successful inet_ntop writes a bounded terminated address.
    let length = unsafe { std::ffi::CStr::from_ptr(text.as_ptr()) }
        .to_bytes()
        .len();
    // SAFETY: the stack text remains readable for the copy.
    unsafe { swr_v1_core_string_set(address, text.as_ptr(), length) }
}

#[repr(C)]
pub struct ResolverText {
    pub bytes: [c_char; 46],
    pub valid: bool,
}
/// Formats one actual address without entering a C++ vector callback.
/// # Safety
/// owner is live/synchronized and index names one address in its current vector.
unsafe fn swr_v1_core_resolver_format(owner: &ResolverRequest, index: usize) -> ResolverText {
    // SAFETY: read its family without retaining a borrow across libc.
    let family = unsafe { (*owner.fields.get()).family };
    let storage = owner.results.get();
    // SAFETY: obtain the live vector's data without exposing a Rust reference.
    let data = unsafe { (*storage).as_ptr() };
    // SAFETY: index is within that live address span.
    let address = unsafe { data.add(index) };
    let input = if family == libc::AF_INET6 {
        // SAFETY: the address storage contains a full sockaddr_in6.
        unsafe { ptr::addr_of!((*address).sin6_addr).cast() }
    } else {
        // SAFETY: the original API interprets other families through sockaddr_in.
        unsafe { ptr::addr_of!((*address.cast::<libc::sockaddr_in>()).sin_addr).cast() }
    };
    let mut text = ResolverText {
        bytes: [0; 46],
        valid: false,
    };
    // SAFETY: a complete address field and bounded terminated output buffer.
    text.valid = !unsafe { inet_ntop(family, input, text.bytes.as_mut_ptr(), 46) }.is_null();
    text
}

#[repr(C)]
pub struct ResolverCompletion {
    pub error: c_int,
    pub failed: bool,
}
/// Decides completion after native await returns. A failed await must not read
/// the worker-owned request error, which may still be changing after timeout.
/// # Safety
/// `request_error` may be null. Otherwise it is readable only when `retval != -1`;
/// a failed await deliberately permits a still-inaccessible worker field.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_core_resolver_finish(
    retval: isize,
    event_error: c_int,
    request_error: *const c_int,
    timeout_error: c_int,
    resolution_timeout: c_int,
) -> ResolverCompletion {
    let failed = retval == -1
        || (!request_error.is_null() && {
            // SAFETY: await succeeded, so the caller lends its synchronized error slot.
            unsafe { *request_error != 0 }
        });
    ResolverCompletion {
        failed,
        error: if event_error == timeout_error {
            resolution_timeout
        } else {
            event_error
        },
    }
}

/// Creates request hints and both input strings together; no arbitrary payload
/// exists during construction. Null owner reports byte OOM.
/// # Safety
/// name/service hold their declared readable byte lengths.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_core_resolver_create(
    family: c_int,
    socktype: c_int,
    protocol: c_int,
    name: *const c_char,
    name_length: usize,
    service: *const c_char,
    service_length: usize,
) -> PreparedResolver {
    let prepared = swr_v1_core_resolver_new(family, socktype, protocol);
    // SAFETY: fresh stable slot, readable input, and no native callback registered.
    let name_status = unsafe { swr_v1_core_string_set(prepared.name, name, name_length) };
    let service_status = if name_status == 0 {
        // SAFETY: the second fresh slot has no callback and the input is readable.
        unsafe { swr_v1_core_string_set(prepared.service, service, service_length) }
    } else {
        name_status
    };
    if service_status != 0 {
        // SAFETY: construction has exposed no view; return its unique allocation.
        unsafe { swr_v1_core_resolver_free(prepared.owner) };
        return PreparedResolver {
            owner: ptr::null_mut(),
            fields: ptr::null_mut(),
            name: ptr::null(),
            service: ptr::null(),
            address: ptr::null(),
        };
    }
    prepared
}

/// Returns the next printable address. Native code appends its owned output only
/// after this leaf returns, so vector allocation exceptions cross no Rust frame.
/// # Safety
/// owner is live/synchronized; no result storage mutation occurs until this
/// enumeration ends.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_core_resolver_next(
    owner: &ResolverRequest,
    cursor: &mut usize,
) -> ResolverText {
    // SAFETY: the caller lends stable live address storage.
    let span = unsafe { swr_v1_core_resolver_span(owner) };
    let mut index = *cursor;
    while index < span.length {
        // SAFETY: index remains within the stable address span.
        let text = unsafe { swr_v1_core_resolver_format(owner, index) };
        index += 1;
        // Publish progress before native code can append/throw.
        *cursor = index;
        if text.valid {
            return text;
        }
    }
    ResolverText {
        bytes: [0; 46],
        valid: false,
    }
}

#[cfg(target_pointer_width = "64")]
const _: () = {
    assert!(
        size_of::<ResolverFields>() == 20,
        "native DNS hint record size"
    );
    assert!(
        size_of::<PreparedResolver>() == 40,
        "native DNS prepared owner size"
    );
    assert!(
        size_of::<ResolverAddresses>() == 24,
        "native DNS vector span size"
    );
    assert!(
        size_of::<ResolverText>() == 47,
        "native DNS address text size"
    );
    assert!(
        size_of::<ResolverCompletion>() == 8,
        "native DNS completion size"
    );
};

#[cfg(test)]
mod completion_tests {
    use super::*;
    #[test]
    fn failed_await_never_reads_worker_error() {
        let inaccessible = core::ptr::without_provenance::<c_int>(1);
        let completion = unsafe { swr_v1_core_resolver_finish(-1, 4003, inaccessible, 4003, 712) };
        assert!(completion.failed);
        assert_eq!(completion.error, 712);
        let completed_error = 7;
        assert!(
            unsafe { swr_v1_core_resolver_finish(0, 0, &raw const completed_error, 4003, 712) }
                .failed
        );
        assert!(!unsafe { swr_v1_core_resolver_finish(1, 0, core::ptr::null(), 4003, 712) }.failed);
    }

    #[test]
    fn the_last_reference_frees_the_record() {
        let prepared = swr_v1_core_resolver_new(libc::AF_INET, 0, 0);
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
