//! Always-available libc resolver providers shared by native and async DNS requests.
use crate::sys::Text;
use std::ffi::{CStr, c_char, c_int};

/// `SW_DNS_LOOKUP_RANDOM`, masked out of `gethostbyname`'s flags.
const LOOKUP_RANDOM: c_int = 1 << 11;

#[cfg(not(target_os = "linux"))]
unsafe extern "C" {
    fn gethostbyname2(name: *const c_char, family: c_int) -> *mut libc::hostent;
}

/// Serializes `gethostbyname2`'s shared result.
#[cfg(not(target_os = "linux"))]
static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// A resolver's `hostent`, as `gethostbyname` and c-ares fill one: `h_addr_list` is
/// a null-terminated list (unaligned on some libcs, Apple's) of addresses of
/// `h_length` bytes each, all live as long as the entry is borrowed.
#[repr(transparent)]
#[derive(Clone, Copy)]
pub struct Hostent<'a>(&'a libc::hostent);

impl<'a> Hostent<'a> {
    /// Whether `h_addrtype` is `AF_INET`.
    #[must_use]
    pub fn is_inet(self) -> bool {
        self.0.h_addrtype == libc::AF_INET
    }

    /// The entry's addresses, in the list's order, `h_length` bytes each.
    pub fn addresses(self) -> impl Iterator<Item = &'a [u8]> {
        let length = usize::try_from(self.0.h_length).unwrap_or(0);
        let mut slot = self.0.h_addr_list;
        std::iter::from_fn(move || {
            if slot.is_null() {
                return None;
            }
            // SAFETY: `slot` is a slot of the null-terminated list, at or before its
            // null (the type's invariant), read unaligned.
            let address = unsafe { slot.read_unaligned() };
            if address.is_null() {
                return None;
            }
            slot = slot.wrapping_add(1);
            // SAFETY: each address holds `h_length` bytes, live with the entry.
            Some(unsafe { std::slice::from_raw_parts(address.cast::<u8>(), length) })
        })
    }
}

/// `network::gethostbyname(flags, name)`: the first address of `name` for the
/// flags' family (`h_length` bytes, zero if it has none) and its length, or none if
/// it does not resolve.
fn lookup(flags: c_int, name: &CStr) -> Option<([c_char; 46], usize)> {
    let family = flags & !LOOKUP_RANDOM;
    // The entry's lists live in `buffer`, which outlives the reads below.
    #[cfg(target_os = "linux")]
    let mut buffer: Vec<c_char> = vec![0; 256];
    #[cfg(target_os = "linux")]
    let entry = {
        let mut entry: libc::hostent = crate::sys::zeroed();
        let mut result = std::ptr::null_mut();
        let mut error = 0;
        loop {
            let rc = crate::lent::lend_out(&mut buffer, |at, length| {
                crate::lent::gethostbyname2_r(
                    name.into(),
                    family,
                    &mut entry,
                    at,
                    length,
                    &mut result,
                    &mut error,
                )
            });
            if rc == libc::ERANGE {
                buffer.resize(buffer.len() * 2, 0);
                continue;
            }
            if rc != 0 || result.is_null() {
                return None;
            }
            break;
        }
        entry
    };
    // Held until the shared result is read below.
    #[cfg(not(target_os = "linux"))]
    let _held = LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    #[cfg(not(target_os = "linux"))]
    let entry = {
        // SAFETY: a terminated name; the lock serializes gethostbyname2's shared result.
        let entry = unsafe { gethostbyname2(name.as_ptr(), family) };
        // SAFETY: a non-null result is the resolver's entry, read under the lock.
        *unsafe { entry.as_ref() }?
    };
    let length = usize::try_from(entry.h_length).unwrap_or(0).min(46);
    let mut bytes: [c_char; 46] = [0; 46];
    // The resolver filled `entry`.
    if let Some(first) = Hostent(&entry).addresses().next() {
        for (byte, &address) in bytes.iter_mut().zip(first) {
            *byte = c_char::from_ne_bytes([address]);
        }
    }
    Some((bytes, length))
}

/// What `getaddrinfo` found.
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
    let mut list = None;
    let error = getaddrinfo(
        host.into(),
        service.map(Text::from),
        &Hints::new(hints),
        &mut list,
    );
    if error != 0 {
        return Resolved { error, count: 0 };
    }
    let mut count = 0;
    let mut diagnostic_failed = false;
    let mut entry = list
        .as_ref()
        .map_or(std::ptr::null_mut(), |list| list.0.as_ptr());
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
    if let Some(list) = list {
        freeaddrinfo(list);
    }
    Resolved {
        error: if diagnostic_failed {
            DIAGNOSTIC_FAILED
        } else {
            0
        },
        count: c_int::try_from(count).unwrap_or(0),
    }
}

/// `network::gethostbyname(flags, name, addr)`: the first address of `name` for the
/// flags' family, in room for any family's address; none if it does not resolve.
#[must_use]
pub fn gethostbyname(flags: c_int, name: &CStr) -> Option<[c_char; 46]> {
    lookup(flags, name).map(|(binary, _)| binary)
}

/// `inet_ntop`'s output size: the one value, the length of the text array it fills
/// (`INET6_ADDRSTRLEN`).
#[repr(u32)]
#[derive(Clone, Copy)]
enum TextRoom {
    Address = 46,
}

/// `getaddrinfo`'s hints: an `addrinfo` whose pointers are null, as POSIX requires of
/// every hint but the family, socket type, protocol and flags. Only [`Hints::new`]
/// makes one.
#[repr(transparent)]
struct Hints(libc::addrinfo);

impl Hints {
    /// The family, socket type and protocol, flags zero.
    fn new((family, socktype, protocol): (c_int, c_int, c_int)) -> Self {
        let mut request: libc::addrinfo = crate::sys::zeroed();
        (request.ai_family, request.ai_socktype, request.ai_protocol) =
            (family, socktype, protocol);
        Self(request)
    }
}

/// The head of a list `getaddrinfo` stored, not yet freed. Only `getaddrinfo` makes one
/// (nothing here constructs it) and only `freeaddrinfo` takes it, by value, so each
/// list is freed at most once.
#[repr(transparent)]
struct AddressList(std::ptr::NonNull<libc::addrinfo>);

// libc's. Each reads its integers and `Text`s, writes the one object a reference names
// and reads or writes no other caller memory, so they are declared `safe`.
unsafe extern "C" {
    /// Writes the text of `source`'s address (4 bytes for `AF_INET`, 16 for
    /// `AF_INET6`; another family reads nothing and fails) into at most `capacity`
    /// bytes of `output`, which `TextRoom` fixes at its length: `output`, or null.
    safe fn inet_ntop(
        family: c_int,
        source: &[c_char; 16],
        output: &mut [c_char; TextRoom::Address as usize],
        capacity: TextRoom,
    ) -> *const c_char;
    /// Resolves `host` and `service` (none for none) under `hints`, storing a new
    /// list (none for an empty one) when it returns 0.
    safe fn getaddrinfo(
        host: Text<'_>,
        service: Option<Text<'_>>,
        hints: &Hints,
        list: &mut Option<AddressList>,
    ) -> c_int;
    /// Frees a list `getaddrinfo` stored, which the handle's move hands over.
    safe fn freeaddrinfo(list: AddressList);
}

/// `inet_ntop(family, binary)`'s text, or none for another family or a failure.
#[must_use]
pub fn ntop(family: c_int, binary: &[c_char; 16]) -> Option<[c_char; 46]> {
    let mut text: [c_char; 46] = [0; 46];
    let result = inet_ntop(family, binary, &mut text, TextRoom::Address);
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
