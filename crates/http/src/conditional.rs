//! Conditional-date policy over a [`Calendar`].

use std::ffi::CStr;
use swoole_rs_locale as locale;
pub use swoole_rs_locale::Tm;
static FORMATS: [&CStr; 4] = [
    c"%a, %d %b %Y %T GMT",
    c"%a, %d %b %Y %T UTC",
    c"%A, %d-%b-%y %T GMT",
    c"%a %b %e %T %Y",
];
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

/// Which conditional header a date comes from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Conditional {
    /// `If-Modified-Since`, read in the server's timezone.
    ModifiedSince,
    /// `If-Range`.
    Range,
}

/// Upstream's `is_modified` (`If-Modified-Since`: `date`, less `timezone`, is not
/// before the file's modification time `mtime`) and `is_modified_range` (an
/// `If-Range` date other than the file's). The date is read as a C string, in the
/// first of [`FORMATS`] that parses it. Empty and oversized dates finish before any
/// calendar call.
pub fn conditional(
    date: &[u8],
    kind: Conditional,
    mtime: i64,
    timezone: i64,
    calendar: &impl Calendar,
) -> bool {
    if date.is_empty() || kind == Conditional::ModifiedSince && date.len() > 63 {
        return false;
    }
    let text = [date, b"\0"].concat();
    let Ok(text) = CStr::from_bytes_until_nul(&text) else {
        return false;
    };
    let mut parsed = locale::zeroed();
    let found = FORMATS
        .iter()
        .any(|format| calendar.parse(text, format, &mut parsed));
    match kind {
        Conditional::ModifiedSince => {
            found
                && (calendar.to_time(&mut parsed).checked_sub(timezone))
                    .is_some_and(|time| time >= mtime)
        }
        Conditional::Range => {
            // Upstream reads the file's date even when no format parsed.
            let file = calendar.to_gmt(mtime);
            let (true, Some(mut file)) = (found, file) else {
                return false;
            };
            calendar.to_time(&mut parsed) != calendar.to_time(&mut file)
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
        for kind in [Conditional::ModifiedSince, Conditional::Range] {
            assert!(!conditional(b"", kind, 0, 0, &Unreachable));
        }
        let long = [b'x'; 64];
        assert!(!conditional(
            &long,
            Conditional::ModifiedSince,
            0,
            0,
            &Unreachable
        ));
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
    /// A calendar that parses when `parses`, answers `time` from `to_time`, and
    /// records each call.
    struct Script {
        parses: bool,
        time: i64,
        calls: std::cell::RefCell<Vec<&'static str>>,
    }
    impl Script {
        fn new(parses: bool, time: i64) -> Self {
            Self {
                parses,
                time,
                calls: std::cell::RefCell::default(),
            }
        }
        fn calls(&self) -> Vec<&'static str> {
            self.calls.borrow().clone()
        }
    }
    impl Calendar for Script {
        fn parse(&self, _: &CStr, _: &CStr, _: &mut Tm) -> bool {
            self.calls.borrow_mut().push("parse");
            self.parses
        }
        fn to_time(&self, _: &mut Tm) -> i64 {
            self.calls.borrow_mut().push("to_time");
            self.time
        }
        fn to_gmt(&self, _: i64) -> Option<Tm> {
            self.calls.borrow_mut().push("to_gmt");
            Some(locale::zeroed())
        }
    }
    #[test]
    fn range_parse_failure_still_requests_gmtime_and_since_does_not() {
        let since = Script::new(false, 0);
        assert!(!conditional(
            b"yesterday",
            Conditional::ModifiedSince,
            0,
            0,
            &since
        ));
        assert_eq!(since.calls(), ["parse"; 4]);
        let range = Script::new(false, 0);
        assert!(!conditional(b"yesterday", Conditional::Range, 0, 0, &range));
        assert_eq!(
            range.calls(),
            ["parse", "parse", "parse", "parse", "to_gmt"]
        );
    }
    #[test]
    fn signed_time_comparison_is_checked() {
        let since = |time, timezone, mtime| {
            conditional(
                b"today",
                Conditional::ModifiedSince,
                mtime,
                timezone,
                &Script::new(true, time),
            )
        };
        assert!(!since(i64::MIN, 1, i64::MIN), "an overflowing date fails");
        assert!(since(100, 1, 99));
        assert!(!since(100, 1, 100));
        assert!(!conditional(
            &[b'x'; 64],
            Conditional::ModifiedSince,
            0,
            0,
            &Unreachable
        ));
        assert!(FORMATS.get(4).is_none());
    }
}
