//! Static path/selection policy with native filesystem and allocation boundaries.
use crate::{INVALID, OVERFLOW};

/// The next effect consumed by static-file path selection.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum StaticPhase {
    #[default]
    Root,
    Locations,
    Stat,
    CanonicalPath,
    CanonicalLink,
    TargetStat,
    Mime,
    Done,
}

#[derive(Default)]
pub struct HttpStaticPath {
    pub phase: StaticPhase,
    pub last: bool,
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
    if state.phase != StaticPhase::Locations {
        return Err(INVALID);
    }
    let found = prefix(url, location, true);
    if found {
        state.last = true;
    }
    Ok(found)
}
/// What the path policy asks for next.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Action {
    /// Selection ended: whether the file is served.
    Done(bool),
    /// Visit the configured locations.
    Locations,
    BadRequest,
    Lstat,
    Realpath,
    /// Look up the filename's MIME type.
    Mime,
    /// `catch_error`.
    Catch,
}
fn selected(state: &mut HttpStaticPath, facts: u32) -> Action {
    if facts & DIRECTORY != 0 && facts & (INDEX | AUTOINDEX) != 0 {
        state.phase = StaticPhase::Done;
        Action::Done(true)
    } else {
        state.phase = StaticPhase::Mime;
        Action::Mime
    }
}
/// Resume selection into the filename buffer `output`: the next [`Action`], or
/// `INVALID`/`OVERFLOW`. Facts: locations1, statOK2, canonicalOK4, link8, dir16, file32, index64, auto128, MIME256.
/// The caller keeps the URL, root and state between steps, and NUL-terminates a
/// canonical path it writes into `output` before resuming a realpath step.
pub(crate) fn path_step(
    url: &[u8],
    root: &[u8],
    output: &mut Vec<u8>,
    capacity: usize,
    state: &mut HttpStaticPath,
    facts: u32,
) -> Result<Action, i32> {
    match state.phase {
        StaticPhase::Root => {
            let url_length = url
                .iter()
                .position(|&b| b == b'?')
                .or_else(|| url.iter().position(|&b| b == b'#'))
                .unwrap_or(url.len());
            state.url_length = url_length;
            state.phase = StaticPhase::Locations;
            return Ok(Action::Locations);
        }
        StaticPhase::Locations => {
            if facts & LOCATIONS != 0 && !state.last {
                state.phase = StaticPhase::Done;
                return Ok(Action::Done(false));
            }
            let Some(url) = url.get(..state.url_length) else {
                return Err(INVALID);
            };
            let Some(length) = root.len().checked_add(url.len()).filter(|&n| n < capacity) else {
                state.phase = StaticPhase::Done;
                return Ok(Action::Catch);
            };
            let path = span(output, capacity, 0, length + 1).ok_or(OVERFLOW)?;
            let (prefix, tail) = path.split_at_mut(root.len());
            prefix.copy_from_slice(root);
            let mut bytes = url.iter().copied();
            let mut written = 0;
            for slot in tail {
                let Some(byte) = bytes.next() else {
                    *slot = 0;
                    break;
                };
                let decoded = if byte == b'%' {
                    bytes
                        .next()
                        .and_then(crate::hex)
                        .zip(bytes.next().and_then(crate::hex))
                        .map(|(high, low)| (high << 4) | low)
                } else {
                    Some(byte)
                };
                let Some(byte) = decoded.filter(|&b| b != 0) else {
                    state.phase = StaticPhase::Done;
                    return Ok(Action::BadRequest);
                };
                *slot = byte;
                written += 1;
            }
            state.filename_length = root.len() + written;
            state.phase = StaticPhase::Stat;
            return Ok(Action::Lstat);
        }
        StaticPhase::Stat
        | StaticPhase::CanonicalPath
        | StaticPhase::CanonicalLink
        | StaticPhase::TargetStat
        | StaticPhase::Mime => {}
        StaticPhase::Done => return Err(INVALID),
    }
    if state.filename_length >= capacity {
        return Err(INVALID);
    }
    let filename = output.get(..state.filename_length).unwrap_or_default();
    match state.phase {
        StaticPhase::Stat => {
            if facts & STAT_OK == 0 {
                state.phase = StaticPhase::Done;
                return Ok(Action::Catch);
            }
            if filename.windows(2).any(|v| v == b"..") {
                state.phase = StaticPhase::CanonicalPath;
                Ok(Action::Realpath)
            } else if facts & LINK != 0 {
                state.phase = StaticPhase::CanonicalLink;
                Ok(Action::Realpath)
            } else {
                Ok(selected(state, facts))
            }
        }
        StaticPhase::CanonicalPath => {
            if facts & CANONICAL_OK == 0
                || filename.get(root.len()) != Some(&b'/')
                || !prefix(filename, root, false)
            {
                state.phase = StaticPhase::Done;
                Ok(Action::Catch)
            } else {
                Ok(selected(state, facts))
            }
        }
        StaticPhase::CanonicalLink => {
            if facts & CANONICAL_OK == 0 {
                state.phase = StaticPhase::Done;
                Ok(Action::Catch)
            } else {
                state.phase = StaticPhase::TargetStat;
                Ok(Action::Lstat)
            }
        }
        StaticPhase::TargetStat => {
            if facts & STAT_OK == 0 {
                state.phase = StaticPhase::Done;
                Ok(Action::Catch)
            } else {
                Ok(selected(state, facts))
            }
        }
        StaticPhase::Mime => {
            state.phase = StaticPhase::Done;
            Ok(Action::Done(
                (facts & MIME != 0 || state.last) && facts & FILE != 0,
            ))
        }
        _ => Err(INVALID),
    }
}

/// Keep the filename allocation within the native path buffer, including its NUL.
#[cfg_attr(
    protocol_buffer_lint,
    expect(
        clippy::disallowed_methods,
        reason = "the requested filename size is checked against the native path capacity"
    )
)]
fn reserve_filename(output: &mut Vec<u8>, capacity: usize, to: usize) -> Option<()> {
    if to > capacity {
        return None;
    }
    output
        .try_reserve_exact(to.saturating_sub(output.len()))
        .ok()
}

/// `output[from..to]`, zero-filled past what was written so far; None past `capacity`
/// or when allocation fails. Reserve only what is written: most requests miss the
/// configured static locations after copying just the root. Zeroing `PATH_MAX` up front
/// cost every request 4 KiB of memset, where upstream's `char filename[PATH_MAX]` is
/// left uninitialised.
pub(crate) fn span(
    output: &mut Vec<u8>,
    capacity: usize,
    from: usize,
    to: usize,
) -> Option<&mut [u8]> {
    reserve_filename(output, capacity, to)?;
    if output.len() < to {
        #[cfg_attr(
            protocol_buffer_lint,
            expect(
                clippy::disallowed_methods,
                reason = "reserve_filename admitted this length within the native path capacity"
            )
        )]
        output.resize(to, 0);
    }
    output.get_mut(from..to)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn query_bytes_do_not_size_the_filename_buffer() {
        let mut url = b"/a?".to_vec();
        url.extend_from_slice(&[b'x'; 1024]);
        let mut out = Vec::new();
        let mut state = HttpStaticPath::default();
        assert_eq!(
            path_step(&url, b"/r", &mut out, 4096, &mut state, 0),
            Ok(Action::Locations)
        );
        assert!(out.capacity() < url.len());
        assert_eq!(
            path_step(&url, b"/r", &mut out, 4096, &mut state, 0),
            Ok(Action::Lstat)
        );
        assert_eq!(out, b"/r/a\0");
    }

    #[test]
    fn selected_url_is_decoded_after_matching_the_location() {
        let mut out = vec![0xa5; 64];
        let mut state = HttpStaticPath {
            filename_length: 12,
            ..Default::default()
        };
        let url = b"/a%2eb#before?query";
        let root = b"/+%";
        assert_eq!(
            path_step(url, root, &mut out, 64, &mut state, LOCATIONS),
            Ok(Action::Locations)
        );
        assert_eq!(out, [0xa5; 64]);
        assert_eq!(location_match(url, b"/A", &mut state), Ok(true));
        assert_eq!(
            path_step(url, root, &mut out, 64, &mut state, LOCATIONS),
            Ok(Action::Lstat)
        );
        assert_eq!(state.filename_length, 14);
        assert_eq!(&out[..15], b"/+%/a.b#before\0");
        assert_eq!(
            path_step(url, root, &mut out, 64, &mut state, STAT_OK | FILE),
            Ok(Action::Mime)
        );
        assert_eq!(
            path_step(url, root, &mut out, 64, &mut state, STAT_OK | FILE),
            Ok(Action::Done(true))
        );
    }
    #[test]
    fn location_rejection_precedes_root_size_checks() {
        assert!(prefix(b"/a\0different", b"/A\0suffix", true));
        assert!(!prefix(b"/A/path", b"/a", false));
        let mut state = HttpStaticPath::default();
        let mut out = Vec::new();
        assert_eq!(
            path_step(b"/", b"oversized", &mut out, 4, &mut state, 0),
            Ok(Action::Locations)
        );
        assert_eq!(
            path_step(b"/", b"oversized", &mut out, 4, &mut state, LOCATIONS),
            Ok(Action::Done(false))
        );
        state = HttpStaticPath::default();
        assert_eq!(
            path_step(b"x", b"1234", &mut out, 4, &mut state, 0),
            Ok(Action::Locations)
        );
        assert_eq!(
            path_step(b"x", b"1234", &mut out, 4, &mut state, 0),
            Ok(Action::Catch)
        );
        assert_eq!(out, []);
    }
}
