//! Static path/selection policy with native filesystem and allocation boundaries.
use crate::{INVALID, OVERFLOW, bytes, swr_v1_http_url_decode};

#[repr(C)]
#[derive(Default)]
pub struct HttpStaticPath {
    pub phase: u32,
    pub last: u32,
    pub url_length: usize,
    pub filename_length: usize,
}
pub(crate) const LOCATIONS: u32 = 1;
pub(crate) const STAT_OK: u32 = 2;
pub(crate) const CANONICAL_OK: u32 = 4;
pub(crate) const LINK: u32 = 8;
pub(crate) const DIRECTORY: u32 = 16;
pub(crate) const FILE: u32 = 32;
pub(crate) const INDEX: u32 = 64;
pub(crate) const AUTOINDEX: u32 = 128;
pub(crate) const MIME: u32 = 256;
pub(crate) fn prefix(input: &[u8], expected: &[u8], ignore_case: bool) -> bool {
    if input.len() < expected.len() {
        return false;
    }
    if ignore_case {
        return crate::case_prefix(input, expected);
    }
    for (&left, &right) in input.iter().zip(expected) {
        if left != right {
            return false;
        }
        // Native strncasecmp/strncmp stop at a matching NUL, not at span end.
        if left == 0 {
            break;
        }
    }
    true
}
/// Feed a native-owned location entry between path steps, preserving the
/// full untrimmed URL comparison and monotonically set `last` flag.
/// # Safety
/// Input spans are initialized; aligned state is writable and disjoint. No
/// allocation, callback or retained Rust reference occurs.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_http_static_location(
    url: *const u8,
    url_length: usize,
    location: *const u8,
    location_length: usize,
    state: *mut HttpStaticPath,
) -> i32 {
    if state.is_null() {
        return INVALID;
    }
    // SAFETY: the caller lends `url_length` initialized URL bytes.
    let url = unsafe { bytes(url, url_length) };
    // SAFETY: the caller lends `location_length` initialized location bytes.
    let location = unsafe { bytes(location, location_length) };
    let (url, location) = match (url, location) {
        (Ok(a), Ok(b)) => (a, b),
        (Err(e), _) | (_, Err(e)) => return e,
    };
    // SAFETY: `state` is non-null (checked above), aligned and exclusive per the caller contract.
    match location_match(url, location, unsafe { &mut *state }) {
        Ok(found) => i32::from(found),
        Err(error) => error,
    }
}
pub(crate) fn location_match(
    url: &[u8],
    location: &[u8],
    state: &mut HttpStaticPath,
) -> Result<bool, i32> {
    if state.phase != 1 {
        return Err(INVALID);
    }
    let found = prefix(url, location, true);
    if found {
        state.last = 1;
    }
    Ok(found)
}
fn selected(state: &mut HttpStaticPath, facts: u32) -> i32 {
    if facts & DIRECTORY != 0 && facts & (INDEX | AUTOINDEX) != 0 {
        state.phase = 8;
        1
    } else {
        state.phase = 7;
        6
    }
}
/// Resume selection: 0=false,1=true,2=visit locations,3=assign native `dir_path`,
/// `4=lstat,5=realpath/get_absolute_path,6=MIME` lookup,7=native `catch_error`.
/// Facts: locations1,statOK2,canonicalOK4,link8,dir16,file32,index64,auto128,MIME256.
/// # Safety
/// URL/root spans are initialized and disjoint from output/state. Output is
/// writable through capacity, initially may be uninitialized. Native preserves
/// inputs/state between steps and initializes canonical output through its NUL
/// before resuming a realpath step. No borrow survives native work.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_http_static_path(
    url: *const u8,
    url_length: usize,
    root: *const u8,
    root_length: usize,
    output: *mut u8,
    capacity: usize,
    state: *mut HttpStaticPath,
    facts: u32,
) -> i32 {
    if state.is_null() || output.is_null() {
        return INVALID;
    }
    if capacity > isize::MAX as usize {
        return OVERFLOW;
    }
    // SAFETY: the caller lends `url_length` initialized URL bytes.
    let url = unsafe { bytes(url, url_length) };
    // SAFETY: the caller lends `root_length` initialized root bytes.
    let root = unsafe { bytes(root, root_length) };
    let (url, root) = match (url, root) {
        (Ok(a), Ok(b)) => (a, b),
        (Err(e), _) | (_, Err(e)) => return e,
    };
    // SAFETY: `state` is non-null (checked above), aligned and exclusive.
    let state = unsafe { &mut *state };
    // SAFETY: `output` is non-null and writable through `capacity <= isize::MAX`, disjoint
    // from the inputs and state; the phase contract is static_path's.
    unsafe { path_step(url, root, output, capacity, state, facts) }
}

// Inputs and state are already typed and checked by the public C boundary or
// the Rust configuration owner. Only the caller-owned possibly uninitialized
// filename backing requires unsafe access; its phase/publication contract is
// exactly static_path's existing contract. No borrow survives Native effects.
pub(crate) unsafe fn path_step(
    url: &[u8],
    root: &[u8],
    output: *mut u8,
    capacity: usize,
    state: &mut HttpStaticPath,
    facts: u32,
) -> i32 {
    match state.phase {
        0 => {
            // Old root copy precedes location rejection. Reject only its old
            // out-of-allocation case; exact-capacity copies remain observable.
            if root.len() > capacity {
                return OVERFLOW;
            }
            state.url_length = url
                .iter()
                .position(|&b| b == b'?')
                .or_else(|| url.iter().position(|&b| b == b'#'))
                .unwrap_or(url.len());
            // SAFETY: `root.len() <= capacity` writable output bytes, disjoint from `root`.
            unsafe { std::ptr::copy_nonoverlapping(root.as_ptr(), output, root.len()) };
            state.phase = 1;
            return 2;
        }
        1 => {
            if facts & LOCATIONS != 0 && state.last == 0 {
                state.phase = 8;
                return 0;
            }
            if state.url_length > url.len() {
                return INVALID;
            }
            let Some(length) = root
                .len()
                .checked_add(state.url_length)
                .filter(|&n| n < capacity)
            else {
                state.phase = 8;
                return 7;
            };
            // `root.len() + url_length == length < capacity` writable output bytes.
            // SAFETY: as above; `url_length <= url.len()` readable bytes.
            unsafe {
                std::ptr::copy_nonoverlapping(
                    url.as_ptr(),
                    output.wrapping_add(root.len()),
                    state.url_length,
                );
            };
            // SAFETY: as above.
            unsafe { output.wrapping_add(length).write(0) };
            state.phase = 2;
            return 3;
        }
        2 => {
            if state.url_length > url.len() {
                return INVALID;
            }
            let Some(length) = root
                .len()
                .checked_add(state.url_length)
                .filter(|&n| n < capacity)
            else {
                return INVALID;
            };
            // SAFETY: phase 1 initialized `length + 1 <= capacity` output bytes.
            let result =
                unsafe { swr_v1_http_url_decode(output, length, &raw mut state.filename_length) };
            if result != 0 {
                return result;
            }
            state.phase = 3;
            return 4;
        }
        3..=7 => {}
        _ => return INVALID,
    }
    if state.filename_length >= capacity {
        return INVALID;
    }
    // SAFETY: `filename_length < capacity`, and earlier phases (or native canonicalization)
    // initialized those bytes.
    let filename = unsafe { std::slice::from_raw_parts(output, state.filename_length) };
    match state.phase {
        3 => {
            if facts & STAT_OK == 0 {
                state.phase = 8;
                return 7;
            }
            if filename.windows(2).any(|v| v == b"..") {
                state.phase = 4;
                5
            } else if facts & LINK != 0 {
                state.phase = 5;
                5
            } else {
                selected(state, facts)
            }
        }
        4 => {
            if facts & CANONICAL_OK == 0
                || filename.get(root.len()) != Some(&b'/')
                || !prefix(filename, root, false)
            {
                state.phase = 8;
                7
            } else {
                selected(state, facts)
            }
        }
        5 => {
            if facts & CANONICAL_OK == 0 {
                state.phase = 8;
                7
            } else {
                state.phase = 6;
                4
            }
        }
        6 => {
            if facts & STAT_OK == 0 {
                state.phase = 8;
                7
            } else {
                selected(state, facts)
            }
        }
        7 => {
            state.phase = 8;
            i32::from((facts & MIME != 0 || state.last != 0) && facts & FILE != 0)
        }
        _ => INVALID,
    }
}

/// Append an index filename using the pinned logical-length/slash behavior.
/// Logical length intentionally stays unchanged in the native class.
/// # Safety
/// Storage is writable through capacity; byte at `logical_length` is initialized.
/// Name is readable/initialized and disjoint from storage. No callback occurs.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_http_static_index_path(
    output: *mut u8,
    capacity: usize,
    logical_length: usize,
    name: *const u8,
    name_length: usize,
) -> i32 {
    if output.is_null() || logical_length >= capacity {
        return INVALID;
    }
    if capacity > isize::MAX as usize {
        return OVERFLOW;
    }
    // SAFETY: the caller lends `name_length` initialized bytes at `name` (null only when empty).
    let name = match unsafe { bytes(name, name_length) } {
        Ok(v) => v,
        Err(e) => return e,
    };
    // SAFETY: `logical_length < capacity`, and the contract makes that byte initialized.
    let slash = unsafe { output.wrapping_add(logical_length).read() } != b'/';
    let Some(end) = logical_length
        .checked_add(usize::from(slash))
        .and_then(|n| n.checked_add(name.len()))
        .filter(|&n| n < capacity)
    else {
        return OVERFLOW;
    };
    if slash {
        // SAFETY: as above; writable.
        unsafe { output.wrapping_add(logical_length).write(b'/') };
    }
    // `logical_length + slash + name.len() == end < capacity` writable bytes, disjoint
    // from `name`.
    // SAFETY: as above.
    unsafe {
        std::ptr::copy_nonoverlapping(
            name.as_ptr(),
            output.wrapping_add(logical_length + usize::from(slash)),
            name.len(),
        );
    };
    // SAFETY: as above.
    unsafe { output.wrapping_add(end).write(0) };
    0
}

#[cfg(test)]
mod tests {
    use super::*;
    unsafe fn step(
        url: &[u8],
        root: &[u8],
        out: &mut [u8],
        state: &mut HttpStaticPath,
        facts: u32,
    ) -> i32 {
        unsafe {
            swr_v1_http_static_path(
                url.as_ptr(),
                url.len(),
                root.as_ptr(),
                root.len(),
                out.as_mut_ptr(),
                out.len(),
                state,
                facts,
            )
        }
    }
    #[test]
    fn path_publication_waits_for_native_directory_allocation() {
        let mut out = [0xa5; 64];
        let mut state = HttpStaticPath {
            filename_length: 12,
            ..Default::default()
        };
        let url = b"/a%2eb#before?query";
        let root = b"/r";
        assert_eq!(
            unsafe { step(url, root, &mut out, &mut state, LOCATIONS) },
            2
        );
        assert_eq!(&out[..3], &[b'/', b'r', 0xa5]);
        assert_eq!(state.filename_length, 12);
        assert_eq!(
            unsafe {
                swr_v1_http_static_location(
                    url.as_ptr(),
                    url.len(),
                    b"/A".as_ptr(),
                    2,
                    &raw mut state,
                )
            },
            1
        );
        assert_eq!(
            unsafe { step(url, root, &mut out, &mut state, LOCATIONS) },
            3
        );
        assert_eq!(&out[..17], b"/r/a%2eb#before\0\xa5");
        assert_eq!(state.filename_length, 12);
        assert_eq!(
            unsafe { step(url, root, &mut out, &mut state, LOCATIONS) },
            4
        );
        assert_eq!(state.filename_length, 13);
        assert_eq!(&out[..14], b"/r/a.b#before\0");
        assert_eq!(
            unsafe { step(url, root, &mut out, &mut state, STAT_OK | FILE) },
            6
        );
        assert_eq!(
            unsafe { step(url, root, &mut out, &mut state, STAT_OK | FILE) },
            1
        );
    }
    #[test]
    fn c_string_prefix_rules_and_root_copy_overflow_are_explicit() {
        assert!(prefix(b"/a\0different", b"/A\0suffix", true));
        assert!(!prefix(b"/A/path", b"/a", false));
        let mut out = [0xa5; 4];
        let mut state = HttpStaticPath::default();
        assert_eq!(
            unsafe { step(b"/", b"oversized", &mut out, &mut state, 0) },
            OVERFLOW
        );
        assert_eq!(out, [0xa5; 4]);
        assert_eq!(state.phase, 0);
        assert_eq!(unsafe { step(b"x", b"1234", &mut out, &mut state, 0) }, 2);
        assert_eq!(out, *b"1234");
        assert_eq!(unsafe { step(b"x", b"1234", &mut out, &mut state, 0) }, 7);
    }
}
