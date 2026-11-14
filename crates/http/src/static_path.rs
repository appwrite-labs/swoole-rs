//! Static path/selection policy with native filesystem and allocation boundaries.
use crate::{INVALID, OVERFLOW};

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
/// A configured location against the URL, in the locations step: true, recording
/// the path as the request's last, when the URL starts with it (any case).
///
/// # Errors
/// `INVALID` outside the locations step.
pub fn location_match(
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
/// Resume selection into the filename buffer `output`: 0=false, 1=true, 2=visit
/// locations, 3=assign `dir_path`, 4=lstat, 5=realpath, 6=MIME lookup, 7=`catch_error`.
/// Facts: locations1, statOK2, canonicalOK4, link8, dir16, file32, index64, auto128, MIME256.
/// The caller keeps the URL, root and state between steps, and NUL-terminates a
/// canonical path it writes into `output` before resuming a realpath step.
pub(crate) fn path_step(
    url: &[u8],
    root: &[u8],
    output: &mut Vec<u8>,
    capacity: usize,
    state: &mut HttpStaticPath,
    facts: u32,
) -> i32 {
    match state.phase {
        0 => {
            // Old root copy precedes location rejection. Reject only its old
            // out-of-allocation case; exact-capacity copies remain observable.
            let Some(head) = span(output, capacity, 0, root.len()) else {
                return OVERFLOW;
            };
            state.url_length = url
                .iter()
                .position(|&b| b == b'?')
                .or_else(|| url.iter().position(|&b| b == b'#'))
                .unwrap_or(url.len());
            head.copy_from_slice(root);
            state.phase = 1;
            return 2;
        }
        1 => {
            if facts & LOCATIONS != 0 && state.last == 0 {
                state.phase = 8;
                return 0;
            }
            let Some(url) = url.get(..state.url_length) else {
                return INVALID;
            };
            let Some(length) = root.len().checked_add(url.len()).filter(|&n| n < capacity) else {
                state.phase = 8;
                return 7;
            };
            let Some((nul, path)) =
                span(output, capacity, root.len(), length + 1).and_then(<[u8]>::split_last_mut)
            else {
                return INVALID;
            };
            path.copy_from_slice(url);
            *nul = 0;
            state.phase = 2;
            return 3;
        }
        2 => {
            if state.url_length > url.len() {
                return INVALID;
            }
            if state.url_length > url.len() {
                return INVALID;
            }
            let Some(path) = root
                .len()
                .checked_add(state.url_length)
                .filter(|&n| n < capacity)
                .and_then(|length| output.get_mut(..length))
            else {
                return INVALID;
            };
            let length = crate::url_decode(path);
            if let Some(nul) = output.get_mut(length) {
                *nul = 0;
            }
            state.filename_length = length;
            state.phase = 3;
            return 4;
        }
        3..=7 => {}
        _ => return INVALID,
    }
    if state.filename_length >= capacity {
        return INVALID;
    }
    let filename = output.get(..state.filename_length).unwrap_or_default();
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

/// `output[from..to]`, zero-filled past what was written so far; None past `capacity`.
/// The buffer grows only as far as it is written: zeroing all `PATH_MAX` bytes up front
/// cost every request 4 KiB of memset, where upstream's `char filename[PATH_MAX]` is
/// left uninitialised.
pub(crate) fn span(
    output: &mut Vec<u8>,
    capacity: usize,
    from: usize,
    to: usize,
) -> Option<&mut [u8]> {
    if to > capacity {
        return None;
    }
    if output.len() < to {
        output.resize(to, 0);
    }
    output.get_mut(from..to)
}

/// Append an index filename using the pinned logical-length/slash behavior.
/// Logical length intentionally stays unchanged in the native class.
pub(crate) fn static_index_path(
    output: &mut Vec<u8>,
    capacity: usize,
    logical_length: usize,
    name: &[u8],
) -> i32 {
    let Some(room) = capacity
        .checked_sub(logical_length)
        .filter(|&room| room > 0)
    else {
        return INVALID;
    };
    let slash = output.get(logical_length) != Some(&b'/');
    let length = usize::from(slash) + name.len();
    if length >= room {
        return OVERFLOW;
    }
    let Some(rest) = span(
        output,
        capacity,
        logical_length,
        logical_length + length + 1,
    ) else {
        return OVERFLOW;
    };
    let tail = slash
        .then_some(b'/')
        .into_iter()
        .chain(name.iter().copied())
        .chain([0]);
    for (slot, byte) in rest.iter_mut().zip(tail) {
        *slot = byte;
    }
    0
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn index_path_appends_after_one_slash() {
        let mut out = b"/a\0\0\0\0\0\0".to_vec();
        assert_eq!(static_index_path(&mut out, 8, 2, b"ix"), 0);
        assert_eq!(out, b"/a/ix\0\0\0");
        let mut out = b"/a/\0\0\0".to_vec();
        assert_eq!(static_index_path(&mut out, 6, 2, b"ix"), 0);
        assert_eq!(out, b"/aix\0\0");
        assert_eq!(static_index_path(&mut out, 6, 2, b"ixxx"), OVERFLOW);
        assert_eq!(static_index_path(&mut out, 6, 6, b""), INVALID);
        let mut out = b"/a".to_vec();
        assert_eq!(static_index_path(&mut out, 8, 2, b"ix"), 0);
        assert_eq!(out, b"/a/ix\0");
    }
    #[test]
    fn path_publication_waits_for_native_directory_allocation() {
        let mut out = vec![0xa5; 64];
        let mut state = HttpStaticPath {
            filename_length: 12,
            ..Default::default()
        };
        let url = b"/a%2eb#before?query";
        let root = b"/r";
        assert_eq!(path_step(url, root, &mut out, 64, &mut state, LOCATIONS), 2);
        assert_eq!(&out[..3], &[b'/', b'r', 0xa5]);
        assert_eq!(state.filename_length, 12);
        assert_eq!(location_match(url, b"/A", &mut state), Ok(true));
        assert_eq!(path_step(url, root, &mut out, 64, &mut state, LOCATIONS), 3);
        assert_eq!(&out[..17], b"/r/a%2eb#before\0\xa5");
        assert_eq!(state.filename_length, 12);
        assert_eq!(path_step(url, root, &mut out, 64, &mut state, LOCATIONS), 4);
        assert_eq!(state.filename_length, 13);
        assert_eq!(&out[..14], b"/r/a.b#before\0");
        assert_eq!(
            path_step(url, root, &mut out, 64, &mut state, STAT_OK | FILE),
            6
        );
        assert_eq!(
            path_step(url, root, &mut out, 64, &mut state, STAT_OK | FILE),
            1
        );
    }
    #[test]
    fn c_string_prefix_rules_and_root_copy_overflow_are_explicit() {
        assert!(prefix(b"/a\0different", b"/A\0suffix", true));
        assert!(!prefix(b"/A/path", b"/a", false));
        let mut out = vec![0xa5; 4];
        let mut state = HttpStaticPath::default();
        assert_eq!(
            path_step(b"/", b"oversized", &mut out, 4, &mut state, 0),
            OVERFLOW
        );
        assert_eq!(out, [0xa5; 4]);
        assert_eq!(state.phase, 0);
        assert_eq!(path_step(b"x", b"1234", &mut out, 4, &mut state, 0), 2);
        assert_eq!(out, *b"1234");
        assert_eq!(path_step(b"x", b"1234", &mut out, 4, &mut state, 0), 7);
    }
}
