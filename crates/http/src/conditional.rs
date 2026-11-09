//! Conditional-date policy over a [`Calendar`].
use crate::{INVALID, OVERFLOW};
use std::ffi::CStr;
use swoole_rs_locale as locale;
pub use swoole_rs_locale::Tm;
static FORMATS: [&CStr; 4] = [
    c"%a, %d %b %Y %T GMT",
    c"%a, %d %b %Y %T UTC",
    c"%A, %d-%b-%y %T GMT",
    c"%a %b %e %T %Y",
];
#[derive(Default)]
struct HttpConditional {
    phase: u32,
    kind: u32,
    valid: u32,
    length: usize,
    timezone: i64,
    file_mtime: i64,
    value: i64,
    parsed_time: i64,
}
/// Kind0=If-Modified-Since; kind1=If-Range mismatch. Returns native actions:
/// 0=false,1..4=strptime format0..3,5=gmtime(file),6=mktime(parsed),
/// 7=mktime(file GMT),8=true. Native stores result validity/value before resume.
/// State is retained unchanged except native result fields.
/// Native calendar objects survive between calls; Rust borrows none of them.
fn conditional_step(state: &mut HttpConditional) -> i32 {
    if state.kind > 1 {
        return INVALID;
    }
    match state.phase {
        0 => {
            if state.length > isize::MAX as usize {
                return OVERFLOW;
            }
            if state.length == 0 || state.kind == 0 && state.length > 63 {
                state.phase = 10;
                return 0;
            }
            state.phase = 1;
            1
        }
        1..=4 => {
            if state.valid != 0 {
                if state.kind == 0 {
                    state.phase = 6;
                    6
                } else {
                    state.phase = 5;
                    5
                }
            } else if state.phase < 4 {
                state.phase += 1;
                state.phase.cast_signed()
            } else if state.kind == 1 {
                state.phase = 9;
                5
            } else {
                state.phase = 10;
                0
            }
        }
        5 => {
            if state.valid == 0 {
                state.phase = 10;
                return 0;
            }
            state.phase = 6;
            6
        }
        6 => {
            if state.kind == 0 {
                state.phase = 10;
                state
                    .value
                    .checked_sub(state.timezone)
                    .map_or(
                        OVERFLOW,
                        |parsed| if parsed >= state.file_mtime { 8 } else { 0 },
                    )
            } else {
                state.parsed_time = state.value;
                state.phase = 7;
                7
            }
        }
        7 => {
            state.phase = 10;
            if state.parsed_time == state.value {
                0
            } else {
                8
            }
        }
        // Legacy If-Range calls gmtime even if all date formats failed.
        9 => {
            state.phase = 10;
            0
        }
        _ => INVALID,
    }
}
/// The calendar a conditional request's dates are parsed and compared in: libc's by
/// default, or one that records its calls (the static handler's tests).
pub trait Calendar {
    /// `strptime(date, format, tm)`: whether `date` parsed.
    fn parse(&self, date: &CStr, format: &CStr, tm: &mut Tm) -> bool {
        locale::strptime(date, format, tm)
    }
    /// `mktime(tm)`.
    fn to_time(&self, tm: &mut Tm) -> i64 {
        locale::mktime(tm)
    }
    /// `gmtime(&time)`, copied.
    fn to_gmt(&self, time: i64) -> Option<Tm> {
        locale::gmtime(time)
    }
}

/// Upstream's `is_modified` (kind 0: `If-Modified-Since`, with the server's
/// `timezone`) and `is_modified_range` (kind 1: an `If-Range` date other than the
/// file's) of `date` against the file's modification time `mtime`. The date is read
/// as a C string. Empty and oversized dates finish before any calendar call.
pub fn conditional(
    date: &[u8],
    kind: u32,
    mtime: i64,
    timezone: i64,
    calendar: &impl Calendar,
) -> bool {
    let text = [date, b"\0"].concat();
    let Ok(text) = CStr::from_bytes_until_nul(&text) else {
        return false;
    };
    let mut state = HttpConditional {
        kind,
        length: date.len(),
        ..HttpConditional::default()
    };
    let mut parsed = locale::zeroed();
    let mut file = None;
    loop {
        let action = conditional_step(&mut state);
        match action {
            1..=4 => {
                let format = FORMATS
                    .get((action - 1).cast_unsigned() as usize)
                    .copied()
                    .unwrap_or_default();
                state.valid = u32::from(calendar.parse(text, format, &mut parsed));
            }
            5 => {
                file = calendar.to_gmt(mtime);
                state.valid = u32::from(file.is_some());
            }
            6 => {
                state.value = calendar.to_time(&mut parsed);
                if kind == 0 {
                    state.timezone = timezone;
                    state.file_mtime = mtime;
                }
            }
            7 => {
                if let Some(file) = file.as_mut() {
                    state.value = calendar.to_time(file);
                }
            }
            _ => return action == 8,
        }
    }
}

/// `get_date()` (now, when `mtime` is None) or `get_date_last_modified()`: the time
/// as an HTTP date, empty when it has no calendar date.
#[must_use]
pub fn http_date(mtime: Option<i64>) -> Vec<u8> {
    let time = mtime.unwrap_or_else(locale::now);
    let Some(tm) = locale::gmtime(time) else {
        return Vec::new();
    };
    let mut value = [0u8; 64];
    let length = locale::strftime(&mut value, c"%a, %d %b %Y %H:%M:%S %Z", &tm);
    value.get(..length).unwrap_or_default().to_vec()
}

#[cfg(test)]
mod tests {
    use super::*;
    /// A calendar no call may reach.
    struct Unreachable;
    impl Calendar for Unreachable {
        fn parse(&self, _: &CStr, _: &CStr, _: &mut Tm) -> bool {
            panic!("parse")
        }
        fn to_time(&self, _: &mut Tm) -> i64 {
            panic!("to_time")
        }
        fn to_gmt(&self, _: i64) -> Option<Tm> {
            panic!("to_gmt")
        }
    }
    #[test]
    fn owned_calendar_keeps_empty_and_oversized_date_metadata_lazy() {
        for kind in [0, 1] {
            assert!(!conditional(b"", kind, 0, 0, &Unreachable));
        }
        assert!(!conditional(&[b'x'; 64], 0, 0, 0, &Unreachable));
    }
    #[test]
    fn http_dates_format_gmt() {
        // gmtime's zone is GMT in glibc and UTC in macOS's libc.
        let epoch = http_date(Some(0));
        assert!(
            epoch.starts_with(b"Thu, 01 Jan 1970 00:00:00 "),
            "{epoch:?}"
        );
        assert_eq!(epoch.len(), 29);
        assert_eq!(http_date(None).len(), 29);
    }
    #[test]
    fn range_parse_failure_still_requests_gmtime_and_since_does_not() {
        for kind in [0, 1] {
            let mut state = HttpConditional {
                kind,
                length: 12,
                ..Default::default()
            };
            for action in 1..=4 {
                assert_eq!(conditional_step(&mut state), action);
            }
            if kind == 1 {
                assert_eq!(conditional_step(&mut state), 5);
            }
            assert_eq!(conditional_step(&mut state), 0);
        }
    }
    #[test]
    fn signed_time_comparison_and_invalid_metadata_are_checked() {
        let mut state = HttpConditional {
            kind: 0,
            phase: 6,
            value: i64::MIN,
            timezone: 1,
            ..Default::default()
        };
        assert_eq!(conditional_step(&mut state), OVERFLOW);
        state = HttpConditional {
            kind: 0,
            phase: 6,
            value: 100,
            timezone: 1,
            file_mtime: 99,
            ..Default::default()
        };
        assert_eq!(conditional_step(&mut state), 8);
        state = HttpConditional {
            kind: 0,
            length: 64,
            ..Default::default()
        };
        assert_eq!(conditional_step(&mut state), 0);
        assert!(FORMATS.get(4).is_none());
    }
}
