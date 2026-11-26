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
    /// Interpret the fields as UTC.
    fn to_time(&self, tm: Tm) -> i64 {
        locale::timegm(tm)
    }
    /// `gmtime(&time)`, copied.
    fn to_gmt(&self, time: i64) -> Option<Tm> {
        locale::gmtime(time)
    }
}

/// Which conditional header a date comes from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Conditional {
    /// `If-Modified-Since`, compared in UTC.
    ModifiedSince,
    /// `If-Range`.
    Range,
}

fn parse_http_date(date: &[u8], calendar: &impl Calendar) -> Option<i64> {
    if date.is_empty() || date.len() > 63 {
        return None;
    }
    let mut text = [0; 64];
    text.get_mut(..date.len())?.copy_from_slice(date);
    let date = CStr::from_bytes_with_nul(text.get(..=date.len())?).ok()?;
    let fields = |tm: Tm| {
        (
            tm.tm_year, tm.tm_mon, tm.tm_mday, tm.tm_hour, tm.tm_min, tm.tm_sec,
        )
    };
    FORMATS.iter().find_map(|format| {
        let mut parsed = locale::zeroed();
        if !calendar.parse(date, format, &mut parsed) {
            return None;
        }
        let time = calendar.to_time(parsed);
        (fields(calendar.to_gmt(time)?) == fields(parsed)).then_some(time)
    })
}

/// Compare a complete HTTP date against the file modification time.
pub fn conditional(date: &[u8], kind: Conditional, mtime: i64, calendar: &impl Calendar) -> bool {
    let date = parse_http_date(date, calendar);
    match kind {
        Conditional::ModifiedSince => date.is_some_and(|time| time >= mtime),
        Conditional::Range => date != Some(mtime),
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
    let length = locale::strftime(&mut value, FORMATS[0], &tm);
    value.get(..length).unwrap_or_default().to_vec()
}

#[cfg(test)]
mod tests {
    use super::*;
    struct Native;
    impl Calendar for Native {}

    #[test]
    fn http_dates_are_utc_and_strict() {
        let mtime = 1_730_000_000;
        let date = b"Sun, 27 Oct 2024 03:33:20 GMT";
        assert_eq!(http_date(Some(mtime)), date);
        let now = http_date(None);
        assert_eq!(now.len(), 29);
        assert_eq!(now.get(25..), Some(b" GMT".as_slice()));
        assert!(conditional(
            date,
            Conditional::ModifiedSince,
            mtime,
            &Native
        ));
        for invalid in [
            b"".as_slice(),
            &[b'x'; 64],
            b"Sun, 27 Oct 2024 02:33:20 GMT",
            b"Sun, 31 Feb 2024 03:33:20 GMT",
            b"Sun, 27 Oct 2024 03:33:20 GMT trailing",
            b"Sun, 27 Oct 2024 03:33:20 GMT\0",
            b"invalid",
        ] {
            assert!(!conditional(
                invalid,
                Conditional::ModifiedSince,
                mtime,
                &Native
            ));
            assert!(conditional(invalid, Conditional::Range, mtime, &Native));
        }
        assert!(!conditional(date, Conditional::Range, mtime, &Native));
    }
}
