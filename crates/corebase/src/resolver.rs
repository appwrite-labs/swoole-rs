//! Always-available libc resolver providers shared by native and async DNS requests.
use std::ffi::{CStr, c_char, c_int};

/// `SW_DNS_LOOKUP_RANDOM`, masked out of `gethostbyname`'s flags.
const LOOKUP_RANDOM: c_int = 1 << 11;

#[cfg(target_os = "linux")]
unsafe extern "C" {
    fn gethostbyname2_r(
        name: *const c_char,
        family: c_int,
        entry: *mut libc::hostent,
        buffer: *mut c_char,
        length: usize,
        result: *mut *mut libc::hostent,
        error: *mut c_int,
    ) -> c_int;
}
#[cfg(not(target_os = "linux"))]
unsafe extern "C" {
    fn gethostbyname2(name: *const c_char, family: c_int) -> *mut libc::hostent;
}

/// Copies the entry's first address (`h_length` bytes, zero if it has none) to `addr`.
///
/// # Safety
/// `entry` is a resolved hostent; `addr` holds `h_length` bytes.
unsafe fn first_address(entry: &libc::hostent, addr: *mut c_char) {
    let length = usize::try_from(entry.h_length).unwrap_or(0).min(46);
    let mut bytes = [0u8; 46];
    // SAFETY: `h_addr_list` is the resolver's null-terminated list, which some
    // libcs (Apple's) do not align.
    let first = unsafe { entry.h_addr_list.read_unaligned() };
    if !first.is_null() {
        // SAFETY: each address holds `h_length` bytes.
        unsafe { std::ptr::copy_nonoverlapping(first.cast::<u8>(), bytes.as_mut_ptr(), length) };
    }
    // SAFETY: the caller's `addr` holds `h_length` bytes.
    unsafe { std::ptr::copy_nonoverlapping(bytes.as_ptr(), addr.cast(), length) };
}

/// `network::gethostbyname(flags, name, addr)`: the first address of `name` for the
/// flags' family into `addr`; 0, or -1 if it does not resolve.
///
/// # Safety
/// `name` is NUL-terminated; `addr` holds an address of the family.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_clients_gethostbyname(
    flags: c_int,
    name: *const c_char,
    addr: *mut c_char,
) -> c_int {
    let family = flags & !LOOKUP_RANDOM;
    #[cfg(target_os = "linux")]
    {
        let mut entry: libc::hostent = crate::sys::zeroed();
        let mut buffer = vec![0 as c_char; 256];
        let mut result = std::ptr::null_mut();
        let mut error = 0;
        loop {
            // SAFETY: every out-parameter is valid for the call.
            let rc = unsafe {
                gethostbyname2_r(
                    name,
                    family,
                    &raw mut entry,
                    buffer.as_mut_ptr(),
                    buffer.len(),
                    &raw mut result,
                    &raw mut error,
                )
            };
            if rc == libc::ERANGE {
                buffer.resize(buffer.len() * 2, 0);
                continue;
            }
            if rc != 0 || result.is_null() {
                return -1;
            }
            break;
        }
        // SAFETY: `entry` was filled by the resolver; `addr` is the caller's.
        unsafe { first_address(&entry, addr) };
    }
    #[cfg(not(target_os = "linux"))]
    {
        static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
        let _held = LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        // SAFETY: the lock serializes gethostbyname2's shared result.
        let entry = unsafe { gethostbyname2(name, family) };
        // SAFETY: a non-null result is the resolver's entry, read under the lock.
        let Some(entry) = (unsafe { entry.as_ref() }) else {
            return -1;
        };
        // SAFETY: as above.
        unsafe { first_address(entry, addr) };
    }
    0
}

/// What `getaddrinfo` found.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Resolved {
    /// getaddrinfo's error, or 0.
    pub error: c_int,
    /// Addresses written to `results`.
    pub count: c_int,
}

/// Internal resolver permits a contained native diagnostic failure. The sentinel
/// remains internal and is never published as the request's libc error.
pub(crate) const DIAGNOSTIC_FAILED: c_int = c_int::MIN;
/// `network::getaddrinfo`: resolves `host` (and `service`, none for none) into at
/// most `results.len()` socket addresses; `unknown` reports an entry of another
/// family, which keeps its slot, and false from it stops the walk.
pub(crate) fn resolve_addresses(
    host: &CStr,
    service: Option<&CStr>,
    hints: (c_int, c_int, c_int),
    results: &mut [libc::sockaddr_in6],
    mut unknown: impl FnMut(c_int) -> bool,
) -> Resolved {
    let mut request: libc::addrinfo = crate::sys::zeroed();
    (request.ai_family, request.ai_socktype, request.ai_protocol) = hints;
    let service = service.map_or(std::ptr::null(), CStr::as_ptr);
    let mut list = std::ptr::null_mut();
    // SAFETY: terminated strings; `list` receives the resolver's list.
    let error =
        unsafe { libc::getaddrinfo(host.as_ptr(), service, &raw const request, &raw mut list) };
    if error != 0 {
        return Resolved { error, count: 0 };
    }
    let mut count = 0;
    let mut diagnostic_failed = false;
    let mut entry = list;
    for slot in results.iter_mut() {
        // SAFETY: `entry` walks the resolver's list until null.
        let Some(item) = (unsafe { entry.as_ref() }) else {
            break;
        };
        let size = match item.ai_family {
            libc::AF_INET => size_of::<libc::sockaddr_in>(),
            libc::AF_INET6 => size_of::<libc::sockaddr_in6>(),
            other => {
                if !unknown(other) {
                    diagnostic_failed = true;
                    break;
                }
                0
            }
        };
        let slot = std::ptr::from_mut(slot).cast::<u8>();
        // SAFETY: `ai_addr` holds the family's address; the slot holds a sockaddr_in6.
        unsafe { std::ptr::copy_nonoverlapping(item.ai_addr.cast::<u8>(), slot, size) };
        count += 1;
        entry = item.ai_next;
    }
    // SAFETY: `list` came from getaddrinfo.
    unsafe { libc::freeaddrinfo(list) };
    Resolved {
        error: if diagnostic_failed {
            DIAGNOSTIC_FAILED
        } else {
            0
        },
        count: c_int::try_from(count).unwrap_or(0),
    }
}

/// `swr_v1_clients_gethostbyname` into room for any family's address.
#[must_use]
pub fn gethostbyname(flags: c_int, name: &CStr) -> Option<[c_char; 46]> {
    let mut binary = [0 as c_char; 46];
    // SAFETY: a terminated name, and `first_address` copies at most 46 bytes.
    let status = unsafe { swr_v1_clients_gethostbyname(flags, name.as_ptr(), binary.as_mut_ptr()) };
    (status == 0).then_some(binary)
}

// SAFETY: libc's.
unsafe extern "C" {
    fn inet_ntop(
        family: c_int,
        source: *const core::ffi::c_void,
        output: *mut c_char,
        capacity: libc::socklen_t,
    ) -> *const c_char;
}

/// `inet_ntop(family, binary)`'s text, or none for another family or a failure.
#[must_use]
pub fn ntop(family: c_int, binary: &[c_char; 16]) -> Option<[c_char; 46]> {
    let mut text = [0 as c_char; 46];
    // SAFETY: room for an address of either family (4 or 16 bytes) and for its text;
    // another family reads nothing.
    let result = unsafe { inet_ntop(family, binary.as_ptr().cast(), text.as_mut_ptr(), 46) };
    (!result.is_null()).then_some(text)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[cfg_attr(miri, ignore = "Miri has no getaddrinfo")]
    fn resolves_addresses_through_libc() {
        let mut results: [libc::sockaddr_in6; 16] = unsafe { std::mem::zeroed() };
        let hints = (libc::AF_INET, libc::SOCK_STREAM, 0);
        let found = resolve_addresses(c"127.0.0.1", Some(c"80"), hints, &mut results, |_| true);
        assert_eq!(found, Resolved { error: 0, count: 1 });
        let v4 = unsafe { &*results.as_ptr().cast::<libc::sockaddr_in>() };
        assert_eq!(
            (u16::from_be(v4.sin_port), v4.sin_addr.s_addr.to_ne_bytes()),
            (80, [127, 0, 0, 1])
        );
        let hints = (libc::AF_INET, 0, 0);
        let service = Some(c"no-such-service-swr");
        let failed = resolve_addresses(c"127.0.0.1", service, hints, &mut results, |_| true);
        assert_ne!(failed.error, 0);
    }
}
