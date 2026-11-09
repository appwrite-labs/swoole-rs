//! Conditional-date policy; libc calendar objects remain in the native facade.
use crate::{INVALID, OVERFLOW};
use std::ffi::{CStr, c_char};
static FORMATS: [&CStr; 4] = [
    c"%a, %d %b %Y %T GMT",
    c"%a, %d %b %Y %T UTC",
    c"%A, %d-%b-%y %T GMT",
    c"%a %b %e %T %Y",
];
#[repr(C)]
#[derive(Default)]
pub struct HttpConditional {
    pub phase: u32,
    pub kind: u32,
    pub valid: u32,
    pub length: usize,
    pub timezone: i64,
    pub file_mtime: i64,
    pub value: i64,
    pub parsed_time: i64,
}
/// Native strptime format, immutable for process lifetime. Unknown index is null.
fn date_format(index: u32) -> *const c_char {
    FORMATS
        .get(index as usize)
        .map_or(std::ptr::null(), |format| format.as_ptr())
}
/// Kind0=If-Modified-Since; kind1=If-Range mismatch. Returns native actions:
/// 0=false,1..4=strptime format0..3,5=gmtime(file),6=mktime(parsed),
/// 7=mktime(file GMT),8=true. Native stores result validity/value before resume.
/// State is retained unchanged except native result fields.
/// Native calendar objects survive between calls; Rust borrows none of them.
fn conditional(state: &mut HttpConditional) -> i32 {
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
/// Optional calendar primitives for callers with an existing native calendar provider.
/// Production uses the same platform libc directly when this pointer is null.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct HttpCalendar {
    pub tm_size: usize,
    pub tm_align: usize,
    pub parse: unsafe extern "C" fn(*const c_char, *const c_char, *mut core::ffi::c_void) -> bool,
    pub to_time: unsafe extern "C" fn(*mut core::ffi::c_void) -> i64,
    pub to_gmt: unsafe extern "C" fn(i64) -> *mut core::ffi::c_void,
}

unsafe extern "C" fn calendar_parse(
    date: *const c_char,
    format: *const c_char,
    parsed: *mut core::ffi::c_void,
) -> bool {
    // SAFETY: the calendar protocol lends terminated input and one initialized libc tm.
    !unsafe { libc::strptime(date, format, parsed.cast()) }.is_null()
}
unsafe extern "C" fn calendar_time(parsed: *mut core::ffi::c_void) -> i64 {
    // SAFETY: the calendar protocol lends one initialized libc tm for normalization.
    unsafe { libc::mktime(parsed.cast()) }
}
unsafe extern "C" fn calendar_gmt(value: i64) -> *mut core::ffi::c_void {
    // SAFETY: value is a live time_t and the returned platform static view is used before another GMT call.
    unsafe { libc::gmtime(&raw const value) }.cast()
}

/// Execute the owned conditional policy and calendar operations. Empty/oversized
/// dates finish before either metadata slot is read, preserving lazy observation.
/// # Safety
/// Nonempty `date` names `length` bytes followed by a NUL; empty input may be null. Metadata slots are live whenever
/// requested by the policy. A non-null calendar is immutable for this call, uses
/// the matching libc tm layout and its callbacks do not unwind or bail out.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v3_http_conditional(
    date: *const c_char,
    length: usize,
    kind: u32,
    file_mtime: *const i64,
    timezone: *const libc::c_long,
    calendar: *const HttpCalendar,
) -> i32 {
    let calendar = if calendar.is_null() {
        HttpCalendar {
            tm_size: core::mem::size_of::<libc::tm>(),
            tm_align: core::mem::align_of::<libc::tm>(),
            parse: calendar_parse,
            to_time: calendar_time,
            to_gmt: calendar_gmt,
        }
    } else {
        // SAFETY: the caller lends an immutable initialized calendar descriptor.
        unsafe { *calendar }
    };
    if calendar.tm_size != core::mem::size_of::<libc::tm>()
        || calendar.tm_align != core::mem::align_of::<libc::tm>()
    {
        return INVALID;
    }
    let mut state = HttpConditional {
        kind,
        length,
        ..HttpConditional::default()
    };
    // SAFETY: zero initializes every numeric and pointer field of libc tm, as native tm{} did.
    let mut parsed: libc::tm = unsafe { core::mem::zeroed() };
    let mut file = core::ptr::null_mut();
    loop {
        let action = conditional(&mut state);
        match action {
            1..=4 => {
                // SAFETY: the caller lends terminated date bytes; parsed is this call's initialized calendar storage.
                state.valid = u32::from(unsafe {
                    (calendar.parse)(
                        date,
                        date_format((action - 1).cast_unsigned()),
                        core::ptr::from_mut(&mut parsed).cast(),
                    )
                });
            }
            5 => {
                if file_mtime.is_null() {
                    return INVALID;
                }
                // SAFETY: the policy now requests the caller's live mtime slot.
                let value = unsafe { *file_mtime };
                // SAFETY: the supplied calendar consumes this scalar and retains its own GMT storage.
                file = unsafe { (calendar.to_gmt)(value) };
                state.valid = u32::from(!file.is_null());
            }
            6 => {
                // SAFETY: parsed is this call's initialized and previously parsed tm storage.
                state.value =
                    unsafe { (calendar.to_time)(core::ptr::from_mut(&mut parsed).cast()) };
                if kind == 0 {
                    if file_mtime.is_null() || timezone.is_null() {
                        return INVALID;
                    }
                    // SAFETY: the policy requests this live timezone slot only after parsing.
                    state.timezone = unsafe { *timezone };
                    // SAFETY: the caller lends the live mtime slot for this observation.
                    state.file_mtime = unsafe { *file_mtime };
                }
            }
            7 => {
                // SAFETY: a successful GMT observation precedes this policy action.
                state.value = unsafe { (calendar.to_time)(file) };
            }
            _ => return i32::from(action == 8),
        }
    }
}

/// Format a stored modification time, or current time when the slot is null.
/// # Safety
/// A non-null slot lends one initialized i64. Output lends capacity writable bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v3_http_date(
    file_mtime: *const i64,
    output: *mut u8,
    capacity: usize,
) -> usize {
    if output.is_null() || capacity == 0 {
        return 0;
    }
    let time = if file_mtime.is_null() {
        // SAFETY: a null out pointer requests only the current scalar time.
        unsafe { libc::time(core::ptr::null_mut()) }
    } else {
        // SAFETY: the caller lends this initialized time slot.
        unsafe { *file_mtime }
    };
    // SAFETY: time is a live time_t; its borrowed static calendar view is used immediately.
    let tm = unsafe { libc::gmtime(&raw const time) };
    if tm.is_null() {
        return 0;
    }
    // SAFETY: output is writable through capacity; format is terminated and tm is a live calendar view.
    unsafe {
        libc::strftime(
            output.cast(),
            capacity,
            c"%a, %d %b %Y %H:%M:%S %Z".as_ptr(),
            tm,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn owned_calendar_keeps_empty_and_oversized_date_metadata_lazy() {
        for kind in [0, 1] {
            assert_eq!(
                unsafe {
                    swr_v3_http_conditional(
                        core::ptr::null(),
                        0,
                        kind,
                        core::ptr::null(),
                        core::ptr::null(),
                        core::ptr::null(),
                    )
                },
                0
            );
        }
        let mut date = [b'x'; 65];
        date[64] = 0;
        assert_eq!(
            unsafe {
                swr_v3_http_conditional(
                    date.as_ptr().cast(),
                    64,
                    0,
                    core::ptr::null(),
                    core::ptr::null(),
                    core::ptr::null(),
                )
            },
            0
        );
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
                assert_eq!(conditional(&mut state), action);
            }
            if kind == 1 {
                assert_eq!(conditional(&mut state), 5);
            }
            assert_eq!(conditional(&mut state), 0);
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
        assert_eq!(conditional(&mut state), OVERFLOW);
        state = HttpConditional {
            kind: 0,
            phase: 6,
            value: 100,
            timezone: 1,
            file_mtime: 99,
            ..Default::default()
        };
        assert_eq!(conditional(&mut state), 8);
        state = HttpConditional {
            kind: 0,
            length: 64,
            ..Default::default()
        };
        assert_eq!(conditional(&mut state), 0);
        assert!(date_format(4).is_null());
    }
}
